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
//! | `BadRequest`                    | 400    | empty                                 |
//! | `Unavailable(&'static str)`     | 503    | empty; a capability this deployment does not have |
//! | `Internal(anyhow::Error)`       | 500    | empty; logged with `error!`           |
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
    /// The request is malformed in a way the handler detected itself
    /// (bad cursor, unparseable path segment).
    BadRequest,
    /// The hub is not configured for this: attachments without an object
    /// store. Not a fault of the request, and not a bug — a deployment
    /// that left the capability out.
    Unavailable(&'static str),
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
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The error envelope, when the status carries one (`422` / `409`).
    fn envelope(&self) -> Option<ErrorEnvelope> {
        match self {
            Self::Validation(errors) | Self::AttachmentMissing(errors) => Some(ErrorEnvelope {
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
            Self::BadRequest => write!(f, "bad request"),
            Self::Unavailable(what) => write!(f, "unavailable: {what}"),
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
            StoreError::NamespaceUnknown(_)
            | StoreError::NotAnOrganisation(_)
            | StoreError::RecordNotFound
            | StoreError::VersionNotFound
            | StoreError::AlreadyTombstoned => Self::NotFound,
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

    fn inferred_responses(
        ctx: &mut GenContext,
        operation: &mut Operation,
    ) -> Vec<(Option<ApiStatus>, Response)> {
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
        vec![
            (
                Some(ApiStatus::Code(422)),
                envelope("The record is invalid; every violation is listed."),
            ),
            (
                Some(ApiStatus::Code(409)),
                envelope("State conflict: an attachment is not ready, or the label is taken."),
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
        ]
    }
}
