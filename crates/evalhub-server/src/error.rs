//! `ApiError`: the one error type the handlers return, and its HTTP mapping.
//!
//! | Variant                         | Status | Body                                  |
//! | ------------------------------- | ------ | ------------------------------------- |
//! | `Validation(Vec<error::Error>)` | 422    | `{ errors: [{ path, code, hint }] }`  |
//! | `AttachmentMissing`             | 409    | `{ errors: [{ code: attachment_missing }] }` |
//! | `LabelInUse`                    | 409    | `{ errors: [{ code: label_in_use }] }`|
//! | `NotFound`                      | 404    | empty                                 |
//! | `Forbidden`                     | 403    | empty                                 |
//! | `Unauthorized`                  | 401    | empty                                 |
//! | `SessionRejected { secure }`    | 401    | empty; clears the session cookie      |
//! | `BadRequest`                    | 400    | empty                                 |
//! | `Unavailable(&'static str)`     | 503    | empty; a capability this deployment does not have |
//! | `NotImplemented(&'static str)`  | 501    | empty; a format the hub has not decided on yet    |
//! | `RegistryEntryExists(String)`   | 409    | `{ errors: [{ code: registry_entry_exists }] }`   |
//! | `Conflict(Vec<error::Error>)`   | 409    | `{ errors: [...] }`, every entry a `409` code (`run_deleted`, `attachment_missing`) |
//! | `TooLarge(Vec<error::Error>)`   | 413    | `{ errors: [{ code: body_too_large \| batch_too_large }] }` |
//! | `Internal(anyhow::Error)`       | 500    | empty; logged with `error!`           |
//!
//! # Store errors
//!
//! | `StoreError`                    | Answer |
//! | ------------------------------- | ------ |
//! | `RecordNotFound`, `VersionNotFound`, `AlreadyTombstoned`, `NamespaceUnknown`, `NotAnOrganisation`, `RegistryEntryNotFound`, `RunNotFound` | `404` |
//! | `RunCardsUnknown`               | `404`, empty, whichever Card was unknown and why (see below) |
//! | `RunDeleted`                    | `409 run_deleted` at `/run_id` |
//! | `RunsRejected`                  | `422`, or `409` when every entry of every element is a `409` code; see [`ApiError::runs_rejected`] |
//! | `CardRunsRejected`              | `422` with the store's entries (`run_unknown`, `run_not_in_used_set`) |
//! | `LabelInUse`                    | `409 label_in_use` |
//! | `RegistryEntryExists`           | `409 registry_entry_exists` |
//! | `RegistryCoreReadOnly`          | `403` |
//! | `LabelInvalid`                  | `400` |
//! | anything else, `Canonical` included | `500`, as an internal error |
//!
//! `RunCardsUnknown` carries the Card names the run projection could not
//! join, but the answer is the plain `404` every other unknown or private
//! thing gets: a Card that does not exist, one the caller may not see and
//! one that does not use the Eval must not be told apart, and a body
//! listing them would at best repeat what the caller sent.
//!
//! `RunsRejected` is the one store error whose status depends on its
//! content. A run write refused only for state the producer cannot fix in
//! the body (the id was deleted, an attachment is not confirmed yet) is a
//! conflict, `409`, as `attachment_missing` is on a record; one with any
//! validation failure is `422`, because the body has to change first and
//! the `409` entries ride along so the producer fixes everything in one
//! round trip. `ErrorCode::status` is the one table of which code is which.
//!
//! Domain errors from the library crates (`thiserror` enums) convert into
//! these variants at the handler boundary; `anyhow` is used only for the
//! `Internal` case and in `main`. Nothing internal (SQL text, paths,
//! panics) reaches a response body.
//!
//! The type also implements `aide::OperationOutput`, so a handler returning
//! `Result<T, ApiError>` documents the `401` / `403` / `404` / `409` / `422`
//! responses in `openapi.json` without each handler repeating them.

use aide::OperationOutput;
use aide::generate::GenContext;
use aide::openapi::{Operation, Response, StatusCode as ApiStatus};
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response as HttpResponse};
use tracing::error;

use evalhub_schema::error::{ErrorCode, ErrorEntry, ErrorEnvelope};
use evalhub_store::error::StoreError;

/// Every way a handler can fail, mapped to a status and a body.
#[derive(Debug)]
pub enum ApiError {
    /// The record failed validation; every violation is listed.
    Validation(Vec<ErrorEntry>),
    /// An `attachments[].sha256` is not uploaded and confirmed.
    AttachmentMissing(Vec<ErrorEntry>),
    /// The requested label already names another version of the record.
    LabelInUse,
    /// The organisation slug is already taken by a user or an organisation.
    NamespaceInUse(String),
    /// The thing does not exist, or the caller may not know that it does.
    NotFound,
    /// The token lacks the scope or namespace for this action.
    Forbidden,
    /// No token, or a token the hub does not recognise.
    Unauthorized,
    /// The credential came from the session cookie and was refused, so the
    /// response clears the cookie rather than leaving the browser to
    /// present it again on every request. `secure` mirrors the flag the
    /// cookie was set with; a browser ignores a clearing header whose
    /// attributes do not match.
    SessionRejected {
        /// Whether the cookie being cleared was marked `Secure`.
        secure: bool,
    },
    /// The request is malformed in a way the handler detected itself
    /// (bad cursor, unparseable path segment).
    BadRequest,
    /// The hub is not configured for this: attachments without an object
    /// store. Not a fault of the request, and not a bug — a deployment
    /// that left the capability out.
    Unavailable(&'static str),
    /// A format or capability the hub has not decided on yet. The hint
    /// says what the open question is.
    NotImplemented(&'static str),
    /// A registry address that is already taken. Entries are immutable,
    /// so a correction is a new version rather than a second write.
    RegistryEntryExists(String),
    /// A state conflict listed entry by entry: every entry carries a `409`
    /// code (`run_deleted`, `attachment_missing`).
    Conflict(Vec<ErrorEntry>),
    /// The request carries more than the configured limits allow
    /// (`body_too_large`, `batch_too_large`).
    TooLarge(Vec<ErrorEntry>),
    /// Anything the hub did not expect. Logged; the body is empty.
    Internal(anyhow::Error),
}

impl ApiError {
    /// The HTTP status this error is answered with.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::AttachmentMissing(_) | Self::LabelInUse | Self::NamespaceInUse(_) => {
                StatusCode::CONFLICT
            }
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Unauthorized | Self::SessionRejected { .. } => StatusCode::UNAUTHORIZED,
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            Self::RegistryEntryExists(_) | Self::Conflict(_) => StatusCode::CONFLICT,
            Self::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// A refused run write ([`StoreError::RunsRejected`]) as a response.
    ///
    /// With `indexed`, each entry's `path` is prefixed with
    /// `/runs/{index}`, the element's position in the request's `runs[]`
    /// (a batch, or the `runs[]` of an `evalhub.eval/1.0` body), so every
    /// failing element is named with its index; without it (a single
    /// `PUT`) the paths point into the run body as sent.
    ///
    /// The status is `409` when every entry of every element carries a
    /// `409` code (`run_deleted`, `attachment_missing`), `422` otherwise;
    /// see the module doc.
    pub fn runs_rejected(
        rejections: Vec<evalhub_store::error::RunRejection>,
        indexed: bool,
    ) -> Self {
        let errors: Vec<ErrorEntry> = rejections
            .into_iter()
            .flat_map(|r| {
                let index = r.index;
                r.errors.into_iter().map(move |mut e| {
                    if indexed {
                        e.path = format!("/runs/{index}{}", e.path);
                    }
                    e
                })
            })
            .collect();
        if !errors.is_empty() && errors.iter().all(|e| e.code.status() == 409) {
            Self::Conflict(errors)
        } else {
            Self::Validation(errors)
        }
    }

    /// The error envelope, when the status carries one (`422` / `409` /
    /// `413`).
    fn envelope(&self) -> Option<ErrorEnvelope> {
        match self {
            Self::Validation(errors)
            | Self::AttachmentMissing(errors)
            | Self::Conflict(errors)
            | Self::TooLarge(errors) => Some(ErrorEnvelope {
                errors: errors.clone(),
            }),
            Self::LabelInUse => Some(ErrorEnvelope {
                errors: vec![ErrorEntry {
                    path: "label".to_string(),
                    code: ErrorCode::LabelInUse,
                    hint: Some("choose another label or move this one with PATCH".to_string()),
                }],
            }),
            Self::NamespaceInUse(ns) => Some(ErrorEnvelope {
                errors: vec![ErrorEntry {
                    path: "ns".to_string(),
                    code: ErrorCode::NamespaceInUse,
                    hint: Some(format!("`{ns}` is already a user or an organisation")),
                }],
            }),
            Self::RegistryEntryExists(address) => Some(ErrorEnvelope {
                errors: vec![ErrorEntry {
                    path: String::new(),
                    code: ErrorCode::RegistryEntryExists,
                    hint: Some(format!(
                        "`{address}` is registered; a correction is a new version"
                    )),
                }],
            }),
            _ => None,
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(e) => write!(f, "validation failed with {} error(s)", e.len()),
            Self::AttachmentMissing(_) => write!(f, "attachment missing"),
            Self::LabelInUse => write!(f, "label in use"),
            Self::NamespaceInUse(ns) => write!(f, "namespace {ns} in use"),
            Self::NotFound => write!(f, "not found"),
            Self::Forbidden => write!(f, "forbidden"),
            Self::Unauthorized => write!(f, "unauthorized"),
            Self::SessionRejected { .. } => write!(f, "session rejected"),
            Self::BadRequest => write!(f, "bad request"),
            Self::Unavailable(what) => write!(f, "unavailable: {what}"),
            Self::NotImplemented(what) => write!(f, "not implemented: {what}"),
            Self::RegistryEntryExists(address) => write!(f, "registry entry exists: {address}"),
            Self::Conflict(e) => write!(f, "conflict with {} entr(y/ies)", e.len()),
            Self::TooLarge(_) => write!(f, "request too large"),
            Self::Internal(e) => write!(f, "internal error: {e:#}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self::Internal(e)
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::LabelInUse => Self::LabelInUse,
            StoreError::LabelInvalid => Self::BadRequest,
            StoreError::NamespaceInUse(ns) => Self::NamespaceInUse(ns),
            // A namespace the hub does not know cannot be written to by
            // anyone; the caller's token would not cover it either, so the
            // answer is the same as for a namespace they may not see.
            StoreError::RegistryCoreReadOnly => Self::Forbidden,
            StoreError::RegistryEntryExists(address) => Self::RegistryEntryExists(address),
            StoreError::RegistryEntryNotFound(_) => Self::NotFound,
            StoreError::NamespaceUnknown(_)
            | StoreError::NotAnOrganisation(_)
            | StoreError::RecordNotFound
            | StoreError::VersionNotFound
            | StoreError::AlreadyTombstoned
            | StoreError::RunNotFound => Self::NotFound,
            // Which Cards, and why, is deliberately not said; see the
            // module doc.
            StoreError::RunCardsUnknown(_) => Self::NotFound,
            StoreError::RunDeleted => Self::Conflict(vec![ErrorEntry {
                path: "/run_id".to_string(),
                code: ErrorCode::RunDeleted,
                hint: Some("the run was deleted; a deleted run is not changed again".to_string()),
            }]),
            // A batch or a 1.0 body; a single `PUT` converts it itself,
            // without the index prefix.
            StoreError::RunsRejected(rejections) => Self::runs_rejected(rejections, true),
            StoreError::CardRunsRejected(errors) => Self::Validation(errors),
            // Needs the record's attachment paths to name the offenders;
            // the handler converts it before it reaches here.
            other => Self::Internal(anyhow::Error::new(other)),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> HttpResponse {
        if let Self::Internal(e) = &self {
            error!(error = %format!("{e:#}"), "request failed");
        }
        let status = self.status();
        if let Self::SessionRejected { secure } = self {
            return (
                status,
                [(
                    axum::http::header::SET_COOKIE,
                    crate::auth::cleared_session_header(secure),
                )],
            )
                .into_response();
        }
        match self.envelope() {
            Some(body) => (status, Json(body)).into_response(),
            None => status.into_response(),
        }
    }
}

impl OperationOutput for ApiError {
    type Inner = ErrorEnvelope;

    fn operation_response(ctx: &mut GenContext, operation: &mut Operation) -> Option<Response> {
        Json::<ErrorEnvelope>::operation_response(ctx, operation)
    }

    /// The error responses every handler returning `ApiError` documents.
    ///
    /// `413` is listed only for an operation that takes a request body:
    /// the router's `DefaultBodyLimit` refuses bodies, so an operation
    /// without one (a `GET`, a `HEAD`) can never answer it. aide adds the
    /// handler's inputs before its outputs, so `request_body` is already
    /// set when this runs. Handlers that do not return `ApiError`
    /// (`healthz`, `whoami`) document none of these.
    fn inferred_responses(
        ctx: &mut GenContext,
        operation: &mut Operation,
    ) -> Vec<(Option<ApiStatus>, Response)> {
        let takes_body = operation.request_body.is_some();
        let mut envelope = |description: &str| {
            let mut r =
                Json::<ErrorEnvelope>::operation_response(ctx, operation).unwrap_or_default();
            r.description = description.to_string();
            r
        };
        let empty = |description: &str| Response {
            description: description.to_string(),
            ..Default::default()
        };
        let mut responses = vec![
            (
                Some(ApiStatus::Code(422)),
                envelope("The record is invalid; every violation is listed."),
            ),
            (
                Some(ApiStatus::Code(409)),
                envelope(
                    "State conflict: an attachment is not ready, the label is taken, or the \
                     run was deleted.",
                ),
            ),
            (
                Some(ApiStatus::Code(404)),
                empty("Not found, or private to a namespace the caller cannot see."),
            ),
            (
                Some(ApiStatus::Code(403)),
                empty("The token lacks the scope or namespace."),
            ),
            (
                Some(ApiStatus::Code(401)),
                empty("No token, or an unrecognised one."),
            ),
        ];
        if takes_body {
            responses.push((
                Some(ApiStatus::Code(413)),
                envelope(
                    "The body is larger than `limits.body_bytes`, or a batch carries more \
                     runs than `limits.batch_runs`.",
                ),
            ));
        }
        responses
    }
}
