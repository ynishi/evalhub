//! The error envelope and the closed set of error codes.
//!
//! Every rejection the hub makes has the same shape:
//!
//! ```text
//! 422 { "errors": [ { "path": "results[0].metric", "code": "metric_id_invalid", "hint": "..." }, ... ] }
//! ```
//!
//! Validation collects *all* errors before returning, so a producer fixes a
//! record in one round trip rather than one error at a time. `path` is a
//! JSON-pointer-like location in the submitted record; `code` is one of the
//! variants below and is listed as an enum in the OpenAPI document; `hint`
//! is prose for a human and is not part of the contract.
//!
//! | Code                     | Status | Meaning                                                              |
//! | ------------------------ | ------ | -------------------------------------------------------------------- |
//! | `schema`                 | 422    | JSON Schema violation (unknown key, wrong type, missing required)    |
//! | `number_too_large`       | 422    | integer outside ±2^53                                                |
//! | `counts_missing`         | 422    | `results` non-empty but no `counts`                                  |
//! | `counts_inconsistent`    | 422    | `attempted < completed + failed + skipped + errored`                 |
//! | `attachment_ref_unknown` | 422    | `samples_ref` / `calls` / `artifacts[]` / `error.log` not in `attachments[].path` |
//! | `attachment_path_invalid`| 422    | duplicate, `..`, or absolute `attachments[].path`                    |
//! | `metric_id_invalid`      | 422    | `results[].metric`, `run_results[].metric` or a run's `metrics` key not of the form `{ns}/{name}` |
//! | `type_mismatch`          | 422    | query: operator applied to a path of another type                    |
//! | `not_indexed`            | 422    | query: operator needs an index the path does not have                |
//! | `unknown_path`           | 422    | query: path not in the schema                                        |
//! | `runs_moved`             | 422    | an `evalhub.eval/2.0` header carries `runs`; runs are written apart  |
//! | `run_status_detail`      | 422    | `error` present iff `status` is `error`, `skip_reason` iff `skipped` |
//! | `run_id_invalid`         | 422    | `run_id` empty, containing `/`, or over 200 bytes                    |
//! | `run_id_mismatch`        | 422    | the `run_id` in the body differs from the one in the path            |
//! | `run_result_value_or_label` | 422 | a `run_results[]` element has neither `value` nor `label`            |
//! | `run_results_eval_unknown`  | 422 | `run_results[].eval` is not the record of any `core/uses_eval`       |
//! | `run_unknown`            | 422    | a `run_id` in the used set has no row in that Eval                   |
//! | `run_not_in_used_set`    | 422    | `run_results[].run_id` outside the Card's used set for its `eval`    |
//! | `too_many_run_results`   | 422    | more `run_results[]` than the configured limit                       |
//! | `batch_duplicate_run_id` | 422    | the same `run_id` twice in one batch                                 |
//! | `attachment_missing`     | 409    | `attachments[].sha256` not uploaded and confirmed                    |
//! | `label_in_use`           | 409    | `@{label}` already names another version                             |
//! | `namespace_in_use`       | 409    | the organisation slug is already a user or an organisation           |
//! | `registry_entry_exists`  | 409    | that registry address is taken; entries are immutable                |
//! | `run_deleted`            | 409    | writing a `run_id` that was deleted; a deleted id is never reused    |
//! | `batch_too_large`        | 413    | more runs in one batch than the configured count                     |
//! | `body_too_large`         | 413    | request body over the configured size in bytes                       |
//!
//! A `413` carries this body only with the two `413` codes above. Other
//! statuses carry no body of this shape: `404` for anything the caller
//! may not know exists (including private records), `403` for a token
//! without the scope, `401` for no or an invalid token, `400` for a body that
//! is not JSON.
//!
//! Codes are added, never renamed or reused. A client switching on `code`
//! must treat an unknown code as a generic 422.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The closed set of error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// JSON Schema violation: unknown key, wrong type, missing required key.
    Schema,
    /// An integer outside ±2^53.
    NumberTooLarge,
    /// `results` is non-empty but `counts` is absent.
    CountsMissing,
    /// `counts.attempted < completed + failed + skipped + errored`.
    CountsInconsistent,
    /// A `samples_ref`, `calls`, `artifacts[]` or `error.log` entry names no `attachments[].path`.
    AttachmentRefUnknown,
    /// An `attachments[].path` is duplicated, absolute, or contains `..`.
    AttachmentPathInvalid,
    /// A `results[].metric`, a `run_results[].metric` or a key of a run's
    /// `metrics` is not of the form `{ns}/{name}`.
    MetricIdInvalid,
    /// Query: operator applied to a path of another type.
    TypeMismatch,
    /// Query: operator needs an index the path does not have.
    NotIndexed,
    /// Query: path not in the schema.
    UnknownPath,
    /// An `attachments[].sha256` has not been uploaded and confirmed.
    AttachmentMissing,
    /// The requested `@{label}` already names another version.
    LabelInUse,
    /// The requested organisation slug is already a user or an organisation.
    NamespaceInUse,
    /// A registry entry already exists at that address. Entries are
    /// immutable, so a correction is a new version.
    RegistryEntryExists,
    /// An `evalhub.eval/2.0` Eval header carries `runs`. Runs are not part
    /// of the header; they are written one by one or in a batch.
    RunsMoved,
    /// A run's `status` and its detail disagree: `error` must be present
    /// exactly when `status` is `error`, `skip_reason` exactly when it is
    /// `skipped`.
    RunStatusDetail,
    /// A `run_id` is empty, contains `/`, or is longer than 200 bytes.
    RunIdInvalid,
    /// The `run_id` in the body differs from the one in the request path.
    RunIdMismatch,
    /// A `run_results[]` element has neither `value` nor `label`.
    RunResultValueOrLabel,
    /// A `run_results[].eval` is not the record of any of the Card's
    /// `core/uses_eval` relations.
    RunResultsEvalUnknown,
    /// A `run_id` in the Card's used set has no row in that Eval (or the
    /// Eval is one the writer may not see).
    RunUnknown,
    /// A `run_results[].run_id` is outside the Card's used set for its `eval`.
    RunNotInUsedSet,
    /// A Card version carries more `run_results[]` than the hub's configured limit.
    TooManyRunResults,
    /// The `run_id` names a run that was deleted. A deleted run's id is not
    /// reused, because Cards that judged it would then point at different content.
    RunDeleted,
    /// The same `run_id` appears more than once in one batch.
    BatchDuplicateRunId,
    /// A batch carries more runs than the hub's configured count.
    BatchTooLarge,
    /// The request body is larger than the hub's configured size in bytes.
    BodyTooLarge,
}

impl ErrorCode {
    /// The HTTP status this code is returned with.
    pub const fn status(self) -> u16 {
        match self {
            ErrorCode::AttachmentMissing
            | ErrorCode::LabelInUse
            | ErrorCode::NamespaceInUse
            | ErrorCode::RegistryEntryExists
            | ErrorCode::RunDeleted => 409,
            ErrorCode::BatchTooLarge | ErrorCode::BodyTooLarge => 413,
            _ => 422,
        }
    }
}

/// One rejection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ErrorEntry {
    /// Location in the submitted document, JSON-pointer-like (`results[0].metric`).
    pub path: String,
    /// The code.
    pub code: ErrorCode,
    /// Prose for a human. Not part of the contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// The body of every `422` and `409` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ErrorEnvelope {
    /// Every rejection found, in document order.
    pub errors: Vec<ErrorEntry>,
}
