//! The Card record: what was measured, how, and what the score was.
//!
//! A Card is the unit a harness publishes after an evaluation. It is a claim
//! made by the producer, kept verbatim by the hub. The hub adds an id, a
//! sequence number, a content hash, badges and fingerprints around it, but
//! never edits the record itself; a correction is a new version.
//!
//! # Shape
//!
//! ```text
//! Card
//! ├── schema        "evalhub.card/1.1" (or "evalhub.card/1.0")
//! ├── title         free text
//! ├── producer      { name, version }            who wrote this record
//! ├── model / task / harness / generation / trial / grading / env
//! │                 seven facets (see `facet`)   what was measured, under what conditions
//! ├── results[]     { metric, value, n, aggregation, uncertainty, by, samples_ref }
//! ├── counts        { attempted, completed, failed, skipped, errored }
//! ├── run_results[] { eval, run_id, metric, value?, label?, by?, from? }
//! │                 per-run verdicts on the runs of a cited Eval (1.1)
//! ├── relations[]   { type, to, attrs }          edges to other records
//! ├── attachments[] { path, sha256, size, media_type }
//! ├── redaction     { applied, method, fields }
//! ├── provenance    { source_type, evaluator_relationship }
//! └── ext           { "{ns}/{name}": { ... } }   producer-private extension
//! ```
//!
//! A complete example:
//!
//! ```json
//! {
//!   "schema": "evalhub.card/1.1",
//!   "title": "qwen3.6-32b on single2 K4",
//!   "producer": {"name": "my-harness", "version": "0.4.1"},
//!   "model": {"id": "qwen3.6-32b", "provider": "local", "quantization": {"method": "awq", "dtype": "int4"}},
//!   "task": {"id": "single2", "version": "3", "split": "test", "n": 33, "seed": 7},
//!   "harness": {"name": "my-harness", "version": "0.4.1", "prompt_template_sha256": "…"},
//!   "generation": {"temperature": 0.0, "max_tokens": 4096},
//!   "trial": {"k": 4, "pass_at_k": true, "max_turns": 20, "sandbox": {"image": "ghcr.io/x/y", "digest": "sha256:…"}},
//!   "grading": {"graders": [{"name": "exact_match", "kind": "deterministic"}]},
//!   "env": {"git": {"origin": "https://github.com/x/y", "commit": "abc123", "dirty": false}},
//!   "results": [{
//!     "metric": "core/pass_rate",
//!     "value": 0.62, "n": 33,
//!     "aggregation": "mean",
//!     "uncertainty": {"stderr": 0.08, "ci": {"level": 0.95, "low": 0.45, "high": 0.77}},
//!     "by": {"grader": "exact_match"},
//!     "samples_ref": "samples.jsonl"
//!   }],
//!   "counts": {"attempted": 33, "completed": 30, "failed": 2, "skipped": 1, "errored": 0},
//!   "relations": [{"type": "core/uses_eval", "to": "alice/single2-k4@3"}],
//!   "attachments": [{"path": "samples.jsonl", "sha256": "…", "size": 12345, "media_type": "application/x-ndjson"}],
//!   "redaction": {"applied": true, "method": "regex+llm", "fields": ["samples.jsonl:response"]},
//!   "provenance": {"source_type": "evaluation_run", "evaluator_relationship": "first_party"},
//!   "ext": {"alice/qwen-loop": {"rung": 4}}
//! }
//! ```
//!
//! # Results
//!
//! `results[].metric` is a registry id in the form `{ns}/{name}`
//! (`core/pass_rate`). A metric the registry does not know is still accepted
//! if it has that form; a bare name is rejected (`metric_id_invalid`). Whether
//! the metric is registered is surfaced as the `metric_registered` badge, not
//! as an error — the hub records claims, it does not gate on vocabulary.
//!
//! `aggregation` names how `value` was derived from the samples (`mean`,
//! `pass_at_k`, `median`, `sum`, `custom`). `uncertainty` carries whatever the
//! producer computed (standard error, a confidence interval). `by` records the
//! grader or other partition the number applies to. `samples_ref` names an
//! entry in `attachments[]` holding the per-sample rows; the hub checks that
//! the path exists in `attachments[]` and nothing else about it.
//!
//! # Counts
//!
//! If `results` is non-empty, `counts` is required and must satisfy
//! `attempted >= completed + failed + skipped + errored`. This is the one
//! arithmetic rule the hub enforces, because a score without the denominator
//! it was computed over is not comparable to anything.
//!
//! # Run results
//!
//! ```text
//! relations[core/uses_eval] { to: "{ns}/{name}@{seq}", attrs: { runs?: [run_id] } }
//! run_results[]             { eval: "{ns}/{name}", run_id, metric, value?, label?, by?, from? }
//! ```
//!
//! `results[]` is the aggregate; `run_results[]` is the verdict per run
//! that the aggregate was computed from. A run of an Eval records what
//! happened and what was measured directly (`crate::run`); it never says
//! whether it passed. That judgement depends on a grader, so it is the
//! Card's claim and lives here: one element per `(run, metric)` the Card
//! judged, with a number (`value`), a label (`label`, such as `pass` or
//! `fail`), or both. At least one of the two is present
//! (`run_result_value_or_label`). `by` partitions the verdict the way
//! `results[].by` does, and `from` optionally names the run's `metrics`
//! keys the verdict was derived from. The hub does not recompute anything
//! and does not check that `results[]` agrees with `run_results[]`; how the
//! verdicts were produced is the grading facet's to describe.
//!
//! Re-grading appends a Card version; the earlier verdicts stay with the
//! earlier version. Grading the same runs with a different grader is a
//! different Card.
//!
//! `run_results` was added in `evalhub.card/1.1`. It is an optional key, so
//! the bump is minor: one schema document serves both identifiers, a Card
//! declaring `evalhub.card/1.0` may carry `run_results` too, and Cards
//! stored under 1.0 are not rewritten.
//!
//! ## The used set
//!
//! Which runs a Card used is decided per cited Eval record, and that set of
//! `run_id`s is the Card's *used set* for the record. For each Eval record,
//! it is the union of `attrs.runs` over the Card's `core/uses_eval`
//! relations that resolve to that record; or, when none of those relations
//! carries `attrs.runs`, every run of the record that is neither archived
//! nor deleted at the moment the Card is posted. The hub fixes the used set
//! when the Card is posted and remembers each run's content hash with it,
//! which is how a later change to a run is detected.
//!
//! Against the used set:
//!
//! - `run_results[].eval` names the record (`{ns}/{name}`, no `@seq`) of
//!   one of the Card's `core/uses_eval` relations
//!   (`run_results_eval_unknown`).
//! - every `run_id` in a used set has a row in that Eval, archived and
//!   deleted rows included (`run_unknown`). A run is always in the hub, so
//!   a missing row is a mistake, not an unresolved reference, and an Eval
//!   the writer may not see answers the same way as a run that does not
//!   exist.
//! - every `run_results[].run_id` is in the used set of its `eval`
//!   (`run_not_in_used_set`).
//!
//! # Provenance and redaction
//!
//! `provenance` says where the numbers came from (`evaluation_run`,
//! `paper`, `model_card`, …) and who ran it relative to the model
//! (`first_party`, `third_party`). `redaction` says whether the producer
//! removed anything before publishing, by what method, and from which fields.
//! The hub does not redact; it only makes the producer's statement visible
//! and searchable, and awards the `redacted` badge when `applied` is true.
//!
//! # Fields the hub owns
//!
//! `id`, `version_id`, `seq`, `label`, `content_hash`, `created_at`,
//! `changed[]`, `badges[]` and the per-facet fingerprints are not part of
//! this type. They are returned alongside it in API responses and are
//! documented in `evalhub_server::api::records`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Attachment, Ext, Producer, Redaction, Relation};
use crate::facet::{Env, Generation, Grading, Harness, Model, Task, Trial};

/// A Card: what was measured, how, and what the score was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Card {
    /// Schema identifier: `evalhub.card/1.1`, or `evalhub.card/1.0`, which is
    /// still accepted. Both are described by this one schema.
    #[schemars(extend("enum" = ["evalhub.card/1.0", "evalhub.card/1.1"]))]
    pub schema: String,
    /// Human-readable title.
    pub title: String,
    /// The software that wrote this record.
    pub producer: Producer,
    /// The model facet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    /// The task facet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Task>,
    /// The harness facet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<Harness>,
    /// The generation facet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<Generation>,
    /// The trial facet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial: Option<Trial>,
    /// The grading facet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grading: Option<Grading>,
    /// The environment facet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Env>,
    /// The scores.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<ResultEntry>,
    /// Item counts the scores were computed over. Required when `results` is non-empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<Counts>,
    /// Per-run verdicts on the runs of the Evals this Card uses
    /// (`core/uses_eval`). Each `run_id` must be in the Card's used set for
    /// its `eval`. Added in `evalhub.card/1.1`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run_results: Vec<RunResult>,
    /// Edges to other records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    /// Files this record refers to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// What the producer removed before publishing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction: Option<Redaction>,
    /// Where the numbers came from and who ran the evaluation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    /// Producer-private extensions keyed by namespace.
    #[serde(default)]
    pub ext: Ext,
}

/// One score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResultEntry {
    /// Metric as a registry id, `{ns}/{name}` (for example `core/pass_rate`).
    pub metric: String,
    /// The score.
    pub value: f64,
    /// Number of samples the score was computed over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    /// How `value` was derived from the samples.
    pub aggregation: Aggregation,
    /// Uncertainty the producer computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<Uncertainty>,
    /// The partition this score applies to (for example which grader).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<serde_json::Map<String, serde_json::Value>>,
    /// `attachments[].path` of the per-sample rows this score summarises.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples_ref: Option<String>,
}

/// The Card's verdict on one run for one metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunResult {
    /// The Eval record the run belongs to, `{ns}/{name}` (no `@seq`: runs
    /// belong to the record, not to a version). Must be the record of one of
    /// this Card's `core/uses_eval` relations.
    pub eval: String,
    /// The run judged. Must be in this Card's used set for `eval`.
    pub run_id: String,
    /// Metric as a registry id, `{ns}/{name}` (for example `core/pass`).
    pub metric: String,
    /// The verdict as a number. At least one of `value` and `label` is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// The verdict as a label (for example `pass`, `fail`). At least one of
    /// `value` and `label` is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The partition this verdict applies to (for example which grader).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<serde_json::Map<String, serde_json::Value>>,
    /// Keys of the run's `metrics` the verdict was derived from. Informational;
    /// the hub does not recompute.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<String>,
}

/// How a score was derived from its samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    /// Arithmetic mean over samples.
    Mean,
    /// pass@k over attempts.
    PassAtK,
    /// Median over samples.
    Median,
    /// Sum over samples.
    Sum,
    /// A producer-defined aggregation; describe it in `ext`.
    Custom,
}

/// Uncertainty of a score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Uncertainty {
    /// Standard error of the score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<f64>,
    /// Confidence interval of the score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<ConfidenceInterval>,
}

/// A confidence interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfidenceInterval {
    /// Confidence level, for example `0.95`.
    pub level: f64,
    /// Lower bound.
    pub low: f64,
    /// Upper bound.
    pub high: f64,
}

/// Item counts. `attempted >= completed + failed + skipped + errored`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Counts {
    /// Items the evaluation set out to run.
    pub attempted: u64,
    /// Items that produced a gradable response.
    #[serde(default)]
    pub completed: u64,
    /// Items that were graded as failing.
    #[serde(default)]
    pub failed: u64,
    /// Items skipped by the harness.
    #[serde(default)]
    pub skipped: u64,
    /// Items that errored before grading.
    #[serde(default)]
    pub errored: u64,
}

/// Where the numbers came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// What kind of source the numbers were taken from.
    pub source_type: SourceType,
    /// Who ran the evaluation relative to the model's maker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator_relationship: Option<EvaluatorRelationship>,
}

/// The kind of source a Card's numbers were taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    /// The producer ran the evaluation and recorded the result.
    EvaluationRun,
    /// Transcribed from a paper.
    Paper,
    /// Transcribed from a model card.
    ModelCard,
    /// Transcribed from a leaderboard.
    Leaderboard,
    /// Something else; describe it in `ext`.
    Other,
}

/// Who ran the evaluation relative to the model's maker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvaluatorRelationship {
    /// The model's maker evaluated its own model.
    FirstParty,
    /// Someone else evaluated it.
    ThirdParty,
}
