//! Named records and their versions.
//!
//! # Create or append
//!
//! `POST /{cards|evals}/{ns}/{name}` always means "append a version". If
//! the name does not exist it is created in the same transaction (the
//! caller must hold `write` on `ns`). The sequence is:
//!
//! ```text
//! 1. lock the record row (or insert it)                    SELECT ... FOR UPDATE
//! 2. read the latest non-tombstoned version's content_hash
//! 3. equal to the new hash?  → return it, 200, done        (idempotent)
//! 4. every attachments[].sha256 is `ready`?                 else 409 attachment_missing
//! 5. resolve relations[].to; look the harness and metrics up in the registry
//! 6. seq = max(seq) + 1; changed[] = top-level keys whose value differs
//! 7. badges = badges_for(facts)                             the caller's rule, the store's facts
//! 8. insert versions, fingerprints, results, relations, attachment_refs, audit
//! 9. commit → 201 { id, version_id, seq, label, content_hash, changed[], badges[] }
//! ```
//!
//! The row lock in step 1 is what makes `seq` gap-free under concurrent
//! posts to the same name. Different names do not contend. Steps 4 and 5
//! run inside the same transaction, so the version that lands has seen
//! the attachment states and relation targets it was checked against.
//!
//! # Relation resolution
//!
//! A `{ns}/{name}@{seq}` target resolves to the `version_id` of that
//! version, whatever its record type and whether or not it is tombstoned
//! (a tombstone still has a `version_id`), provided the writer may see it
//! ([`NewVersion::readable_ns`] plus the namespace written to). A version
//! the writer may not see is treated exactly as one that does not exist:
//! stored unresolved, `refs_resolved` not awarded. A Card and an Eval may
//! share `{ns}/{name}`; when the caller knows the target kind (from the
//! relation type's registry entry) it passes it, otherwise the name must be
//! unambiguous among the versions the writer may see, or the reference is
//! stored unresolved. An unresolved target
//! keeps its text in `to_external` and is re-attempted lazily by the
//! traversal in [`crate::relations`]; `external:` / `hf:` targets are
//! stored as text and never resolve.
//!
//! # Addressing
//!
//! A version is addressed as `@{seq}` or `@{label}`. `seq` is the hub's;
//! `label` is a client-chosen slug, unique within the name, re-pointable
//! with `PATCH .../label`, and **never purely numeric** so it cannot be
//! mistaken for a `seq`. No address is derived from `content_hash`.
//! Re-pointing ([`set_label`]) moves the label from whichever version
//! holds it to the addressed one in one transaction; the ingest path
//! (`?label=`) refuses a taken label instead, because a `POST` that
//! silently moved a label would be a surprise.
//!
//! # `changed[]`
//!
//! Each version records which top-level keys differ from the version
//! before it (`results`, `counts`, `model`, …). It costs one comparison at
//! write time and lets the version list show at a glance that "version 4
//! changed the results" without diffing bodies. It is kept on tombstones.
//!
//! # Tombstones
//!
//! [`tombstone`] sets `tombstoned_at`, a reason and an optional note,
//! nulls the body and deletes the version's `attachment_refs` (so the
//! object's reference count drops and the GC may collect it). Everything
//! else — `version_id`, `content_hash`, `changed[]`, `badges[]`,
//! fingerprints, results, relations — stays, so an edge pointing at the
//! version still resolves and shows what it points at. "Latest" skips
//! tombstones; `@{seq}` returns one with `body: None`.
//!
//! # Visibility
//!
//! `records.visibility` is `private` or `public`, set per name with
//! `PATCH .../settings`. Every read path in this module takes the caller's
//! namespaces and filters on them, so a private record is indistinguishable
//! from a nonexistent one (`404`) to anyone without access. There is no
//! query that returns a private record's existence. Write paths do not
//! filter the record written: the server has already checked that the
//! caller holds `write` on the namespace, so an absent record is
//! [`StoreError::RecordNotFound`]. What a write *refers to* is filtered:
//! relation targets resolve only to versions the writer may see (above).
//!
//! # Listing
//!
//! [`list`] pages over records (not versions) that have at least one live
//! version, with the visibility predicate `visibility = 'public' OR ns =
//! ANY(caller_namespaces)` applied unconditionally. `search` is a
//! case-insensitive substring match on the name or the latest version's
//! title. Pagination is keyset: the sort key plus `records.id`, compared
//! as a row value, so a page is stable while records are being created.
//! The cursor is returned decoded ([`ListCursor`]); signing and encoding
//! it is the server's business.
//!
//! # What this module takes and returns
//!
//! The body arrives already canonicalised (`serde_json::Value`) with its
//! `content_hash` computed by `evalhub_core`; fingerprints, results and
//! attachment references arrive as plain data extracted by the caller.
//! This module stores bytes and compares hashes, it never re-derives
//! them. Identifiers are `uuid::Uuid` at this layer: the hub mints ULIDs
//! and stores them in `uuid` columns, and the conversion between the two
//! spellings is the caller's.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::audit::{NewAudit, append};
use crate::error::{SQLSTATE_FOREIGN_KEY, SQLSTATE_UNIQUE, StoreError, violated_constraint};

/// The two record kinds. Stored as `records.type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    /// A Card: what was measured, how, and the score.
    Card,
    /// An Eval: the material a Card was measured from.
    Eval,
}

impl RecordType {
    /// The value stored in `records.type`.
    pub fn as_str(self) -> &'static str {
        match self {
            RecordType::Card => "card",
            RecordType::Eval => "eval",
        }
    }
}

/// Per-name visibility. Stored as `records.visibility`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Readable only by callers whose token covers the namespace. The default.
    Private,
    /// Readable by anyone.
    Public,
}

impl Visibility {
    /// The value stored in `records.visibility`.
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Private => "private",
            Visibility::Public => "public",
        }
    }

    /// Parse the stored value. The column is CHECK-constrained; anything
    /// else is a migration bug and reads as private.
    pub fn parse(s: &str) -> Visibility {
        if s == "public" {
            Visibility::Public
        } else {
            Visibility::Private
        }
    }
}

/// Why a version was tombstoned. Stored as `versions.tombstone_reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TombstoneReason {
    /// The publisher withdrew it.
    Withdrawn,
    /// It duplicated another version or record.
    Duplicate,
    /// Removed by the operator after a takedown request.
    Takedown,
    /// Anything else; the note says what.
    Other,
}

impl TombstoneReason {
    /// The stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            TombstoneReason::Withdrawn => "withdrawn",
            TombstoneReason::Duplicate => "duplicate",
            TombstoneReason::Takedown => "takedown",
            TombstoneReason::Other => "other",
        }
    }

    /// Parse a stored or client-supplied value. `None` for anything else.
    pub fn parse(s: &str) -> Option<TombstoneReason> {
        Some(match s {
            "withdrawn" => TombstoneReason::Withdrawn,
            "duplicate" => TombstoneReason::Duplicate,
            "takedown" => TombstoneReason::Takedown,
            "other" => TombstoneReason::Other,
            _ => return None,
        })
    }
}

/// What a tombstone request carries besides the address.
#[derive(Debug, Clone, Copy)]
pub struct TombstoneRequest<'a> {
    /// Why the version is being removed.
    pub reason: TombstoneReason,
    /// Free text for the reader, if any.
    pub note: Option<&'a str>,
}

/// The tombstone on a version, when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    /// When it was tombstoned.
    pub at: DateTime<Utc>,
    /// Why.
    pub reason: TombstoneReason,
    /// Free text, if the writer gave one.
    pub note: Option<String>,
}

/// Who performed a write, for the `audit` row. Both are optional because
/// the CLI bootstrap acts without a token.
#[derive(Debug, Clone, Copy, Default)]
pub struct Actor {
    /// The user behind the token, if any.
    pub user_id: Option<Uuid>,
    /// The token presented, if any.
    pub token_id: Option<Uuid>,
}

/// One `results[]` entry, as a row of the `results` table.
#[derive(Debug, Clone, Copy)]
pub struct NewResult<'a> {
    /// Position in `results[]`.
    pub ordinal: i32,
    /// `results[].metric`, `{ns}/{name}`.
    pub metric: &'a str,
    /// `results[].aggregation`.
    pub aggregation: &'a str,
    /// `results[].value`.
    pub value: f64,
    /// `results[].n`.
    pub n: Option<i32>,
    /// `results[].by`.
    pub by: Option<&'a Value>,
}

/// Where a relation points.
#[derive(Debug, Clone, Copy)]
pub enum RelationTarget<'a> {
    /// A version on this hub, `{ns}/{name}@{seq}`. `record_type` is the
    /// target kind when the caller knows it (from the relation type);
    /// `None` resolves only if the name is unambiguous across kinds.
    Version {
        /// Namespace of the target.
        ns: &'a str,
        /// Name of the target.
        name: &'a str,
        /// Sequence number of the target version.
        seq: i32,
        /// Kind of the target, if known.
        record_type: Option<RecordType>,
    },
    /// Anything outside the hub (`external:https://…`, `hf:org/repo@sha`),
    /// stored as text.
    External(&'a str),
}

/// One `relations[]` entry to write from the new version.
#[derive(Debug, Clone, Copy)]
pub struct NewRelation<'a> {
    /// `relations[].type`, a registry id.
    pub relation_type: &'a str,
    /// `relations[].to`, parsed.
    pub target: RelationTarget<'a>,
    /// `relations[].attrs`.
    pub attrs: Option<&'a Value>,
}

/// One `attachments[]` entry: the path the record uses and the object it
/// names.
#[derive(Debug, Clone, Copy)]
pub struct NewAttachmentRef<'a> {
    /// `attachments[].path`.
    pub path: &'a str,
    /// `attachments[].sha256`, decoded.
    pub sha256: [u8; 32],
}

/// A version to append. The body is the canonical record and
/// `content_hash` is its sha256; both are computed by the caller, as are
/// the fingerprints and the rows extracted from the body.
#[derive(Debug, Clone, Copy)]
pub struct NewVersion<'a> {
    /// Card or Eval.
    pub record_type: RecordType,
    /// Namespace the name lives in. Must exist.
    pub ns: &'a str,
    /// The record name within the namespace.
    pub name: &'a str,
    /// The canonical record body.
    pub body: &'a Value,
    /// sha256 of the canonical bytes.
    pub content_hash: &'a [u8; 32],
    /// Optional label for the new version; must be unique within the name.
    pub label: Option<&'a str>,
    /// Who is writing.
    pub actor: Actor,
    /// Namespaces whose private records the writer may read. Relation
    /// targets resolve only to versions visible under these plus `ns`
    /// itself (writing there implies reading there).
    pub readable_ns: &'a [String],
    /// Per-facet fingerprints, `(facet, sha256)`.
    pub fingerprints: &'a [(&'a str, [u8; 32])],
    /// `results[]` as rows.
    pub results: &'a [NewResult<'a>],
    /// `relations[]`, targets parsed.
    pub relations: &'a [NewRelation<'a>],
    /// `attachments[]`; every digest must already be `ready`.
    pub attachments: &'a [NewAttachmentRef<'a>],
}

/// What the store learned while ingesting a version, handed to the
/// caller's badge rule and returned with the outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IngestFacts {
    /// Every `relations[].to` naming a hub version resolved. Vacuously
    /// true when there are none.
    pub all_refs_resolved: bool,
    /// `harness.name@version` is a `harnesses` registry entry.
    pub harness_registered: bool,
    /// Every `results[].metric` is a `metrics` registry entry. Vacuously
    /// true when there are no results; the badge rule decides what that
    /// means.
    pub all_metrics_registered: bool,
}

/// What the hub knows about a version besides its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionMeta {
    /// The record's stable id (`records.id`).
    pub record_id: Uuid,
    /// This version's id.
    pub version_id: Uuid,
    /// Hub-assigned, gap-free per name, starting at 1.
    pub seq: i32,
    /// Client-chosen label, if any.
    pub label: Option<String>,
    /// sha256 of the canonical body.
    pub content_hash: Vec<u8>,
    /// When the hub stored it.
    pub created_at: DateTime<Utc>,
    /// Top-level keys that differ from the previous version.
    pub changed: Vec<String>,
    /// Badges awarded at ingest.
    pub badges: Vec<String>,
    /// The record's visibility at read time.
    pub visibility: Visibility,
}

/// A version as read back: its metadata and, unless tombstoned, its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredVersion {
    /// The hub's facts about the version.
    pub meta: VersionMeta,
    /// The canonical body; `None` exactly when `tombstone` is `Some`.
    pub body: Option<Value>,
    /// The tombstone, if the version has one.
    pub tombstone: Option<Tombstone>,
}

/// One row of a version list: the metadata without the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionSummary {
    /// The hub's facts about the version.
    pub meta: VersionMeta,
    /// The tombstone, if the version has one.
    pub tombstone: Option<Tombstone>,
}

/// Result of [`create_or_append`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    /// A new version was written (`201`).
    Created {
        /// The version written.
        meta: VersionMeta,
        /// What the store found while writing it.
        facts: IngestFacts,
    },
    /// The body equals the latest version's; nothing was written (`200`).
    Existing(VersionMeta),
}

impl CreateOutcome {
    /// The version, whether new or existing.
    pub fn meta(&self) -> &VersionMeta {
        match self {
            CreateOutcome::Created { meta, .. } | CreateOutcome::Existing(meta) => meta,
        }
    }
}

/// Append a version to `{type}/{ns}/{name}`, creating the name if needed,
/// in one transaction. See the module doc for the sequence.
///
/// `new_ids` is called once and yields `(record_id, version_id)`; the
/// record id is used only when the name is new, the version id only when a
/// version is written. `badges_for` is called once, inside the
/// transaction, with the facts the store established; its output is
/// stored as `badges[]`.
///
/// Errors: [`StoreError::NamespaceUnknown`] when `ns` has no row,
/// [`StoreError::AttachmentMissing`] when a referenced object is not
/// `ready`, [`StoreError::LabelInUse`] when `label` already names a
/// version of this record, [`StoreError::Query`] otherwise. Nothing is
/// written on any error.
pub async fn create_or_append(
    pool: &PgPool,
    new: NewVersion<'_>,
    new_ids: impl FnOnce() -> (Uuid, Uuid),
    // `Send + Sync` so that a caller holding this reference across the
    // transaction's awaits still has a `Send` future, which is what an
    // axum handler requires.
    badges_for: &(dyn Fn(&IngestFacts) -> Vec<String> + Send + Sync),
) -> Result<CreateOutcome, StoreError> {
    let (candidate_record_id, version_id) = new_ids();
    let record_type = new.record_type.as_str();

    let mut tx = pool.begin().await?;

    // Step 1: insert the name if new, then lock its row. The unique index
    // makes a concurrent insert wait for this transaction, and FOR UPDATE
    // serialises appends to the same name.
    let inserted = sqlx::query!(
        "INSERT INTO records (id, type, ns, name) VALUES ($1, $2, $3, $4)
         ON CONFLICT (type, ns, name) DO NOTHING",
        candidate_record_id,
        record_type,
        new.ns,
        new.name,
    )
    .execute(&mut *tx)
    .await;
    if let Err(e) = inserted {
        if let Some((code, constraint)) = violated_constraint(&e)
            && code == SQLSTATE_FOREIGN_KEY
            && constraint == "records_ns_fkey"
        {
            return Err(StoreError::NamespaceUnknown(new.ns.to_owned()));
        }
        return Err(e.into());
    }

    let record = sqlx::query!(
        "SELECT id, visibility FROM records WHERE type = $1 AND ns = $2 AND name = $3 FOR UPDATE",
        record_type,
        new.ns,
        new.name,
    )
    .fetch_one(&mut *tx)
    .await?;
    let record_id = record.id;
    let visibility = Visibility::parse(&record.visibility);

    // Step 2: the latest live version.
    let latest = sqlx::query!(
        "SELECT version_id, seq, label, content_hash, body, created_at, changed, badges
         FROM versions
         WHERE record_id = $1 AND tombstoned_at IS NULL
         ORDER BY seq DESC LIMIT 1",
        record_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    // Step 3: idempotent hit.
    if let Some(l) = &latest
        && l.content_hash.as_slice() == new.content_hash
    {
        let meta = VersionMeta {
            record_id,
            version_id: l.version_id,
            seq: l.seq,
            label: l.label.clone(),
            content_hash: l.content_hash.clone(),
            created_at: l.created_at,
            changed: l.changed.clone(),
            badges: l.badges.clone(),
            visibility,
        };
        tx.commit().await?;
        return Ok(CreateOutcome::Existing(meta));
    }

    // Step 4: every referenced attachment is ready.
    let missing = missing_attachments(&mut tx, new.attachments).await?;
    if !missing.is_empty() {
        return Err(StoreError::AttachmentMissing(missing));
    }

    // Step 5: relation targets and registry lookups.
    let mut readable_ns = new.readable_ns.to_vec();
    if !readable_ns.iter().any(|n| n == new.ns) {
        readable_ns.push(new.ns.to_owned());
    }
    let mut resolved: Vec<Option<Uuid>> = Vec::with_capacity(new.relations.len());
    let mut all_refs_resolved = true;
    for rel in new.relations {
        let target = match rel.target {
            RelationTarget::Version {
                ns,
                name,
                seq,
                record_type,
            } => {
                let found =
                    resolve_version(&mut tx, ns, name, seq, record_type, &readable_ns).await?;
                if found.is_none() {
                    all_refs_resolved = false;
                }
                found
            }
            RelationTarget::External(_) => None,
        };
        resolved.push(target);
    }
    let harness_registered = harness_registered(&mut tx, new.body).await?;
    let all_metrics_registered = all_metrics_registered(&mut tx, new.results).await?;
    let facts = IngestFacts {
        all_refs_resolved,
        harness_registered,
        all_metrics_registered,
    };

    // Step 6: next seq, counting tombstoned versions too so a seq is never
    // reused; changed[] against the previous live body.
    let max_seq = sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(seq), 0) AS "max!" FROM versions WHERE record_id = $1"#,
        record_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    let seq = max_seq + 1;
    let changed = changed_keys(latest.as_ref().and_then(|l| l.body.as_ref()), new.body);

    // Step 7: the caller's badge rule over the store's facts.
    let badges = badges_for(&facts);

    // Step 8: the version row and its side tables.
    let content_hash: &[u8] = new.content_hash;
    let inserted = sqlx::query!(
        "INSERT INTO versions (version_id, record_id, seq, label, content_hash, body, changed, badges)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING created_at",
        version_id,
        record_id,
        seq,
        new.label,
        content_hash,
        new.body,
        &changed,
        &badges,
    )
    .fetch_one(&mut *tx)
    .await;
    let row = match inserted {
        Ok(row) => row,
        Err(e) => {
            if let Some((code, constraint)) = violated_constraint(&e)
                && code == SQLSTATE_UNIQUE
                && constraint == "versions_record_id_label_key"
            {
                return Err(StoreError::LabelInUse);
            }
            return Err(e.into());
        }
    };

    for (facet, fingerprint) in new.fingerprints {
        let fp: &[u8] = fingerprint;
        sqlx::query!(
            "INSERT INTO fingerprints (version_id, facet, fingerprint) VALUES ($1, $2, $3)",
            version_id,
            *facet,
            fp,
        )
        .execute(&mut *tx)
        .await?;
    }
    for r in new.results {
        sqlx::query!(
            "INSERT INTO results (version_id, ordinal, metric, aggregation, value, n, by)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            version_id,
            r.ordinal,
            r.metric,
            r.aggregation,
            r.value,
            r.n,
            r.by,
        )
        .execute(&mut *tx)
        .await?;
    }
    for (rel, to_version_id) in new.relations.iter().zip(&resolved) {
        let to_external = match (rel.target, to_version_id) {
            (_, Some(_)) => None,
            (RelationTarget::External(s), None) => Some(s.to_owned()),
            (RelationTarget::Version { ns, name, seq, .. }, None) => {
                Some(format!("{ns}/{name}@{seq}"))
            }
        };
        sqlx::query!(
            "INSERT INTO relations (from_version_id, type, to_version_id, to_external, attrs)
             VALUES ($1, $2, $3, $4, $5)",
            version_id,
            rel.relation_type,
            *to_version_id,
            to_external,
            rel.attrs,
        )
        .execute(&mut *tx)
        .await?;
    }
    for a in new.attachments {
        let sha: &[u8] = &a.sha256;
        sqlx::query!(
            "INSERT INTO attachment_refs (version_id, sha256, path) VALUES ($1, $2, $3)",
            version_id,
            sha,
            a.path,
        )
        .execute(&mut *tx)
        .await?;
    }

    let action = if seq == 1 {
        "record.create"
    } else {
        "version.append"
    };
    let subject = format!("{record_type}/{}/{}@{seq}", new.ns, new.name);
    append(
        &mut tx,
        NewAudit {
            actor: new.actor,
            ns: Some(new.ns),
            action,
            subject: Some(&subject),
            detail: Some(serde_json::json!({
                "version_id": version_id,
                "content_hash": hex_lower(new.content_hash),
            })),
        },
    )
    .await?;

    // Step 9.
    tx.commit().await?;
    Ok(CreateOutcome::Created {
        meta: VersionMeta {
            record_id,
            version_id,
            seq,
            label: new.label.map(str::to_owned),
            content_hash: new.content_hash.to_vec(),
            created_at: row.created_at,
            changed,
            badges,
            visibility,
        },
        facts,
    })
}

/// The digests among `attachments` with no `ready` row, in input order,
/// deduplicated.
async fn missing_attachments(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    attachments: &[NewAttachmentRef<'_>],
) -> Result<Vec<[u8; 32]>, StoreError> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }
    let wanted: Vec<Vec<u8>> = attachments.iter().map(|a| a.sha256.to_vec()).collect();
    let ready: Vec<Vec<u8>> = sqlx::query_scalar!(
        "SELECT sha256 FROM attachments WHERE sha256 = ANY($1) AND state = 'ready'",
        &wanted,
    )
    .fetch_all(&mut **tx)
    .await?;
    let ready: HashSet<Vec<u8>> = ready.into_iter().collect();
    let mut seen = HashSet::new();
    Ok(attachments
        .iter()
        .filter(|a| !ready.contains(a.sha256.as_slice()) && seen.insert(a.sha256))
        .map(|a| a.sha256)
        .collect())
}

/// The `version_id` of `{ns}/{name}@{seq}`, of the given kind or, when
/// none is given, of whichever kind has it if exactly one does. Only
/// versions visible to `readable_ns` are candidates, so an invisible one
/// neither resolves nor makes a visible one ambiguous.
async fn resolve_version(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    ns: &str,
    name: &str,
    seq: i32,
    record_type: Option<RecordType>,
    readable_ns: &[String],
) -> Result<Option<Uuid>, StoreError> {
    let rows = sqlx::query!(
        "SELECT v.version_id, r.type AS record_type, r.visibility
         FROM versions v JOIN records r ON r.id = v.record_id
         WHERE r.ns = $1 AND r.name = $2 AND v.seq = $3
           AND ($4::text IS NULL OR r.type = $4)
         ORDER BY r.type",
        ns,
        name,
        seq,
        record_type.map(RecordType::as_str),
    )
    .fetch_all(&mut **tx)
    .await?;
    let visible: Vec<Uuid> = rows
        .into_iter()
        .filter(|r| crate::relations::visible_to(&r.visibility, ns, readable_ns))
        .map(|r| r.version_id)
        .collect();
    Ok(match visible.as_slice() {
        [one] => Some(*one),
        _ => None,
    })
}

/// Whether `body.harness.name@version` names a `harnesses` registry entry.
async fn harness_registered(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    body: &Value,
) -> Result<bool, StoreError> {
    let Some(harness) = body.get("harness") else {
        return Ok(false);
    };
    let (Some(name), Some(version)) = (
        harness.get("name").and_then(Value::as_str),
        harness.get("version").and_then(Value::as_str),
    ) else {
        return Ok(false);
    };
    let Some((ns, id)) = name.split_once('/') else {
        return Ok(false);
    };
    let found = sqlx::query_scalar!(
        r#"SELECT EXISTS(
             SELECT 1 FROM registry
             WHERE kind = 'harnesses' AND ns = $1 AND id = $2 AND version = $3
           ) AS "exists!""#,
        ns,
        id,
        version,
    )
    .fetch_one(&mut **tx)
    .await?;
    Ok(found)
}

/// Whether every distinct `results[].metric` names a `metrics` registry
/// entry (any version). Vacuously true with no results.
async fn all_metrics_registered(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    results: &[NewResult<'_>],
) -> Result<bool, StoreError> {
    let metrics: HashSet<&str> = results.iter().map(|r| r.metric).collect();
    for metric in metrics {
        let Some((ns, id)) = metric.split_once('/') else {
            return Ok(false);
        };
        let found = sqlx::query_scalar!(
            r#"SELECT EXISTS(
                 SELECT 1 FROM registry WHERE kind = 'metrics' AND ns = $1 AND id = $2
               ) AS "exists!""#,
            ns,
            id,
        )
        .fetch_one(&mut **tx)
        .await?;
        if !found {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The row shape every version read maps from.
struct VersionRow {
    record_id: Uuid,
    visibility: String,
    version_id: Uuid,
    seq: i32,
    label: Option<String>,
    content_hash: Vec<u8>,
    body: Option<Value>,
    created_at: DateTime<Utc>,
    changed: Vec<String>,
    badges: Vec<String>,
    tombstoned_at: Option<DateTime<Utc>>,
    tombstone_reason: Option<String>,
    tombstone_note: Option<String>,
}

impl VersionRow {
    fn tombstone(&self) -> Option<Tombstone> {
        let at = self.tombstoned_at?;
        Some(Tombstone {
            at,
            reason: self
                .tombstone_reason
                .as_deref()
                .and_then(TombstoneReason::parse)
                .unwrap_or(TombstoneReason::Other),
            note: self.tombstone_note.clone(),
        })
    }

    fn meta(&self) -> VersionMeta {
        VersionMeta {
            record_id: self.record_id,
            version_id: self.version_id,
            seq: self.seq,
            label: self.label.clone(),
            content_hash: self.content_hash.clone(),
            created_at: self.created_at,
            changed: self.changed.clone(),
            badges: self.badges.clone(),
            visibility: Visibility::parse(&self.visibility),
        }
    }

    fn into_stored(self) -> StoredVersion {
        let tombstone = self.tombstone();
        let meta = self.meta();
        StoredVersion {
            meta,
            body: self.body,
            tombstone,
        }
    }

    fn into_summary(self) -> VersionSummary {
        VersionSummary {
            tombstone: self.tombstone(),
            meta: self.meta(),
        }
    }
}

/// The latest live version of `{type}/{ns}/{name}`, or `None` when the
/// name does not exist, has no live version, or is private and
/// `caller_namespaces` does not contain `ns`. The cases are deliberately
/// indistinguishable.
pub async fn get_latest(
    pool: &PgPool,
    record_type: RecordType,
    ns: &str,
    name: &str,
    caller_namespaces: &[String],
) -> Result<Option<StoredVersion>, StoreError> {
    let row = sqlx::query_as!(
        VersionRow,
        "SELECT r.id AS record_id, r.visibility,
                v.version_id, v.seq, v.label, v.content_hash, v.body, v.created_at, v.changed, v.badges,
                v.tombstoned_at, v.tombstone_reason, v.tombstone_note
         FROM records r
         JOIN versions v ON v.record_id = r.id
         WHERE r.type = $1 AND r.ns = $2 AND r.name = $3
           AND (r.visibility = 'public' OR r.ns = ANY($4))
           AND v.tombstoned_at IS NULL
         ORDER BY v.seq DESC LIMIT 1",
        record_type.as_str(),
        ns,
        name,
        caller_namespaces,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(VersionRow::into_stored))
}

/// Version `@{seq}` of `{type}/{ns}/{name}`, with the same visibility
/// rule as [`get_latest`]. A tombstoned version is returned with `body:
/// None` and its `tombstone`.
pub async fn get_by_seq(
    pool: &PgPool,
    record_type: RecordType,
    ns: &str,
    name: &str,
    seq: i32,
    caller_namespaces: &[String],
) -> Result<Option<StoredVersion>, StoreError> {
    let row = sqlx::query_as!(
        VersionRow,
        "SELECT r.id AS record_id, r.visibility,
                v.version_id, v.seq, v.label, v.content_hash, v.body, v.created_at, v.changed, v.badges,
                v.tombstoned_at, v.tombstone_reason, v.tombstone_note
         FROM records r
         JOIN versions v ON v.record_id = r.id
         WHERE r.type = $1 AND r.ns = $2 AND r.name = $3 AND v.seq = $4
           AND (r.visibility = 'public' OR r.ns = ANY($5))",
        record_type.as_str(),
        ns,
        name,
        seq,
        caller_namespaces,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(VersionRow::into_stored))
}

/// Version `@{label}` of `{type}/{ns}/{name}`, with the same visibility
/// rule as [`get_latest`].
pub async fn get_by_label(
    pool: &PgPool,
    record_type: RecordType,
    ns: &str,
    name: &str,
    label: &str,
    caller_namespaces: &[String],
) -> Result<Option<StoredVersion>, StoreError> {
    let row = sqlx::query_as!(
        VersionRow,
        "SELECT r.id AS record_id, r.visibility,
                v.version_id, v.seq, v.label, v.content_hash, v.body, v.created_at, v.changed, v.badges,
                v.tombstoned_at, v.tombstone_reason, v.tombstone_note
         FROM records r
         JOIN versions v ON v.record_id = r.id
         WHERE r.type = $1 AND r.ns = $2 AND r.name = $3 AND v.label = $4
           AND (r.visibility = 'public' OR r.ns = ANY($5))",
        record_type.as_str(),
        ns,
        name,
        label,
        caller_namespaces,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(VersionRow::into_stored))
}

/// Every version of `{type}/{ns}/{name}`, ascending `seq`, tombstones
/// included (with their tombstone, without their body). `None` when the
/// record is invisible to the caller, per [`get_latest`].
pub async fn list_versions(
    pool: &PgPool,
    record_type: RecordType,
    ns: &str,
    name: &str,
    caller_namespaces: &[String],
) -> Result<Option<Vec<VersionSummary>>, StoreError> {
    let visible = sqlx::query_scalar!(
        r#"SELECT EXISTS(
             SELECT 1 FROM records
             WHERE type = $1 AND ns = $2 AND name = $3
               AND (visibility = 'public' OR ns = ANY($4))
           ) AS "exists!""#,
        record_type.as_str(),
        ns,
        name,
        caller_namespaces,
    )
    .fetch_one(pool)
    .await?;
    if !visible {
        return Ok(None);
    }
    let rows = sqlx::query_as!(
        VersionRow,
        "SELECT r.id AS record_id, r.visibility,
                v.version_id, v.seq, v.label, v.content_hash, NULL::jsonb AS body, v.created_at, v.changed, v.badges,
                v.tombstoned_at, v.tombstone_reason, v.tombstone_note
         FROM records r
         JOIN versions v ON v.record_id = r.id
         WHERE r.type = $1 AND r.ns = $2 AND r.name = $3
         ORDER BY v.seq ASC",
        record_type.as_str(),
        ns,
        name,
    )
    .fetch_all(pool)
    .await?;
    Ok(Some(
        rows.into_iter().map(VersionRow::into_summary).collect(),
    ))
}

/// The per-facet fingerprints of a version, keyed by facet name. Empty
/// for an unknown version.
pub async fn load_fingerprints(
    pool: &PgPool,
    version_id: Uuid,
) -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let rows = sqlx::query!(
        "SELECT facet, fingerprint FROM fingerprints WHERE version_id = $1",
        version_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| (r.facet, r.fingerprint)).collect())
}

/// The canonical body and current badges of one live version, by id.
///
/// The badge recomputation job needs both: the body to re-derive the
/// badges that come from the record, and the badges it already has,
/// because `refs_resolved` records what was true at ingest and is never
/// recomputed. A tombstoned version has no body and is not returned.
pub async fn body_and_badges(
    pool: &PgPool,
    version_id: Uuid,
) -> Result<Option<(Value, Vec<String>)>, StoreError> {
    let row = sqlx::query!(
        "SELECT body, badges FROM versions
         WHERE version_id = $1 AND tombstoned_at IS NULL",
        version_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| r.body.map(|body| (body, r.badges))))
}

/// Lock `{type}/{ns}/{name}` for a write and return its id and
/// visibility, or [`StoreError::RecordNotFound`].
async fn lock_record(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    record_type: RecordType,
    ns: &str,
    name: &str,
) -> Result<(Uuid, Visibility), StoreError> {
    let row = sqlx::query!(
        "SELECT id, visibility FROM records WHERE type = $1 AND ns = $2 AND name = $3 FOR UPDATE",
        record_type.as_str(),
        ns,
        name,
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(StoreError::RecordNotFound)?;
    Ok((row.id, Visibility::parse(&row.visibility)))
}

/// Point `label` at version `@{seq}` of `{type}/{ns}/{name}`, removing it
/// from whichever version held it. Returns the labelled version.
///
/// Errors: [`StoreError::LabelInvalid`] for a purely numeric label,
/// [`StoreError::RecordNotFound`], [`StoreError::VersionNotFound`].
pub async fn set_label(
    pool: &PgPool,
    record_type: RecordType,
    ns: &str,
    name: &str,
    seq: i32,
    label: &str,
    actor: Actor,
) -> Result<VersionMeta, StoreError> {
    if label.is_empty() || label.bytes().all(|b| b.is_ascii_digit()) {
        return Err(StoreError::LabelInvalid);
    }
    let mut tx = pool.begin().await?;
    let (record_id, visibility) = lock_record(&mut tx, record_type, ns, name).await?;
    sqlx::query!(
        "UPDATE versions SET label = NULL WHERE record_id = $1 AND label = $2",
        record_id,
        label,
    )
    .execute(&mut *tx)
    .await?;
    let row = sqlx::query!(
        "UPDATE versions SET label = $3
         WHERE record_id = $1 AND seq = $2
         RETURNING version_id, seq, label, content_hash, created_at, changed, badges",
        record_id,
        seq,
        label,
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(StoreError::VersionNotFound)?;
    let subject = format!("{}/{ns}/{name}@{seq}", record_type.as_str());
    append(
        &mut tx,
        NewAudit {
            actor,
            ns: Some(ns),
            action: "label.set",
            subject: Some(&subject),
            detail: Some(serde_json::json!({ "label": label })),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(VersionMeta {
        record_id,
        version_id: row.version_id,
        seq: row.seq,
        label: row.label,
        content_hash: row.content_hash,
        created_at: row.created_at,
        changed: row.changed,
        badges: row.badges,
        visibility,
    })
}

/// Set the visibility of `{type}/{ns}/{name}`.
///
/// Errors: [`StoreError::RecordNotFound`].
pub async fn set_visibility(
    pool: &PgPool,
    record_type: RecordType,
    ns: &str,
    name: &str,
    visibility: Visibility,
    actor: Actor,
) -> Result<(), StoreError> {
    let mut tx = pool.begin().await?;
    let (record_id, _) = lock_record(&mut tx, record_type, ns, name).await?;
    sqlx::query!(
        "UPDATE records SET visibility = $2 WHERE id = $1",
        record_id,
        visibility.as_str(),
    )
    .execute(&mut *tx)
    .await?;
    let subject = format!("{}/{ns}/{name}", record_type.as_str());
    append(
        &mut tx,
        NewAudit {
            actor,
            ns: Some(ns),
            action: "settings.visibility",
            subject: Some(&subject),
            detail: Some(serde_json::json!({ "visibility": visibility.as_str() })),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Tombstone version `@{seq}` of `{type}/{ns}/{name}`: null the body,
/// record the reason, drop the version's attachment references. See the
/// module doc for what stays. Returns the version's metadata.
///
/// Errors: [`StoreError::RecordNotFound`], [`StoreError::VersionNotFound`],
/// [`StoreError::AlreadyTombstoned`].
pub async fn tombstone(
    pool: &PgPool,
    record_type: RecordType,
    ns: &str,
    name: &str,
    seq: i32,
    request: TombstoneRequest<'_>,
    actor: Actor,
) -> Result<VersionMeta, StoreError> {
    let TombstoneRequest { reason, note } = request;
    let mut tx = pool.begin().await?;
    let (record_id, visibility) = lock_record(&mut tx, record_type, ns, name).await?;
    let row = sqlx::query!(
        "UPDATE versions
         SET tombstoned_at = now(), tombstone_reason = $3, tombstone_note = $4, body = NULL
         WHERE record_id = $1 AND seq = $2 AND tombstoned_at IS NULL
         RETURNING version_id, seq, label, content_hash, created_at, changed, badges",
        record_id,
        seq,
        reason.as_str(),
        note,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        let exists = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM versions WHERE record_id = $1 AND seq = $2) AS "exists!""#,
            record_id,
            seq,
        )
        .fetch_one(&mut *tx)
        .await?;
        return Err(if exists {
            StoreError::AlreadyTombstoned
        } else {
            StoreError::VersionNotFound
        });
    };
    sqlx::query!(
        "DELETE FROM attachment_refs WHERE version_id = $1",
        row.version_id,
    )
    .execute(&mut *tx)
    .await?;
    let subject = format!("{}/{ns}/{name}@{seq}", record_type.as_str());
    append(
        &mut tx,
        NewAudit {
            actor,
            ns: Some(ns),
            action: "version.tombstone",
            subject: Some(&subject),
            detail: Some(serde_json::json!({
                "version_id": row.version_id,
                "reason": reason.as_str(),
                "note": note,
            })),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(VersionMeta {
        record_id,
        version_id: row.version_id,
        seq: row.seq,
        label: row.label,
        content_hash: row.content_hash,
        created_at: row.created_at,
        changed: row.changed,
        badges: row.badges,
        visibility,
    })
}

/// Sort order of [`list`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListSort {
    /// Newest record first (by `records.created_at`). The default.
    #[default]
    CreatedDesc,
    /// Oldest record first.
    CreatedAsc,
    /// By name, ascending.
    NameAsc,
}

/// The keyset position after a page: the last row's sort keys and id.
/// Opaque to clients once the server has signed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListCursor {
    /// `records.created_at` of the last row.
    pub created_at: DateTime<Utc>,
    /// `records.name` of the last row.
    pub name: String,
    /// `records.id` of the last row; the tie-breaker.
    pub id: Uuid,
}

/// Parameters of [`list`].
#[derive(Debug, Clone)]
pub struct ListParams<'a> {
    /// Cards or Evals.
    pub record_type: RecordType,
    /// Restrict to one namespace.
    pub ns: Option<&'a str>,
    /// Case-insensitive substring of the name or the latest title.
    pub search: Option<&'a str>,
    /// Sort order.
    pub sort: ListSort,
    /// Continue after this position.
    pub cursor: Option<ListCursor>,
    /// Page size.
    pub limit: u32,
}

/// One row of a record list: the record and its latest live version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// Namespace.
    pub ns: String,
    /// Name.
    pub name: String,
    /// Visibility.
    pub visibility: Visibility,
    /// When the name was created.
    pub created_at: DateTime<Utc>,
    /// The latest live version.
    pub latest: VersionMeta,
    /// `title` of the latest live version.
    pub title: Option<String>,
}

/// Page over records with at least one live version, visible to the
/// caller. Returns the page and, when more rows remain, the cursor for
/// the next call. See the module doc for the predicate and the keyset.
pub async fn list(
    pool: &PgPool,
    params: ListParams<'_>,
    caller_namespaces: &[String],
) -> Result<(Vec<ListItem>, Option<ListCursor>), StoreError> {
    let limit = i64::from(params.limit.max(1));
    let mut q: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT r.ns, r.name, r.visibility, r.created_at AS record_created_at, r.id AS record_id,
                v.version_id, v.seq, v.label, v.content_hash, v.created_at, v.changed, v.badges, v.title
         FROM records r
         JOIN LATERAL (
             SELECT version_id, seq, label, content_hash, created_at, changed, badges, title
             FROM versions
             WHERE record_id = r.id AND tombstoned_at IS NULL
             ORDER BY seq DESC LIMIT 1
         ) v ON true
         WHERE r.type = ",
    );
    q.push_bind(params.record_type.as_str());
    q.push(" AND (r.visibility = 'public' OR r.ns = ANY(");
    q.push_bind(caller_namespaces.to_vec());
    q.push("))");
    if let Some(ns) = params.ns {
        q.push(" AND r.ns = ");
        q.push_bind(ns.to_owned());
    }
    if let Some(search) = params.search.filter(|s| !s.is_empty()) {
        let pattern = format!("%{}%", escape_like(search));
        q.push(" AND (r.name ILIKE ");
        q.push_bind(pattern.clone());
        q.push(" ESCAPE '\\' OR v.title ILIKE ");
        q.push_bind(pattern);
        q.push(" ESCAPE '\\')");
    }
    if let Some(c) = &params.cursor {
        match params.sort {
            ListSort::CreatedDesc => {
                q.push(" AND (r.created_at, r.id) < (");
                q.push_bind(c.created_at);
                q.push(", ");
                q.push_bind(c.id);
                q.push(")");
            }
            ListSort::CreatedAsc => {
                q.push(" AND (r.created_at, r.id) > (");
                q.push_bind(c.created_at);
                q.push(", ");
                q.push_bind(c.id);
                q.push(")");
            }
            ListSort::NameAsc => {
                q.push(" AND (r.name, r.id) > (");
                q.push_bind(c.name.clone());
                q.push(", ");
                q.push_bind(c.id);
                q.push(")");
            }
        }
    }
    q.push(match params.sort {
        ListSort::CreatedDesc => " ORDER BY r.created_at DESC, r.id DESC",
        ListSort::CreatedAsc => " ORDER BY r.created_at ASC, r.id ASC",
        ListSort::NameAsc => " ORDER BY r.name ASC, r.id ASC",
    });
    q.push(" LIMIT ");
    q.push_bind(limit + 1);

    let rows = q.build().fetch_all(pool).await?;
    let mut items: Vec<ListItem> = rows
        .iter()
        .map(|row| -> Result<ListItem, sqlx::Error> {
            let visibility: String = row.try_get("visibility")?;
            Ok(ListItem {
                ns: row.try_get("ns")?,
                name: row.try_get("name")?,
                visibility: Visibility::parse(&visibility),
                created_at: row.try_get("record_created_at")?,
                latest: VersionMeta {
                    record_id: row.try_get("record_id")?,
                    version_id: row.try_get("version_id")?,
                    seq: row.try_get("seq")?,
                    label: row.try_get("label")?,
                    content_hash: row.try_get("content_hash")?,
                    created_at: row.try_get("created_at")?,
                    changed: row.try_get("changed")?,
                    badges: row.try_get("badges")?,
                    visibility: Visibility::parse(&visibility),
                },
                title: row.try_get("title")?,
            })
        })
        .collect::<Result<_, _>>()?;
    let next = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|i| ListCursor {
            created_at: i.created_at,
            name: i.name.clone(),
            id: i.latest.record_id,
        })
    } else {
        None
    };
    Ok((items, next))
}

/// Escape `%`, `_` and `\` so a search string matches literally under
/// `LIKE … ESCAPE '\'`.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Sorted top-level keys whose value differs between `previous` and
/// `next`, including keys present on only one side. With no previous body
/// every key of `next` is "changed".
fn changed_keys(previous: Option<&Value>, next: &Value) -> Vec<String> {
    let empty = serde_json::Map::new();
    let prev = previous.and_then(Value::as_object).unwrap_or(&empty);
    let next = next.as_object().unwrap_or(&empty);
    let mut keys: Vec<String> = prev
        .keys()
        .chain(next.keys())
        .filter(|k| prev.get(*k) != next.get(*k))
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_keys_first_version_lists_every_key() {
        let next = serde_json::json!({"b": 1, "a": {"x": 1}});
        assert_eq!(changed_keys(None, &next), vec!["a", "b"]);
    }

    #[test]
    fn changed_keys_reports_differences_and_removals() {
        let prev = serde_json::json!({"a": 1, "b": 2, "c": 3});
        let next = serde_json::json!({"a": 1, "b": 20, "d": 4});
        assert_eq!(changed_keys(Some(&prev), &next), vec!["b", "c", "d"]);
    }

    #[test]
    fn escape_like_escapes_wildcards() {
        assert_eq!(escape_like("a%b_c\\d"), "a\\%b\\_c\\\\d");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[test]
    fn tombstone_reason_round_trips() {
        for r in [
            TombstoneReason::Withdrawn,
            TombstoneReason::Duplicate,
            TombstoneReason::Takedown,
            TombstoneReason::Other,
        ] {
            assert_eq!(TombstoneReason::parse(r.as_str()), Some(r));
        }
        assert_eq!(TombstoneReason::parse("gone"), None);
    }
}
