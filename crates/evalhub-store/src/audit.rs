//! Append-only audit rows.
//!
//! `audit (id, at, actor_user_id, actor_token_id, ns, action, subject,
//! detail jsonb)`. Written in the same transaction as the action it
//! records, for: record creation, version append, tombstone, settings
//! change, label change, org membership change, token issue and revoke,
//! registry writes, and every run write that changes something. Reads are
//! `GET /audit?ns=&cursor=`, restricted to namespaces the caller holds
//! `admin` on.
//!
//! Rows are never updated or deleted by the application. The table has no
//! `UPDATE` or `DELETE` grant for the application role in the migration,
//! so this is enforced by Postgres, not by discipline.
//!
//! # Actions
//!
//! | `action`              | `subject`                          | written by                          |
//! | --------------------- | ---------------------------------- | ----------------------------------- |
//! | `record.create`       | `{type}/{ns}/{name}@1`             | `records::create_or_append`         |
//! | `version.append`      | `{type}/{ns}/{name}@{seq}`         | `records::create_or_append`         |
//! | `version.tombstone`   | `{type}/{ns}/{name}@{seq}`         | `records::tombstone`                |
//! | `label.set`           | `{type}/{ns}/{name}@{seq}`         | `records::set_label`                |
//! | `settings.visibility` | `{type}/{ns}/{name}`               | `records::set_visibility`           |
//! | `token.create`        | `{token_id}`                       | `auth::create_token`, one row per namespace |
//! | `token.revoke`        | `{token_id}`                       | `auth::revoke_token`, one row per namespace |
//! | `org.create`          | `{ns}`                             | `auth::create_org`                  |
//! | `org.member.add`      | `{login}`                          | `auth::add_org_member`              |
//! | `org.member.remove`   | `{login}`                          | `auth::remove_org_member`           |
//! | `run.create`          | `eval/{ns}/{name}/runs/{run_id}`   | `runs::put` / `put_batch`, and the 1.0 ingest; one row per run created |
//! | `run.update`          | `eval/{ns}/{name}/runs/{run_id}`   | as `run.create`; one row per run overwritten (not for an unchanged one) |
//! | `run.archive`         | `eval/{ns}/{name}/runs/{run_id}`   | `runs::archive`                     |
//! | `run.unarchive`       | `eval/{ns}/{name}/runs/{run_id}`   | `runs::unarchive`                   |
//! | `run.delete`          | `eval/{ns}/{name}/runs/{run_id}`   | `runs::tombstone`                   |
//! | `migration.runs_split` | `eval/{ns}/{name}@{seq}`          | `data_migrations` (`0003_runs_split`); one row per rewritten version, `{ version_id, old_content_hash, new_content_hash }` |
//! | `migration.card_runs_unmatched` | `card/{ns}/{name}@{seq}` | `data_migrations` (`0003_runs_split`); one row per (Card version, Eval) whose used `run_id`s have no row, `{ card_version_id, eval, run_ids }` |
//!
//! The two `migration.*` actions are written by `evalhub migrate`, not by a
//! request, so both actor columns are NULL, as for the CLI bootstrap.
//!
//! `ns` is the namespace the action concerns; it is what the read side
//! filters on, so an action touching several namespaces writes one row
//! per namespace.
//!
//! # Paging
//!
//! [`list`] pages newest first by `id`, which is a `bigserial`, so the
//! cursor is simply the smallest `id` on the previous page.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::StoreError;
use crate::records::Actor;

/// One audit row as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRow {
    /// Monotonic row id; the paging cursor.
    pub id: i64,
    /// When the action happened.
    pub at: DateTime<Utc>,
    /// The user behind the action, if any (the CLI bootstrap has none).
    pub actor_user_id: Option<Uuid>,
    /// The token presented, if any.
    pub actor_token_id: Option<Uuid>,
    /// The namespace the action concerns.
    pub ns: Option<String>,
    /// What happened; see the module table.
    pub action: String,
    /// What it happened to.
    pub subject: Option<String>,
    /// Action-specific facts.
    pub detail: Option<Value>,
}

/// What a write hands to [`append`].
pub(crate) struct NewAudit<'a> {
    pub actor: Actor,
    pub ns: Option<&'a str>,
    pub action: &'a str,
    pub subject: Option<&'a str>,
    pub detail: Option<Value>,
}

/// Append one row inside the caller's transaction, so the row exists
/// exactly when the action it records does.
pub(crate) async fn append(
    tx: &mut Transaction<'_, Postgres>,
    a: NewAudit<'_>,
) -> Result<(), StoreError> {
    sqlx::query!(
        "INSERT INTO audit (actor_user_id, actor_token_id, ns, action, subject, detail)
         VALUES ($1, $2, $3, $4, $5, $6)",
        a.actor.user_id,
        a.actor.token_id,
        a.ns,
        a.action,
        a.subject,
        a.detail,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The audit rows for `ns`, newest first. `before_id` is the cursor
/// returned by the previous call; `limit` rows are returned and the
/// cursor is `Some` when more remain. The caller has already checked
/// that it holds `admin` on `ns`.
pub async fn list(
    pool: &PgPool,
    ns: &str,
    before_id: Option<i64>,
    limit: u32,
) -> Result<(Vec<AuditRow>, Option<i64>), StoreError> {
    let fetch = i64::from(limit) + 1;
    let rows = sqlx::query!(
        "SELECT id, at, actor_user_id, actor_token_id, ns, action, subject, detail
         FROM audit
         WHERE ns = $1 AND ($2::bigint IS NULL OR id < $2)
         ORDER BY id DESC
         LIMIT $3",
        ns,
        before_id,
        fetch,
    )
    .fetch_all(pool)
    .await?;
    let mut rows: Vec<AuditRow> = rows
        .into_iter()
        .map(|r| AuditRow {
            id: r.id,
            at: r.at,
            actor_user_id: r.actor_user_id,
            actor_token_id: r.actor_token_id,
            ns: r.ns,
            action: r.action,
            subject: r.subject,
            detail: r.detail,
        })
        .collect();
    let next = if rows.len() > limit as usize {
        rows.truncate(limit as usize);
        rows.last().map(|r| r.id)
    } else {
        None
    };
    Ok((rows, next))
}
