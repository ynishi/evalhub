//! Identity rows: users, namespaces and tokens.
//!
//! The semantics of tokens, scopes and organisation roles live in
//! `evalhub_server::auth`; this module only reads and writes the
//! `users`, `namespaces`, `orgs` and `tokens` tables. Three things it
//! promises:
//!
//! - A token's secret is never stored. Callers hand in the sha256 of the
//!   secret, and [`find_token`] looks up by that hash.
//! - A namespace is created explicitly ([`ensure_namespace`]), by user or
//!   organisation creation, never as a side effect of a record write.
//! - Revocation is a timestamp, not a delete: [`find_token`] returns
//!   revoked tokens with `revoked_at` set so the caller can say "revoked"
//!   rather than "unknown".

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{SQLSTATE_UNIQUE, StoreError, violated_constraint};

/// What a namespace belongs to. Stored as `namespaces.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespaceKind {
    /// A user's personal namespace.
    User,
    /// An organisation.
    Org,
}

impl NamespaceKind {
    /// The value stored in `namespaces.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            NamespaceKind::User => "user",
            NamespaceKind::Org => "org",
        }
    }
}

/// A token's scope. Stored as `tokens.scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Read private records in the token's namespaces.
    Read,
    /// `Read` plus every write.
    Write,
    /// `Write` plus membership, token issue for the org, audit.
    Admin,
}

impl Scope {
    /// The value stored in `tokens.scope`.
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Admin => "admin",
        }
    }

    /// Parse the stored value. The column is CHECK-constrained, so an
    /// unknown value is a migration bug and reads as the least scope.
    pub fn parse(s: &str) -> Scope {
        match s {
            "admin" => Scope::Admin,
            "write" => Scope::Write,
            _ => Scope::Read,
        }
    }
}

/// A token as looked up by its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    /// `tokens.token_id`.
    pub token_id: Uuid,
    /// The owning user.
    pub user_id: Uuid,
    /// The owning user's login.
    pub login: String,
    /// The token's scope.
    pub scope: Scope,
    /// Namespaces the token covers.
    pub namespaces: Vec<String>,
    /// Set once revoked; the caller treats a set value as "not valid".
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Create the `namespaces` row for `ns` (and the `orgs` row for an org) if
/// it does not exist. Idempotent.
pub async fn ensure_namespace(
    pool: &PgPool,
    ns: &str,
    kind: NamespaceKind,
) -> Result<(), StoreError> {
    let mut tx = pool.begin().await?;
    sqlx::query!(
        "INSERT INTO namespaces (ns, kind) VALUES ($1, $2) ON CONFLICT (ns) DO NOTHING",
        ns,
        kind.as_str(),
    )
    .execute(&mut *tx)
    .await?;
    if kind == NamespaceKind::Org {
        sqlx::query!(
            "INSERT INTO orgs (ns) VALUES ($1) ON CONFLICT (ns) DO NOTHING",
            ns
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Create a user and its personal namespace (`ns == login`) in one
/// transaction. Returns the new `user_id`. Errors with
/// [`StoreError::LoginInUse`] when the login is taken.
pub async fn create_user(pool: &PgPool, login: &str) -> Result<Uuid, StoreError> {
    let user_id = Uuid::new_v4();
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query!(
        "INSERT INTO users (user_id, login) VALUES ($1, $2)",
        user_id,
        login
    )
    .execute(&mut *tx)
    .await;
    if let Err(e) = inserted {
        if let Some((code, constraint)) = violated_constraint(&e)
            && code == SQLSTATE_UNIQUE
            && constraint == "users_login_key"
        {
            return Err(StoreError::LoginInUse(login.to_owned()));
        }
        return Err(e.into());
    }
    sqlx::query!(
        "INSERT INTO namespaces (ns, kind) VALUES ($1, 'user') ON CONFLICT (ns) DO NOTHING",
        login,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(user_id)
}

/// Store a token. `token_hash` is the sha256 of the secret the caller
/// generated; the secret itself never reaches this crate. Returns the
/// `token_id`.
pub async fn create_token(
    pool: &PgPool,
    user_id: Uuid,
    scope: Scope,
    namespaces: &[String],
    token_hash: &[u8; 32],
) -> Result<Uuid, StoreError> {
    let token_id = Uuid::new_v4();
    let hash: &[u8] = token_hash;
    sqlx::query!(
        "INSERT INTO tokens (token_id, user_id, token_hash, scope, namespaces) VALUES ($1, $2, $3, $4, $5)",
        token_id,
        user_id,
        hash,
        scope.as_str(),
        namespaces,
    )
    .execute(pool)
    .await?;
    Ok(token_id)
}

/// Look a token up by the sha256 of its secret. `None` when no token has
/// that hash; a revoked token is returned with `revoked_at` set.
pub async fn find_token(
    pool: &PgPool,
    token_hash: &[u8; 32],
) -> Result<Option<TokenRow>, StoreError> {
    let hash: &[u8] = token_hash;
    let row = sqlx::query!(
        "SELECT t.token_id, t.user_id, u.login, t.scope, t.namespaces, t.revoked_at
         FROM tokens t JOIN users u ON u.user_id = t.user_id
         WHERE t.token_hash = $1",
        hash,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| TokenRow {
        token_id: r.token_id,
        user_id: r.user_id,
        login: r.login,
        scope: Scope::parse(&r.scope),
        namespaces: r.namespaces,
        revoked_at: r.revoked_at,
    }))
}
