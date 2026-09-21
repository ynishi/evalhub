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
//!
//! # Absent versus `null` in these types
//!
//! Every optional key below is an `Option<T>`; `null` and an absent key both
//! deserialise to `None` when a record is read *through these types*. The
//! hub does not lose the distinction: it validates and fingerprints the
//! record as a JSON value (see `evalhub_core`), where `null` and absence are
//! different, and stores the canonical bytes. The generated schema allows
//! `null` for every optional key.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Ext, ext_is_empty};

/// The model that was evaluated.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Model {
    /// Model identifier as the producer names it (for example `qwen3.6-32b`).
    pub id: String,
    /// Who served the model (`openai`, `anthropic`, `local`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Exact revision: a commit, a checkpoint tag, or a provider snapshot date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Endpoint the model was reached through, when that changes the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Inference engine that ran the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Engine>,
    /// Quantisation applied to the weights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization: Option<Quantization>,
    /// Availability of the weights (`open`, `api`, `private`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,
    /// Context window in tokens. Recorded; does not affect the fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-fingerprint" = false))]
    pub context_window: Option<u64>,
    /// Extensions keyed by namespace; not fingerprinted unless registered.
    #[serde(default, skip_serializing_if = "ext_is_empty")]
    pub ext: Ext,
}

/// Inference engine name and version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Engine {
    /// Engine name (`vllm`, `llama.cpp`, `transformers`, …).
    pub name: String,
    /// Engine version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Weight quantisation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Quantization {
    /// Method (`awq`, `gptq`, `gguf`, …).
    pub method: String,
    /// Storage type (`int4`, `int8`, `fp8`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dtype: Option<String>,
}

/// The task or benchmark that was run.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// Task identifier as the producer names it (for example `single2`).
    pub id: String,
    /// Task version, as published by the task's author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Exact revision of the task material (a commit or dataset revision).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Split that was evaluated (`test`, `validation`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<String>,
    /// Named task configuration, when the task has more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// Number of items evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    /// Seed used to select or order items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Whether items were shuffled before evaluation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shuffled: Option<bool>,
    /// Where the task data came from (a URL or dataset id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_data: Option<String>,
    /// sha256 over the sorted list of sample ids that were evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_ids_sha256: Option<String>,
    /// Extensions keyed by namespace; not fingerprinted unless registered.
    #[serde(default, skip_serializing_if = "ext_is_empty")]
    pub ext: Ext,
}

/// The harness that drove the evaluation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Harness {
    /// Harness name.
    pub name: String,
    /// Harness version.
    pub version: String,
    /// sha256 of the harness configuration that was in effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_sha256: Option<String>,
    /// sha256 of the prompt template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_template_sha256: Option<String>,
    /// sha256 of the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_sha256: Option<String>,
    /// Number of few-shot examples in the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub few_shot: Option<u32>,
    /// Extensions keyed by namespace; not fingerprinted unless registered.
    #[serde(default, skip_serializing_if = "ext_is_empty")]
    pub ext: Ext,
}

/// Sampling parameters.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Generation {
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Nucleus sampling threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Top-k sampling cutoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Maximum tokens generated per response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    /// Sampling seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Number of completions requested per prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// Stop sequences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    /// Reasoning (extended thinking) settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    /// Extensions keyed by namespace; not fingerprinted unless registered.
    #[serde(default, skip_serializing_if = "ext_is_empty")]
    pub ext: Ext,
}

/// Reasoning settings passed to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reasoning {
    /// Provider-defined effort level (`low`, `medium`, `high`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Token budget for reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u64>,
}

/// How each item was attempted.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Trial {
    /// Attempts per item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k: Option<u32>,
    /// Whether the score is pass@k over the attempts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass_at_k: Option<bool>,
    /// Maximum agent turns per attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// Timeout per attempt in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Retries on error per attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
    /// Sandbox the attempt ran in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<Sandbox>,
    /// Extensions keyed by namespace; not fingerprinted unless registered.
    #[serde(default, skip_serializing_if = "ext_is_empty")]
    pub ext: Ext,
}

/// Container image the attempts ran in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sandbox {
    /// Image reference (for example `ghcr.io/org/image:tag`).
    pub image: String,
    /// Content digest of the image (`sha256:…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// How responses were scored. Cards only.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Grading {
    /// Graders that produced the scores.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graders: Vec<Grader>,
    /// Judge model, when a grader is a model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge: Option<Judge>,
    /// sha256 of the rubric given to the graders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubric_sha256: Option<String>,
    /// How several graders' verdicts were combined (`majority`, `all`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregation: Option<String>,
    /// Threshold a score had to reach to count as a pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Extensions keyed by namespace; not fingerprinted unless registered.
    #[serde(default, skip_serializing_if = "ext_is_empty")]
    pub ext: Ext,
}

/// One grader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Grader {
    /// Grader name (`exact_match`, `llm_judge`, …).
    pub name: String,
    /// Grader kind (`deterministic`, `model`, `human`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// The judge model used by a model grader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Judge {
    /// Judge model identifier.
    pub id: String,
    /// Who served the judge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Exact revision of the judge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

/// The software environment the evaluation ran in.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Env {
    /// Git state of the code that ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<Git>,
    /// Pinned packages that matter to the result.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<Package>,
    /// Operating system description. Recorded; does not affect the fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-fingerprint" = false))]
    pub os: Option<String>,
    /// Hardware description. Recorded; does not affect the fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-fingerprint" = false))]
    pub hardware: Option<Hardware>,
    /// Extensions keyed by namespace; not fingerprinted unless registered.
    #[serde(default, skip_serializing_if = "ext_is_empty")]
    pub ext: Ext,
}

/// Git state of the evaluating code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Git {
    /// Remote the code came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Commit that ran.
    pub commit: String,
    /// Whether the working tree had uncommitted changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
}

/// A pinned package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
}

/// Hardware the evaluation ran on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Hardware {
    /// Accelerator model (for example `A100`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
    /// Number of accelerators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_count: Option<u32>,
    /// CPU model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// Memory in gigabytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<f64>,
}
