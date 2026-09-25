//! The 1.0 shape of the Eval record (`evalhub.eval/1.0`), with `runs[]` in
//! the body.
//!
//! **Deprecated: accepted by 0.2.0 only, removed in 0.3.0.** This module exists for the
//! compatibility window the crate's versioning rule promises (see
//! "Versioning of the schema itself" in the crate doc): removing `runs`
//! from the body is a major bump of the record schema, and the hub keeps
//! accepting the previous major for one release. 0.2.0 accepts a
//! `evalhub.eval/1.0` body and converts it at ingest into a
//! `evalhub.eval/2.0` header ([`super::Eval`]) plus one [`crate::run::Run`]
//! per element of `runs[]`. 0.3.0 removes this module, the
//! `schemas/eval.json` document generated from it, and the conversion.
//!
//! New producers write [`super::Eval`] and [`crate::run::Run`]. Nothing
//! here is extended: a 1.0 body is frozen as it was in 0.1.x, and
//! `schemas/eval.json` is byte-for-byte the document 0.1.x served, so a
//! 0.1.x client that validates against it keeps working for the window.
//!
//! The note is prose rather than `#[deprecated]` on purpose. The attribute
//! would fire on every internal use (the schema tests, the 1.0 → 2.0
//! conversion, the data migration), and the workspace builds with
//! `clippy -D warnings`, so each of those uses would need an `allow` that
//! says nothing the date in this paragraph does not.
//!
//! # Shape
//!
//! ```text
//! Eval (1.0)
//! ├── schema        "evalhub.eval/1.0"
//! ├── title / producer / eval_kind / origin
//! ├── model / task / harness / generation / trial / env
//! ├── runs[]        { run_id, started_at, ended_at, outcome, calls, artifacts[] }
//! ├── relations[] / attachments[] / redaction
//! └── ext
//! ```
//!
//! `outcome` mixed the state of the execution (`error`, `skipped`) with a
//! verdict on it (`pass`, `fail`). In 2.0 the run keeps only the state
//! ([`crate::run::RunStatus`]) and the verdict moves to the Card
//! ([`crate::card::RunResult`]); the conversion keeps the dropped `outcome`
//! verbatim in the run's `meta`.
//!
//! The doc comments on the types below are part of `schemas/eval.json`
//! (they become its `description`s) and are therefore frozen with it.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{EvalKind, Origin};
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
