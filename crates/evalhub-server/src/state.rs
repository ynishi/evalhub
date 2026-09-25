//! Shared state handed to every handler.

use std::sync::Arc;

use aide::openapi::OpenApi;
use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use evalhub_query::typecheck::{ExtSchema, PathTable};
use evalhub_schema::RecordKind;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::auth::CursorSigner;
use crate::config::Config;
use crate::error::ApiError;

/// The query path tables, one per record kind, and the registry entries
/// the run projection's table is built from.
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
    /// The `applied` extension schemas both tables were built with. The
    /// run projection (`GET /evals/{ns}/{name}/runs`) has no fixed table:
    /// its `results[{card}]…` paths depend on the Cards a request names,
    /// so [`PathTables::for_runs`] builds one per request from these.
    pub ext: Vec<ExtSchema>,
}

impl PathTables {
    /// The table for one record kind.
    pub fn for_kind(&self, kind: RecordKind) -> &PathTable {
        match kind {
            RecordKind::Card => &self.card,
            RecordKind::Eval => &self.eval,
        }
    }

    /// The run projection's table for a request naming `cards`, with the
    /// same extension schemas the record tables hold. A few dozen entries;
    /// built per request.
    pub fn for_runs(&self, cards: &[evalhub_query::CardRef]) -> PathTable {
        PathTable::for_runs(cards).with_ext(&self.ext)
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
            ext: Vec::new(),
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
        *self.0.write().await = Arc::new(PathTables {
            card,
            eval,
            ext: entries.to_vec(),
        });
    }
}

impl Default for PathTableHandle {
    fn default() -> Self {
        Self::from_schema()
    }
}

/// The key the UI session cookie is encrypted with, and whether that
/// cookie is marked `Secure`.
///
/// `auth.cookie_key` is hashed to 32 bytes and expanded into the
/// signing and encryption halves `axum-extra` wants, the same treatment
/// [`CursorSigner`] gives `auth.cursor_key`. With no key configured the
/// process generates one, and every session ends when it restarts.
///
/// `Secure` tells the browser to send the cookie over HTTPS only, which
/// would make it invisible to a developer running `evalhub serve` on
/// `127.0.0.1`. The flag is therefore set unless `bind` is a loopback
/// address: a hub reachable from another machine gets it, a local one
/// does not. A deployment behind a proxy binds a routable address (or
/// `0.0.0.0`) and is covered.
#[derive(Clone)]
pub struct CookieKey {
    key: Key,
    secure: bool,
}

impl std::fmt::Debug for CookieKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CookieKey")
            .field("key", &"[redacted]")
            .field("secure", &self.secure)
            .finish()
    }
}

impl CookieKey {
    /// From `auth.cookie_key` (any length; it is hashed) and the address
    /// the server binds. `None` generates a key for this process only.
    pub fn new(configured: Option<&str>, bind: &str) -> Self {
        let key = match configured {
            Some(k) => {
                let seed: [u8; 32] = Sha256::digest(k.as_bytes()).into();
                Key::derive_from(&seed)
            }
            None => Key::generate(),
        };
        Self {
            key,
            secure: !binds_loopback(bind),
        }
    }

    /// Whether the session cookie should carry `Secure`.
    pub fn secure(&self) -> bool {
        self.secure
    }
}

/// Whether `bind` names an address only this machine can reach.
///
/// A value that is not an address at all — a host name, or something
/// malformed — is treated as reachable, so the doubtful case gets the
/// stricter flag rather than the convenient one.
fn binds_loopback(bind: &str) -> bool {
    // The configured form, `127.0.0.1:8080` or `[::1]:8080`.
    if let Ok(addr) = bind.parse::<std::net::SocketAddr>() {
        return addr.ip().is_loopback();
    }
    // A bare address, which `TcpListener::bind` would refuse, read here
    // anyway so the flag does not hinge on a port being present.
    if let Ok(ip) = bind.parse::<std::net::IpAddr>() {
        return ip.is_loopback();
    }
    false
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.key.clone()
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
    /// Encrypts the UI session cookie with `auth.cookie_key`.
    pub cookie_key: CookieKey,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secure_follows_the_bind_address() {
        // A hub only this machine can reach is usually plain http, and a
        // `Secure` cookie would never be sent back to it.
        for local in ["127.0.0.1:8080", "[::1]:8080", "127.0.0.1", "::1"] {
            assert!(!CookieKey::new(None, local).secure(), "{local} is loopback");
        }
        // Anything reachable from elsewhere gets the flag, including the
        // wildcard and a host name the server cannot resolve here.
        for remote in ["0.0.0.0:8080", "10.0.0.4:8080", "hub.example:443", ""] {
            assert!(
                CookieKey::new(None, remote).secure(),
                "{remote} is not loopback"
            );
        }
    }

    #[test]
    fn a_configured_key_is_stable_and_an_absent_one_is_not() {
        let a = CookieKey::new(Some("shared"), "127.0.0.1:8080");
        let b = CookieKey::new(Some("shared"), "127.0.0.1:8080");
        assert_eq!(a.key.master(), b.key.master(), "sessions survive a restart");
        let c = CookieKey::new(Some("other"), "127.0.0.1:8080");
        assert_ne!(a.key.master(), c.key.master());
        let d = CookieKey::new(None, "127.0.0.1:8080");
        let e = CookieKey::new(None, "127.0.0.1:8080");
        assert_ne!(d.key.master(), e.key.master(), "generated per process");
    }
}
