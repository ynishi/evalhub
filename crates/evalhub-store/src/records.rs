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
//! 4. seq = max(seq) + 1
//! 5. changed[] = top-level keys whose value differs from the previous version
//! 6. insert versions, fingerprints, results, relations, attachment_refs, audit
//! 7. commit → 201 { id, version_id, seq, label, content_hash, changed[], badges[] }
//! ```
//!
//! The row lock in step 1 is what makes `seq` gap-free under concurrent
//! posts to the same name. Different names do not contend.
//!
//! # Addressing
//!
//! A version is addressed as `@{seq}` or `@{label}`. `seq` is the hub's;
//! `label` is a client-chosen slug, unique within the name, re-pointable
//! with `PATCH .../label`, and **never purely numeric** so it cannot be
//! mistaken for a `seq`. No address is derived from `content_hash`.
//!
//! # `changed[]`
//!
//! Each version records which top-level keys differ from the version
//! before it (`results`, `counts`, `model`, …). It costs one comparison at
//! write time and lets the version list show at a glance that "version 4
//! changed the results" without diffing bodies. It is kept on tombstones.
//!
//! # Visibility
//!
//! `records.visibility` is `private` or `public`, set per name with
//! `PATCH .../settings`. Every read path in this module takes the caller's
//! namespaces and filters on them, so a private record is indistinguishable
//! from a nonexistent one (`404`) to anyone without access. There is no
//! query that returns a private record's existence.
//!
//! # What this module takes and returns
//!
//! The body arrives already canonicalised (`serde_json::Value`) with its
//! `content_hash` computed by `evalhub_core`; this module stores bytes and
//! compares hashes, it never re-derives them. Identifiers are `uuid::Uuid`
//! at this layer: the hub mints ULIDs and stores them in `uuid` columns,
//! and the conversion between the two spellings is the caller's.
//!
//! Only `versions` and `audit` are written today. `fingerprints`,
//! `results`, `relations` and `attachment_refs` join the same transaction
//! when the ingest path grows those steps.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

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

    fn parse(s: &str) -> Visibility {
        // The column has a CHECK constraint; anything else is a migration bug.
        if s == "public" {
            Visibility::Public
        } else {
            Visibility::Private
        }
    }
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

/// A version to append. The body is the canonical record and
/// `content_hash` is its sha256; both are computed by the caller.
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
    /// Badges awarded at ingest. Empty until the badge step exists.
    pub badges: Vec<String>,
    /// The record's visibility at read time.
    pub visibility: Visibility,
}

/// A version as read back: its metadata and, unless tombstoned, its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredVersion {
    /// The hub's facts about the version.
    pub meta: VersionMeta,
    /// The canonical body, `None` once tombstoned.
    pub body: Option<Value>,
}

/// Result of [`create_or_append`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    /// A new version was written (`201`).
    Created(VersionMeta),
    /// The body equals the latest version's; nothing was written (`200`).
    Existing(VersionMeta),
}

impl CreateOutcome {
    /// The version, whether new or existing.
    pub fn meta(&self) -> &VersionMeta {
        match self {
            CreateOutcome::Created(m) | CreateOutcome::Existing(m) => m,
        }
    }
}

/// Append a version to `{type}/{ns}/{name}`, creating the name if needed,
/// in one transaction. See the module doc for the sequence.
///
/// `new_ids` is called once and yields `(record_id, version_id)`; the
/// record id is used only when the name is new, the version id only when a
/// version is written. Errors: [`StoreError::NamespaceUnknown`] when `ns`
/// has no row, [`StoreError::LabelInUse`] when `label` already names a
/// version of this record, [`StoreError::Query`] otherwise.
pub async fn create_or_append(
    pool: &PgPool,
    new: NewVersion<'_>,
    new_ids: impl FnOnce() -> (Uuid, Uuid),
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

    // Step 4: next seq, counting tombstoned versions too so a seq is never
    // reused.
    let max_seq = sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(seq), 0) AS "max!" FROM versions WHERE record_id = $1"#,
        record_id,
    )
    .fetch_one(&mut *tx)
    .await?;
    let seq = max_seq + 1;

    // Step 5: changed[] against the previous live body.
    let changed = changed_keys(latest.as_ref().and_then(|l| l.body.as_ref()), new.body);

    // Step 6: the version row.
    let content_hash: &[u8] = new.content_hash;
    let inserted = sqlx::query!(
        "INSERT INTO versions (version_id, record_id, seq, label, content_hash, body, changed)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING created_at, badges",
        version_id,
        record_id,
        seq,
        new.label,
        content_hash,
        new.body,
        &changed,
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

    let action = if seq == 1 {
        "record.create"
    } else {
        "version.append"
    };
    let subject = format!("{record_type}/{}/{}@{seq}", new.ns, new.name);
    let detail = serde_json::json!({
        "version_id": version_id,
        "content_hash": hex_lower(new.content_hash),
    });
    sqlx::query!(
        "INSERT INTO audit (actor_user_id, actor_token_id, ns, action, subject, detail)
         VALUES ($1, $2, $3, $4, $5, $6)",
        new.actor.user_id,
        new.actor.token_id,
        new.ns,
        action,
        subject,
        detail,
    )
    .execute(&mut *tx)
    .await?;

    // Step 7.
    tx.commit().await?;
    Ok(CreateOutcome::Created(VersionMeta {
        record_id,
        version_id,
        seq,
        label: new.label.map(str::to_owned),
        content_hash: new.content_hash.to_vec(),
        created_at: row.created_at,
        changed,
        badges: row.badges,
        visibility,
    }))
}

/// The latest live version of `{type}/{ns}/{name}`, or `None` when the
/// name does not exist or is private and `caller_namespaces` does not
/// contain `ns`. The two cases are deliberately indistinguishable.
pub async fn get_latest(
    pool: &PgPool,
    record_type: RecordType,
    ns: &str,
    name: &str,
    caller_namespaces: &[String],
) -> Result<Option<StoredVersion>, StoreError> {
    let row = sqlx::query!(
        "SELECT r.id AS record_id, r.visibility,
                v.version_id, v.seq, v.label, v.content_hash, v.body, v.created_at, v.changed, v.badges
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
    Ok(row.map(|r| StoredVersion {
        meta: VersionMeta {
            record_id: r.record_id,
            version_id: r.version_id,
            seq: r.seq,
            label: r.label,
            content_hash: r.content_hash,
            created_at: r.created_at,
            changed: r.changed,
            badges: r.badges,
            visibility: Visibility::parse(&r.visibility),
        },
        body: r.body,
    }))
}

/// Version `@{seq}` of `{type}/{ns}/{name}`, with the same visibility
/// rule as [`get_latest`]. A tombstoned version is returned with `body:
/// None`.
pub async fn get_by_seq(
    pool: &PgPool,
    record_type: RecordType,
    ns: &str,
    name: &str,
    seq: i32,
    caller_namespaces: &[String],
) -> Result<Option<StoredVersion>, StoreError> {
    let row = sqlx::query!(
        "SELECT r.id AS record_id, r.visibility,
                v.version_id, v.seq, v.label, v.content_hash, v.body, v.created_at, v.changed, v.badges
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
    Ok(row.map(|r| StoredVersion {
        meta: VersionMeta {
            record_id: r.record_id,
            version_id: r.version_id,
            seq: r.seq,
            label: r.label,
            content_hash: r.content_hash,
            created_at: r.created_at,
            changed: r.changed,
            badges: r.badges,
            visibility: Visibility::parse(&r.visibility),
        },
        body: r.body,
    }))
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
}
