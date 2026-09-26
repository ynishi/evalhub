//! `GET /audit?ns&cursor`, `admin` on `ns` required. Rows are append-only;
//! see `evalhub_store::audit`.
//!
//! The log answers "who changed what, when" for one namespace: record
//! creation and version appends, tombstones, label and visibility changes,
//! relation additions, token issue and revocation, organisation membership,
//! and every run write that changed something (`run.create`, `run.update`,
//! `run.archive`, `run.unarchive`, `run.delete`, subject
//! `eval/{ns}/{name}/runs/{run_id}`). The rows `evalhub migrate` writes
//! (`migration.runs_split` with each rewritten Eval version's old and new
//! `content_hash`, `migration.card_runs_unmatched`) carry the namespace
//! they concern and no actor, and are listed with the rest. The handler
//! filters by namespace only, never by action, so an action the store
//! adds appears here without a change. Rows are newest first and paged by
//! an opaque cursor, like every other listing.

use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use evalhub_schema::query::Page;
use evalhub_store::audit;
use evalhub_store::auth::Scope;

use crate::auth::Auth;
use crate::error::ApiError;
use crate::state::AppState;

/// Query string of `GET /audit`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AuditQuery {
    /// Namespace to read the log of. The caller needs `admin` on it.
    pub ns: String,
    /// Cursor from a previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// Page size, 1–200; default 50.
    pub limit: Option<u32>,
}

/// One entry of the log.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AuditEntry {
    /// When it happened.
    pub at: DateTime<Utc>,
    /// Login of the user behind the action, if there was one.
    pub actor: Option<String>,
    /// Identifier of the token presented, if one was.
    pub token_id: Option<String>,
    /// The namespace the action concerns.
    pub ns: Option<String>,
    /// What happened, such as `version.append` or `org.member.add`.
    pub action: String,
    /// What it happened to, such as `card/alice/single2@3`.
    pub subject: Option<String>,
    /// Action-specific facts.
    pub detail: Option<Value>,
}

/// `GET /api/v1/audit?ns&cursor&limit` — the namespace's audit log.
pub async fn list(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Page<AuditEntry>>, ApiError> {
    if !caller.is_admin(&query.ns) {
        return Err(ApiError::Forbidden);
    }
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let before_id = match &query.cursor {
        None => None,
        Some(c) => {
            let payload = state.cursors.verify(c).ok_or(ApiError::BadRequest)?;
            let text = String::from_utf8(payload).map_err(|_| ApiError::BadRequest)?;
            Some(text.parse::<i64>().map_err(|_| ApiError::BadRequest)?)
        }
    };
    let (rows, next) = audit::list(state.db()?, &query.ns, before_id, limit).await?;
    let items = rows
        .into_iter()
        .map(|r| AuditEntry {
            at: r.at,
            // The log stores ids; a login would go stale if a user were
            // renamed, which is why the id is what is written down.
            actor: r.actor_user_id.map(|u| u.to_string()),
            token_id: r.actor_token_id.map(|t| t.to_string()),
            ns: r.ns,
            action: r.action,
            subject: r.subject,
            detail: r.detail,
        })
        .collect();
    Ok(Json(Page {
        items,
        next_cursor: next.map(|id| state.cursors.sign(id.to_string().as_bytes())),
    }))
}

/// Scope the handler asks for, named here so the route table can cite it.
pub const REQUIRED_SCOPE: Scope = Scope::Admin;
