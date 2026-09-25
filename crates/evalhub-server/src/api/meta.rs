//! Meta endpoints: health, identity, and the contract itself.

use std::sync::Arc;

use aide::axum::IntoApiResponse;
use aide::openapi::OpenApi;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use schemars::JsonSchema;
use serde::Serialize;

use crate::auth::MaybeAuth;
use crate::state::AppState;

/// `GET /api/v1/healthz`
#[derive(Debug, Serialize, JsonSchema)]
pub struct Health {
    /// Always `ok` when the process answers.
    pub status: &'static str,
    /// `connected` when a database pool is open, `not_configured` otherwise.
    pub database: &'static str,
}

/// Liveness. Answers as long as the process is up; does not touch the
/// database (a readiness probe that does is a later endpoint).
pub async fn healthz(State(state): State<AppState>) -> impl IntoApiResponse {
    Json(Health {
        status: "ok",
        database: if state.db.is_some() {
            "connected"
        } else {
            "not_configured"
        },
    })
}

/// `GET /api/v1/whoami`
#[derive(Debug, Serialize, JsonSchema)]
pub struct Whoami {
    /// Login of the authenticated user, or `null` for an anonymous caller.
    pub user: Option<String>,
    /// Namespaces the presented token covers. Empty when anonymous.
    pub namespaces: Vec<String>,
    /// Scope of the presented token, or `null` when anonymous.
    pub scope: Option<String>,
}

/// Identity of the caller as the hub sees it: the token's user, scope
/// and namespaces, or all empty for an anonymous caller.
pub async fn whoami(MaybeAuth(caller): MaybeAuth) -> impl IntoApiResponse {
    Json(match caller.identity {
        None => Whoami {
            user: None,
            namespaces: Vec::new(),
            scope: None,
        },
        Some(id) => Whoami {
            user: Some(id.login),
            namespaces: id.namespaces,
            scope: Some(id.scope.as_str().to_string()),
        },
    })
}

/// `GET /openapi.json` — the assembled OpenAPI 3.1 document.
pub async fn openapi(State(state): State<AppState>) -> Json<Arc<OpenApi>> {
    Json(state.openapi.clone())
}

/// `GET /schemas/{name}` — one of the generated JSON Schemas
/// (`evalhub_schema::SCHEMA_NAMES`: `card`, `eval` (the 1.0 Eval, until
/// 0.3.0), `eval-2`, `run`, `error`, `query`), with `$id` set to the URL it was
/// fetched from so that `$ref`s resolve to the same bytes.
///
/// The URL is rebuilt from the `Host` header and, when a proxy sets it,
/// `X-Forwarded-Proto`; otherwise the scheme is `http`.
pub async fn schema(Path(name): Path<String>, headers: HeaderMap) -> Response {
    let Some((_, schema)) = evalhub_schema::all_schemas()
        .into_iter()
        .find(|(n, _)| *n == name)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut value = schema.to_value();
    if let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
    {
        let proto = headers
            .get("x-forwarded-proto")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("http");
        if let Some(obj) = value.as_object_mut() {
            obj.insert(
                "$id".to_string(),
                serde_json::Value::String(format!("{proto}://{host}/schemas/{name}")),
            );
        }
    }
    (
        [(axum::http::header::CONTENT_TYPE, "application/schema+json")],
        Json(value),
    )
        .into_response()
}
