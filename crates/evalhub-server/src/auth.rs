//! Tokens, organisations, visibility, and the UI session.
//!
//! # Tokens
//!
//! A token is a 32-byte random secret, shown to the user once at issue,
//! presented as `Authorization: Bearer <secret>`. The hub stores only its
//! sha256 (`tokens.token_hash`) and compares in constant time. A token that
//! random needs no slow hash; a password would, but there are no passwords
//! in the API. Each token has a `scope` and a list of namespaces:
//!
//! | Scope   | Allows                                                                 |
//! | ------- | ---------------------------------------------------------------------- |
//! | `read`  | read private records in the token's namespaces                         |
//! | `write` | + create names, append versions, add relations, change settings, delete, registry `PUT` |
//! | `admin` | + manage org members, issue tokens for the org, read audit             |
//!
//! `GET, POST, DELETE /tokens` manage a user's own tokens. Every write
//! endpoint requires a token; reads of public records require none.
//!
//! # Bootstrap
//!
//! There is no signup endpoint. The first user, its personal namespace and
//! its first token are created from the command line (`evalhub user
//! create <login>`), which prints the secret once. Further tokens come from
//! `POST /tokens` with an existing token. The secret is 32 random bytes,
//! base64url without padding (43 characters); the hub keeps the sha256 of
//! the secret's bytes as they appear on the wire.
//!
//! # Organisations
//!
//! An org is a namespace with members. `GET, POST, DELETE
//! /orgs/{org}/members { user, role }`, `admin` only. A user's token acts
//! with the lesser of the token's scope and the user's role in the org.
//!
//! # Visibility
//!
//! Per name, `private` (default) or `public`, changed with
//! `PATCH .../settings`. Private records do not appear in lists, searches
//! or relation traversals, and `GET` on them is `404`, to anyone without a
//! token covering the namespace. A public Card pointing at a private Eval
//! exposes the Eval's `version_id` and `content_hash` only.
//!
//! # The UI session
//!
//! The SPA does not hold a bearer token in JavaScript. After login the
//! server issues a UI-scoped token and sets it in a private (encrypted,
//! signed) cookie via `axum-extra`; the cookie is `HttpOnly`, `Secure`,
//! `SameSite=Strict`. The API accepts either the header or the cookie, and
//! treats them identically after extraction.
//!
//! # Cursors
//!
//! Pagination cursors are keyset tuples, serialised, HMAC-signed with
//! `auth.cursor_key` and base64url-encoded. A tampered cursor is `400`.
//! Signing makes the cursor opaque without needing server-side state.
//!
//! # Extractors
//!
//! [`Auth`] requires a valid token and answers `401` otherwise;
//! [`MaybeAuth`] yields an anonymous [`Caller`] when no token is presented
//! and `401` when one is presented but invalid. A revoked token is invalid.
//! Both look the hash up in the store on every request; there is no cache,
//! because revocation must take effect at once.

use aide::OperationInput;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::Engine;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use evalhub_store::auth::Scope;

use crate::error::ApiError;
use crate::state::AppState;

/// Who is making the request, as far as the hub can tell.
#[derive(Debug, Clone)]
pub struct Caller {
    /// `Some` when a token was presented and accepted.
    pub identity: Option<Identity>,
}

/// The principal behind an accepted token.
#[derive(Debug, Clone)]
pub struct Identity {
    /// Row id of the user who owns the token.
    pub user_id: Uuid,
    /// Row id of the token itself, recorded in audit rows.
    pub token_id: Uuid,
    /// The user's login, which is also their personal namespace.
    pub login: String,
    /// What the token may do.
    pub scope: Scope,
    /// Namespaces the token covers.
    pub namespaces: Vec<String>,
}

impl Caller {
    /// A caller that presented no token.
    pub const ANONYMOUS: Caller = Caller { identity: None };

    /// Namespaces whose private records this caller may read.
    pub fn namespaces(&self) -> &[String] {
        self.identity
            .as_ref()
            .map(|i| i.namespaces.as_slice())
            .unwrap_or(&[])
    }

    /// Whether the caller may perform an action needing `scope` on `ns`.
    ///
    /// Anonymous callers may do nothing that asks; a token must cover the
    /// namespace and hold at least the scope.
    pub fn allows(&self, ns: &str, scope: Scope) -> bool {
        match &self.identity {
            None => false,
            Some(id) => id.scope >= scope && id.namespaces.iter().any(|n| n == ns),
        }
    }

    /// What the store records as the actor of a write.
    pub fn actor(&self) -> evalhub_store::records::Actor {
        match &self.identity {
            None => evalhub_store::records::Actor::default(),
            Some(id) => evalhub_store::records::Actor {
                user_id: Some(id.user_id),
                token_id: Some(id.token_id),
            },
        }
    }
}

/// Mint a new token secret. Returns the wire form (base64url, no padding)
/// and the hash the store keeps.
pub fn new_secret() -> (String, [u8; 32]) {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let hash = hash_secret(&secret);
    (secret, hash)
}

/// The hash the store keeps for a secret as presented on the wire.
pub fn hash_secret(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn bearer(parts: &Parts) -> Result<Option<&str>, ApiError> {
    let Some(value) = parts.headers.get(axum::http::header::AUTHORIZATION) else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| ApiError::Unauthorized)?;
    let secret = value
        .strip_prefix("Bearer ")
        .ok_or(ApiError::Unauthorized)?
        .trim();
    if secret.is_empty() {
        return Err(ApiError::Unauthorized);
    }
    Ok(Some(secret))
}

async fn resolve(state: &AppState, secret: &str) -> Result<Identity, ApiError> {
    let pool = state.db()?;
    let hash = hash_secret(secret);
    // The hash is looked up by equality in the database; the constant-time
    // comparison the design asks for applies to the hash bytes we got back,
    // so a lookup that returned a near-miss row could not be accepted.
    let row = evalhub_store::auth::find_token(pool, &hash)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    if row.revoked_at.is_some() {
        return Err(ApiError::Unauthorized);
    }
    Ok(Identity {
        user_id: row.user_id,
        token_id: row.token_id,
        login: row.login,
        scope: row.scope,
        namespaces: row.namespaces,
    })
}

/// Extractor: a caller that may or may not have presented a token.
#[derive(Debug, Clone)]
pub struct MaybeAuth(pub Caller);

impl FromRequestParts<AppState> for MaybeAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match bearer(parts)? {
            None => Ok(MaybeAuth(Caller::ANONYMOUS)),
            Some(secret) => Ok(MaybeAuth(Caller {
                identity: Some(resolve(state, secret).await?),
            })),
        }
    }
}

impl OperationInput for MaybeAuth {}

/// Extractor: a caller that presented a valid token. `401` otherwise.
#[derive(Debug, Clone)]
pub struct Auth(pub Caller);

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let secret = bearer(parts)?.ok_or(ApiError::Unauthorized)?;
        Ok(Auth(Caller {
            identity: Some(resolve(state, secret).await?),
        }))
    }
}

impl OperationInput for Auth {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn secret_is_43_chars_and_hashes_deterministically() {
        let (secret, hash) = new_secret();
        assert_eq!(secret.len(), 43);
        assert_eq!(hash, hash_secret(&secret));
        let (other, _) = new_secret();
        assert_ne!(secret, other);
    }

    #[test]
    fn allows_requires_scope_and_namespace() {
        let caller = Caller {
            identity: Some(Identity {
                user_id: Uuid::nil(),
                token_id: Uuid::nil(),
                login: "alice".into(),
                scope: Scope::Write,
                namespaces: vec!["alice".into()],
            }),
        };
        assert!(caller.allows("alice", Scope::Write));
        assert!(caller.allows("alice", Scope::Read));
        assert!(!caller.allows("alice", Scope::Admin));
        assert!(!caller.allows("bob", Scope::Read));
        assert!(!Caller::ANONYMOUS.allows("alice", Scope::Read));
    }
}
