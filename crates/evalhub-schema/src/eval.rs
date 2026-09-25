//! The Eval record: the material a Card was measured from.
//!
//! An Eval is a named, versioned header plus the runs that belong to it. It
//! is what makes two Cards comparable: if both point at the same Eval
//! version through `core/uses_eval`, they were measured on the same
//! material, and the comparison view in `evalhub_server::api::relations`
//! can line them up.
//!
//! The name is deliberately not "Source". Source suggests source code; an
//! Eval is the evaluation itself, as a body of material, distinct from the
//! Card that summarises it.
//!
//! # Shape
//!
//! ```text
//! Eval (record, {ns}/{name})
//! ├── header        this type; one immutable body per version
//! │   ├── schema        "evalhub.eval/2.0"
//! │   ├── title
//! │   ├── producer      { name, version }
//! │   ├── eval_kind     run_set | prompt_set | task_set | trace_set
//! │   ├── origin        live | reconstructed | imported
//! │   ├── model / task / harness / generation / trial / env
//! │   │                 defaults for the runs (no grading facet)
//! │   ├── relations[]   derived_from / subset_of → other Evals
//! │   ├── attachments[]
//! │   ├── redaction
//! │   └── ext
//! └── runs          rows of the record, outside every version
//!     Run { run_id, status, error?, skip_reason?, started_at?, ended_at?,
//!           model / task / harness / generation / trial / env,
//!           calls?, artifacts[], attachments[], metrics{}, ext, meta? }
//!                   see `crate::run`
//! ```
//!
//! The header is what this type describes and what `POST /evals/{ns}/{name}`
//! appends as a version. The runs are not in it. They belong to the
//! *record*, not to a version: a run is written on its own, keyed by
//! `(record, run_id)`, and adding or overwriting one does not append a
//! header version. A header version is appended when the description or the
//! default conditions change. This is the change from `evalhub.eval/1.0`,
//! whose body carried `runs[]` (the 1.0 shape is kept in [`v1`] for the one
//! release that still accepts it).
//!
//! A header example:
//!
//! ```json
//! {
//!   "schema": "evalhub.eval/2.0",
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
//!   "relations": [{"type": "core/subset_of", "to": "alice/single2-full@1"}],
//!   "attachments": [
//!     {"path": "tasks.jsonl", "sha256": "…", "size": 90210, "media_type": "application/x-ndjson"}
//!   ],
//!   "redaction": {"applied": false},
//!   "ext": {}
//! }
//! ```
//!
//! # `eval_kind` and `origin`
//!
//! `eval_kind` says what the material is. It no longer decides whether the
//! Eval has runs: a header is complete with zero runs (a `run_set` header
//! is typically posted first and its runs written afterwards), and an Eval
//! of any `eval_kind` may have runs. The 1.0 rule "`runs` is required when
//! `eval_kind` is `run_set`" is gone.
//!
//! `origin` says how the material came to exist: `live` (captured while the
//! evaluation ran), `reconstructed` (rebuilt afterwards from logs),
//! `imported` (converted from another format, such as an Inspect `.eval`
//! file or an EEE datastore entry). A reader comparing Cards can weigh a
//! reconstructed Eval differently; the hub does not.
//!
//! # Conditions are per run; the header's facets are defaults
//!
//! One session may change the model, the temperature or the harness
//! between runs, so the conditions a run was measured under belong to the
//! run: [`crate::run::Run`] has the same six facets as the header. The
//! header's facets are *defaults*. A run that omits a facet is given the
//! header's value at the time it is written, and the copy is stored with
//! the run; changing the header's defaults later does not change the
//! conditions of runs already written. A facet the run carries itself wins
//! over the default. Fingerprints are computed per run from the run's own
//! facets; the header's fingerprint remains, as the fingerprint of the
//! defaults.
//!
//! # Tasks and runs are not separate types
//!
//! There is no sample or task type beside the run. A run's `calls` log
//! carries the inputs (the prompt, the reference) along with the outputs,
//! which is enough material. A set of tasks with nothing executed yet is an
//! Eval whose runs have empty outputs, and the link between a task Eval and
//! an execution Eval is an ordinary relation between Evals
//! (`core/derived_from`, `core/subset_of`). Repeated attempts at the same
//! task (`trial.k`, a rerun after an error) are separate runs; overwriting a
//! run is for correcting that same attempt. A producer that wants to prove
//! two runs saw the same input puts a hash of it in the run's `ext`.
//!
//! # A run has no verdict
//!
//! A run records what happened ([`crate::run::RunStatus`]: `ok`, `error`,
//! `skipped`) and what was measured directly ([`crate::run::Run::metrics`]).
//! It never records whether it passed. Pass and fail depend on a grader and
//! a threshold, and the same material can be graded several ways, so the
//! verdict lives on the Card that graded it, in
//! [`crate::card::Card::run_results`].
//!
//! # No grading facet
//!
//! An Eval (header and runs alike) has six of the seven facets. Grading
//! belongs to the Card, because the same material can be graded several
//! ways, and each grading is a different Card pointing at the same Eval.
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

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Attachment, Ext, Producer, Redaction, Relation};
use crate::facet::{Env, Generation, Harness, Model, Task, Trial};

pub mod v1;

/// An Eval header: what the material is, how it came to exist, and the
/// default conditions for its runs. The runs themselves are not part of the
/// body; they belong to the record and are written one by one
/// (`schemas/run.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Eval {
    /// Schema identifier; must be `evalhub.eval/2.0`.
    #[schemars(extend("const" = "evalhub.eval/2.0"))]
    pub schema: String,
    /// Human-readable title.
    pub title: String,
    /// The software that wrote this record.
    pub producer: Producer,
    /// What the material is.
    pub eval_kind: EvalKind,
    /// How the material came to exist.
    pub origin: Origin,
    /// The model facet: the default for runs that do not carry their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,
    /// The task facet: the default for runs that do not carry their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Task>,
    /// The harness facet: the default for runs that do not carry their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<Harness>,
    /// The generation facet: the default for runs that do not carry their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<Generation>,
    /// The trial facet: the default for runs that do not carry their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial: Option<Trial>,
    /// The environment facet: the default for runs that do not carry their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Env>,
    /// Edges to other Evals (`core/derived_from`, `core/subset_of`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    /// Files this header refers to. A run's own files are in the run's
    /// `attachments[]`, not here.
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
