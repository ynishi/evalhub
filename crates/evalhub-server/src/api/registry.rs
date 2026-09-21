//! Registry handlers. `core/` is read-only; `PUT` of an `ext_schema`
//! returns `202` and starts the index job. See `evalhub_store::registry`.
//!
//! The registry is the hub's vocabulary: what a metric means and which
//! way is up, which harness versions exist, what a relation type connects,
//! and which `ext` keys are typed well enough to index. Nothing here gates
//! ingest — a record citing an unregistered metric is still stored, it
//! simply does not earn the badge — but a few things the hub will not do
//! without an entry, because they would mean guessing: ordering by a
//! metric, and serving a range query over an extension key.
//!
//! | Route                                           | Needs                        |
//! | ----------------------------------------------- | ---------------------------- |
//! | `GET /registry/{kind}?ns=`                      | nothing; the vocabulary is public |
//! | `GET /registry/{kind}/{ns}/{id}@{version}`      | nothing                      |
//! | `PUT /registry/{kind}/{ns}/{id}@{version}`      | `write` on `ns`              |
//!
//! Entries are immutable: a second `PUT` of the same address is `409`.
//! A correction is a new version, which is what keeps a fingerprint
//! computed under one extension schema meaningful after the next.
//!
//! `core/` is seeded by migration `0002` and refuses writes with `403`.
//!
//! # `ext_schemas` take a moment
//!
//! Registering an extension schema answers `202 { state: "applying" }`:
//! the paths it declares need expression indexes, and those are built
//! `CONCURRENTLY` by [`crate::jobs`] rather than inside the request. Until
//! they are valid the paths behave as unregistered — `eq` and `exists`
//! only — and the entry reads back as `applying`. When the job finishes,
//! the state is `applied` and the query language accepts ranges and sorts
//! on them.

use aide::axum::IntoApiResponse;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use evalhub_store::auth::Scope;
use evalhub_store::registry::{self, CORE_NS, Entry, Kind, State as EntryState};

use crate::auth::{Auth, MaybeAuth};
use crate::error::ApiError;
use crate::state::AppState;

/// Largest page a registry listing returns.
const MAX_LIMIT: u32 = 200;
/// Page size when the request does not say.
const DEFAULT_LIMIT: u32 = 50;

/// One registry entry.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EntryDto {
    /// `metrics`, `harnesses`, `relation_types` or `ext_schemas`.
    pub kind: String,
    /// Namespace that owns it.
    pub ns: String,
    /// Identifier within the namespace.
    pub id: String,
    /// Version of the entry. Entries are immutable; a change is a new one.
    pub version: String,
    /// The definition, whose shape depends on the kind.
    pub body: Value,
    /// `applied`, or `applying` while its indexes are being built.
    pub state: String,
    /// When it was registered.
    pub created_at: DateTime<Utc>,
}

impl From<Entry> for EntryDto {
    fn from(e: Entry) -> Self {
        Self {
            kind: e.kind.as_str().to_string(),
            ns: e.ns,
            id: e.id,
            version: e.version,
            body: e.body,
            state: e.state.as_str().to_string(),
            created_at: e.created_at,
        }
    }
}

/// A page of entries. The registry is small, so this pages by offset
/// rather than by cursor.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EntryPage {
    /// The entries, ordered by kind, namespace, id and version.
    pub items: Vec<EntryDto>,
}

/// Path of `GET /registry/{kind}`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KindPath {
    /// `metrics`, `harnesses`, `relation_types` or `ext_schemas`.
    pub kind: String,
}

/// Query string of `GET /registry/{kind}`.
///
/// Named for the registry rather than `ListQuery`, so that a generated
/// client gets one type per listing instead of `ListQuery` and
/// `ListQuery2`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RegistryListQuery {
    /// Only entries owned by this namespace.
    pub ns: Option<String>,
    /// Page size, 1–200; default 50.
    pub limit: Option<u32>,
    /// How many entries to skip.
    pub offset: Option<u32>,
}

/// Path of one entry: `/registry/{kind}/{ns}/{id}` where `id` carries
/// `@{version}`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EntryPath {
    /// `metrics`, `harnesses`, `relation_types` or `ext_schemas`.
    pub kind: String,
    /// Namespace that owns the entry.
    pub ns: String,
    /// Identifier followed by `@{version}`.
    pub id: String,
}

fn parse_kind(kind: &str) -> Result<Kind, ApiError> {
    Kind::parse(kind).ok_or(ApiError::NotFound)
}

/// Split `id@version`; both halves must be non-empty.
fn split_version(id: &str) -> Result<(&str, &str), ApiError> {
    match id.split_once('@') {
        Some((id, version)) if !id.is_empty() && !version.is_empty() => Ok((id, version)),
        _ => Err(ApiError::BadRequest),
    }
}

/// `GET /api/v1/registry/{kind}?ns&limit&offset` — the vocabulary.
pub async fn list(
    State(state): State<AppState>,
    MaybeAuth(_caller): MaybeAuth,
    Path(path): Path<KindPath>,
    Query(query): Query<RegistryListQuery>,
) -> Result<Json<EntryPage>, ApiError> {
    let kind = parse_kind(&path.kind)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let items = registry::list(
        state.db()?,
        Some(kind),
        query.ns.as_deref(),
        limit,
        query.offset.unwrap_or(0),
    )
    .await?
    .into_iter()
    .map(EntryDto::from)
    .collect();
    Ok(Json(EntryPage { items }))
}

/// `GET /api/v1/registry/{kind}/{ns}/{id}@{version}` — one entry.
pub async fn get(
    State(state): State<AppState>,
    MaybeAuth(_caller): MaybeAuth,
    Path(path): Path<EntryPath>,
) -> Result<Json<EntryDto>, ApiError> {
    let kind = parse_kind(&path.kind)?;
    let (id, version) = split_version(&path.id)?;
    let entry = registry::get(state.db()?, kind, &path.ns, id, version)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(EntryDto::from(entry)))
}

/// `PUT /api/v1/registry/{kind}/{ns}/{id}@{version}` — register one.
///
/// `201` for a definition that takes effect at once, `202` for an
/// `ext_schema` whose indexes are still being built.
pub async fn put(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Path(path): Path<EntryPath>,
    Json(body): Json<Value>,
) -> Result<impl IntoApiResponse, ApiError> {
    let kind = parse_kind(&path.kind)?;
    let (id, version) = split_version(&path.id)?;
    if path.ns == CORE_NS {
        return Err(ApiError::Forbidden);
    }
    if !caller.allows(&path.ns, Scope::Write) {
        return Err(ApiError::Forbidden);
    }
    // An extension schema that declares no indexable path would land in
    // `applying` and never leave, so it is refused here with the reason.
    if kind == Kind::ExtSchemas {
        let key = format!("{}/{}", path.ns, id);
        match registry::typed_paths(&key, &body) {
            Err(message) => {
                return Err(ApiError::Validation(vec![
                    evalhub_schema::error::ErrorEntry {
                        path: "/schema".to_string(),
                        code: evalhub_schema::error::ErrorCode::Schema,
                        hint: Some(message),
                    },
                ]));
            }
            Ok(paths) if paths.is_empty() => {
                return Err(ApiError::Validation(vec![
                    evalhub_schema::error::ErrorEntry {
                        path: "/schema".to_string(),
                        code: evalhub_schema::error::ErrorCode::Schema,
                        hint: Some(
                            "no indexable path: give the schema `properties` with \
                             `number`, `string` or `boolean` leaves"
                                .to_string(),
                        ),
                    },
                ]));
            }
            Ok(_) => {}
        }
    }

    let entry = registry::put(
        state.db()?,
        kind,
        &path.ns,
        id,
        version,
        &body,
        caller.actor(),
    )
    .await?;

    // A new harness or metric can change `harness_registered` /
    // `metric_registered` on versions already stored. The sweep picks
    // those up; there is no queue to lose.
    let status = match entry.state {
        EntryState::Applying => StatusCode::ACCEPTED,
        EntryState::Applied => StatusCode::CREATED,
    };
    Ok((status, Json(EntryDto::from(entry))))
}
