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
//! | `attachment_ref_unknown` | 422    | `samples_ref` / `calls` / `artifacts[]` not in `attachments[].path`  |
//! | `attachment_path_invalid`| 422    | duplicate, `..`, or absolute `attachments[].path`                    |
//! | `metric_id_invalid`      | 422    | `results[].metric` not of the form `{ns}/{name}`                     |
//! | `type_mismatch`          | 422    | query: operator applied to a path of another type                    |
//! | `not_indexed`            | 422    | query: operator needs an index the path does not have                |
//! | `unknown_path`           | 422    | query: path not in the schema                                        |
//! | `attachment_missing`     | 409    | `attachments[].sha256` not uploaded and confirmed                    |
//! | `label_in_use`           | 409    | `@{label}` already names another version                             |
//! | `namespace_in_use`       | 409    | the organisation slug is already a user or an organisation           |
//!
//! Other statuses carry no body of this shape: `404` for anything the caller
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
    /// A `samples_ref`, `calls` or `artifacts[]` entry names no `attachments[].path`.
    AttachmentRefUnknown,
    /// An `attachments[].path` is duplicated, absolute, or contains `..`.
    AttachmentPathInvalid,
    /// A `results[].metric` is not of the form `{ns}/{name}`.
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
}

impl ErrorCode {
    /// The HTTP status this code is returned with.
    pub const fn status(self) -> u16 {
        match self {
            ErrorCode::AttachmentMissing | ErrorCode::LabelInUse | ErrorCode::NamespaceInUse => 409,
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
