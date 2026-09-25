//! The Run: one execution inside an Eval, as a row of the record.
//!
//! A run is what a harness did once: one attempt at one task (or at a whole
//! session of tasks, if that is the harness's unit), under stated
//! conditions, with its log, what it produced, and what was measured
//! directly from it. Runs belong to the Eval *record*, not to a header
//! version (see [`crate::eval`]): a run is written on its own, keyed by
//! `(record, run_id)`, and writing one does not append a header version.
//!
//! # Shape
//!
//! ```text
//! Run
//! ├── run_id        URL segment, unique within the Eval record
//! ├── status        ok | error | skipped
//! ├── error         { kind, message?, log? }    exactly when status = error
//! ├── skip_reason   free text                   exactly when status = skipped
//! ├── started_at / ended_at
//! ├── model / task / harness / generation / trial / env
//! │                 the conditions of this run; the header's facets are defaults
//! ├── calls         attachments[].path of the model calls
//! ├── artifacts[]   attachments[].path of what the run produced
//! ├── attachments[] { path, sha256, size, media_type }   this run's own files
//! ├── metrics       { "{ns}/{name}": number }   measured directly from the run
//! ├── ext           { "{ns}/{name}": { ... } }  producer-private, namespaced
//! └── meta          any JSON                    never interpreted
//! ```
//!
//! An example:
//!
//! ```json
//! {
//!   "run_id": "r1",
//!   "status": "ok",
//!   "started_at": "2026-09-20T10:00:00Z", "ended_at": "2026-09-20T10:02:11Z",
//!   "model": {"id": "qwen3.6-32b"},
//!   "generation": {"temperature": 0.0},
//!   "calls": "calls/r1.jsonl",
//!   "artifacts": ["artifacts/r1/diff.patch"],
//!   "attachments": [
//!     {"path": "calls/r1.jsonl", "sha256": "…", "size": 90210, "media_type": "application/x-ndjson"},
//!     {"path": "artifacts/r1/diff.patch", "sha256": "…", "size": 3311, "media_type": "text/x-diff"}
//!   ],
//!   "metrics": {"core/tokens_out": 1834, "core/duration_ms": 131000},
//!   "ext": {},
//!   "meta": {"worker": "gpu-3"}
//! }
//! ```
//!
//! # `run_id`
//!
//! `run_id` is the run's address inside its Eval and appears as one path
//! segment of a URL. It is non-empty, contains no `/`, and is at most 200
//! bytes of UTF-8 (`run_id_invalid` otherwise). Nothing else is imposed:
//! every `run_id` a 0.1.x producer wrote into `runs[]` meets these rules.
//! Writing a run whose `run_id` already exists in the record overwrites it;
//! repeated attempts at the same task are separate runs with separate ids.
//!
//! # `status`, `error`, `skip_reason`
//!
//! `status` is the state of the execution and nothing more:
//!
//! - `ok`: the run went to completion.
//! - `error`: the run was executed and failed along the way (budget
//!   exhausted, timeout, crash, an API error). `error` is required and
//!   says how.
//! - `skipped`: the producer deliberately did not execute it (left out by a
//!   limit, already done when a session resumed). `skip_reason` is
//!   required.
//!
//! `error` is present exactly when `status` is `error`, and `skip_reason`
//! exactly when `status` is `skipped` (`run_status_detail` otherwise).
//!
//! A run carries no verdict. Whether it passed depends on a grader and a
//! threshold, and the same run can be graded several ways, so pass and fail
//! are the Card's claim, in [`crate::card::Card::run_results`].
//!
//! `error.kind` is a short slug the producer chooses (`budget_exhausted`,
//! `timeout`, `crash`), of the form `[a-z0-9][a-z0-9._-]*`. It is not an
//! enum: harnesses fail in more ways than a closed list can name, and the
//! value is for grouping and filtering, not for the hub to act on. The
//! value `unrecorded` is reserved in practice for runs converted from
//! `evalhub.eval/1.0`, whose `outcome` said that a run errored or was
//! skipped but not why; the conversion writes `unrecorded` as the
//! `error.kind` or the `skip_reason` rather than guessing.
//!
//! # Facets
//!
//! A run has the six facets of an Eval header, with the same types
//! ([`crate::facet`]). They are the conditions this run was measured
//! under. A facet the run omits is filled from the header's default when
//! the run is written, and stored with the run, so a later change to the
//! header does not change the conditions of a run already written. The
//! fingerprint of each facet is computed per run.
//!
//! # Attachments, `calls`, `artifacts`
//!
//! A run has its own `attachments[]`, of the same type as a record's
//! ([`crate::common::Attachment`]). `calls`, `artifacts[]` and
//! `error.log` name a `path` in *this run's* `attachments[]`, never the
//! header's: a run's files change with the run, and pointing into the
//! header would make adding a run append a header version. The same checks
//! apply as on a record (`attachment_ref_unknown`,
//! `attachment_path_invalid`, `attachment_missing`), per run.
//!
//! # What the hub interprets and what it does not
//!
//! | Key                         | The hub                                                          |
//! | --------------------------- | ---------------------------------------------------------------- |
//! | `run_id`                    | checks the form; addresses the run by it                         |
//! | `status`, `error.kind`      | checks the status/detail pairing; filters and counts by them     |
//! | facets                      | fills defaults from the header; fingerprints them                |
//! | `metrics`                   | checks each key is `{ns}/{name}`; indexes the numbers            |
//! | `attachments[]` and pointers| checks references resolve and the objects are uploaded           |
//! | `ext`                       | stores; searchable, typed once the namespace registers a schema  |
//! | `error.message`, `skip_reason`, `started_at`, `ended_at` | stores and returns     |
//! | `meta`                      | stores and returns; never validates, indexes or queries it       |
//!
//! `metrics` holds values measured directly from the run: token counts,
//! durations, cost, an exit code, the length of an output, basic statistics
//! within the run. A key is a registry id of the same form as a Card's
//! `results[].metric` (`metric_id_invalid` otherwise); a metric the
//! registry does not know is accepted. A value that depends on how the run
//! is graded does not belong here; it is the Card's, in `run_results`,
//! which may name the metrics it was derived from (`from`).
//!
//! `meta` is the one place the hub never looks into. It is for whatever the
//! producer wants kept with the run and returned verbatim, and it is where
//! the 1.0 conversion keeps the values it had to drop (the old `outcome`).
//! `ext` is different: it is namespaced, and a namespace can register a
//! schema to make its keys typed and queryable.
//!
//! As with a record, the run is the producer's claim and the hub's facts
//! (its content hash, its fingerprints, whether it was archived) are
//! returned beside it, not inside it.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Attachment, Ext};
use crate::facet::{Env, Generation, Harness, Model, Task, Trial};

/// One run of an Eval: one execution, its conditions, its files and what
/// was measured directly from it. A run records what happened, never
/// whether it passed; the verdict is a Card's `run_results`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// Run identifier, unique within the Eval record. One URL segment:
    /// non-empty, no `/`, at most 200 bytes.
    pub run_id: String,
    /// The state of the execution: `ok`, `error` or `skipped`.
    pub status: RunStatus,
    /// How the run failed. Present exactly when `status` is `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RunError>,
    /// Why the producer did not execute the run. Present exactly when
    /// `status` is `skipped`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
    /// When the run started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the run ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// The model this run used. Filled from the Eval header when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    /// The task this run attempted. Filled from the Eval header when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Task>,
    /// The harness that drove this run. Filled from the Eval header when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<Harness>,
    /// The sampling parameters of this run. Filled from the Eval header when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<Generation>,
    /// How this run's attempt was made. Filled from the Eval header when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial: Option<Trial>,
    /// The environment this run executed in. Filled from the Eval header when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Env>,
    /// This run's `attachments[].path` holding the model calls made during the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<String>,
    /// This run's `attachments[].path` entries of artifacts the run produced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
    /// Files this run refers to. `calls`, `artifacts[]` and `error.log` point here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// Values measured directly from the run, keyed by metric registry id
    /// `{ns}/{name}` (for example `core/tokens_out`). Grader-dependent
    /// values belong to a Card's `run_results`, not here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, f64>,
    /// Producer-private extensions keyed by namespace.
    #[serde(default)]
    pub ext: Ext,
    /// Free-form JSON kept with the run and returned verbatim. The hub never
    /// validates, indexes or queries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// The state of a run's execution. Not a verdict: pass and fail are a Card's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// The run went to completion.
    Ok,
    /// The run was executed and failed along the way; `error` says how.
    Error,
    /// The producer deliberately did not execute the run; `skip_reason` says why.
    Skipped,
}

/// How a run failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunError {
    /// Short slug naming the kind of failure, `[a-z0-9][a-z0-9._-]*`
    /// (for example `timeout`, `budget_exhausted`, `crash`). Chosen by the
    /// producer; not a closed list. `unrecorded` means the kind is not known.
    pub kind: String,
    /// What went wrong, for a human.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// This run's `attachments[].path` holding the failure's log (stderr, a trace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}
