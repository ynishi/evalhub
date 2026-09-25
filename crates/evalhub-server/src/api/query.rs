//! `POST /{cards|evals}/query`.
//!
//! Parse and type-check with `evalhub_query` (errors → `422`), attach the
//! caller's namespaces for the visibility predicate, decode and verify the
//! cursor, compile and run with `evalhub_store::query_sql`, sign the next
//! cursor. The handler holds the current `PathTable`, swapped when an
//! `ext_schema` becomes `applied`.
//!
//! # The one check the query language cannot make
//!
//! `results[{metric}].value` may be sorted on only when the metric is a
//! registry entry, because `lower_is_better` is what says which way is up
//! and the hub will not guess. The type check has no registry, so it
//! accepts any well-formed metric id and this handler rejects the unknown
//! ones with `422 not_indexed`.
//!
//! The registry entry is what makes a metric sortable at all: knowing
//! `lower_is_better` is knowing which way is up. The direction itself
//! still comes from the request, because `sort[].dir` is a required field
//! of the query envelope — there is no "unspecified" to fill in. A client
//! that wants "best first" reads `lower_is_better` from
//! `GET /registry/metrics/{ns}/{id}@{version}` and asks for `asc` or
//! `desc` accordingly.
//!
//! # Expansion costs
//!
//! `expand=fingerprints` and `expand=relations` are one extra read per
//! hit. At a page of at most 200 that is acceptable and keeps the SQL
//! compiler free of joins it would only need here; if a page ever grows
//! past that, these become batch reads in the store.
//!
//! # Paging
//!
//! The cursor is the store's keyset tuple (the sort keys of the last row
//! plus its `version_id`), JSON-serialised and HMAC-signed by
//! [`crate::auth::CursorSigner`]. A cursor the hub did not sign, or one
//! whose key count no longer matches the sort, is `400`: the page
//! sequence has to start again rather than silently skip rows.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use evalhub_core::ContentHash;
use evalhub_core::{RecordId, VersionId};
use evalhub_query::ir;
use evalhub_schema::RecordKind;
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_schema::query::{Page, QueryRequest};
use evalhub_store::records;
use evalhub_store::registry::{self, Kind};
use evalhub_store::{query_sql, relations};

use crate::api::relations::{RelationDto, Withheld, withhold};
use crate::auth::MaybeAuth;
use crate::error::ApiError;
use crate::state::AppState;

/// Largest page a query may ask for.
const MAX_LIMIT: u32 = 200;
/// Page size when the request does not say.
const DEFAULT_LIMIT: u32 = 50;

/// One matched version, with what the hub knows about it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct QueryHitDto {
    /// Identifier of the named record, stable across versions (ULID).
    pub id: String,
    /// Identifier of this version (ULID).
    pub version_id: String,
    /// Namespace the record lives in.
    pub ns: String,
    /// Name of the record within the namespace.
    pub name: String,
    /// Sequence number within the name.
    pub seq: i32,
    /// Client-chosen label of this version, if it has one.
    pub label: Option<String>,
    /// Hex sha256 of the canonical record.
    pub content_hash: String,
    /// When the hub stored this version.
    pub created_at: DateTime<Utc>,
    /// Top-level keys whose value differs from the previous version.
    pub changed: Vec<String>,
    /// Badges awarded at ingest.
    pub badges: Vec<String>,
    /// The canonical record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Value>,
    /// Per-facet fingerprints as hex, with `expand=fingerprints`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprints: Option<BTreeMap<String, String>>,
    /// Outgoing edges, with `expand=relations`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relations: Option<Vec<RelationDto>>,
    /// What was removed from `record` because the caller may not see it.
    /// When present, `record` is not the stored body and does not match
    /// `content_hash`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withheld: Option<Withheld>,
}

impl From<query_sql::QueryHit> for QueryHitDto {
    fn from(hit: query_sql::QueryHit) -> Self {
        let content_hash = hit
            .content_hash
            .as_slice()
            .try_into()
            .map(ContentHash::from_bytes)
            .map(|h| h.as_hex())
            .unwrap_or_default();
        Self {
            id: RecordId::from_uuid(hit.record_id).to_string(),
            version_id: VersionId::from_uuid(hit.version_id).to_string(),
            ns: hit.ns,
            name: hit.name,
            seq: hit.seq,
            label: hit.label,
            content_hash,
            created_at: hit.created_at,
            changed: hit.changed,
            badges: hit.badges,
            record: hit.body,
            fingerprints: None,
            relations: None,
            withheld: None,
        }
    }
}

/// `{ns}/{name}` of a registry id, or `None` when it has no namespace.
fn split_id(id: &str) -> Option<(&str, &str)> {
    id.split_once('/')
}

/// Confirm every metric a sort names is in the registry.
///
/// Sorting by a measure the hub cannot orient is the one thing the type
/// check cannot rule out on its own, because the vocabulary of metrics
/// lives in the registry rather than in the schema.
async fn check_metric_sorts(
    pool: &evalhub_store::PgPool,
    query: &ir::Query,
) -> Result<(), ApiError> {
    let mut errors = Vec::new();
    for (i, sort) in query.sort.iter().enumerate() {
        let ir::SortKey::Metric(metric) = &sort.key else {
            continue;
        };
        let Some((ns, id)) = split_id(metric) else {
            errors.push(ErrorEntry {
                path: format!("/sort/{i}/path"),
                code: ErrorCode::UnknownPath,
                hint: Some("a metric id is `{ns}/{name}`".to_string()),
            });
            continue;
        };
        // Any version of the metric will do: `lower_is_better` is a fact
        // about the measure, not about the entry's revision.
        let entries = registry::list(pool, Some(Kind::Metrics), Some(ns), 100, 0).await?;
        let Some(entry) = entries.into_iter().find(|e| e.id == id) else {
            errors.push(ErrorEntry {
                path: format!("/sort/{i}/path"),
                code: ErrorCode::NotIndexed,
                hint: Some(format!(
                    "`{metric}` is not in the registry, so the hub does not know whether \
                     a higher value is better; register it with PUT /registry/metrics/{ns}/{id}@1"
                )),
            });
            continue;
        };
        // The entry exists, which is all this check needs; the direction
        // came with the request.
        let _ = entry;
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError::Validation(errors))
    }
}

async fn query(
    kind: RecordKind,
    state: AppState,
    caller: MaybeAuth,
    request: QueryRequest,
) -> Result<Json<Page<QueryHitDto>>, ApiError> {
    let MaybeAuth(caller) = caller;
    let pool = state.db()?;
    let caller_ns = caller.namespaces();

    let cursor = match &request.cursor {
        None => None,
        Some(c) => {
            let payload = state.cursors.verify(c).ok_or(ApiError::BadRequest)?;
            Some(serde_json::from_slice::<ir::Cursor>(&payload).map_err(|_| ApiError::BadRequest)?)
        }
    };
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let tables = state.path_tables.current().await;
    let compiled = evalhub_query::compile(&request, kind, tables.for_kind(kind), limit, cursor)
        .map_err(ApiError::Validation)?;
    check_metric_sorts(pool, &compiled).await?;

    let (hits, next) = query_sql::run(pool, &compiled, &caller_ns).await?;

    let want_fingerprints = compiled.expand.contains(&ir::Expand::Fingerprints);
    let want_relations = compiled.expand.contains(&ir::Expand::Relations);
    let version_ids: Vec<_> = hits.iter().map(|h| h.version_id).collect();
    // What this reader may not see of each hit's body: targets of its
    // relations, and (a Card's) the Evals its run_results judge. Either
    // one is enough to redact; see `crate::api::relations`.
    let hidden = relations::hidden_targets(pool, &version_ids, &caller_ns).await?;
    let hidden_evals = match kind {
        RecordKind::Card => relations::hidden_evals(pool, &version_ids, &caller_ns).await?,
        RecordKind::Eval => Default::default(),
    };
    let mut items = Vec::with_capacity(hits.len());
    for hit in hits {
        let version_id = hit.version_id;
        let mut dto = QueryHitDto::from(hit);
        if let Some(record) = dto.record.as_mut() {
            dto.withheld = withhold(
                record,
                hidden.get(&version_id).map_or(&[], Vec::as_slice),
                hidden_evals.get(&version_id).map_or(&[], Vec::as_slice),
            );
        }
        if want_fingerprints {
            let map = records::load_fingerprints(pool, version_id).await?;
            dto.fingerprints = Some(
                map.into_iter()
                    .map(|(facet, bytes)| (facet, hex::encode(bytes)))
                    .collect(),
            );
        }
        if want_relations {
            let edges = relations::outgoing(pool, version_id, None, &caller_ns).await?;
            dto.relations = Some(edges.into_iter().map(RelationDto::from).collect());
        }
        items.push(dto);
    }

    let next_cursor = match next {
        None => None,
        Some(cursor) => Some(state.cursors.sign(
            &serde_json::to_vec(&cursor).map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?,
        )),
    };
    Ok(Json(Page { items, next_cursor }))
}

/// `POST /api/v1/cards/query` — search Cards.
pub async fn query_cards(
    State(state): State<AppState>,
    caller: MaybeAuth,
    Json(request): Json<QueryRequest>,
) -> Result<Json<Page<QueryHitDto>>, ApiError> {
    query(RecordKind::Card, state, caller, request).await
}

/// `POST /api/v1/evals/query` — search Evals.
pub async fn query_evals(
    State(state): State<AppState>,
    caller: MaybeAuth,
    Json(request): Json<QueryRequest>,
) -> Result<Json<Page<QueryHitDto>>, ApiError> {
    query(RecordKind::Eval, state, caller, request).await
}
