//! Shared state handed to every handler.

use std::sync::Arc;

use aide::openapi::OpenApi;
use evalhub_query::typecheck::{ExtSchema, PathTable};
use evalhub_schema::RecordKind;
use tokio::sync::RwLock;

use crate::auth::CursorSigner;
use crate::config::Config;
use crate::error::ApiError;

/// The query path tables, one per record kind.
///
/// A table is the vocabulary of the query language: which paths exist,
/// what type each has, and whether an index can serve an order or a
/// range on it. The facet paths come from the committed schema and never
/// change while the process runs; the `ext` paths come from the registry
/// and appear when the index job finishes building their expression
/// indexes.
#[derive(Debug)]
pub struct PathTables {
    /// Paths of a Card.
    pub card: PathTable,
    /// Paths of an Eval.
    pub eval: PathTable,
}

impl PathTables {
    /// The table for one record kind.
    pub fn for_kind(&self, kind: RecordKind) -> &PathTable {
        match kind {
            RecordKind::Card => &self.card,
            RecordKind::Eval => &self.eval,
        }
    }
}

/// A handle on the current [`PathTables`], shared by the handlers and the
/// index job.
///
/// Reads clone an `Arc`, so a query in flight keeps the table it started
/// with even if the job swaps in a new one mid-request: a page and its
/// cursor then agree about what the paths meant. Writes happen once per
/// applied `ext_schema`, which is rare.
#[derive(Clone, Debug)]
pub struct PathTableHandle(Arc<RwLock<Arc<PathTables>>>);

impl PathTableHandle {
    /// The tables as the committed schema alone describes them: every
    /// facet path, no registered extension.
    pub fn from_schema() -> Self {
        Self(Arc::new(RwLock::new(Arc::new(PathTables {
            card: PathTable::from_schema(RecordKind::Card),
            eval: PathTable::from_schema(RecordKind::Eval),
        }))))
    }

    /// The tables as they stand now.
    pub async fn current(&self) -> Arc<PathTables> {
        self.0.read().await.clone()
    }

    /// Rebuild both tables from the schema plus `entries`, which are the
    /// registry's `applied` extension schemas.
    pub async fn rebuild(&self, entries: &[ExtSchema]) {
        let card = PathTable::from_schema(RecordKind::Card).with_ext(entries);
        let eval = PathTable::from_schema(RecordKind::Eval).with_ext(entries);
        *self.0.write().await = Arc::new(PathTables { card, eval });
    }
}

impl Default for PathTableHandle {
    fn default() -> Self {
        Self::from_schema()
    }
}

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
    /// The query vocabulary, swapped when an `ext_schema` is applied.
    pub path_tables: PathTableHandle,
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
