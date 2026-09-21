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
//!
//! Other statuses carry no body of this shape: `404` for anything the caller
//! may not know exists (including private records), `403` for a token
//! without the scope, `401` for no or an invalid token, `400` for a body that
//! is not JSON.
//!
//! Codes are added, never renamed or reused. A client switching on `code`
//! must treat an unknown code as a generic 422.
