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
//!    5b. a Card: FOR SHARE on each Eval its core/uses_eval edges resolved to,
//!        fix the used set per Eval record, check run_results[] against it
//!                                                           else 422 run_unknown /
//!                                                                run_not_in_used_set
//! 6. seq = max(seq) + 1; changed[] = top-level keys whose value differs
//! 7. badges = badges_for(facts)                             the caller's rule, the store's facts
//! 8. insert versions, fingerprints, results, relations, attachment_refs, audit;
//!    a Card also run_results and card_eval_runs (each used run, its content_hash now)
//! 9. commit → 201 { id, version_id, seq, label, content_hash, changed[], badges[] }
//! ```
//!
//! The row lock in step 1 is what makes `seq` gap-free under concurrent
//! posts to the same name. Different names do not contend. Steps 4 and 5
//! run inside the same transaction, so the version that lands has seen
//! the attachment states and relation targets it was checked against.
//!
//! # A Card's runs (step 5b)
//!
//! A Card judges runs, and the hub remembers which runs it used and what
//! they were when it did. After the relations resolve, for every
//! `core/uses_eval` edge that resolved to a version of an Eval record the
//! writer may see (resolution only reaches such versions), the Eval
//! record's row is locked `FOR SHARE` and the Card's *used set* for that
//! record is fixed: the union of `attrs.runs` over the edges into the
//! record, or, when none of them carries `attrs.runs`, every run of the
//! record neither archived nor tombstoned now. An edge that stayed
//! unresolved (external, not yet existing, or not visible to the writer)
//! has no used set. The rule and the lock order are
//! `crate::used_set`'s, shared with [`crate::relations::add`]:
//!
//! ```text
//! Card record  FOR UPDATE (step 1)  →  Eval records FOR SHARE, by id  (step 5b)
//! run write:   Eval record FOR UPDATE, no Card lock
//! ```
//!
//! so a run write waits for a Card that is reading its Eval's runs and the
//! reverse, never in a cycle, and `card_eval_runs` records the hashes the
//! rows had when the Card committed.
//!
//! Step 5 resolved the edges, and read each Eval's visibility, without a
//! lock. `set_visibility` updates the record row under `FOR UPDATE`, so
//! the share lock of step 5b waits for one in flight and then reads the
//! visibility that holds until this Card commits. Step 5b re-checks it
//! there: an edge into an Eval the writer may no longer see is demoted to
//! unresolved (stored as text, no used set, no `refs_resolved`), exactly
//! as if step 5 had found it private, so a Card never fixes a used set in,
//! or learns run existence from, an Eval its writer cannot see.
//!
//! `body.run_results[]` is read here, from the body: the server extracts
//! `results[]` into [`NewVersion::results`], but a Card's judgements are
//! checked against state only the store has, so [`NewVersion`] carries no
//! field for them. Each element's `eval` (`{ns}/{name}`) is matched to the
//! record of a resolved edge; its `run_id` must be in that record's used
//! set. The refusals, all collected into
//! [`StoreError::CardRunsRejected`]:
//!
//! | Case                                                         | Code                  | Path                              |
//! | ------------------------------------------------------------ | --------------------- | --------------------------------- |
//! | an `attrs.runs` element with no row in the Eval              | `run_unknown`         | `/relations/{j}/attrs/runs/{k}`   |
//! | `run_results[].eval` resolved to nothing (external, absent, not visible) | `run_unknown` | `/run_results/{i}/run_id`   |
//! | `run_results[].run_id` with no row                           | `run_unknown`         | `/run_results/{i}/run_id`         |
//! | a row, outside the used set                                  | `run_not_in_used_set` | `/run_results/{i}/run_id`         |
//!
//! A run of an Eval the writer may not see, a run of an Eval that does not
//! exist and a run missing from a visible Eval produce the same entry,
//! byte for byte. Archived and tombstoned runs have rows and so may be
//! named; a Card's judgement outlives the run it judged.
//! `run_results_eval_unknown` (an `eval` that is no `core/uses_eval`
//! target of the Card at all) and the count limit are checked before the
//! store, by `evalhub_core` and the server.
//!
//! At step 8 the elements become `run_results` rows (position, Eval record,
//! run, metric, value, label, `by`) and the used sets `card_eval_runs`
//! rows. The idempotent hit (step 3) returns before step 5, so re-posting
//! a Card body fixes nothing again: the version, and its used set, are the
//! ones already stored.
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
//! # The `evalhub.eval/1.0` arm
//!
//! Release 0.2.0 still accepts an Eval body declaring `evalhub.eval/1.0`
//! (header and `runs[]` in one body); removing this arm is the 0.3.0
//! change. [`ingest`] converts it before step 1, with
//! `evalhub_core::eval::split_v1`, the one implementation of the
//! conversion:
//!
//! ```text
//! 1.0 body ──split_v1──▶ 2.0 header ──▶ steps 1–8 above, as for a 2.0 post
//!                    └─▶ runs[]     ──▶ crate::runs::write_runs, the put_batch code,
//!                                       after step 3 or 8, same transaction, same lock
//! ```
//!
//! The stored body is the 2.0 header and the returned `content_hash` is
//! the header's, computed here (the caller's `content_hash` of the 1.0
//! body is not used). The caller's fingerprints, relations and attachment
//! rows were extracted from the 1.0 body and are the header's too: the
//! conversion changes only `schema` and `runs`. The runs are already
//! materialised from the posted header, so the run write's own
//! materialisation copies nothing.
//!
//! Idempotency holds on both halves separately. Re-posting the same 1.0
//! body hits step 3 for the header and `unchanged` for every run: no
//! version, no run row, no audit row. Re-posting it with one more run hits
//! step 3 for the header and creates one run row, so a 0.1.x client that
//! appends runs no longer appends header versions. A changed header is a
//! new version as always. A refused run refuses the whole post
//! ([`StoreError::RunsRejected`], indexed by position in the posted
//! `runs[]`); a `run_id` repeated inside `runs[]` is not refused (0.1.x
//! accepted it; the last element wins and [`Converted`] lists it).
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

use evalhub_core::eval::EvalSchema;

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
    /// `relations[]`, targets parsed, in body order: an edge's position
    /// here is the `j` of `/relations/{j}/attrs/runs/{k}` when a Card's
    /// used set is refused (see "A Card's runs").
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

/// What the ingest of an `evalhub.eval/1.0` body did besides the header.
/// See "The `evalhub.eval/1.0` arm" in the module doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    /// The schema the body declared: always `evalhub.eval/1.0` in release
    /// 0.2.0. The stored header declares `evalhub.eval/2.0`.
    pub converted_from: &'static str,
    /// The runs split off the body, as [`crate::runs::put_batch`] reports
    /// them: one entry per distinct `run_id`, in the order each id first
    /// appeared in `runs[]`, and the record's `runs_hash` after the write.
    pub runs: crate::runs::RunsWritten,
    /// Every `run_id` that appeared more than once in `runs[]`; the last
    /// element carrying it is the one written. 0.1.x accepted such bodies,
    /// so the ingest does not refuse them; the server may report them.
    pub duplicate_run_ids: Vec<String>,
}

/// Result of [`ingest`]: the header outcome and, for an
/// `evalhub.eval/1.0` body, what happened to its runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    /// The header version, new or existing.
    pub outcome: CreateOutcome,
    /// `Some` exactly when the body declared `evalhub.eval/1.0`.
    pub converted: Option<Converted>,
}

/// Append a version to `{type}/{ns}/{name}`, creating the name if needed,
/// in one transaction. [`ingest`] without the conversion report; see it
/// for the contract.
pub async fn create_or_append(
    pool: &PgPool,
    new: NewVersion<'_>,
    new_ids: impl FnOnce() -> (Uuid, Uuid),
    badges_for: &(dyn Fn(&IngestFacts) -> Vec<String> + Send + Sync),
) -> Result<CreateOutcome, StoreError> {
    Ok(ingest(pool, new, new_ids, badges_for).await?.outcome)
}

/// Append a version to `{type}/{ns}/{name}`, creating the name if needed,
/// in one transaction. See the module doc for the sequence, and "The
/// `evalhub.eval/1.0` arm" for an Eval body that declares
/// `evalhub.eval/1.0`: its runs are split off and written as run rows in
/// the same transaction, the stored body and the returned `content_hash`
/// are the 2.0 header's (`new.content_hash` is not used for it), and
/// [`Ingested::converted`] reports the runs.
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
/// version of this record, [`StoreError::RunsRejected`] when a run of a
/// 1.0 body is refused, [`StoreError::CardRunsRejected`] when a Card's
/// used set or `run_results[]` names a run it may not (see "A Card's
/// runs" in the module doc), [`StoreError::Query`] otherwise. Nothing is
/// written on any error.
pub async fn ingest(
    pool: &PgPool,
    new: NewVersion<'_>,
    new_ids: impl FnOnce() -> (Uuid, Uuid),
    // `Send + Sync` so that a caller holding this reference across the
    // transaction's awaits still has a `Send` future, which is what an
    // axum handler requires.
    badges_for: &(dyn Fn(&IngestFacts) -> Vec<String> + Send + Sync),
) -> Result<Ingested, StoreError> {
    // The 1.0 arm: split before anything else, so that every step below
    // sees the 2.0 header as the body.
    let split = (new.record_type == RecordType::Eval
        && evalhub_core::eval::declared_schema(new.body) == Some(EvalSchema::V1))
    .then(|| evalhub_core::eval::split_v1(new.body));
    let header_owned: Option<(Value, [u8; 32])> = match &split {
        Some(split) => {
            let (bytes, hash) = evalhub_core::canonical::hash_value(&split.header)?;
            let canonical: Value = serde_json::from_slice(&bytes).map_err(|e| {
                StoreError::Canonical(evalhub_core::canonical::CanonicalError::Serialize(e))
            })?;
            Some((canonical, *hash.as_bytes()))
        }
        None => None,
    };
    let (body, header_hash): (&Value, &[u8; 32]) = match &header_owned {
        Some((body, hash)) => (body, hash),
        None => (new.body, new.content_hash),
    };
    let posted = new.body;
    let new = NewVersion {
        body,
        content_hash: header_hash,
        ..new
    };

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

    // Step 3: idempotent hit. A 1.0 body still writes its runs: the
    // header is the same, the runs may not be.
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
        let converted = match &split {
            Some(split) => {
                Some(write_converted_runs(&mut tx, record_id, &new, split, posted).await?)
            }
            None => None,
        };
        tx.commit().await?;
        return Ok(Ingested {
            outcome: CreateOutcome::Existing(meta),
            converted,
        });
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
    let mut resolved: Vec<Option<ResolvedVersion>> = Vec::with_capacity(new.relations.len());
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
    // Step 5b: a Card's used sets and its `run_results[]`, against the
    // Evals its `core/uses_eval` edges resolved to.
    // It may demote an edge to unresolved (an Eval made private since it
    // resolved), so `all_refs_resolved` is recomputed after it.
    let card_runs = if new.record_type == RecordType::Card {
        let card_runs = fix_card_runs(&mut tx, &new, &mut resolved, &readable_ns).await?;
        all_refs_resolved = new
            .relations
            .iter()
            .zip(&resolved)
            .all(|(rel, r)| r.is_some() || matches!(rel.target, RelationTarget::External(_)));
        Some(card_runs)
    } else {
        None
    };
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
    if let Some(card_runs) = &card_runs {
        insert_run_results(&mut tx, version_id, &card_runs.run_results).await?;
        crate::used_set::insert(&mut tx, version_id, &card_runs.used).await?;
    }
    for (rel, resolved) in new.relations.iter().zip(&resolved) {
        let to_version_id = resolved.as_ref().map(|r| r.version_id);
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
            to_version_id,
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

    // The runs of a 1.0 body, after the header they materialise from.
    let converted = match &split {
        Some(split) => Some(write_converted_runs(&mut tx, record_id, &new, split, posted).await?),
        None => None,
    };

    // Step 9.
    tx.commit().await?;
    Ok(Ingested {
        outcome: CreateOutcome::Created {
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
        },
        converted,
    })
}

/// Write the runs [`evalhub_core::eval::split_v1`] split off a 1.0 body,
/// through the same code as [`crate::runs::put_batch`], inside the
/// ingest's transaction and lock. `new.body` is the stored 2.0 header;
/// the runs were already materialised from it, so the write's own
/// materialisation copies nothing. `posted` is the 1.0 body as posted; a
/// refused run is reported with its position in its `runs[]` (the last
/// element with that id, the one the conversion kept).
async fn write_converted_runs(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    record_id: Uuid,
    new: &NewVersion<'_>,
    split: &evalhub_core::eval::SplitV1,
    posted: &Value,
) -> Result<Converted, StoreError> {
    let inputs: Vec<crate::runs::RunInput<'_>> = split
        .runs
        .iter()
        .map(|(run_id, body)| crate::runs::RunInput { run_id, body })
        .collect();
    let written = crate::runs::write_runs(
        tx, record_id, new.ns, new.name, new.body, &inputs, new.actor,
    )
    .await;
    let written = match written {
        Ok(w) => w,
        Err(StoreError::RunsRejected(mut rejections)) => {
            // Positions in `split.runs` → positions in the posted `runs[]`.
            let posted: Vec<Option<&str>> = posted
                .get("runs")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|e| e.get("run_id").and_then(Value::as_str))
                        .collect()
                })
                .unwrap_or_default();
            for r in &mut rejections {
                if let Some(i) = posted.iter().rposition(|id| *id == Some(r.run_id.as_str())) {
                    r.index = i;
                }
            }
            return Err(StoreError::RunsRejected(rejections));
        }
        Err(e) => return Err(e),
    };
    Ok(Converted {
        converted_from: EvalSchema::V1.id(),
        runs: written,
        duplicate_run_ids: split.duplicate_run_ids.clone(),
    })
}

/// One `run_results[]` element as a row of `run_results`.
struct RunResultRow {
    ordinal: i32,
    eval_record_id: Uuid,
    run_id: String,
    metric: String,
    value: Option<f64>,
    label: Option<String>,
    by: Option<Value>,
}

/// What step 5b fixed for a Card version, for step 8 to write.
struct CardRuns {
    /// The used set per Eval record.
    used: BTreeMap<Uuid, crate::used_set::UsedSet>,
    /// `run_results[]`, every element inside its used set.
    run_results: Vec<RunResultRow>,
}

/// Step 5b for a Card: lock the Evals its `core/uses_eval` edges resolved
/// to (`FOR SHARE`, see [`crate::used_set`]), fix a used set per Eval
/// record, and check `body.run_results[]` against them. Every refusal is
/// collected; any refusal is [`StoreError::CardRunsRejected`].
///
/// `resolved` is aligned with `new.relations`; an edge's position there is
/// the `j` of `/relations/{j}/attrs/runs/{k}`. An edge that resolved to
/// nothing, or to a version that is not an Eval's, has no used set. A
/// `run_results[].eval` is matched by its `{ns}/{name}` to the records of
/// the resolved edges; one that matches none resolved to nothing, and is
/// `run_unknown` like a missing run.
async fn fix_card_runs(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    new: &NewVersion<'_>,
    resolved: &mut [Option<ResolvedVersion>],
    readable_ns: &[String],
) -> Result<CardRuns, StoreError> {
    // Lock the Evals the `core/uses_eval` edges resolved to, and re-check
    // under the lock that the writer may still see each: step 5 read the
    // visibility without a lock, and a `set_visibility` committed since
    // must not let this Card fix a used set in an Eval its writer can no
    // longer see (run existence would become observable). An edge into
    // such an Eval is demoted to unresolved, exactly as if step 5 had
    // found it private.
    let candidates: std::collections::BTreeSet<Uuid> = new
        .relations
        .iter()
        .zip(resolved.iter())
        .filter_map(|(rel, target)| {
            let target = target.as_ref()?;
            (rel.relation_type == crate::relations::USES_EVAL
                && target.record_type == RecordType::Eval.as_str())
            .then_some(target.record_id)
        })
        .collect();
    let visible = crate::used_set::lock_evals(tx, &candidates, readable_ns).await?;
    for target in resolved.iter_mut() {
        if target
            .as_ref()
            .is_some_and(|t| candidates.contains(&t.record_id) && !visible.contains(&t.record_id))
        {
            *target = None;
        }
    }

    let mut uses = Vec::new();
    let mut eval_records: BTreeMap<String, Uuid> = BTreeMap::new();
    for (j, (rel, target)) in new.relations.iter().zip(resolved.iter()).enumerate() {
        let (Some(target), RelationTarget::Version { ns, name, .. }) = (target, rel.target) else {
            continue;
        };
        if rel.relation_type != crate::relations::USES_EVAL
            || target.record_type != RecordType::Eval.as_str()
        {
            continue;
        }
        eval_records.insert(format!("{ns}/{name}"), target.record_id);
        uses.push(crate::used_set::UsesEval {
            eval_record_id: target.record_id,
            runs: crate::used_set::UsesEval::runs_of(rel.attrs),
            runs_path: Some(format!("/relations/{j}/attrs/runs")),
        });
    }

    let items: Vec<(usize, &Value)> = new
        .body
        .get("run_results")
        .and_then(Value::as_array)
        .map(|a| a.iter().enumerate().collect())
        .unwrap_or_default();
    if uses.is_empty() && items.is_empty() {
        return Ok(CardRuns {
            used: BTreeMap::new(),
            run_results: Vec::new(),
        });
    }

    let mut errors = Vec::new();
    let used = crate::used_set::fix(tx, &uses, &mut errors).await?;

    // The elements the server's validation let through carry the three
    // strings; anything else is skipped rather than guessed at.
    let mut refs = Vec::with_capacity(items.len());
    let mut rows = Vec::with_capacity(items.len());
    for (i, item) in items {
        let (Some(eval), Some(run_id), Some(metric)) = (
            item.get("eval").and_then(Value::as_str),
            item.get("run_id").and_then(Value::as_str),
            item.get("metric").and_then(Value::as_str),
        ) else {
            continue;
        };
        let eval_record_id = eval_records.get(eval).copied();
        refs.push(crate::used_set::RunResultRef {
            index: i,
            eval_record_id,
            run_id,
        });
        if let (Some(eval_record_id), Ok(ordinal)) = (eval_record_id, i32::try_from(i)) {
            rows.push(RunResultRow {
                ordinal,
                eval_record_id,
                run_id: run_id.to_owned(),
                metric: metric.to_owned(),
                value: item.get("value").and_then(Value::as_f64),
                label: item.get("label").and_then(Value::as_str).map(str::to_owned),
                by: item.get("by").filter(|b| !b.is_null()).cloned(),
            });
        }
    }
    crate::used_set::check_run_results(tx, &refs, &used, &mut errors).await?;
    if !errors.is_empty() {
        crate::used_set::sort_entries(&mut errors);
        return Err(StoreError::CardRunsRejected(errors));
    }
    Ok(CardRuns {
        used,
        run_results: rows,
    })
}

/// Step 8 for a Card's `run_results[]`: one row per element, in one
/// statement.
async fn insert_run_results(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    version_id: Uuid,
    rows: &[RunResultRow],
) -> Result<(), StoreError> {
    if rows.is_empty() {
        return Ok(());
    }
    let ordinals: Vec<i32> = rows.iter().map(|r| r.ordinal).collect();
    let evals: Vec<Uuid> = rows.iter().map(|r| r.eval_record_id).collect();
    let run_ids: Vec<String> = rows.iter().map(|r| r.run_id.clone()).collect();
    let metrics: Vec<String> = rows.iter().map(|r| r.metric.clone()).collect();
    let values: Vec<Option<f64>> = rows.iter().map(|r| r.value).collect();
    let labels: Vec<Option<String>> = rows.iter().map(|r| r.label.clone()).collect();
    let bys: Vec<Option<Value>> = rows.iter().map(|r| r.by.clone()).collect();
    sqlx::query!(
        "INSERT INTO run_results (version_id, ordinal, eval_record_id, run_id, metric, value, label, by)
         SELECT $1, o, e, i, m, v, l, b
         FROM UNNEST($2::int4[], $3::uuid[], $4::text[], $5::text[], $6::float8[], $7::text[], $8::jsonb[])
           AS t (o, e, i, m, v, l, b)",
        version_id,
        &ordinals,
        &evals,
        &run_ids,
        &metrics,
        &values as &[Option<f64>],
        &labels as &[Option<String>],
        &bys as &[Option<Value>],
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
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

/// A relation target resolved at ingest: the version, and the record it
/// belongs to (the used set of a `core/uses_eval` edge is fixed per Eval
/// record, not per version).
#[derive(Debug, Clone)]
struct ResolvedVersion {
    version_id: Uuid,
    record_id: Uuid,
    record_type: String,
}

/// `{ns}/{name}@{seq}`, of the given kind or, when none is given, of
/// whichever kind has it if exactly one does. Only versions visible to
/// `readable_ns` are candidates, so an invisible one neither resolves nor
/// makes a visible one ambiguous.
async fn resolve_version(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    ns: &str,
    name: &str,
    seq: i32,
    record_type: Option<RecordType>,
    readable_ns: &[String],
) -> Result<Option<ResolvedVersion>, StoreError> {
    let rows = sqlx::query!(
        "SELECT v.version_id, v.record_id, r.type AS record_type, r.visibility
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
    let mut visible: Vec<ResolvedVersion> = rows
        .into_iter()
        .filter(|r| crate::relations::visible_to(&r.visibility, ns, readable_ns))
        .map(|r| ResolvedVersion {
            version_id: r.version_id,
            record_id: r.record_id,
            record_type: r.record_type,
        })
        .collect();
    Ok(if visible.len() == 1 {
        visible.pop()
    } else {
        None
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
/// visibility, or [`StoreError::RecordNotFound`]. The same `FOR UPDATE`
/// the ingest takes; [`crate::runs`] takes it before every run write.
pub(crate) async fn lock_record(
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

/// Runs of an Eval by `status`, over the runs that are neither archived
/// nor deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunsByStatus {
    /// `status = ok`.
    pub ok: i64,
    /// `status = error`.
    pub error: i64,
    /// `status = skipped`.
    pub skipped: i64,
}

/// The `runs` block of an Eval's envelope. See [`runs_summary`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunsSummary {
    /// Runs that are neither archived nor deleted.
    pub count: i64,
    /// `count`, split by `status`.
    pub by_status: RunsByStatus,
    /// Archived runs that are not deleted. `None` unless the reader is a
    /// member of the namespace.
    pub archived: Option<i64>,
    /// Deleted (tombstoned) runs, archived or not. `None` unless the
    /// reader is a member of the namespace.
    pub deleted: Option<i64>,
    /// `records.runs_hash`: the digest over every run, archived and
    /// deleted included, the same for every reader. The hash of the empty
    /// set (`sha256("[]")`) when no run was ever written.
    pub runs_hash: Vec<u8>,
}

/// The run summary of the Eval `record_id` for its envelope. `member` says
/// whether the reader is a member of the record's namespace; it decides
/// only whether `archived` and `deleted` are filled.
///
/// The runs belong to the record, not to a version, so the summary is the
/// same whichever `@seq` the reader asked for. The caller has already
/// resolved the record under the visibility rule (and so knows it has a
/// live header); this reads by id and filters nothing. An unknown id reads
/// as zero runs. Cost: one aggregate over the record's runs (the primary
/// key's prefix) and one row read.
pub async fn runs_summary(
    pool: &PgPool,
    record_id: Uuid,
    member: bool,
) -> Result<RunsSummary, StoreError> {
    let row = sqlx::query!(
        r#"SELECT
             COUNT(*) FILTER (WHERE archived_at IS NULL AND tombstoned_at IS NULL) AS "count!",
             COUNT(*) FILTER (WHERE archived_at IS NULL AND tombstoned_at IS NULL AND status = 'ok') AS "ok!",
             COUNT(*) FILTER (WHERE archived_at IS NULL AND tombstoned_at IS NULL AND status = 'error') AS "error!",
             COUNT(*) FILTER (WHERE archived_at IS NULL AND tombstoned_at IS NULL AND status = 'skipped') AS "skipped!",
             COUNT(*) FILTER (WHERE archived_at IS NOT NULL AND tombstoned_at IS NULL) AS "archived!",
             COUNT(*) FILTER (WHERE tombstoned_at IS NOT NULL) AS "deleted!",
             (SELECT runs_hash FROM records WHERE id = $1) AS runs_hash
           FROM runs WHERE record_id = $1"#,
        record_id,
    )
    .fetch_one(pool)
    .await?;
    let runs_hash = match row.runs_hash {
        Some(h) => h,
        None => crate::runs::empty_runs_hash()?,
    };
    Ok(RunsSummary {
        count: row.count,
        by_status: RunsByStatus {
            ok: row.ok,
            error: row.error,
            skipped: row.skipped,
        },
        archived: member.then_some(row.archived),
        deleted: member.then_some(row.deleted),
        runs_hash,
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
