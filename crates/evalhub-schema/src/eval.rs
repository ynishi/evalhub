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
