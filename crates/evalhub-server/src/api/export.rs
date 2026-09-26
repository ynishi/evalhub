//! Export handlers.
//!
//! - `hf-model-index`: projects a Card's `results` / `model` / `task` onto
//!   the `model-index` YAML a Hugging Face model card embeds. Read
//!   compatibility only; the hub does not write to the Hub.
//! - `bundle`: a tar of the record JSON plus its attachments. For a public
//!   Card that references a private Eval, the bundle carries the Eval's
//!   `version_id` and attachment sha256s but not the bytes.
//!
//! Neither format carries a Card's `run_results`, and neither may start
//! to without the redaction the read paths apply (`withhold` in
//! `crate::api::relations`): a judgement names an Eval and one of its
//! runs, and for an Eval the reader may not see, both are withheld.
//! `hf-model-index` projects `results`, `model` and `task` only.
//!
//! # `hf-model-index`
//!
//! The projection is deliberately lossy and one-way. A Card carries seven
//! facets, uncertainty, counts, provenance and redaction; `model-index`
//! carries a task, a dataset and a list of metric values. What survives is
//! the score and enough of its context to find the Card again, which is
//! what the `source` link is for:
//!
//! ```yaml
//! model-index:
//!   - name: qwen3.6-32b
//!     results:
//!       - task: { type: single2, name: single2 }
//!         dataset: { type: single2, name: single2, split: test }
//!         metrics:
//!           - { type: core/pass_rate, value: 0.62, name: core/pass_rate }
//!         source: { name: "evalhub alice/single2@1", url: "https://…" }
//! ```
//!
//! An Eval has no results, so asking for this format on one is `400`
//! rather than an empty document: a model card with an empty
//! `model-index` would claim less than nothing.
//!
//! The YAML writer is `serde_norway`, the maintained fork of
//! `serde_yaml`; `serde_yml`, the other successor, publishes itself as
//! deprecated and unmaintained.
//!
//! # `bundle` is not implemented (M4)
//!
//! It is not merely unwritten: as specified it contradicts a decision the
//! rest of the hub is built on. `evalhub_store::objects` states that the
//! hub never receives attachment bytes — uploads and downloads are
//! presigned URLs straight to the object store, which is what lets the
//! server stay stateless and lets egress-free storage do the serving. A
//! `bundle` that tars attachments streams every one of them through the
//! hub, which is the opposite. Whether the answer is a redirect to a
//! pre-built bundle, a manifest of presigned URLs, or an accepted
//! exception for small records, is a design question and not a coding
//! task. Until it is answered the endpoint is `501` with that hint.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use evalhub_store::records::{self, RecordType};

use crate::api::records::RecordPath;
use crate::auth::MaybeAuth;
use crate::error::ApiError;
use crate::state::AppState;

/// Query string of the export endpoints.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportQuery {
    /// `hf-model-index` or `bundle`.
    pub format: String,
}

/// One entry of a `model-index` `metrics` list.
#[derive(Debug, Serialize)]
struct HfMetric {
    #[serde(rename = "type")]
    metric_type: String,
    value: f64,
    name: String,
}

/// The `task` of a `model-index` result.
#[derive(Debug, Serialize)]
struct HfTask {
    #[serde(rename = "type")]
    task_type: String,
    name: String,
}

/// The `dataset` of a `model-index` result.
#[derive(Debug, Serialize)]
struct HfDataset {
    #[serde(rename = "type")]
    dataset_type: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    split: Option<String>,
}

/// Where the numbers came from, so a reader can get back to the Card.
#[derive(Debug, Serialize)]
struct HfSource {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

/// One `model-index` result.
#[derive(Debug, Serialize)]
struct HfResult {
    task: HfTask,
    dataset: HfDataset,
    metrics: Vec<HfMetric>,
    source: HfSource,
}

/// One `model-index` entry.
#[derive(Debug, Serialize)]
struct HfModel {
    name: String,
    results: Vec<HfResult>,
}

/// The document a model card embeds.
#[derive(Debug, Serialize)]
struct HfModelIndex {
    #[serde(rename = "model-index")]
    model_index: Vec<HfModel>,
}

/// The URL this request arrived on, for the `source` link.
fn self_url(headers: &HeaderMap, path: &str) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("http");
    Some(format!("{proto}://{host}{path}"))
}

/// Project a Card body onto `model-index`.
fn to_model_index(
    body: &Value,
    subject: &str,
    url: Option<String>,
) -> Result<HfModelIndex, ApiError> {
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .filter(|r| !r.is_empty())
        .ok_or(ApiError::BadRequest)?;

    let model_name = body
        .pointer("/model/id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let task_id = body
        .pointer("/task/id")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let split = body
        .pointer("/task/split")
        .and_then(Value::as_str)
        .map(str::to_string);

    // Every result of one Card shares its task and dataset, so they
    // become one `model-index` result with several metrics.
    let metrics = results
        .iter()
        .filter_map(|r| {
            let metric = r.get("metric").and_then(Value::as_str)?;
            let value = r.get("value").and_then(Value::as_f64)?;
            Some(HfMetric {
                metric_type: metric.to_string(),
                value,
                name: metric.to_string(),
            })
        })
        .collect::<Vec<_>>();
    if metrics.is_empty() {
        return Err(ApiError::BadRequest);
    }

    Ok(HfModelIndex {
        model_index: vec![HfModel {
            name: model_name,
            results: vec![HfResult {
                task: HfTask {
                    task_type: task_id.clone(),
                    name: task_id.clone(),
                },
                dataset: HfDataset {
                    dataset_type: task_id.clone(),
                    name: task_id,
                    split,
                },
                metrics,
                source: HfSource {
                    name: format!("evalhub {subject}"),
                    url,
                },
            }],
        }],
    })
}

async fn export(
    record_type: RecordType,
    state: AppState,
    caller: MaybeAuth,
    path: RecordPath,
    query: ExportQuery,
    headers: HeaderMap,
    request_path: String,
) -> Result<Response, ApiError> {
    let MaybeAuth(caller) = caller;
    let pool = state.db()?;
    let caller_ns = caller.namespaces();

    let (name, seq) = match path.name.split_once('@') {
        None => (path.name.as_str(), None),
        Some((name, suffix)) => match suffix.parse::<i32>() {
            Ok(seq) if seq >= 1 => (name, Some(seq)),
            _ => return Err(ApiError::NotFound),
        },
    };
    let stored = match seq {
        None => records::get_latest(pool, record_type, &path.ns, name, &caller_ns).await?,
        Some(seq) => {
            records::get_by_seq(pool, record_type, &path.ns, name, seq, &caller_ns).await?
        }
    }
    .ok_or(ApiError::NotFound)?;
    let body = stored.body.ok_or(ApiError::NotFound)?;
    let subject = format!("{}/{}@{}", path.ns, name, stored.meta.seq);

    match query.format.as_str() {
        "hf-model-index" => {
            if record_type != RecordType::Card {
                return Err(ApiError::BadRequest);
            }
            let doc = to_model_index(&body, &subject, self_url(&headers, &request_path))?;
            let yaml = serde_norway::to_string(&doc)
                .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
            Ok(([(header::CONTENT_TYPE, "text/yaml; charset=utf-8")], yaml).into_response())
        }
        "bundle" => Err(ApiError::NotImplemented(
            "the bundle format is not decided yet: tarring attachments would stream them \
             through the hub, which the presigned-URL design exists to avoid",
        )),
        _ => Err(ApiError::BadRequest),
    }
}

/// `GET /api/v1/cards/{ns}/{name}[@{seq}]/export?format=` — project a Card.
pub async fn export_card(
    State(state): State<AppState>,
    caller: MaybeAuth,
    Path(path): Path<RecordPath>,
    Query(query): Query<ExportQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let request_path = format!(
        "/api/v1/cards/{}/{}/export?format={}",
        path.ns, path.name, query.format
    );
    export(
        RecordType::Card,
        state,
        caller,
        path,
        query,
        headers,
        request_path,
    )
    .await
}

/// `GET /api/v1/evals/{ns}/{name}[@{seq}]/export?format=` — project an Eval.
///
/// An Eval has no results, so `hf-model-index` is `400` here; the format
/// exists for Cards.
pub async fn export_eval(
    State(state): State<AppState>,
    caller: MaybeAuth,
    Path(path): Path<RecordPath>,
    Query(query): Query<ExportQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let request_path = format!(
        "/api/v1/evals/{}/{}/export?format={}",
        path.ns, path.name, query.format
    );
    export(
        RecordType::Eval,
        state,
        caller,
        path,
        query,
        headers,
        request_path,
    )
    .await
}

/// Keeps the status code used for an undecided format visible to a reader
/// of this module.
pub const BUNDLE_STATUS: StatusCode = StatusCode::NOT_IMPLEMENTED;
