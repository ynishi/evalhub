//! Shared state handed to every handler.

use std::sync::Arc;

use aide::openapi::OpenApi;

use crate::config::Config;

/// Cloned into each handler by axum. Everything inside is cheap to clone.
#[derive(Clone)]
pub struct AppState {
    /// Effective configuration.
    pub config: Arc<Config>,
    /// Postgres pool, when a database is configured.
    pub db: Option<evalhub_store::PgPool>,
    /// The assembled OpenAPI document.
    pub openapi: Arc<OpenApi>,
}
