//! Record handlers: create/append, read, list, versions, label, settings,
//! tombstone.
//!
//! Response envelope for a version (what the hub adds around the client's
//! record):
//!
//! ```text
//! {
//!   "id", "version_id", "seq", "label", "content_hash", "created_at",
//!   "changed": [...], "badges": [...],
//!   "record": { ...the canonical body... },
//!   "fingerprints": { "model": "…", ... },      // expand=fingerprints
//!   "relations": [...],                          // expand=relations
//!   "tombstone": { "at", "reason", "note" }      // when tombstoned; record is absent
//! }
//! ```
//!
//! `POST` semantics — idempotent on `content_hash`, new `seq` otherwise —
//! are in `evalhub_store::records`. `?label=` on `POST` labels the new
//! version in the same transaction (`409 label_in_use` if taken).
//!
//! # What `POST` checks today
//!
//! The body is parsed into the record type (`Card` / `Eval`), which
//! rejects unknown keys outside `ext`, wrong types and missing required
//! keys; each failure is a `422` with one `schema` entry. The full
//! validator (`evalhub_core::validate`: every error collected, the
//! semantic rules, the 2^53 check) replaces this in M2; the response
//! shape does not change. What is stored is the client's JSON,
//! canonicalised — never the re-serialised struct.
//!
//! # Addressing
//!
//! `GET …/{name}` is the latest live version; `GET …/{name}@{seq}` one
//! version. `@{label}` addressing arrives with labels in M2 and is `404`
//! until then. `POST` always targets the name.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use evalhub_core::canonical::hash_value;
use evalhub_core::{ContentHash, RecordId, VersionId};
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_store::auth::Scope;
use evalhub_store::records::{self, CreateOutcome, NewVersion, RecordType, VersionMeta};

use crate::auth::{Auth, MaybeAuth};
use crate::error::ApiError;
use crate::state::AppState;

/// Path of a record: `/{cards|evals}/{ns}/{name}`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordPath {
    /// Namespace: a user login or an organisation slug.
    pub ns: String,
    /// Record name, optionally followed by `@{seq}` to address one version.
    pub name: String,
}

/// Query string of `POST`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PostQuery {
    /// Label to attach to the new version. Never purely numeric.
    pub label: Option<String>,
}

/// What the hub adds around a stored version.
#[derive(Debug, Serialize, JsonSchema)]
pub struct VersionEnvelope {
    /// Identifier of the named record, stable across versions (ULID).
    pub id: String,
    /// Identifier of this version (ULID). Relations point at these.
    pub version_id: String,
    /// Sequence number within the name, from 1, gap-free.
    pub seq: i32,
    /// Client-chosen label of this version, if any.
    pub label: Option<String>,
    /// Hex sha256 of the canonical record.
    pub content_hash: String,
    /// When the hub stored this version.
    pub created_at: DateTime<Utc>,
    /// Top-level keys whose value differs from the previous version.
    pub changed: Vec<String>,
    /// Badges awarded at ingest.
    pub badges: Vec<String>,
    /// The canonical record. Absent in `POST` responses and for tombstones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Value>,
}

impl VersionEnvelope {
    fn from_meta(meta: VersionMeta, record: Option<Value>) -> Self {
        // The column is a 32-byte sha256 by construction; a row that is
        // not (impossible through this code) is reported as empty rather
        // than crashing the read path.
        let hash = meta
            .content_hash
            .as_slice()
            .try_into()
            .map(ContentHash::from_bytes)
            .map(|h| h.as_hex())
            .unwrap_or_default();
        Self {
            id: RecordId::from_uuid(meta.record_id).to_string(),
            version_id: VersionId::from_uuid(meta.version_id).to_string(),
            seq: meta.seq,
            label: meta.label,
            content_hash: hash,
            created_at: meta.created_at,
            changed: meta.changed,
            badges: meta.badges,
            record,
        }
    }
}

fn schema_error(hint: String) -> ApiError {
    ApiError::Validation(vec![ErrorEntry {
        path: String::new(),
        code: ErrorCode::Schema,
        hint: Some(hint),
    }])
}

/// The structural check of M1: the body must parse as the record type and
/// declare the matching `schema`.
fn check_shape(record_type: RecordType, body: &Value) -> Result<(), ApiError> {
    let declared = body.get("schema").and_then(Value::as_str);
    let expected = match record_type {
        RecordType::Card => {
            serde_json::from_value::<evalhub_schema::card::Card>(body.clone())
                .map_err(|e| schema_error(e.to_string()))?;
            evalhub_schema::CARD_SCHEMA
        }
        RecordType::Eval => {
            serde_json::from_value::<evalhub_schema::eval::Eval>(body.clone())
                .map_err(|e| schema_error(e.to_string()))?;
            evalhub_schema::EVAL_SCHEMA
        }
    };
    if declared != Some(expected) {
        return Err(ApiError::Validation(vec![ErrorEntry {
            path: "schema".to_string(),
            code: ErrorCode::Schema,
            hint: Some(format!("expected \"{expected}\"")),
        }]));
    }
    Ok(())
}

async fn post(
    record_type: RecordType,
    state: AppState,
    caller: Auth,
    path: RecordPath,
    query: PostQuery,
    body: Value,
) -> Result<(StatusCode, Json<VersionEnvelope>), ApiError> {
    let Auth(caller) = caller;
    if !caller.allows(&path.ns, Scope::Write) {
        return Err(ApiError::Forbidden);
    }
    if path.name.contains('@') {
        return Err(ApiError::BadRequest);
    }
    if let Some(label) = &query.label
        && (label.is_empty() || label.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(ApiError::BadRequest);
    }
    check_shape(record_type, &body)?;

    let (canonical, hash) = hash_value(&body).map_err(|e| schema_error(e.to_string()))?;
    // Store the canonical form, re-parsed: key order and number formatting
    // are then exactly what the hash was computed over.
    let canonical: Value = serde_json::from_slice(&canonical)
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;

    let outcome = records::create_or_append(
        state.db()?,
        NewVersion {
            record_type,
            ns: &path.ns,
            name: &path.name,
            body: &canonical,
            content_hash: hash.as_bytes(),
            label: query.label.as_deref(),
            actor: caller.actor(),
        },
        || (RecordId::new().as_uuid(), VersionId::new().as_uuid()),
    )
    .await?;

    Ok(match outcome {
        CreateOutcome::Created(meta) => (
            StatusCode::CREATED,
            Json(VersionEnvelope::from_meta(meta, None)),
        ),
        CreateOutcome::Existing(meta) => {
            (StatusCode::OK, Json(VersionEnvelope::from_meta(meta, None)))
        }
    })
}

async fn get(
    record_type: RecordType,
    state: AppState,
    caller: MaybeAuth,
    path: RecordPath,
) -> Result<Json<VersionEnvelope>, ApiError> {
    let MaybeAuth(caller) = caller;
    let pool = state.db()?;
    let (name, seq) = match path.name.split_once('@') {
        None => (path.name.as_str(), None),
        Some((name, suffix)) => match suffix.parse::<i32>() {
            Ok(seq) if seq >= 1 => (name, Some(seq)),
            // A label, or garbage: labels are not addressable yet.
            _ => return Err(ApiError::NotFound),
        },
    };
    let stored = match seq {
        None => records::get_latest(pool, record_type, &path.ns, name, caller.namespaces()).await?,
        Some(seq) => {
            records::get_by_seq(pool, record_type, &path.ns, name, seq, caller.namespaces()).await?
        }
    }
    .ok_or(ApiError::NotFound)?;
    Ok(Json(VersionEnvelope::from_meta(stored.meta, stored.body)))
}

/// `POST /api/v1/cards/{ns}/{name}?label=` — append a Card version.
pub async fn post_card(
    State(state): State<AppState>,
    caller: Auth,
    Path(path): Path<RecordPath>,
    Query(query): Query<PostQuery>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<VersionEnvelope>), ApiError> {
    post(RecordType::Card, state, caller, path, query, body).await
}

/// `POST /api/v1/evals/{ns}/{name}?label=` — append an Eval version.
pub async fn post_eval(
    State(state): State<AppState>,
    caller: Auth,
    Path(path): Path<RecordPath>,
    Query(query): Query<PostQuery>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<VersionEnvelope>), ApiError> {
    post(RecordType::Eval, state, caller, path, query, body).await
}

/// `GET /api/v1/cards/{ns}/{name}[@{seq}]` — read a Card version.
pub async fn get_card(
    State(state): State<AppState>,
    caller: MaybeAuth,
    Path(path): Path<RecordPath>,
) -> Result<Json<VersionEnvelope>, ApiError> {
    get(RecordType::Card, state, caller, path).await
}

/// `GET /api/v1/evals/{ns}/{name}[@{seq}]` — read an Eval version.
pub async fn get_eval(
    State(state): State<AppState>,
    caller: MaybeAuth,
    Path(path): Path<RecordPath>,
) -> Result<Json<VersionEnvelope>, ApiError> {
    get(RecordType::Eval, state, caller, path).await
}
