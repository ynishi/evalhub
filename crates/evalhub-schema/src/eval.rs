//! The Eval record: the material a Card was measured from.
//!
//! An Eval is a named, versioned set of runs, prompts, tasks or traces. It is
//! what makes two Cards comparable: if both point at the same Eval version
//! through `core/uses_eval`, they were measured on the same material, and the
//! comparison view in `evalhub_server::api::relations` can line them up.
//!
//! The name is deliberately not "Source". Source suggests source code; an
//! Eval is the evaluation itself, as a body of material, distinct from the
//! Card that summarises it.
//!
//! # Shape
//!
//! ```text
//! Eval
//! ├── schema        "evalhub.eval/1.0"
//! ├── title
//! ├── producer      { name, version }
//! ├── eval_kind     run_set | prompt_set | task_set | trace_set
//! ├── origin        live | reconstructed | imported
//! ├── model / task / harness / generation / trial / env   (no grading facet)
//! ├── runs[]        { run_id, started_at, ended_at, outcome, calls, artifacts[] }
//! │                 required when eval_kind = run_set
//! ├── relations[]   derived_from / subset_of → other Evals
//! ├── attachments[]
//! ├── redaction
//! └── ext
//! ```
//!
//! A `run_set` example:
//!
//! ```json
//! {
//!   "schema": "evalhub.eval/1.0",
//!   "title": "single2 K4, 33 runs",
//!   "producer": {"name": "my-harness", "version": "0.4.1"},
//!   "eval_kind": "run_set",
//!   "origin": "live",
//!   "harness": {"name": "my-harness", "version": "0.4.1"},
//!   "model": {"id": "qwen3.6-32b"},
//!   "task": {"id": "single2", "version": "3", "split": "test", "n": 33},
//!   "generation": {"temperature": 0.0},
//!   "trial": {"k": 4},
//!   "env": {"git": {"origin": "https://github.com/x/y", "commit": "abc123", "dirty": false}},
//!   "runs": [{
//!     "run_id": "r1",
//!     "started_at": "2026-09-20T10:00:00Z", "ended_at": "2026-09-20T10:02:11Z",
//!     "outcome": "pass",
//!     "calls": "calls/r1.jsonl",
//!     "artifacts": ["artifacts/r1/diff.patch"]
//!   }],
//!   "relations": [{"type": "core/subset_of", "to": "alice/single2-full@1"}],
//!   "attachments": [
//!     {"path": "calls/r1.jsonl", "sha256": "…", "size": 90210, "media_type": "application/x-ndjson"},
//!     {"path": "artifacts/r1/diff.patch", "sha256": "…", "size": 3311, "media_type": "text/x-diff"}
//!   ],
//!   "redaction": {"applied": false},
//!   "ext": {}
//! }
//! ```
//!
//! # `eval_kind` and `origin`
//!
//! `eval_kind` says what the material is. A `run_set` has `runs[]`, each with
//! an outcome and a pointer (`calls`) to an attachment holding the model
//! calls. The other kinds carry their material entirely in attachments.
//!
//! `origin` says how the material came to exist: `live` (captured while the
//! evaluation ran), `reconstructed` (rebuilt afterwards from logs),
//! `imported` (converted from another format, such as an Inspect `.eval`
//! file or an EEE datastore entry). A reader comparing Cards can weigh a
//! reconstructed Eval differently; the hub does not.
//!
//! # No grading facet
//!
//! An Eval has six of the seven facets. Grading belongs to the Card, because
//! the same material can be graded several ways, and each grading is a
//! different Card pointing at the same Eval.
//!
//! # Privacy across the relation
//!
//! A public Card may point at a private Eval. A reader without access sees
//! that an Eval exists, its `version_id`, and the `content_hash` commitment —
//! enough to know the Card is anchored to something specific — and nothing
//! else. No title, no runs, no attachment URLs.
//!
//! The Card's own body does not reveal more than its edges. On every read
//! that returns a body, a `relations[]` element whose target the reader may
//! not see is removed and listed under `withheld.relations` as the same
//! commitment (`type`, `version_id`, `content_hash`). The body returned is
//! then not the stored one, and its `content_hash` does not match it. A
//! query's `relations.to` condition matches only targets the caller may
//! see, so a search cannot confirm a private name either.
//!
//! A write learns no more than a read. A target the writer may not see is
//! stored unresolved, exactly as one that does not exist: the response and
//! the `refs_resolved` badge are the same in both cases. The edge resolves
//! for a reader once the target becomes visible to them.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Attachment, Ext, Producer, Redaction, Relation};
use crate::facet::{Env, Generation, Harness, Model, Task, Trial};

/// An Eval: the material a Card was measured from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Eval {
    /// Schema identifier; must be `evalhub.eval/1.0`.
    #[schemars(extend("const" = "evalhub.eval/1.0"))]
    pub schema: String,
    /// Human-readable title.
    pub title: String,
    /// The software that wrote this record.
    pub producer: Producer,
    /// What the material is.
    pub eval_kind: EvalKind,
    /// How the material came to exist.
    pub origin: Origin,
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
    /// The environment facet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Env>,
    /// The runs. Required when `eval_kind` is `run_set`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<Run>,
    /// Edges to other Evals (`core/derived_from`, `core/subset_of`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    /// Files this record refers to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// What the producer removed before publishing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction: Option<Redaction>,
    /// Producer-private extensions keyed by namespace.
    #[serde(default)]
    pub ext: Ext,
}

/// What an Eval's material is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvalKind {
    /// A set of runs, each with its model calls.
    RunSet,
    /// A set of prompts.
    PromptSet,
    /// A set of tasks.
    TaskSet,
    /// A set of traces.
    TraceSet,
}

/// How an Eval's material came to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Captured while the evaluation ran.
    Live,
    /// Rebuilt afterwards from logs.
    Reconstructed,
    /// Converted from another format.
    Imported,
}

/// One run in a `run_set`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Run {
    /// Run identifier, unique within the Eval.
    pub run_id: String,
    /// When the run started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// When the run ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// How the run ended.
    pub outcome: Outcome,
    /// `attachments[].path` of the model calls made during the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<String>,
    /// `attachments[].path` entries of artifacts the run produced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The run completed and passed.
    Pass,
    /// The run completed and failed.
    Fail,
    /// The run errored before completion.
    Error,
    /// The run was skipped.
    Skipped,
}
