//! The seven facets: the parameters that decide comparability.
//!
//! A facet groups the parameters that must match for two records to have
//! been measured "the same way" along one axis. Each facet is fingerprinted
//! independently (see `evalhub_core::fingerprint`), so the hub can say "same
//! model, same task, different generation settings" without a rule for what
//! counts as "the same evaluation" overall. That judgement is left to the
//! reader; the hub only exposes the axes.
//!
//! | Facet        | Core keys                                                                                   | Recorded, not fingerprinted |
//! | ------------ | ------------------------------------------------------------------------------------------- | --------------------------- |
//! | `model`      | `id` (required), `provider`, `revision`, `endpoint`, `engine{name,version}`, `quantization{method,dtype}`, `availability` | `context_window` |
//! | `task`       | `id` (required), `version`, `revision`, `split`, `config`, `n`, `seed`, `shuffled`, `source_data`, `sample_ids_sha256` | — |
//! | `harness`    | `name` (required), `version` (required), `config_sha256`, `prompt_template_sha256`, `system_prompt_sha256`, `few_shot` | — |
//! | `generation` | `temperature`, `top_p`, `top_k`, `max_tokens`, `seed`, `n`, `stop[]`, `reasoning{effort,budget_tokens}` | — |
//! | `trial`      | `k`, `pass_at_k`, `max_turns`, `timeout_ms`, `retries`, `sandbox{image,digest}`             | — |
//! | `grading`    | `graders[]{name,kind}`, `judge{id,provider,revision}`, `rubric_sha256`, `aggregation`, `threshold` | — |
//! | `env`        | `git{origin,commit,dirty}`, `packages[]{name,version}`                                      | `os`, `hardware` |
//!
//! # The admission rule for core keys
//!
//! A key goes into a facet's core only if it meets both tests:
//!
//! 1. At least two of the major harnesses or schemas (EEE, Inspect, the
//!    Hugging Face model-index, the OpenAI generation parameters) have a key
//!    with the same meaning.
//! 2. The key is used by a fingerprint or a badge.
//!
//! Anything else goes in that facet's `ext`. The first test keeps the core
//! from becoming one harness's private vocabulary; the second keeps it from
//! collecting keys that are merely interesting. Interoperability is by
//! converter, not by absorbing every format's fields.
//!
//! # `x-fingerprint: false`
//!
//! A key that is recorded but must not influence the fingerprint is marked
//! in the generated schema with `x-fingerprint: false`, written on the Rust
//! field with `#[schemars(extend("x-fingerprint" = false))]`. `context_window`
//! is the canonical example: it describes the model but does not change what
//! was measured. The fingerprint code reads this annotation from the schema;
//! there is no second list to keep in sync.
//!
//! # Facet `ext`
//!
//! Each facet has its own `ext` map, keyed by `{ns}/{name}`, for parameters
//! that fail the admission rule. By default `ext` is excluded from the
//! fingerprint. A namespace that wants its extension to participate registers
//! an `ext_schema` in the registry with `fingerprint: true`; from then on the
//! hub types those paths, indexes them, and folds them into the facet's
//! fingerprint.
