//! Relation handlers: traversal, adding edges, and the comparison view.
//!
//! Semantics in `evalhub_store::relations`. The comparison view
//! (`GET /evals/{ns}/{name}/cards`) is the only endpoint that combines
//! records, and it combines them by lining them up, not by computing over
//! them.
//!
//! ```text
//! POST /{cards|evals}/{ns}/{name}@{seq}/relations {type,to,attrs}  → 201 edge
//! GET  /{cards|evals}/{ns}/{name}[@{seq}]/relations?direction&types&depth&follow_latest
//!                                                                  → { nodes, edges }
//! GET  /evals/{ns}/{name}[@{seq}]/cards?group_by=fingerprint.{facet}
//!                                                                  → { items } or { groups }
//! ```
//!
//! # Endpoints the caller may not see
//!
//! A node whose record is private to a namespace the caller's token does
//! not cover comes back as `{ private: true, version_id, content_hash }`
//! and nothing else: enough to know the edge is anchored to a specific,
//! immutable version, not enough to learn its name, its namespace or its
//! title. That is the commitment the design promises, and it is why a
//! public Card may cite a private Eval without leaking it.
//!
//! The same holds for the body of the version the edge leaves. Every read
//! that returns a record body (`GET …/{name}`, `POST …/query`) removes the
//! `relations[]` elements whose target the reader may not see and lists
//! them, as the same commitment, under `withheld.relations` in the
//! envelope ([`withhold`]). A body with elements withheld is no longer the
//! stored body, so its `content_hash` does not match it: a client that
//! verifies the hash does so on responses without `withheld`.
//!
//! A write never learns more than a read. A target the writer may not see
//! is stored unresolved, like one that does not exist, and the response
//! and the `refs_resolved` badge are the same in both cases. Reading such
//! an edge, a caller without access gets the text back as `unresolved`;
//! once the target is readable to them it resolves.
//!
//! An `external:` or `hf:` target, and a `{ns}/{name}@{seq}` that does not
//! exist yet, appear as text: `{ external: "…" }` and
//! `{ unresolved: "…" }`. A Card published before the Eval it cites is
//! normal, and the edge links up on its own once the Eval exists — the
//! `refs_resolved` badge stays as it was recorded, because it is a fact
//! about publication order.
//!
//! # `group_by`
//!
//! `group_by` takes exactly `fingerprint.{facet}` for one of the seven
//! facets. The response is then `{ groups: [{ key, items }] }` where `key`
//! is that facet's fingerprint in hex, shared by every Card in the group,
//! and `null` for Cards that have no fingerprint for it. Anything else in
//! `group_by` is `400`: a mistyped facet must not silently collapse every
//! Card into one group. Without `group_by` the response is a flat
//! `{ items }`.
//!
//! The hub does not rank the groups or the Cards in them. `same_harness`
//! and `same_model` say whether each Card's harness and model fingerprints
//! equal the Eval's, and that is the whole of its opinion.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use evalhub_core::validate::{RelationTarget as ParsedTarget, parse_relation_target};
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_store::auth::Scope;
use evalhub_store::records::{self, RecordType};
use evalhub_store::relations::{
    self, Direction, EdgeEnd, HiddenTarget, RelationTarget, ResolvedTarget, StoredRelation,
    TraverseParams,
};

use crate::auth::{Auth, MaybeAuth};
use crate::error::ApiError;
use crate::state::AppState;

/// Path of the record an edge leaves or is read from:
/// `/{cards|evals}/{ns}/{name}`. Named apart from the records module's
/// path type so the two do not collide in the generated schema.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RelationPath {
    /// Namespace: a user login or an organisation slug.
    pub ns: String,
    /// Record name, optionally followed by `@{seq}` or `@{label}`.
    pub name: String,
}

/// Body of `POST …/relations`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewRelationBody {
    /// Registry id of the relation type, `{ns}/{name}`.
    #[serde(rename = "type")]
    pub relation_type: String,
    /// Target: `{ns}/{name}@{seq}`, `external:…` or `hf:…`.
    pub to: String,
    /// Free-form attributes for this relation type.
    pub attrs: Option<Value>,
}

/// One endpoint of an edge.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum TargetDto {
    /// A version on this hub the caller may see.
    Version {
        /// The version.
        version_id: String,
        /// Namespace of its record.
        ns: String,
        /// Record name.
        name: String,
        /// Sequence number.
        seq: i32,
        /// `card` or `eval`.
        record_type: String,
        /// Whether the version has been tombstoned.
        tombstoned: bool,
    },
    /// A version the caller may not see: the commitment only.
    Private {
        /// Always `true`; marks the reduced form.
        private: bool,
        /// The version it points at.
        version_id: String,
        /// sha256 of the canonical body, so the reference is still pinned.
        content_hash: String,
    },
    /// A `{ns}/{name}@{seq}` that does not exist yet.
    Unresolved {
        /// The reference as written.
        unresolved: String,
    },
    /// An `external:` or `hf:` target.
    External {
        /// The reference as written.
        external: String,
    },
}

impl From<ResolvedTarget> for TargetDto {
    fn from(t: ResolvedTarget) -> Self {
        match t {
            ResolvedTarget::Version {
                version_id,
                ns,
                name,
                seq,
                record_type,
                tombstoned,
                visible,
                content_hash,
            } => {
                if visible {
                    Self::Version {
                        version_id: version_id.to_string(),
                        ns,
                        name,
                        seq,
                        record_type,
                        tombstoned,
                    }
                } else {
                    Self::Private {
                        private: true,
                        version_id: version_id.to_string(),
                        content_hash: hex::encode(content_hash),
                    }
                }
            }
            ResolvedTarget::Unresolved(text) => Self::Unresolved { unresolved: text },
            ResolvedTarget::External(text) => Self::External { external: text },
        }
    }
}

/// What a read removed from a record body because the reader may not see
/// it. Present in an envelope only when something was removed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Withheld {
    /// One entry per removed `relations[]` element, in body order.
    pub relations: Vec<WithheldRelation>,
}

/// A removed `relations[]` element, reduced to the commitment: the same
/// `version_id` and `content_hash` a `private` endpoint of
/// `GET …/relations` shows.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WithheldRelation {
    /// Registry id of the relation type of the element.
    #[serde(rename = "type")]
    pub relation_type: String,
    /// The version it points at.
    pub version_id: String,
    /// sha256 of that version's canonical body.
    pub content_hash: String,
}

/// Remove from `body.relations[]` every element whose `(type, to)` names
/// one of `hidden`, and say what was removed. `None` when nothing was.
///
/// `to` is parsed rather than compared as text, so a spelling the parser
/// accepts but the stored text does not match (`@01`) is still caught.
pub fn withhold(body: &mut Value, hidden: &[HiddenTarget]) -> Option<Withheld> {
    if hidden.is_empty() {
        return None;
    }
    let elements = body.get_mut("relations")?.as_array_mut()?;
    let mut removed = Vec::new();
    elements.retain(|element| {
        let Some(relation_type) = element.get("type").and_then(Value::as_str) else {
            return true;
        };
        let Some(ParsedTarget::Version { ns, name, seq }) = element
            .get("to")
            .and_then(Value::as_str)
            .and_then(parse_relation_target)
        else {
            return true;
        };
        let found = hidden.iter().find(|h| {
            h.relation_type == relation_type
                && h.ns == ns
                && h.name == name
                && i64::from(h.seq) == i64::from(seq)
        });
        match found {
            Some(h) => {
                removed.push(WithheldRelation {
                    relation_type: h.relation_type.clone(),
                    version_id: h.version_id.to_string(),
                    content_hash: hex::encode(&h.content_hash),
                });
                false
            }
            None => true,
        }
    });
    (!removed.is_empty()).then_some(Withheld { relations: removed })
}

/// An edge as returned by `POST …/relations`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RelationDto {
    /// The version the edge leaves.
    pub from: TargetDto,
    /// Registry id of the relation type.
    #[serde(rename = "type")]
    pub relation_type: String,
    /// The version or external reference it points at.
    pub to: TargetDto,
    /// Free-form attributes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Value>,
}

impl From<StoredRelation> for RelationDto {
    fn from(r: StoredRelation) -> Self {
        Self {
            from: r.from.into(),
            relation_type: r.relation_type,
            to: r.to.into(),
            attrs: r.attrs,
        }
    }
}

/// A node of the relation graph.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NodeDto {
    /// The version.
    pub version_id: String,
    /// Namespace, when the caller may see the record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ns: Option<String>,
    /// Record name, when the caller may see it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Sequence number, when the caller may see it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i32>,
    /// `card` or `eval`, when the caller may see it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_type: Option<String>,
    /// Whether the version has been tombstoned, when the caller may see it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tombstoned: Option<bool>,
    /// `true` on a node the caller may not see; then only `version_id`
    /// and `content_hash` are present.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub private: bool,
    /// sha256 of the canonical body: the commitment, always present.
    pub content_hash: String,
}

/// An edge of the relation graph.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EdgeDto {
    /// Version the edge leaves.
    pub from: String,
    /// Registry id of the relation type.
    #[serde(rename = "type")]
    pub relation_type: String,
    /// Version it points at, when the target is on this hub.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Textual target, for an unresolved or external reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_text: Option<String>,
    /// Free-form attributes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Value>,
}

/// What a traversal found.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphDto {
    /// Every version reached, the start first.
    pub nodes: Vec<NodeDto>,
    /// Every edge followed.
    pub edges: Vec<EdgeDto>,
}

/// Query string of `GET …/relations`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct TraverseQuery {
    /// `out` (default), `in` or `both`.
    pub direction: Option<DirectionParam>,
    /// Comma-separated relation type ids to follow; all by default.
    pub types: Option<String>,
    /// How many edges from the start, 1–5; default 1.
    pub depth: Option<u32>,
    /// Walk from the latest live version of each record instead of the
    /// version the edge names.
    pub follow_latest: Option<bool>,
}

/// Which edges a traversal follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DirectionParam {
    /// Edges leaving the node.
    Out,
    /// Edges entering the node.
    In,
    /// Both.
    Both,
}

impl From<DirectionParam> for Direction {
    fn from(d: DirectionParam) -> Self {
        match d {
            DirectionParam::Out => Self::Out,
            DirectionParam::In => Self::In,
            DirectionParam::Both => Self::Both,
        }
    }
}

/// Query string of the comparison view.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ComparisonQuery {
    /// `fingerprint.{facet}` for one of the seven facets. Groups the
    /// Cards by that fingerprint; omitted, the list is flat.
    pub group_by: Option<String>,
}

/// One Card in the comparison view.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ComparisonRowDto {
    /// The Card's latest live version.
    pub version_id: String,
    /// Namespace.
    pub ns: String,
    /// Record name.
    pub name: String,
    /// Sequence number of that version.
    pub seq: i32,
    /// The record's title.
    pub title: Option<String>,
    /// sha256 of the canonical body.
    pub content_hash: String,
    /// Per-facet fingerprints, hex, keyed by facet name.
    pub fingerprints: BTreeMap<String, String>,
    /// The Card's harness fingerprint equals the Eval's.
    pub same_harness: bool,
    /// The Card's model fingerprint equals the Eval's.
    pub same_model: bool,
}

/// Cards sharing one fingerprint of the facet named in `group_by`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ComparisonGroupDto {
    /// The shared fingerprint in hex, or `null` for Cards with none.
    pub key: Option<String>,
    /// The Cards in the group.
    pub items: Vec<ComparisonRowDto>,
}

/// The comparison view: Cards measured on one Eval version.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ComparisonDto {
    /// Every Card, when `group_by` was not given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<ComparisonRowDto>>,
    /// The Cards grouped by the requested facet fingerprint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<ComparisonGroupDto>>,
}

/// The seven facets, for validating `group_by`.
const FACETS: [&str; 7] = [
    "model",
    "task",
    "harness",
    "generation",
    "trial",
    "grading",
    "env",
];

fn hex_map(fingerprints: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, String> {
    fingerprints
        .into_iter()
        .map(|(facet, bytes)| (facet, hex::encode(bytes)))
        .collect()
}

/// Resolve `{name}` / `{name}@{seq}` / `{name}@{label}` to a version the
/// caller may see.
async fn version_of(
    state: &AppState,
    record_type: RecordType,
    ns: &str,
    name: &str,
    caller_ns: &[String],
) -> Result<records::StoredVersion, ApiError> {
    let pool = state.db()?;
    let stored = match name.split_once('@') {
        None => records::get_latest(pool, record_type, ns, name, caller_ns).await?,
        Some((base, suffix)) if base.is_empty() || suffix.is_empty() => {
            return Err(ApiError::BadRequest);
        }
        Some((base, suffix)) if suffix.bytes().all(|b| b.is_ascii_digit()) => {
            let seq: i32 = suffix.parse().map_err(|_| ApiError::NotFound)?;
            records::get_by_seq(pool, record_type, ns, base, seq, caller_ns).await?
        }
        Some((base, label)) => {
            records::get_by_label(pool, record_type, ns, base, label, caller_ns).await?
        }
    };
    stored.ok_or(ApiError::NotFound)
}

async fn add(
    record_type: RecordType,
    state: AppState,
    caller: Auth,
    path: RelationPath,
    body: NewRelationBody,
) -> Result<(StatusCode, Json<RelationDto>), ApiError> {
    let Auth(caller) = caller;
    if !caller.allows(&path.ns, Scope::Write) {
        return Err(ApiError::Forbidden);
    }
    let caller_ns = caller.namespaces();
    let from = version_of(&state, record_type, &path.ns, &path.name, &caller_ns).await?;

    let parsed = parse_relation_target(&body.to).ok_or_else(|| {
        ApiError::Validation(vec![ErrorEntry {
            path: "/to".to_string(),
            code: ErrorCode::Schema,
            hint: Some(
                "expected `{ns}/{name}@{seq}`, `external:<url>` or `hf:<repo>@<sha>`".to_string(),
            ),
        }])
    })?;
    let target = match &parsed {
        ParsedTarget::Version { ns, name, seq } => RelationTarget::Version {
            ns,
            name,
            seq: i32::try_from(*seq).map_err(|_| {
                ApiError::Validation(vec![ErrorEntry {
                    path: "/to".to_string(),
                    code: ErrorCode::Schema,
                    hint: Some("the sequence number is out of range".to_string()),
                }])
            })?,
        },
        ParsedTarget::External(_) | ParsedTarget::Hf(_) => RelationTarget::External(&body.to),
    };

    let id = caller.identity()?;
    let edge = relations::add(
        state.db()?,
        from.meta.version_id,
        &body.relation_type,
        target,
        body.attrs.as_ref(),
        (Some(id.user_id), Some(id.token_id)),
        &caller_ns,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(edge.into())))
}

async fn traverse(
    record_type: RecordType,
    state: AppState,
    caller: MaybeAuth,
    path: RelationPath,
    query: TraverseQuery,
) -> Result<Json<GraphDto>, ApiError> {
    let MaybeAuth(caller) = caller;
    let caller_ns = caller.namespaces();
    let start = version_of(&state, record_type, &path.ns, &path.name, &caller_ns).await?;
    let types: Option<Vec<String>> = query.types.as_deref().map(|t| {
        t.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    });
    let graph = relations::traverse(
        state.db()?,
        start.meta.version_id,
        TraverseParams {
            direction: query.direction.unwrap_or(DirectionParam::Out).into(),
            types,
            depth: query.depth.unwrap_or(1),
            follow_latest: query.follow_latest.unwrap_or(false),
        },
        &caller_ns,
    )
    .await?;

    Ok(Json(GraphDto {
        nodes: graph
            .nodes
            .into_iter()
            .map(|n| {
                let content_hash = hex::encode(n.content_hash);
                if n.visible {
                    NodeDto {
                        version_id: n.version_id.to_string(),
                        ns: Some(n.ns),
                        name: Some(n.name),
                        seq: Some(n.seq),
                        record_type: Some(n.record_type),
                        tombstoned: Some(n.tombstoned),
                        private: false,
                        content_hash,
                    }
                } else {
                    NodeDto {
                        version_id: n.version_id.to_string(),
                        ns: None,
                        name: None,
                        seq: None,
                        record_type: None,
                        tombstoned: None,
                        private: true,
                        content_hash,
                    }
                }
            })
            .collect(),
        edges: graph
            .edges
            .into_iter()
            .map(|e| {
                let (to, to_text) = match e.to {
                    EdgeEnd::Version(id) => (Some(id.to_string()), None),
                    EdgeEnd::Text(t) => (None, Some(t)),
                };
                EdgeDto {
                    from: e.from.to_string(),
                    relation_type: e.relation_type,
                    to,
                    to_text,
                    attrs: e.attrs,
                }
            })
            .collect(),
    }))
}

/// `POST /api/v1/cards/{ns}/{name}@{seq}/relations` — add an edge.
pub async fn add_card_relation(
    State(state): State<AppState>,
    caller: Auth,
    Path(path): Path<RelationPath>,
    Json(body): Json<NewRelationBody>,
) -> Result<(StatusCode, Json<RelationDto>), ApiError> {
    add(RecordType::Card, state, caller, path, body).await
}

/// `POST /api/v1/evals/{ns}/{name}@{seq}/relations` — add an edge.
pub async fn add_eval_relation(
    State(state): State<AppState>,
    caller: Auth,
    Path(path): Path<RelationPath>,
    Json(body): Json<NewRelationBody>,
) -> Result<(StatusCode, Json<RelationDto>), ApiError> {
    add(RecordType::Eval, state, caller, path, body).await
}

/// `GET /api/v1/cards/{ns}/{name}[@…]/relations` — walk the graph.
pub async fn card_relations(
    State(state): State<AppState>,
    caller: MaybeAuth,
    Path(path): Path<RelationPath>,
    Query(query): Query<TraverseQuery>,
) -> Result<Json<GraphDto>, ApiError> {
    traverse(RecordType::Card, state, caller, path, query).await
}

/// `GET /api/v1/evals/{ns}/{name}[@…]/relations` — walk the graph.
pub async fn eval_relations(
    State(state): State<AppState>,
    caller: MaybeAuth,
    Path(path): Path<RelationPath>,
    Query(query): Query<TraverseQuery>,
) -> Result<Json<GraphDto>, ApiError> {
    traverse(RecordType::Eval, state, caller, path, query).await
}

/// `GET /api/v1/evals/{ns}/{name}[@…]/cards` — the comparison view.
pub async fn eval_cards(
    State(state): State<AppState>,
    MaybeAuth(caller): MaybeAuth,
    Path(path): Path<RelationPath>,
    Query(query): Query<ComparisonQuery>,
) -> Result<Json<ComparisonDto>, ApiError> {
    let facet = match query.group_by.as_deref() {
        None => None,
        Some(spec) => {
            let facet = spec
                .strip_prefix("fingerprint.")
                .filter(|f| FACETS.contains(f))
                .ok_or(ApiError::BadRequest)?;
            Some(facet.to_string())
        }
    };
    let caller_ns = caller.namespaces();
    let eval = version_of(&state, RecordType::Eval, &path.ns, &path.name, &caller_ns).await?;
    let rows = relations::cards_using_eval(state.db()?, eval.meta.version_id, &caller_ns).await?;

    let items: Vec<ComparisonRowDto> = rows
        .into_iter()
        .map(|r| ComparisonRowDto {
            version_id: r.version_id.to_string(),
            ns: r.ns,
            name: r.name,
            seq: r.seq,
            title: r.title,
            content_hash: hex::encode(r.content_hash),
            fingerprints: hex_map(r.fingerprints),
            same_harness: r.same_harness,
            same_model: r.same_model,
        })
        .collect();

    Ok(Json(match facet {
        None => ComparisonDto {
            items: Some(items),
            groups: None,
        },
        Some(facet) => {
            // A `BTreeMap` keyed by the fingerprint, so groups come out in
            // a stable order and `None` (no fingerprint for that facet)
            // sorts first rather than disappearing.
            let mut grouped: BTreeMap<Option<String>, Vec<ComparisonRowDto>> = BTreeMap::new();
            for row in items {
                let key = row.fingerprints.get(&facet).cloned();
                grouped.entry(key).or_default().push(row);
            }
            ComparisonDto {
                items: None,
                groups: Some(
                    grouped
                        .into_iter()
                        .map(|(key, items)| ComparisonGroupDto { key, items })
                        .collect(),
                ),
            }
        }
    }))
}
