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
//! Two rules follow from that, and they are easy to confuse:
//!
//! - **Acting** in a namespace needs a token that *names* it. A personal
//!   token does not reach an organisation even when its owner administers
//!   the organisation; it has to ask for a token that covers it. That is
//!   what makes a leaked token bounded by what it was issued for.
//! - **Issuing** a token is about the user's standing, not the presenting
//!   token's coverage: a user may name their own login and any
//!   organisation where they are an `admin` member. Otherwise whoever
//!   creates an organisation could never obtain a token for it, since the
//!   token that created it does not cover it.
//!
//! The scope ceiling applies to both: a token never mints one that may do
//! more than itself.
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

use std::collections::BTreeMap;

use aide::OperationInput;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
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
    /// The user's role in each organisation among `namespaces`. An org
    /// namespace the token covers but the user is not a member of has no
    /// entry, and the token can do nothing there.
    pub org_roles: BTreeMap<String, Scope>,
}

impl Identity {
    /// What this token may do on `ns`: the token's own scope on the
    /// personal namespace, the lesser of the token's scope and the org role
    /// on an organisation, nothing anywhere else.
    pub fn effective_scope(&self, ns: &str) -> Option<Scope> {
        if !self.namespaces.iter().any(|n| n == ns) {
            return None;
        }
        if ns == self.login {
            return Some(self.scope);
        }
        self.org_roles.get(ns).map(|role| (*role).min(self.scope))
    }
}

impl Caller {
    /// A caller that presented no token.
    pub const ANONYMOUS: Caller = Caller { identity: None };

    /// Namespaces whose private records this caller may read: the token's
    /// personal namespace and the organisations it covers where the user
    /// is a member.
    pub fn namespaces(&self) -> Vec<String> {
        match &self.identity {
            None => Vec::new(),
            Some(id) => id
                .namespaces
                .iter()
                .filter(|ns| id.effective_scope(ns).is_some())
                .cloned()
                .collect(),
        }
    }

    /// Whether the caller may perform an action needing `scope` on `ns`.
    ///
    /// Anonymous callers may do nothing that asks; see
    /// [`Identity::effective_scope`] for how a token's scope combines with
    /// an organisation role.
    pub fn allows(&self, ns: &str, scope: Scope) -> bool {
        self.identity
            .as_ref()
            .and_then(|id| id.effective_scope(ns))
            .is_some_and(|effective| effective >= scope)
    }

    /// Whether the caller holds `admin` on `ns`.
    pub fn is_admin(&self, ns: &str) -> bool {
        self.allows(ns, Scope::Admin)
    }

    /// The identity, or `401` for an anonymous caller.
    pub fn identity(&self) -> Result<&Identity, ApiError> {
        self.identity.as_ref().ok_or(ApiError::Unauthorized)
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
    // One lookup per organisation the token covers. Tokens cover a handful
    // of namespaces, so this stays cheap; a single query would replace it
    // if that ever changes.
    let mut org_roles = BTreeMap::new();
    for ns in row.namespaces.iter().filter(|ns| **ns != row.login) {
        if let Some(role) = evalhub_store::auth::org_role(pool, ns, row.user_id).await? {
            org_roles.insert(ns.clone(), role);
        }
    }
    Ok(Identity {
        user_id: row.user_id,
        token_id: row.token_id,
        login: row.login,
        scope: row.scope,
        namespaces: row.namespaces,
        org_roles,
    })
}

/// Signs and verifies pagination cursors.
///
/// A cursor is `base64url(payload ‖ hmac_sha256(key, payload))` with no
/// padding. The payload is whatever the handler serialised (a keyset
/// tuple); the signature makes it opaque and tamper-evident without any
/// server-side state. The key is `auth.cursor_key`, or a random one per
/// process when unset, in which case cursors do not survive a restart —
/// acceptable for a single instance, a configuration error for several.
#[derive(Clone)]
pub struct CursorSigner {
    key: [u8; 32],
}

impl std::fmt::Debug for CursorSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CursorSigner([redacted])")
    }
}

impl CursorSigner {
    /// From a configured key (any length; it is hashed) or, with `None`,
    /// a random per-process key.
    pub fn new(configured: Option<&str>) -> Self {
        let key = match configured {
            Some(k) => Sha256::digest(k.as_bytes()).into(),
            None => {
                let mut k = [0u8; 32];
                rand::fill(&mut k);
                k
            }
        };
        Self { key }
    }

    fn mac(&self, payload: &[u8]) -> [u8; 32] {
        // A 32-byte key is always a valid HMAC key; the constructor cannot fail.
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key)
            .unwrap_or_else(|_| unreachable!("hmac accepts keys of any length"));
        mac.update(payload);
        mac.finalize().into_bytes().into()
    }

    /// Wrap `payload` into an opaque cursor string.
    pub fn sign(&self, payload: &[u8]) -> String {
        let mut bytes = Vec::with_capacity(payload.len() + 32);
        bytes.extend_from_slice(payload);
        bytes.extend_from_slice(&self.mac(payload));
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    /// Recover the payload from a cursor, or `None` if it was not signed
    /// by this key (or is not base64url at all).
    pub fn verify(&self, cursor: &str) -> Option<Vec<u8>> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(cursor)
            .ok()?;
        if bytes.len() < 32 {
            return None;
        }
        let (payload, tag) = bytes.split_at(bytes.len() - 32);
        let expected = self.mac(payload);
        if expected.ct_eq(tag).into() {
            Some(payload.to_vec())
        } else {
            None
        }
    }
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
    fn cursor_round_trips_and_rejects_tampering() {
        let signer = CursorSigner::new(Some("k"));
        let cursor = signer.sign(b"{\"seq\":3}");
        assert_eq!(signer.verify(&cursor).as_deref(), Some(&b"{\"seq\":3}"[..]));
        let other = CursorSigner::new(Some("other"));
        assert!(other.verify(&cursor).is_none());
        let mut tampered = cursor.clone();
        tampered.replace_range(0..1, if cursor.starts_with('A') { "B" } else { "A" });
        assert!(signer.verify(&tampered).is_none());
        assert!(signer.verify("not base64!").is_none());
        assert!(signer.verify("").is_none());
    }

    fn identity(scope: Scope, namespaces: &[&str], roles: &[(&str, Scope)]) -> Caller {
        Caller {
            identity: Some(Identity {
                user_id: Uuid::nil(),
                token_id: Uuid::nil(),
                login: "alice".into(),
                scope,
                namespaces: namespaces.iter().map(|s| s.to_string()).collect(),
                org_roles: roles.iter().map(|(ns, r)| (ns.to_string(), *r)).collect(),
            }),
        }
    }

    #[test]
    fn allows_requires_scope_and_namespace() {
        let caller = identity(Scope::Write, &["alice"], &[]);
        assert!(caller.allows("alice", Scope::Write));
        assert!(caller.allows("alice", Scope::Read));
        assert!(!caller.allows("alice", Scope::Admin));
        assert!(!caller.allows("bob", Scope::Read));
        assert!(!Caller::ANONYMOUS.allows("alice", Scope::Read));
        assert!(Caller::ANONYMOUS.namespaces().is_empty());
    }

    #[test]
    fn org_role_caps_the_token_scope() {
        // Admin token, but only a writer in the org: write yes, admin no.
        let writer = identity(Scope::Admin, &["alice", "acme"], &[("acme", Scope::Write)]);
        assert!(writer.allows("acme", Scope::Write));
        assert!(!writer.is_admin("acme"));
        assert!(writer.is_admin("alice"));
        // Read token, admin in the org: the token caps it at read.
        let reader = identity(Scope::Read, &["alice", "acme"], &[("acme", Scope::Admin)]);
        assert!(reader.allows("acme", Scope::Read));
        assert!(!reader.allows("acme", Scope::Write));
        // Token covers the org, user is not a member: nothing, and the org
        // is not among the readable namespaces either.
        let outsider = identity(Scope::Admin, &["alice", "acme"], &[]);
        assert!(!outsider.allows("acme", Scope::Read));
        assert_eq!(outsider.namespaces(), vec!["alice".to_string()]);
    }
}
