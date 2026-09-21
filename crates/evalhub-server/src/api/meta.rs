//! Meta endpoints: health, identity, and the contract itself.

use std::sync::Arc;

use aide::axum::IntoApiResponse;
use aide::openapi::OpenApi;
use axum::Json;
use axum::extract::State;
use schemars::JsonSchema;
use serde::Serialize;

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

/// Identity of the caller as the hub sees it. Until token auth is wired,
/// every caller is anonymous.
pub async fn whoami() -> impl IntoApiResponse {
    Json(Whoami {
        user: None,
        namespaces: Vec::new(),
        scope: None,
    })
}

/// `GET /openapi.json` — the assembled OpenAPI 3.1 document.
pub async fn openapi(State(state): State<AppState>) -> Json<Arc<OpenApi>> {
    Json(state.openapi.clone())
}
