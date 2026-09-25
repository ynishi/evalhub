//! Run handlers: the rows of an Eval, written one by one or in a batch,
//! read one by one or as the projection joined with Cards' judgements,
//! archived and deleted.
//!
//! An Eval is a versioned header plus a set of runs that belongs to the
//! record, not to a version (`evalhub_store::runs` has the model, the
//! write ordering and the hashes). Everything here is under
//! `/api/v1/evals/{ns}/{name}`:
//!
//! ```text
//! PUT    …/runs/{run_id}   Run                → 201 created | 200 updated | 200 unchanged
//!                                               { run_id, content_hash, status, result, runs_hash }
//! POST   …/runs:batch      { runs: [Run…] }   → 200 { runs: [{ run_id, content_hash, result }], runs_hash }
//! GET    …/runs?cards&where&sort&limit&cursor&include
//!                                             → 200 { cards: [...], items: [...], next_cursor }
//! GET    …/runs/{run_id}                      → 200 { run_id, content_hash, run, archived, created_at, updated_at }
//!                                               or, deleted, { run_id, content_hash, tombstone }
//! PATCH  …/runs/{run_id}   { archived }       → 200 the run as it now reads
//! DELETE …/runs/{run_id}   { reason, note? }  → 200 { run_id, content_hash, tombstone }
//! ```
//!
//! # Who may do what
//!
//! Runs have no visibility of their own; they follow their Eval. Every
//! route answers `404` for an Eval the caller may not see, exactly as for
//! one that does not exist, and so does every route of an Eval with no
//! live header (every version tombstoned: the runs are kept but
//! unreachable until a header is posted again). A `PATCH …/settings` that
//! makes the Eval private takes effect on the next request; there is no
//! cache.
//!
//! Writes (`PUT`, `POST …:batch`, `PATCH`, `DELETE`) need `write` on the
//! namespace. A caller without it gets `404` when they may not see the
//! Eval and `403` when they may (a public Eval), so a refused write says
//! no more than a read would. Reads need nothing for a public Eval.
//!
//! A *member* is a caller whose token covers the Eval's namespace. Only a
//! member sees archived runs: `GET …/runs/{run_id}` of an archived run is
//! `404` to anyone else, and `include=archived,deleted` on the projection
//! is honoured for members only (for anyone else it changes nothing, so
//! asking reveals nothing).
//!
//! # Writing
//!
//! `PUT` writes the body under the path's `run_id`. The body must carry
//! `run_id` (the run schema requires it) and it must equal the path's
//! (`run_id_mismatch`); the stored run is the body with the facets it
//! omits copied from the latest header. `201` when the id is new, `200`
//! otherwise, and `result` tells an overwrite (`updated`) from an
//! identical write (`unchanged`, nothing written, no audit row).
//!
//! `POST …/runs:batch` applies the same checks and rules to every element
//! of `runs[]`, each carrying its own `run_id`, in one transaction under
//! one lock of the record: all or nothing. More elements than
//! `limits.batch_runs` is `413 batch_too_large` before anything is looked
//! at. A refused batch writes nothing and lists every failing element,
//! each entry's `path` prefixed with `/runs/{index}`; a run id that
//! appears twice is `batch_duplicate_run_id` on the later element. The
//! response is `200` whether or not something was created; `result` per
//! element says what happened to it.
//!
//! A refusal is `422` when any entry is a validation failure and `409`
//! when every entry is a state conflict (`run_deleted`: the id was deleted
//! and is never reused; `attachment_missing`: an attachment is not
//! uploaded and confirmed). See [`crate::error`].
//!
//! # The projection
//!
//! `GET …/runs` is one page of the Eval's runs, each row with its columns
//! (`status`, `error.kind`, `started_at`, `ended_at`), its `metrics` and
//! facet fingerprints, and, per Card named in `cards=`, what that Card's
//! latest live version says about it (`evalhub_store::runs::project`).
//! The query string:
//!
//! | Parameter | Form                                                            |
//! | --------- | --------------------------------------------------------------- |
//! | `cards`   | `{ns}/{name}` of a Card; repeatable, in the order the columns come back |
//! | `where`   | the filter, as JSON text: the query grammar of `GET /schemas/query`, over the run paths below |
//! | `sort`    | `{path}` or `{path}:asc` / `{path}:desc`; repeatable, most significant first; `run_id` is always the last key |
//! | `limit`   | 1–200, default 50                                               |
//! | `cursor`  | the previous page's `next_cursor`                               |
//! | `include` | `archived`, `deleted` or `archived,deleted`; members only       |
//!
//! Paths are the run table's (`evalhub_query::PathTable::for_runs`):
//! `run_id`, `status`, `error.kind`, `started_at`, `ended_at`, the facet
//! leaves (`model.id`, `generation.temperature`, …),
//! `fingerprint.{facet}`, `metrics[{ns}/{name}]`,
//! `results[{card}][{metric}].value` / `.label` for a Card in `cards=`,
//! and registered `ext.{ns}.…` keys.
//!
//! A Card in `cards=` that does not exist, that the caller may not see,
//! or whose latest live version does not use this Eval is `404` — the
//! same empty `404` as for an unknown Eval, so the three cannot be told
//! apart. A value that is not `{ns}/{name}` at all is the same `404`. For
//! each Card the envelope reports its used set: `runs_used`,
//! `used_set_hash` (over the used runs' current content hashes),
//! `posted_used_set_hash` (over the hashes recorded when the Card was
//! posted) and `changed_since_card` (the used runs overwritten since).
//! Every run in a named Card's used set is a row whatever its state, so a
//! judgement of a run since archived or deleted still has a row; such a
//! row shows `state` and nothing of the run to a non-member.
//!
//! # Paging
//!
//! The cursor is `evalhub_query::RunCursor` (the last row's sort keys and
//! its `run_id`) signed by [`crate::auth::CursorSigner`], as every other
//! listing's. Its payload also names the Eval record and the `sort` it was
//! issued for, and carries a tag the record cursors do not have: a cursor
//! from another Eval, another sort or another endpoint is `400`, and the
//! page sequence starts again rather than silently skipping rows.
//!
//! # Deleting
//!
//! `DELETE` is the run's tombstone, with the same reasons as a version's.
//! The body and the run's attachment references go; `run_id`, the content
//! hash, the metrics and the fingerprints stay, so `runs_hash` does not
//! change and a Card that judged the run still has what it judged
//! against. A deleted run reads as `{ run_id, content_hash, tombstone }`.
//! Deleting twice, or archiving a deleted run, is `409 run_deleted`.
//!
//! # Audit
//!
//! `run.create` / `run.update` (one row per run that changed; none for an
//! unchanged one), `run.archive` / `run.unarchive`, `run.delete`, subject
//! `eval/{ns}/{name}/runs/{run_id}`, all readable at `GET /audit`.

use std::collections::BTreeMap;

use aide::OperationInput;
use aide::generate::GenContext;
use aide::openapi::Operation;
use axum::Json;
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use evalhub_core::VersionId;
use evalhub_query::{CardRef, RunCursor};
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_schema::query::{Dir, QueryRequest, Sort};
use evalhub_store::auth::Scope;
use evalhub_store::error::StoreError;
use evalhub_store::records::{self, RecordType, RunsSummary, TombstoneRequest};
use evalhub_store::runs::{
    self, ProjectedCard, ProjectedRun, RunChange, RunInclude, RunState, RunsWritten, StoredRun,
};

use crate::api::records::{TombstoneBody, TombstoneDto};
use crate::auth::{Auth, Caller, MaybeAuth};
use crate::error::ApiError;
use crate::state::AppState;

/// Largest page of the projection.
const MAX_LIMIT: u32 = 200;
/// Page size when the request does not say.
const DEFAULT_LIMIT: u32 = 50;

/// Path of an Eval's runs: `/evals/{ns}/{name}/runs` and
/// `/evals/{ns}/{name}/runs:batch`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct EvalRunsPath {
    /// Namespace of the Eval: a user login or an organisation slug.
    pub ns: String,
    /// Name of the Eval. No `@{seq}`: runs belong to the record, not to a
    /// version.
    pub name: String,
}

/// Path of one run: `/evals/{ns}/{name}/runs/{run_id}`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunPath {
    /// Namespace of the Eval: a user login or an organisation slug.
    pub ns: String,
    /// Name of the Eval. No `@{seq}`: runs belong to the record, not to a
    /// version.
    pub name: String,
    /// The run's id within the Eval: non-empty, no `/`, at most 200 bytes.
    pub run_id: String,
}

/// What a write did to one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunWriteResult {
    /// There was no run with this id; it was created.
    Created,
    /// A run with this id and other content was overwritten.
    Updated,
    /// The run already had this content; nothing was written.
    Unchanged,
}

impl From<RunChange> for RunWriteResult {
    fn from(c: RunChange) -> Self {
        match c {
            RunChange::Created => Self::Created,
            RunChange::Updated => Self::Updated,
            RunChange::Unchanged => Self::Unchanged,
        }
    }
}

/// Response of `PUT …/runs/{run_id}`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunWrittenDto {
    /// The id the run is stored under.
    pub run_id: String,
    /// Hex sha256 of the stored run (`evalhub_core::run::run_content_hash`):
    /// the body as sent, with the facets it omitted copied from the latest
    /// header, in canonical form.
    pub content_hash: String,
    /// The run's `status`: `ok`, `error` or `skipped`.
    pub status: String,
    /// What the write did: `created` (`201`), `updated` or `unchanged`
    /// (both `200`).
    pub result: RunWriteResult,
    /// Hex `runs_hash` of the Eval after the write: the digest over every
    /// run of the record, archived and deleted included.
    pub runs_hash: String,
}

/// Body of `POST …/runs:batch`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchBody {
    /// The runs to write, each a run body (`GET /schemas/run`) carrying
    /// its own `run_id`. At most `limits.batch_runs` elements.
    pub runs: Vec<Value>,
}

/// One element of a batch response.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunWriteDto {
    /// The id the run is stored under.
    pub run_id: String,
    /// Hex sha256 of the stored run.
    pub content_hash: String,
    /// What the write did to this run.
    pub result: RunWriteResult,
}

/// Response of `POST …/runs:batch`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BatchResponse {
    /// One entry per element of the request, in request order.
    pub runs: Vec<RunWriteDto>,
    /// Hex `runs_hash` of the Eval after the batch.
    pub runs_hash: String,
}

impl From<RunsWritten> for BatchResponse {
    fn from(w: RunsWritten) -> Self {
        Self {
            runs: w
                .runs
                .into_iter()
                .map(|r| RunWriteDto {
                    run_id: r.run_id,
                    content_hash: hex::encode(r.content_hash),
                    result: r.change.into(),
                })
                .collect(),
            runs_hash: hex::encode(w.runs_hash),
        }
    }
}

/// The runs split off an `evalhub.eval/1.0` body by `POST /evals/{ns}/{name}`,
/// as `POST …/runs:batch` reports runs, plus the ids the body repeated.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ConvertedRunsDto {
    /// One entry per distinct `run_id`, in the order each id first
    /// appeared in the body's `runs[]`.
    pub runs: Vec<RunWriteDto>,
    /// Hex `runs_hash` of the Eval after the write.
    pub runs_hash: String,
    /// Every `run_id` that appeared more than once in `runs[]`; the last
    /// element carrying it is the one written. 0.1.x accepted such
    /// bodies, so they are not refused.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub duplicate_run_ids: Vec<String>,
}

impl From<records::Converted> for ConvertedRunsDto {
    fn from(c: records::Converted) -> Self {
        let BatchResponse { runs, runs_hash } = c.runs.into();
        Self {
            runs,
            runs_hash,
            duplicate_run_ids: c.duplicate_run_ids,
        }
    }
}

/// Body of `PATCH …/runs/{run_id}`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArchiveBody {
    /// `true` hides the run from everyone but members of the namespace and
    /// from the summary counts; `false` shows it again. Changes no hash.
    pub archived: bool,
}

/// A run as read back.
///
/// A deleted run is `{ run_id, content_hash, tombstone }` and nothing
/// else, as a tombstoned version keeps its commitment and loses its body.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunEnvelope {
    /// The run's id within the Eval.
    pub run_id: String,
    /// Hex sha256 of the stored run. Kept when the run is deleted.
    pub content_hash: String,
    /// The stored run: the body as written, with the facets it omitted
    /// copied from the header then, in canonical form. Absent when deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<Value>,
    /// Whether the run is archived. Only members of the namespace ever see
    /// `true`. Absent when deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// When the id was first written. Absent when deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// When the content last changed, or the run was archived or
    /// unarchived. Absent when deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// Present when the run was deleted; everything but `run_id` and
    /// `content_hash` is then absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tombstone: Option<TombstoneDto>,
}

impl From<StoredRun> for RunEnvelope {
    fn from(r: StoredRun) -> Self {
        let content_hash = hex::encode(&r.content_hash);
        match r.tombstone {
            Some(t) => Self {
                run_id: r.run_id,
                content_hash,
                run: None,
                archived: None,
                created_at: None,
                updated_at: None,
                tombstone: Some(t.into()),
            },
            None => Self {
                run_id: r.run_id,
                content_hash,
                run: r.body,
                archived: Some(r.archived_at.is_some()),
                created_at: Some(r.created_at),
                updated_at: Some(r.updated_at),
                tombstone: None,
            },
        }
    }
}

/// Runs of an Eval by `status`, over those neither archived nor deleted.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunsByStatusDto {
    /// `status = ok`.
    pub ok: i64,
    /// `status = error`.
    pub error: i64,
    /// `status = skipped`.
    pub skipped: i64,
}

/// The `runs` block of an Eval's envelope (`GET /evals/{ns}/{name}[@…]`).
/// The runs belong to the record, so it is the same at every `@seq`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunsSummaryDto {
    /// Runs that are neither archived nor deleted.
    pub count: i64,
    /// `count` by `status`.
    pub by_status: RunsByStatusDto,
    /// Archived runs that are not deleted. Members of the namespace only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<i64>,
    /// Deleted runs. Members of the namespace only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<i64>,
    /// Hex `runs_hash`: the one digest over every run of the record,
    /// archived and deleted included, the same for every reader. It
    /// changes when a run is added or overwritten, so a non-member learns
    /// that some run changed and nothing else. `sha256("[]")` when the
    /// Eval never had a run.
    pub runs_hash: String,
}

impl From<RunsSummary> for RunsSummaryDto {
    fn from(s: RunsSummary) -> Self {
        Self {
            count: s.count,
            by_status: RunsByStatusDto {
                ok: s.by_status.ok,
                error: s.by_status.error,
                skipped: s.by_status.skipped,
            },
            archived: s.archived,
            deleted: s.deleted,
            runs_hash: hex::encode(s.runs_hash),
        }
    }
}

/// Query string of `GET …/runs`, as documented. The handler reads it with
/// [`RunsParams`], because `cards` and `sort` repeat.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunsQuery {
    /// `{ns}/{name}` of a Card whose judgements to join; repeat for
    /// several. Each must be visible to the caller and use this Eval, or
    /// the answer is `404`.
    pub cards: Option<Vec<String>>,
    /// The filter as JSON text, in the query grammar (`GET
    /// /schemas/query`) over the run paths: `run_id`, `status`,
    /// `error.kind`, `started_at`, `ended_at`, facet keys such as
    /// `model.id`, `fingerprint.{facet}`, `metrics[{ns}/{name}]`,
    /// `results[{card}][{metric}].value` / `.label`, and registered `ext`
    /// keys.
    #[serde(rename = "where")]
    pub where_: Option<String>,
    /// A sort key, `{path}`, `{path}:asc` or `{path}:desc`; repeat for
    /// several, most significant first. `run_id` is always the last key.
    pub sort: Option<Vec<String>>,
    /// Page size, 1–200; default 50.
    pub limit: Option<u32>,
    /// Cursor from a previous page's `next_cursor`.
    pub cursor: Option<String>,
    /// `archived`, `deleted` or both, comma-separated: also list those
    /// runs. Honoured for members of the namespace only.
    pub include: Option<String>,
}

/// The parsed query string of `GET …/runs`: [`RunsQuery`] with the
/// repeatable keys collected. An unknown key or a `limit` that is not a
/// number is `400`; a repeated single-valued key keeps its last value.
#[derive(Debug, Default)]
pub struct RunsParams {
    cards: Vec<String>,
    where_: Option<String>,
    sort: Vec<String>,
    limit: Option<u32>,
    cursor: Option<String>,
    include: Option<String>,
}

impl<S: Send + Sync> FromRequestParts<S> for RunsParams {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // `serde_urlencoded` (what axum's `Query` uses) reads a query
        // string as a sequence of pairs, which keeps repeated keys.
        let Query(pairs) = Query::<Vec<(String, String)>>::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::BadRequest)?;
        let mut out = Self::default();
        for (key, value) in pairs {
            match key.as_str() {
                "cards" => out.cards.push(value),
                "where" => out.where_ = Some(value),
                "sort" => out.sort.push(value),
                "limit" => out.limit = Some(value.parse().map_err(|_| ApiError::BadRequest)?),
                "cursor" => out.cursor = Some(value),
                "include" => out.include = Some(value),
                _ => return Err(ApiError::BadRequest),
            }
        }
        Ok(out)
    }
}

impl OperationInput for RunsParams {
    fn operation_input(ctx: &mut GenContext, operation: &mut Operation) {
        Query::<RunsQuery>::operation_input(ctx, operation);
    }
}

/// How a row of the projection may be seen.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStateDto {
    /// Neither archived nor deleted.
    Live,
    /// Archived; only members of the namespace see this state.
    Archived,
    /// Deleted, or, for a reader who is not a member, archived. Only
    /// `run_id`, `content_hash` and the Cards' judgements are shown.
    Deleted,
}

impl From<RunState> for RunStateDto {
    fn from(s: RunState) -> Self {
        match s {
            RunState::Live => Self::Live,
            RunState::Archived => Self::Archived,
            RunState::Deleted => Self::Deleted,
        }
    }
}

/// One `run_results[]` entry of a Card for a run.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunJudgementDto {
    /// The metric judged, `{ns}/{name}`.
    pub metric: String,
    /// The numeric judgement, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// The categorical judgement, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Who or what judged, as the Card says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<Value>,
}

/// What one Card of the request says about one run.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunCardCellDto {
    /// The run is in the Card's used set for this Eval.
    pub used: bool,
    /// The run was overwritten since the Card used it.
    pub changed: bool,
    /// The Card's `run_results[]` entries for the run, in the Card's
    /// order. Empty when the Card did not judge it.
    pub results: Vec<RunJudgementDto>,
}

/// One row of the projection.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunRowDto {
    /// The run's id.
    pub run_id: String,
    /// Hex sha256 of the stored run, in every state.
    pub content_hash: String,
    /// How the caller may see the run. The run's own columns below are
    /// present exactly when it is not `deleted`.
    pub state: RunStateDto,
    /// `ok`, `error` or `skipped`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// `error.kind`, when `status` is `error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    /// `started_at`, when the run carried a parseable one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// `ended_at`, when the run carried a parseable one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// The run's `metrics`, keyed by metric id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<BTreeMap<String, f64>>,
    /// The run's facet fingerprints, hex, keyed by facet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprints: Option<BTreeMap<String, String>>,
    /// Per Card of the request, keyed by `{ns}/{name}`: the Card's
    /// judgements of this run.
    pub cards: BTreeMap<String, RunCardCellDto>,
}

/// A Card of the request, as the projection joined it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunPageCardDto {
    /// The Card, `{ns}/{name}`.
    pub card: String,
    /// Its latest live version, whose judgements are joined (ULID).
    pub version_id: String,
    /// That version's sequence number.
    pub seq: i32,
    /// How many runs of this Eval the version used.
    pub runs_used: i64,
    /// Hex `runs_hash` over the used runs' current content hashes.
    pub used_set_hash: String,
    /// Hex `runs_hash` over the hashes recorded when the Card used the
    /// runs. Equal to `used_set_hash` exactly when `changed_since_card`
    /// is empty.
    pub posted_used_set_hash: String,
    /// The used runs overwritten since the Card used them, by `run_id`.
    pub changed_since_card: Vec<String>,
}

impl From<ProjectedCard> for RunPageCardDto {
    fn from(c: ProjectedCard) -> Self {
        Self {
            card: c.card.to_string(),
            version_id: VersionId::from_uuid(c.version_id).to_string(),
            seq: c.seq,
            runs_used: c.runs_used,
            used_set_hash: hex::encode(c.used_set_hash),
            posted_used_set_hash: hex::encode(c.posted_used_set_hash),
            changed_since_card: c.changed_since_card,
        }
    }
}

/// One page of the run projection.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunPageDto {
    /// The Cards of the request, in request order.
    pub cards: Vec<RunPageCardDto>,
    /// The runs on this page.
    pub items: Vec<RunRowDto>,
    /// Cursor for the next page, or `null` on the last page.
    pub next_cursor: Option<String>,
}

/// What a run cursor carries before signing. The tag and the binding to
/// the Eval and the sort are what make a cursor from anywhere else `400`
/// (see the module doc).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunCursorPayload {
    /// Always `"runs"`; the record cursors carry no such field.
    k: String,
    /// The Eval record the page belongs to.
    e: Uuid,
    /// The `sort` parameters as sent.
    s: Vec<String>,
    /// Where the page stopped.
    c: RunCursor,
}

const CURSOR_TAG: &str = "runs";

/// `404` for a run route of an Eval `caller` may not see, `403` when they
/// may see it but may not write, `Ok` when they may write.
async fn authorize_write(
    state: &AppState,
    caller: &Caller,
    ns: &str,
    name: &str,
) -> Result<(), ApiError> {
    if name.contains('@') {
        return Err(ApiError::BadRequest);
    }
    if caller.allows(ns, Scope::Write) {
        return Ok(());
    }
    let visible = records::get_latest(
        state.db()?,
        RecordType::Eval,
        ns,
        name,
        &caller.namespaces(),
    )
    .await?
    .is_some();
    Err(if visible {
        ApiError::Forbidden
    } else {
        ApiError::NotFound
    })
}

/// `PUT /api/v1/evals/{ns}/{name}/runs/{run_id}` — write one run.
pub async fn put_run(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Path(path): Path<RunPath>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<RunWrittenDto>), ApiError> {
    authorize_write(&state, &caller, &path.ns, &path.name).await?;
    let written = match runs::put(
        state.db()?,
        &path.ns,
        &path.name,
        &path.run_id,
        &body,
        caller.actor(),
    )
    .await
    {
        Ok(w) => w,
        // One element: its entries point into the body as sent.
        Err(StoreError::RunsRejected(r)) => return Err(ApiError::runs_rejected(r, false)),
        Err(e) => return Err(e.into()),
    };
    let runs_hash = hex::encode(&written.runs_hash);
    let run = written
        .runs
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("put returned no run")))?;
    let status = if run.change == RunChange::Created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(RunWrittenDto {
            run_id: run.run_id,
            content_hash: hex::encode(run.content_hash),
            status: run.status,
            result: run.change.into(),
            runs_hash,
        }),
    ))
}

/// `POST /api/v1/evals/{ns}/{name}/runs:batch` — write several runs, all
/// or nothing.
pub async fn batch_runs(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Path(path): Path<EvalRunsPath>,
    Json(body): Json<BatchBody>,
) -> Result<Json<BatchResponse>, ApiError> {
    authorize_write(&state, &caller, &path.ns, &path.name).await?;
    let max = state.config.limits.batch_runs;
    if body.runs.len() > max {
        return Err(ApiError::TooLarge(vec![ErrorEntry {
            path: "/runs".to_string(),
            code: ErrorCode::BatchTooLarge,
            hint: Some(format!(
                "{} runs in one batch; the limit is {max} (limits.batch_runs). \
                 Split the batch.",
                body.runs.len()
            )),
        }]));
    }
    let written = runs::put_batch(
        state.db()?,
        &path.ns,
        &path.name,
        &body.runs,
        caller.actor(),
    )
    .await?;
    Ok(Json(written.into()))
}

/// `GET /api/v1/evals/{ns}/{name}/runs/{run_id}` — read one run.
pub async fn get_run(
    State(state): State<AppState>,
    MaybeAuth(caller): MaybeAuth,
    Path(path): Path<RunPath>,
) -> Result<Json<RunEnvelope>, ApiError> {
    if path.name.contains('@') {
        return Err(ApiError::BadRequest);
    }
    let run = runs::get(
        state.db()?,
        &path.ns,
        &path.name,
        &path.run_id,
        &caller.namespaces(),
    )
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(Json(run.into()))
}

/// `PATCH /api/v1/evals/{ns}/{name}/runs/{run_id}` — archive or unarchive.
pub async fn patch_run(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Path(path): Path<RunPath>,
    Json(body): Json<ArchiveBody>,
) -> Result<Json<RunEnvelope>, ApiError> {
    authorize_write(&state, &caller, &path.ns, &path.name).await?;
    let pool = state.db()?;
    let run = if body.archived {
        runs::archive(pool, &path.ns, &path.name, &path.run_id, caller.actor()).await?
    } else {
        runs::unarchive(pool, &path.ns, &path.name, &path.run_id, caller.actor()).await?
    };
    Ok(Json(run.into()))
}

/// `DELETE /api/v1/evals/{ns}/{name}/runs/{run_id}` — delete (tombstone)
/// a run.
pub async fn delete_run(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Path(path): Path<RunPath>,
    Json(body): Json<TombstoneBody>,
) -> Result<Json<RunEnvelope>, ApiError> {
    authorize_write(&state, &caller, &path.ns, &path.name).await?;
    let run = runs::tombstone(
        state.db()?,
        &path.ns,
        &path.name,
        &path.run_id,
        TombstoneRequest {
            reason: body.reason.into(),
            note: body.note.as_deref(),
        },
        caller.actor(),
    )
    .await?;
    Ok(Json(run.into()))
}

/// `{path}` / `{path}:asc` / `{path}:desc` as a sort key. A suffix after
/// the last `:` that is neither direction is part of the path.
fn parse_sort(spec: &str) -> Sort {
    match spec.rsplit_once(':') {
        Some((path, "asc")) => Sort {
            path: path.to_string(),
            dir: Dir::Asc,
        },
        Some((path, "desc")) => Sort {
            path: path.to_string(),
            dir: Dir::Desc,
        },
        _ => Sort {
            path: spec.to_string(),
            dir: Dir::Asc,
        },
    }
}

/// `include=` as a [`RunInclude`]; anything but `archived` / `deleted`
/// is `400`.
fn parse_include(spec: Option<&str>) -> Result<RunInclude, ApiError> {
    let mut include = RunInclude::default();
    for part in spec
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        match part {
            "archived" => include.archived = true,
            "deleted" => include.deleted = true,
            _ => return Err(ApiError::BadRequest),
        }
    }
    Ok(include)
}

fn row_dto(run: ProjectedRun, cards: &[ProjectedCard]) -> RunRowDto {
    let detail = run.detail;
    RunRowDto {
        run_id: run.run_id,
        content_hash: hex::encode(run.content_hash),
        state: run.state.into(),
        status: detail.as_ref().map(|d| d.status.clone()),
        error_kind: detail.as_ref().and_then(|d| d.error_kind.clone()),
        started_at: detail.as_ref().and_then(|d| d.started_at),
        ended_at: detail.as_ref().and_then(|d| d.ended_at),
        metrics: detail.as_ref().map(|d| d.metrics.clone()),
        fingerprints: detail.map(|d| {
            d.fingerprints
                .into_iter()
                .map(|(facet, fp)| (facet, hex::encode(fp)))
                .collect()
        }),
        cards: cards
            .iter()
            .zip(run.cards)
            .map(|(card, cell)| {
                (
                    card.card.to_string(),
                    RunCardCellDto {
                        used: cell.used,
                        changed: cell.changed,
                        results: cell
                            .results
                            .into_iter()
                            .map(|j| RunJudgementDto {
                                metric: j.metric,
                                value: j.value,
                                label: j.label,
                                by: j.by,
                            })
                            .collect(),
                    },
                )
            })
            .collect(),
    }
}

/// `GET /api/v1/evals/{ns}/{name}/runs` — the run projection.
pub async fn list_runs(
    State(state): State<AppState>,
    MaybeAuth(caller): MaybeAuth,
    Path(path): Path<EvalRunsPath>,
    params: RunsParams,
) -> Result<Json<RunPageDto>, ApiError> {
    if path.name.contains('@') {
        return Err(ApiError::BadRequest);
    }
    let pool = state.db()?;
    let caller_ns = caller.namespaces();
    let eval = records::get_latest(pool, RecordType::Eval, &path.ns, &path.name, &caller_ns)
        .await?
        .ok_or(ApiError::NotFound)?;
    let record_id = eval.meta.record_id;

    // A malformed Card reference is a Card that does not exist: the same
    // 404 as every other unknown Card.
    let cards: Vec<CardRef> = params
        .cards
        .iter()
        .map(|c| CardRef::parse(c).ok_or(ApiError::NotFound))
        .collect::<Result<_, _>>()?;
    let include = parse_include(params.include.as_deref())?;
    let where_ = match &params.where_ {
        None => None,
        Some(text) => Some(serde_json::from_str::<Value>(text).map_err(|e| {
            ApiError::Validation(vec![ErrorEntry {
                path: "/where".to_string(),
                code: ErrorCode::Schema,
                hint: Some(format!("`where` must be JSON text: {e}")),
            }])
        })?),
    };
    let cursor = match &params.cursor {
        None => None,
        Some(c) => {
            let payload = state.cursors.verify(c).ok_or(ApiError::BadRequest)?;
            let p: RunCursorPayload =
                serde_json::from_slice(&payload).map_err(|_| ApiError::BadRequest)?;
            if p.k != CURSOR_TAG || p.e != record_id || p.s != params.sort {
                return Err(ApiError::BadRequest);
            }
            Some(p.c)
        }
    };
    let request = QueryRequest {
        where_,
        sort: params.sort.iter().map(|s| parse_sort(s)).collect(),
        limit: None,
        cursor: None,
        expand: Vec::new(),
        version: None,
    };
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let table = state.path_tables.current().await.for_runs(&cards);
    let query = evalhub_query::compile_runs(&request, &table, limit, cursor)
        .map_err(ApiError::Validation)?;

    let page = match runs::project(pool, record_id, &query, include, &caller_ns).await {
        Ok(Some(page)) => page,
        // The Eval was made private, or lost its last live header, between
        // the lookup above and the projection's snapshot.
        Ok(None) => return Err(ApiError::NotFound),
        // A literal the store cannot bind for its column (a `started_at`
        // that is not RFC 3339): the request's fault, said as such. The
        // message names the literal, never SQL.
        Err(StoreError::QueryUnsupported(hint)) => {
            return Err(ApiError::Validation(vec![ErrorEntry {
                path: "/where".to_string(),
                code: ErrorCode::TypeMismatch,
                hint: Some(hint),
            }]));
        }
        Err(e) => return Err(e.into()),
    };

    let next_cursor = match page.next {
        None => None,
        Some(c) => {
            let payload = serde_json::to_vec(&RunCursorPayload {
                k: CURSOR_TAG.to_string(),
                e: record_id,
                s: params.sort.clone(),
                c,
            })
            .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
            Some(state.cursors.sign(&payload))
        }
    };
    let items = page
        .runs
        .into_iter()
        .map(|r| row_dto(r, &page.cards))
        .collect();
    Ok(Json(RunPageDto {
        cards: page.cards.into_iter().map(RunPageCardDto::from).collect(),
        items,
        next_cursor,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_takes_an_optional_direction_after_the_last_colon() {
        let s = parse_sort("metrics[core/tokens_out]:desc");
        assert_eq!(s.path, "metrics[core/tokens_out]");
        assert_eq!(s.dir, Dir::Desc);
        let s = parse_sort("status");
        assert_eq!(s.path, "status");
        assert_eq!(s.dir, Dir::Asc);
        // Not a direction: the whole text is the path, and the type check
        // reports it.
        let s = parse_sort("status:up");
        assert_eq!(s.path, "status:up");
    }

    #[test]
    fn include_names_archived_and_deleted_only() {
        let i = parse_include(Some("archived, deleted")).ok();
        assert_eq!(
            i,
            Some(RunInclude {
                archived: true,
                deleted: true
            })
        );
        assert_eq!(parse_include(None).ok(), Some(RunInclude::default()));
        assert!(parse_include(Some("everything")).is_err());
    }
}
