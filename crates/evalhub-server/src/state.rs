//! Shared state handed to every handler.

use std::sync::Arc;

use aide::openapi::OpenApi;

use crate::auth::CursorSigner;
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
    /// Object store, when `s3.endpoint` and its credentials are
    /// configured. `None` leaves the attachment endpoints answering
    /// `503`: the rest of the hub works without them.
    pub objects: Option<Arc<evalhub_store::objects::Objects>>,
    /// The assembled OpenAPI document.
    pub openapi: Arc<OpenApi>,
    /// Signs pagination cursors with `auth.cursor_key`.
    pub cursors: CursorSigner,
}

impl AppState {
    /// The pool, or an internal error for a router built without one.
    pub fn db(&self) -> Result<&evalhub_store::PgPool, ApiError> {
        self.db
            .as_ref()
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("no database configured")))
    }

    /// The object store, or `503` when none is configured. Unlike a
    /// missing database this is a deployment choice rather than a bug, so
    /// it is reported to the caller as a capability the hub does not have
    /// right now.
    pub fn objects(&self) -> Result<&evalhub_store::objects::Objects, ApiError> {
        self.objects
            .as_deref()
            .ok_or(ApiError::Unavailable("object storage is not configured"))
    }
}
