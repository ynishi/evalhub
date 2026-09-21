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
//! ├── schema        "evalhub.card/1.0"
//! ├── title         free text
//! ├── producer      { name, version }            who wrote this record
//! ├── model / task / harness / generation / trial / grading / env
//! │                 seven facets (see `facet`)   what was measured, under what conditions
//! ├── results[]     { metric, value, n, aggregation, uncertainty, by, samples_ref }
//! ├── counts        { attempted, completed, failed, skipped, errored }
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
//!   "schema": "evalhub.card/1.0",
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
//!   "relations": [{"type": "core/uses_eval", "to": "alice/single2-k4@3", "attrs": {"runs": ["r1"]}}],
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
