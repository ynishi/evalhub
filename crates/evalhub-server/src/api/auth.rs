//! Token, namespace and organisation handlers. Semantics in
//! [`crate::auth`]. Token secrets are returned exactly once, in the `POST
//! /tokens` response; every later read shows the prefix and the hash only.
//!
//! | Endpoint                          | Who may call it                                   |
//! | --------------------------------- | ------------------------------------------------- |
//! | `POST /session`                   | anyone holding a token; opens a UI session        |
//! | `DELETE /session`                 | the session's own cookie                          |
//! | `GET /tokens`                     | any token; lists its owner's tokens               |
//! | `POST /tokens`                    | any token; see the namespace rule below           |
//! | `DELETE /tokens/{token_id}`       | the owner of that token                           |
//! | `GET /namespaces/{ns}`            | anyone; private records are not counted for them  |
//! | `POST /orgs`                      | any token; the caller becomes its first `admin`   |
//! | `GET /orgs/{org}/members`         | a member of the org                               |
//! | `POST,DELETE /orgs/{org}/members` | `admin` on the org                                |
//!
//! A new token may name the caller's own login and any organisation where
//! the caller is an `admin` *member*, and its scope may not exceed the
//! presenting token's. Membership is read from the organisation, not from
//! the presenting token's namespace list, because the token that creates
//! an organisation does not cover it yet — while the scope ceiling keeps a
//! token from being a ladder: none can mint one that may do more than it.

use aide::axum::IntoApiResponse;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum_extra::extract::PrivateCookieJar;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use evalhub_store::auth::{self as store_auth, NamespaceKind, Scope};

use crate::api::meta::Whoami;
use crate::auth::{Auth, MaybeAuth};
use crate::error::ApiError;
use crate::state::AppState;

/// A scope as it crosses the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeDto {
    /// Read private records in the token's namespaces.
    Read,
    /// Also write records, relations, settings and registry entries.
    Write,
    /// Also manage organisation members, issue organisation tokens, read audit.
    Admin,
}

impl From<Scope> for ScopeDto {
    fn from(s: Scope) -> Self {
        match s {
            Scope::Read => Self::Read,
            Scope::Write => Self::Write,
            Scope::Admin => Self::Admin,
        }
    }
}

impl From<ScopeDto> for Scope {
    fn from(s: ScopeDto) -> Self {
        match s {
            ScopeDto::Read => Self::Read,
            ScopeDto::Write => Self::Write,
            ScopeDto::Admin => Self::Admin,
        }
    }
}

/// One of the caller's tokens. The secret is not here; it was shown once.
#[derive(Debug, Serialize, JsonSchema)]
pub struct TokenDto {
    /// Identifier of the token, used to revoke it.
    pub token_id: String,
    /// First eight hex characters of the stored hash, to tell tokens apart.
    pub prefix: String,
    /// What the token may do.
    pub scope: ScopeDto,
    /// Namespaces it covers.
    pub namespaces: Vec<String>,
    /// When it was issued.
    pub created_at: DateTime<Utc>,
    /// When it was revoked, if it was. A revoked token is refused.
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<store_auth::TokenSummary> for TokenDto {
    fn from(t: store_auth::TokenSummary) -> Self {
        Self {
            token_id: t.token_id.to_string(),
            prefix: t.prefix,
            scope: t.scope.into(),
            namespaces: t.namespaces,
            created_at: t.created_at,
            revoked_at: t.revoked_at,
        }
    }
}

/// Body of `POST /tokens`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewTokenBody {
    /// Scope of the new token; may not exceed the presenting token's.
    pub scope: ScopeDto,
    /// Namespaces it covers: the caller's login, and organisations where
    /// the caller is `admin`.
    pub namespaces: Vec<String>,
}

/// Response of `POST /tokens`. The only time the secret is shown.
#[derive(Debug, Serialize, JsonSchema)]
pub struct IssuedToken {
    /// Identifier of the token, used to revoke it.
    pub token_id: String,
    /// The secret, to be sent as `Authorization: Bearer <secret>`. It is
    /// not stored and cannot be shown again.
    pub secret: String,
    /// What the token may do.
    pub scope: ScopeDto,
    /// Namespaces it covers.
    pub namespaces: Vec<String>,
}

/// A namespace and what it holds.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NamespaceDto {
    /// The namespace: a user login or an organisation slug.
    pub ns: String,
    /// `user` or `org`.
    pub kind: String,
    /// When it was created.
    pub created_at: DateTime<Utc>,
    /// Cards in it that the caller may see.
    pub cards: i64,
    /// Evals in it that the caller may see.
    pub evals: i64,
}

/// Body of `POST /orgs`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewOrgBody {
    /// Slug of the organisation, which is also its namespace.
    pub ns: String,
}

/// A member of an organisation.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MemberDto {
    /// The member's login.
    pub user: String,
    /// Their role in the organisation.
    pub role: ScopeDto,
}

/// Body of `POST /orgs/{org}/members`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewMemberBody {
    /// Login of the user to add.
    pub user: String,
    /// The role they get.
    pub role: ScopeDto,
}

/// A list of things, without paging: these are all small.
///
/// The schema name carries the item type, so a generated client gets one
/// list type per item type instead of `Items` and `Items2`.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(rename = "Items_for_{T}")]
pub struct Items<T> {
    /// The items.
    pub items: Vec<T>,
}

/// `GET /api/v1/tokens` — the caller's own tokens, secrets excluded.
pub async fn list_tokens(
    State(state): State<AppState>,
    Auth(caller): Auth,
) -> Result<Json<Items<TokenDto>>, ApiError> {
    let id = caller.identity()?;
    let items = store_auth::list_tokens(state.db()?, id.user_id)
        .await?
        .into_iter()
        .map(TokenDto::from)
        .collect();
    Ok(Json(Items { items }))
}

/// `POST /api/v1/tokens` — issue a token, returning its secret once.
pub async fn create_token(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Json(body): Json<NewTokenBody>,
) -> Result<(StatusCode, Json<IssuedToken>), ApiError> {
    let id = caller.identity()?;
    let scope: Scope = body.scope.into();
    if scope > id.scope {
        return Err(ApiError::Forbidden);
    }
    if body.namespaces.is_empty() {
        return Err(ApiError::BadRequest);
    }
    let pool = state.db()?;
    for ns in &body.namespaces {
        // Own personal namespace, or an organisation the caller
        // administers. The question is what the *user* may delegate, not
        // what the presenting token happens to cover: otherwise whoever
        // creates an organisation could never issue a token for it.
        if *ns == id.login {
            continue;
        }
        if store_auth::org_role(pool, ns, id.user_id).await? != Some(Scope::Admin) {
            return Err(ApiError::Forbidden);
        }
    }
    let (secret, hash) = crate::auth::new_secret();
    let token_id =
        store_auth::create_token(pool, id.user_id, scope, &body.namespaces, &hash).await?;
    Ok((
        StatusCode::CREATED,
        Json(IssuedToken {
            token_id: token_id.to_string(),
            secret,
            scope: scope.into(),
            namespaces: body.namespaces,
        }),
    ))
}

/// `DELETE /api/v1/tokens/{token_id}` — revoke one of the caller's tokens.
pub async fn revoke_token(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Path(token_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let id = caller.identity()?;
    let token_id = token_id.parse().map_err(|_| ApiError::NotFound)?;
    let revoked =
        store_auth::revoke_token(state.db()?, id.user_id, token_id, caller.actor()).await?;
    if revoked {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// `GET /api/v1/namespaces/{ns}` — what a namespace holds, as this caller
/// may see it.
pub async fn namespace(
    State(state): State<AppState>,
    MaybeAuth(caller): MaybeAuth,
    Path(ns): Path<String>,
) -> Result<Json<NamespaceDto>, ApiError> {
    let info = store_auth::namespace(state.db()?, &ns, &caller.namespaces())
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(NamespaceDto {
        ns: info.ns,
        kind: match info.kind {
            NamespaceKind::User => "user",
            NamespaceKind::Org => "org",
        }
        .to_string(),
        created_at: info.created_at,
        cards: info.cards,
        evals: info.evals,
    }))
}

/// `POST /api/v1/orgs` — create an organisation with the caller as its
/// first `admin`.
pub async fn create_org(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Json(body): Json<NewOrgBody>,
) -> Result<(StatusCode, Json<NamespaceDto>), ApiError> {
    let id = caller.identity()?;
    let pool = state.db()?;
    store_auth::create_org(pool, &body.ns, caller.actor()).await?;
    store_auth::add_org_member(pool, &body.ns, id.user_id, Scope::Admin, caller.actor()).await?;
    let info = store_auth::namespace(pool, &body.ns, std::slice::from_ref(&body.ns))
        .await?
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("organisation vanished after create")))?;
    Ok((
        StatusCode::CREATED,
        Json(NamespaceDto {
            ns: info.ns,
            kind: "org".to_string(),
            created_at: info.created_at,
            cards: info.cards,
            evals: info.evals,
        }),
    ))
}

/// `GET /api/v1/orgs/{org}/members` — the organisation's members.
pub async fn list_members(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Path(org): Path<String>,
) -> Result<Json<Items<MemberDto>>, ApiError> {
    // Membership of any level is enough to see the roster.
    if !caller.allows(&org, Scope::Read) {
        return Err(ApiError::Forbidden);
    }
    let items = store_auth::org_members(state.db()?, &org)
        .await?
        .into_iter()
        .map(|m| MemberDto {
            user: m.login,
            role: m.role.into(),
        })
        .collect();
    Ok(Json(Items { items }))
}

/// `POST /api/v1/orgs/{org}/members` — add or re-role a member.
pub async fn add_member(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Path(org): Path<String>,
    Json(body): Json<NewMemberBody>,
) -> Result<(StatusCode, Json<MemberDto>), ApiError> {
    if !caller.is_admin(&org) {
        return Err(ApiError::Forbidden);
    }
    let pool = state.db()?;
    let (user_id, login) = store_auth::user_by_login(pool, &body.user)
        .await?
        .ok_or(ApiError::NotFound)?;
    let role: Scope = body.role.into();
    store_auth::add_org_member(pool, &org, user_id, role, caller.actor()).await?;
    Ok((
        StatusCode::CREATED,
        Json(MemberDto {
            user: login,
            role: body.role,
        }),
    ))
}

/// `DELETE /api/v1/orgs/{org}/members/{user}` — remove a member.
pub async fn remove_member(
    State(state): State<AppState>,
    Auth(caller): Auth,
    Path((org, user)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    if !caller.is_admin(&org) {
        return Err(ApiError::Forbidden);
    }
    let pool = state.db()?;
    let (user_id, _) = store_auth::user_by_login(pool, &user)
        .await?
        .ok_or(ApiError::NotFound)?;
    let removed = store_auth::remove_org_member(pool, &org, user_id, caller.actor()).await?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

/// Body of `POST /session`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewSessionBody {
    /// A token secret, the same string a client would send as
    /// `Authorization: Bearer …`.
    pub token: String,
}

/// `POST /api/v1/session` — exchange a token for a session cookie.
///
/// The hub has no passwords, so the credential it can check is a token.
/// The presented one is validated exactly as the bearer header is, and
/// then a *second* token is minted for the same user with the same scope
/// and the same namespaces; that one goes into the cookie. The token the
/// operator pasted in is untouched, so ending the session cannot revoke
/// it.
pub async fn create_session(
    State(state): State<AppState>,
    jar: PrivateCookieJar,
    Json(body): Json<NewSessionBody>,
) -> Result<(PrivateCookieJar, Json<Whoami>), ApiError> {
    let pool = state.db()?;
    let presented = crate::auth::hash_secret(&body.token);
    let row = store_auth::find_token(pool, &presented)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    if row.revoked_at.is_some() {
        return Err(ApiError::Unauthorized);
    }

    let (secret, hash) = crate::auth::new_secret();
    store_auth::create_token(pool, row.user_id, row.scope, &row.namespaces, &hash).await?;
    let jar = jar.add(crate::auth::session_cookie_for(
        secret,
        state.cookie_key.secure(),
    ));
    Ok((
        jar,
        Json(Whoami {
            user: Some(row.login),
            namespaces: row.namespaces,
            scope: Some(row.scope.as_str().to_string()),
        }),
    ))
}

/// `DELETE /api/v1/session` — end the session.
///
/// Revokes the token the cookie carries and clears the cookie. A request
/// without a session cookie is still `204`: the caller's wish, that no
/// session remain, already holds.
pub async fn delete_session(
    State(state): State<AppState>,
    jar: PrivateCookieJar,
) -> Result<(PrivateCookieJar, StatusCode), ApiError> {
    let secure = state.cookie_key.secure();
    if let Some(cookie) = jar.get(crate::auth::SESSION_COOKIE) {
        let pool = state.db()?;
        let hash = crate::auth::hash_secret(cookie.value());
        if let Some(row) = store_auth::find_token(pool, &hash).await? {
            let actor = evalhub_store::records::Actor {
                user_id: Some(row.user_id),
                token_id: Some(row.token_id),
            };
            store_auth::revoke_token(pool, row.user_id, row.token_id, actor).await?;
        }
    }
    Ok((
        jar.remove(crate::auth::cleared_session_cookie(secure)),
        StatusCode::NO_CONTENT,
    ))
}

/// Keeps `IntoApiResponse` in scope for the handlers above that return
/// bare status codes.
const _: fn() = || {
    fn assert_into_api_response<T: IntoApiResponse>() {}
    assert_into_api_response::<StatusCode>();
};
