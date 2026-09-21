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
//! # The ingest pipeline
//!
//! `POST` runs the order the `evalhub_core` crate doc describes:
//! `validate` (every error collected into one `422`), canonical form and
//! `content_hash`, per-facet `fingerprints`, then the rows the store keeps
//! beside the body (`results`, `relations`, `attachment_refs`) are cut
//! from the canonical value, and `badges` are computed inside the store's
//! transaction from the facts it established (which references resolved,
//! what the registry knows). A referenced attachment that is not `ready`
//! rolls the whole write back with `409 attachment_missing`, one entry per
//! offending `attachments[i].sha256`. What is stored is the client's JSON,
//! canonicalised — never a re-serialised struct.
//!
//! # Addressing
//!
//! `GET …/{name}` is the latest live version; `GET …/{name}@{seq}` one
//! version by sequence number (a tombstoned one answers with `tombstone`
//! and no `record`); `GET …/{name}@{label}` the version the label points
//! at. A suffix that is all digits is a sequence number, anything else a
//! label. `POST` always targets the name; `PATCH …/label` and `DELETE`
//! target `{name}@{seq}`.
//!
//! # `expand`
//!
//! `?expand=fingerprints,badges,changed,relations`, comma-separated.
//! `badges` and `changed` are always present today; `fingerprints` adds
//! the hex map; `relations` is accepted and reserved for the relations
//! handlers (it is not populated by this module).
//!
//! # Listing
//!
//! `GET /{cards|evals}?ns&search&sort&cursor&limit` pages over records
//! with a live version, newest first by default. The cursor is the
//! store's keyset tuple, JSON-serialised and HMAC-signed by
//! [`crate::auth::CursorSigner`]; a cursor the hub did not sign is `400`.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use evalhub_core::canonical::hash_value;
use evalhub_core::validate::{RelationTarget as ParsedTarget, parse_relation_target};
use evalhub_core::{BadgeInput, ContentHash, RecordId, VersionId, compute_badges};
use evalhub_schema::RecordKind;
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_schema::query::Page;
use evalhub_store::auth::Scope;
use evalhub_store::error::StoreError;
use evalhub_store::records::{
    self, CreateOutcome, ListCursor, ListParams, ListSort, NewAttachmentRef, NewRelation,
    NewResult, NewVersion, RecordType, RelationTarget, StoredVersion, Tombstone, TombstoneReason,
    TombstoneRequest, VersionMeta, VersionSummary, Visibility,
};

use crate::auth::{Auth, MaybeAuth};
use crate::error::ApiError;
use crate::state::AppState;

/// Path of a record: `/{cards|evals}/{ns}/{name}`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordPath {
    /// Namespace: a user login or an organisation slug.
    pub ns: String,
    /// Record name, optionally followed by `@{seq}` or `@{label}` to
    /// address one version.
    pub name: String,
}

/// Query string of `POST`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct PostQuery {
    /// Label to attach to the new version. Never purely numeric.
    pub label: Option<String>,
}

/// Query string of `GET …/{name}`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetQuery {
    /// Comma-separated: `fingerprints`, `badges`, `changed`, `relations`.
    pub expand: Option<String>,
}

/// Query string of `GET /{cards|evals}`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListQuery {
    /// Only records in this namespace.
    pub ns: Option<String>,
    /// Case-insensitive substring of the name or the latest title.
    pub search: Option<String>,
    /// Sort order; default `created_desc`.
    pub sort: Option<SortParam>,
    /// Cursor from a previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// Page size, 1–200; default 50.
    pub limit: Option<u32>,
}

/// Sort orders of the list endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SortParam {
    /// Newest record first.
    CreatedDesc,
    /// Oldest record first.
    CreatedAsc,
    /// By name, ascending.
    NameAsc,
}

impl From<SortParam> for ListSort {
    fn from(s: SortParam) -> Self {
        match s {
            SortParam::CreatedDesc => ListSort::CreatedDesc,
            SortParam::CreatedAsc => ListSort::CreatedAsc,
            SortParam::NameAsc => ListSort::NameAsc,
        }
    }
}

/// Body of `PATCH …/{name}@{seq}/label`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LabelBody {
    /// The label to point at this version. Moved if it named another.
    pub label: String,
}

/// Body of `PATCH …/{name}/settings` and its response.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    /// `public` or `private`.
    pub visibility: VisibilityParam,
}

/// A record's visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityParam {
    /// Readable by anyone.
    Public,
    /// Readable only with a token covering the namespace.
    Private,
}

impl From<Visibility> for VisibilityParam {
    fn from(v: Visibility) -> Self {
        match v {
            Visibility::Public => VisibilityParam::Public,
            Visibility::Private => VisibilityParam::Private,
        }
    }
}

impl From<VisibilityParam> for Visibility {
    fn from(v: VisibilityParam) -> Self {
        match v {
            VisibilityParam::Public => Visibility::Public,
            VisibilityParam::Private => Visibility::Private,
        }
    }
}

/// Body of `DELETE …/{name}@{seq}`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct TombstoneBody {
    /// Why the version is withdrawn.
    pub reason: TombstoneReasonParam,
    /// Free text shown with the tombstone.
    pub note: Option<String>,
}

/// Why a version was tombstoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TombstoneReasonParam {
    /// The producer withdrew it.
    Withdrawn,
    /// It duplicated another version.
    Duplicate,
    /// Removed following a takedown request.
    Takedown,
    /// See the note.
    Other,
}

impl From<TombstoneReasonParam> for TombstoneReason {
    fn from(r: TombstoneReasonParam) -> Self {
        match r {
            TombstoneReasonParam::Withdrawn => TombstoneReason::Withdrawn,
            TombstoneReasonParam::Duplicate => TombstoneReason::Duplicate,
            TombstoneReasonParam::Takedown => TombstoneReason::Takedown,
            TombstoneReasonParam::Other => TombstoneReason::Other,
        }
    }
}

impl From<TombstoneReason> for TombstoneReasonParam {
    fn from(r: TombstoneReason) -> Self {
        match r {
            TombstoneReason::Withdrawn => TombstoneReasonParam::Withdrawn,
            TombstoneReason::Duplicate => TombstoneReasonParam::Duplicate,
            TombstoneReason::Takedown => TombstoneReasonParam::Takedown,
            TombstoneReason::Other => TombstoneReasonParam::Other,
        }
    }
}

/// The tombstone of a withdrawn version.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TombstoneDto {
    /// When it was withdrawn.
    pub at: DateTime<Utc>,
    /// Why.
    pub reason: TombstoneReasonParam,
    /// Free text from the producer, if any.
    pub note: Option<String>,
}

impl From<Tombstone> for TombstoneDto {
    fn from(t: Tombstone) -> Self {
        Self {
            at: t.at,
            reason: t.reason.into(),
            note: t.note,
        }
    }
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
    /// The canonical record. Absent in `POST` responses, in version lists,
    /// and for tombstones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Value>,
    /// Per-facet fingerprints as hex, with `expand=fingerprints`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprints: Option<BTreeMap<String, String>>,
    /// Present when the version was withdrawn; `record` is then absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tombstone: Option<TombstoneDto>,
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
            fingerprints: None,
            tombstone: None,
        }
    }

    fn from_stored(stored: StoredVersion) -> Self {
        let tombstone = stored.tombstone.map(TombstoneDto::from);
        let mut env = Self::from_meta(stored.meta, stored.body);
        env.tombstone = tombstone;
        env
    }

    fn from_summary(summary: VersionSummary) -> Self {
        let mut env = Self::from_meta(summary.meta, None);
        env.tombstone = summary.tombstone.map(TombstoneDto::from);
        env
    }
}

/// One row of `GET /{cards|evals}`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ListItem {
    /// Namespace of the record.
    pub ns: String,
    /// Name of the record.
    pub name: String,
    /// `public` or `private`.
    pub visibility: VisibilityParam,
    /// When the name was created.
    pub created_at: DateTime<Utc>,
    /// `title` of the latest live version.
    pub title: Option<String>,
    /// The latest live version, without its record.
    pub latest: VersionEnvelope,
}

/// The keyset a list cursor carries, before signing.
#[derive(Debug, Serialize, Deserialize)]
struct CursorPayload {
    c: DateTime<Utc>,
    n: String,
    i: Uuid,
}

fn kind_of(record_type: RecordType) -> RecordKind {
    match record_type {
        RecordType::Card => RecordKind::Card,
        RecordType::Eval => RecordKind::Eval,
    }
}

fn type_of(kind: RecordKind) -> RecordType {
    match kind {
        RecordKind::Card => RecordType::Card,
        RecordKind::Eval => RecordType::Eval,
    }
}

fn entry(path: String, code: ErrorCode, hint: impl Into<String>) -> ErrorEntry {
    ErrorEntry {
        path,
        code,
        hint: Some(hint.into()),
    }
}

/// `name` or `name@suffix`, with the suffix classified.
enum Address<'a> {
    Latest(&'a str),
    Seq(&'a str, i32),
    Label(&'a str, &'a str),
}

fn address(name: &str) -> Result<Address<'_>, ApiError> {
    let Some((base, suffix)) = name.split_once('@') else {
        return Ok(Address::Latest(name));
    };
    if base.is_empty() || suffix.is_empty() {
        return Err(ApiError::BadRequest);
    }
    if suffix.bytes().all(|b| b.is_ascii_digit()) {
        match suffix.parse::<i32>() {
            Ok(seq) if seq >= 1 => Ok(Address::Seq(base, seq)),
            _ => Err(ApiError::NotFound),
        }
    } else {
        Ok(Address::Label(base, suffix))
    }
}

fn require_seq(name: &str) -> Result<(&str, i32), ApiError> {
    match address(name)? {
        Address::Seq(base, seq) => Ok((base, seq)),
        _ => Err(ApiError::BadRequest),
    }
}

/// Cut a hex sha256 into bytes, or say where it went wrong.
fn decode_sha(hex_str: &str, path: String) -> Result<[u8; 32], ErrorEntry> {
    let mut out = [0u8; 32];
    if hex_str.len() == 64 && hex::decode_to_slice(hex_str, &mut out).is_ok() {
        Ok(out)
    } else {
        Err(entry(
            path,
            ErrorCode::Schema,
            "sha256 must be 64 hexadecimal characters",
        ))
    }
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
    let kind = kind_of(record_type);

    let mut errors = evalhub_core::validate(kind, &body);
    // sha256 values are decoded here because the store needs the bytes;
    // the validator only checks that the key is a string.
    let mut shas: Vec<(String, [u8; 32])> = Vec::new();
    if let Some(attachments) = body.get("attachments").and_then(Value::as_array) {
        for (i, a) in attachments.iter().enumerate() {
            let (Some(p), Some(h)) = (
                a.get("path").and_then(Value::as_str),
                a.get("sha256").and_then(Value::as_str),
            ) else {
                continue;
            };
            match decode_sha(h, format!("/attachments/{i}/sha256")) {
                Ok(bytes) => shas.push((p.to_string(), bytes)),
                Err(e) => errors.push(e),
            }
        }
    }
    if !errors.is_empty() {
        errors.sort_by(|a, b| a.path.cmp(&b.path));
        return Err(ApiError::Validation(errors));
    }

    let (canonical_bytes, hash) = hash_value(&body).map_err(|e| {
        ApiError::Validation(vec![entry(String::new(), ErrorCode::Schema, e.to_string())])
    })?;
    // Store the canonical form, re-parsed: key order and number formatting
    // are then exactly what the hash was computed over.
    let canonical: Value = serde_json::from_slice(&canonical_bytes)
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;

    let fingerprints = evalhub_core::fingerprints(kind, &canonical)
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    let fingerprint_rows: Vec<(&str, [u8; 32])> = fingerprints
        .iter()
        .map(|(facet, bytes)| (facet.as_str(), *bytes))
        .collect();

    let results: Vec<NewResult<'_>> = canonical
        .get("results")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    Some(NewResult {
                        ordinal: i32::try_from(i).ok()?,
                        metric: r.get("metric")?.as_str()?,
                        aggregation: r.get("aggregation")?.as_str()?,
                        value: r.get("value")?.as_f64()?,
                        n: r.get("n")
                            .and_then(Value::as_i64)
                            .and_then(|n| i32::try_from(n).ok()),
                        by: r.get("by"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // Owned pieces of parsed targets, borrowed by the store rows below.
    struct ParsedRelation<'a> {
        relation_type: &'a str,
        attrs: Option<&'a Value>,
        version: Option<(String, String, i32, Option<RecordType>)>,
        external: Option<&'a str>,
    }
    let parsed: Vec<ParsedRelation<'_>> = canonical
        .get("relations")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let relation_type = r.get("type")?.as_str()?;
                    let to = r.get("to")?.as_str()?;
                    let target_kind = evalhub_core::registry::core_relation_type(relation_type)
                        .map(|d| type_of(d.to));
                    let (version, external) = match parse_relation_target(to)? {
                        ParsedTarget::Version { ns, name, seq } => (
                            Some((ns, name, i32::try_from(seq).ok()?, target_kind)),
                            None,
                        ),
                        ParsedTarget::External(_) | ParsedTarget::Hf(_) => (None, Some(to)),
                    };
                    Some(ParsedRelation {
                        relation_type,
                        attrs: r.get("attrs"),
                        version,
                        external,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let relations: Vec<NewRelation<'_>> = parsed
        .iter()
        .filter_map(|p| {
            let target = match (&p.version, p.external) {
                (Some((ns, name, seq, record_type)), _) => RelationTarget::Version {
                    ns,
                    name,
                    seq: *seq,
                    record_type: *record_type,
                },
                (None, Some(text)) => RelationTarget::External(text),
                (None, None) => return None,
            };
            Some(NewRelation {
                relation_type: p.relation_type,
                target,
                attrs: p.attrs,
            })
        })
        .collect();

    let attachments: Vec<NewAttachmentRef<'_>> = shas
        .iter()
        .map(|(path, sha256)| NewAttachmentRef {
            path,
            sha256: *sha256,
        })
        .collect();

    let badges_for = |facts: &records::IngestFacts| -> Vec<String> {
        compute_badges(
            &canonical,
            &BadgeInput {
                all_refs_resolved: facts.all_refs_resolved,
                harness_registered: facts.harness_registered,
                all_metrics_registered: facts.all_metrics_registered,
            },
        )
        .into_iter()
        .map(|b| b.as_str().to_string())
        .collect()
    };

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
            fingerprints: &fingerprint_rows,
            results: &results,
            relations: &relations,
            attachments: &attachments,
        },
        || (RecordId::new().as_uuid(), VersionId::new().as_uuid()),
        &badges_for,
    )
    .await;

    let outcome = match outcome {
        Ok(o) => o,
        Err(StoreError::AttachmentMissing(missing)) => {
            let errors = shas
                .iter()
                .enumerate()
                .filter(|(_, (_, sha))| missing.contains(sha))
                .map(|(i, _)| {
                    entry(
                        format!("/attachments/{i}/sha256"),
                        ErrorCode::AttachmentMissing,
                        "upload the object and confirm it with POST /attachments/{sha256}/complete first",
                    )
                })
                .collect();
            return Err(ApiError::AttachmentMissing(errors));
        }
        Err(e) => return Err(e.into()),
    };

    Ok(match outcome {
        CreateOutcome::Created { meta, .. } => (
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
    query: GetQuery,
) -> Result<Json<VersionEnvelope>, ApiError> {
    let MaybeAuth(caller) = caller;
    let pool = state.db()?;
    let caller_ns = caller.namespaces();
    let stored = match address(&path.name)? {
        Address::Latest(name) => {
            records::get_latest(pool, record_type, &path.ns, name, &caller_ns).await?
        }
        Address::Seq(name, seq) => {
            records::get_by_seq(pool, record_type, &path.ns, name, seq, &caller_ns).await?
        }
        Address::Label(name, label) => {
            records::get_by_label(pool, record_type, &path.ns, name, label, &caller_ns).await?
        }
    }
    .ok_or(ApiError::NotFound)?;
    let version_id = stored.meta.version_id;
    let mut env = VersionEnvelope::from_stored(stored);
    let expand: Vec<&str> = query
        .expand
        .as_deref()
        .map(|s| s.split(',').map(str::trim).collect())
        .unwrap_or_default();
    if expand.contains(&"fingerprints") {
        let map = records::load_fingerprints(pool, version_id).await?;
        env.fingerprints = Some(
            map.into_iter()
                .map(|(facet, bytes)| (facet, hex::encode(bytes)))
                .collect(),
        );
    }
    Ok(Json(env))
}

async fn versions(
    record_type: RecordType,
    state: AppState,
    caller: MaybeAuth,
    path: RecordPath,
) -> Result<Json<Page<VersionEnvelope>>, ApiError> {
    let MaybeAuth(caller) = caller;
    if path.name.contains('@') {
        return Err(ApiError::BadRequest);
    }
    let items = records::list_versions(
        state.db()?,
        record_type,
        &path.ns,
        &path.name,
        &caller.namespaces(),
    )
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(Json(Page {
        items: items
            .into_iter()
            .map(VersionEnvelope::from_summary)
            .collect(),
        next_cursor: None,
    }))
}

async fn label(
    record_type: RecordType,
    state: AppState,
    caller: Auth,
    path: RecordPath,
    body: LabelBody,
) -> Result<Json<VersionEnvelope>, ApiError> {
    let Auth(caller) = caller;
    if !caller.allows(&path.ns, Scope::Write) {
        return Err(ApiError::Forbidden);
    }
    let (name, seq) = require_seq(&path.name)?;
    let meta = records::set_label(
        state.db()?,
        record_type,
        &path.ns,
        name,
        seq,
        &body.label,
        caller.actor(),
    )
    .await?;
    Ok(Json(VersionEnvelope::from_meta(meta, None)))
}

async fn settings(
    record_type: RecordType,
    state: AppState,
    caller: Auth,
    path: RecordPath,
    body: Settings,
) -> Result<Json<Settings>, ApiError> {
    let Auth(caller) = caller;
    if !caller.allows(&path.ns, Scope::Write) {
        return Err(ApiError::Forbidden);
    }
    if path.name.contains('@') {
        return Err(ApiError::BadRequest);
    }
    records::set_visibility(
        state.db()?,
        record_type,
        &path.ns,
        &path.name,
        body.visibility.into(),
        caller.actor(),
    )
    .await?;
    Ok(Json(body))
}

async fn tombstone(
    record_type: RecordType,
    state: AppState,
    caller: Auth,
    path: RecordPath,
    body: TombstoneBody,
) -> Result<Json<VersionEnvelope>, ApiError> {
    let Auth(caller) = caller;
    if !caller.allows(&path.ns, Scope::Write) {
        return Err(ApiError::Forbidden);
    }
    let (name, seq) = require_seq(&path.name)?;
    let pool = state.db()?;
    records::tombstone(
        pool,
        record_type,
        &path.ns,
        name,
        seq,
        TombstoneRequest {
            reason: body.reason.into(),
            note: body.note.as_deref(),
        },
        caller.actor(),
    )
    .await?;
    // Re-read so the envelope carries the tombstone as stored.
    let stored = records::get_by_seq(pool, record_type, &path.ns, name, seq, &caller.namespaces())
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(VersionEnvelope::from_stored(stored)))
}

async fn list(
    record_type: RecordType,
    state: AppState,
    caller: MaybeAuth,
    query: ListQuery,
) -> Result<Json<Page<ListItem>>, ApiError> {
    let MaybeAuth(caller) = caller;
    let cursor = match &query.cursor {
        None => None,
        Some(c) => {
            let payload = state.cursors.verify(c).ok_or(ApiError::BadRequest)?;
            let p: CursorPayload =
                serde_json::from_slice(&payload).map_err(|_| ApiError::BadRequest)?;
            Some(ListCursor {
                created_at: p.c,
                name: p.n,
                id: p.i,
            })
        }
    };
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let (items, next) = records::list(
        state.db()?,
        ListParams {
            record_type,
            ns: query.ns.as_deref(),
            search: query.search.as_deref(),
            sort: query.sort.map(ListSort::from).unwrap_or_default(),
            cursor,
            limit,
        },
        &caller.namespaces(),
    )
    .await?;
    let next_cursor = match next {
        None => None,
        Some(c) => {
            let payload = serde_json::to_vec(&CursorPayload {
                c: c.created_at,
                n: c.name,
                i: c.id,
            })
            .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
            Some(state.cursors.sign(&payload))
        }
    };
    Ok(Json(Page {
        items: items
            .into_iter()
            .map(|it| ListItem {
                ns: it.ns,
                name: it.name,
                visibility: it.visibility.into(),
                created_at: it.created_at,
                title: it.title,
                latest: VersionEnvelope::from_meta(it.latest, None),
            })
            .collect(),
        next_cursor,
    }))
}

macro_rules! record_handlers {
    ($rt:expr, $post:ident, $get:ident, $versions:ident, $label:ident, $settings:ident, $delete:ident, $list:ident, $noun:literal) => {
        #[doc = concat!("`POST /api/v1/", $noun, "/{ns}/{name}?label=` — append a version.")]
        pub async fn $post(
            State(state): State<AppState>,
            caller: Auth,
            Path(path): Path<RecordPath>,
            Query(query): Query<PostQuery>,
            Json(body): Json<Value>,
        ) -> Result<(StatusCode, Json<VersionEnvelope>), ApiError> {
            post($rt, state, caller, path, query, body).await
        }

        #[doc = concat!("`GET /api/v1/", $noun, "/{ns}/{name}[@{seq}|@{label}]?expand=` — read a version.")]
        pub async fn $get(
            State(state): State<AppState>,
            caller: MaybeAuth,
            Path(path): Path<RecordPath>,
            Query(query): Query<GetQuery>,
        ) -> Result<Json<VersionEnvelope>, ApiError> {
            get($rt, state, caller, path, query).await
        }

        #[doc = concat!("`GET /api/v1/", $noun, "/{ns}/{name}/versions` — every version, oldest first.")]
        pub async fn $versions(
            State(state): State<AppState>,
            caller: MaybeAuth,
            Path(path): Path<RecordPath>,
        ) -> Result<Json<Page<VersionEnvelope>>, ApiError> {
            versions($rt, state, caller, path).await
        }

        #[doc = concat!("`PATCH /api/v1/", $noun, "/{ns}/{name}@{seq}/label` — point a label at a version.")]
        pub async fn $label(
            State(state): State<AppState>,
            caller: Auth,
            Path(path): Path<RecordPath>,
            Json(body): Json<LabelBody>,
        ) -> Result<Json<VersionEnvelope>, ApiError> {
            label($rt, state, caller, path, body).await
        }

        #[doc = concat!("`PATCH /api/v1/", $noun, "/{ns}/{name}/settings` — change visibility.")]
        pub async fn $settings(
            State(state): State<AppState>,
            caller: Auth,
            Path(path): Path<RecordPath>,
            Json(body): Json<Settings>,
        ) -> Result<Json<Settings>, ApiError> {
            settings($rt, state, caller, path, body).await
        }

        #[doc = concat!("`DELETE /api/v1/", $noun, "/{ns}/{name}@{seq}` — tombstone a version.")]
        pub async fn $delete(
            State(state): State<AppState>,
            caller: Auth,
            Path(path): Path<RecordPath>,
            Json(body): Json<TombstoneBody>,
        ) -> Result<Json<VersionEnvelope>, ApiError> {
            tombstone($rt, state, caller, path, body).await
        }

        #[doc = concat!("`GET /api/v1/", $noun, "?ns&search&sort&cursor&limit` — page over records.")]
        pub async fn $list(
            State(state): State<AppState>,
            caller: MaybeAuth,
            Query(query): Query<ListQuery>,
        ) -> Result<Json<Page<ListItem>>, ApiError> {
            list($rt, state, caller, query).await
        }
    };
}

record_handlers!(
    RecordType::Card,
    post_card,
    get_card,
    versions_card,
    label_card,
    settings_card,
    delete_card,
    list_cards,
    "cards"
);
record_handlers!(
    RecordType::Eval,
    post_eval,
    get_eval,
    versions_eval,
    label_eval,
    settings_eval,
    delete_eval,
    list_evals,
    "evals"
);
