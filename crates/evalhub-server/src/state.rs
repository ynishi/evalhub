//! Shared state handed to every handler.

use std::sync::Arc;

use aide::openapi::OpenApi;

use crate::config::Config;
use crate::error::ApiError;

/// Cloned into each handler by axum. Everything inside is cheap to clone.
#[derive(Clone)]
pub struct AppState {
    /// Effective configuration.
    pub config: Arc<Config>,
    /// Postgres pool, when a database is configured. `serve` refuses to
    /// start without one; `None` only occurs in tests of the meta routes.
    pub db: Option<evalhub_store::PgPool>,
    /// The assembled OpenAPI document.
    pub openapi: Arc<OpenApi>,
}

impl AppState {
    /// The pool, or an internal error for a router built without one.
    pub fn db(&self) -> Result<&evalhub_store::PgPool, ApiError> {
        self.db
            .as_ref()
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("no database configured")))
    }
}
