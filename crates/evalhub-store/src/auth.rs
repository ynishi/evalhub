//! Identity rows: users, namespaces, organisations and tokens.
//!
//! The semantics of tokens, scopes and organisation roles live in
//! `evalhub_server::auth`; this module only reads and writes the
//! `users`, `namespaces`, `orgs`, `org_members` and `tokens` tables.
//! Three things it promises:
//!
//! - A token's secret is never stored. Callers hand in the sha256 of the
//!   secret, and [`find_token`] looks up by that hash. What a listing
//!   shows of a token is a prefix of that hash, never the secret.
//! - A namespace is created explicitly ([`create_user`], [`create_org`],
//!   [`ensure_namespace`]), never as a side effect of a record write.
//! - Revocation is a timestamp, not a delete: [`find_token`] returns
//!   revoked tokens with `revoked_at` set so the caller can say "revoked"
//!   rather than "unknown".
//!
//! # Organisations
//!
//! An organisation is a namespace of kind `org` with an `orgs` row and
//! members in `org_members (ns, user_id, role)`. A user's effective scope
//! in an org is the lesser of their token's scope and their role
//! ([`org_role`]); that combination is the server's rule, this module
//! only stores the role.
//!
//! Every write here appends an audit row (see [`crate::audit`]); token
//! writes append one row per namespace the token covers, so that an
//! org's admin sees tokens issued for the org.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit::{NewAudit, append};
use crate::error::{SQLSTATE_FOREIGN_KEY, SQLSTATE_UNIQUE, StoreError, violated_constraint};
use crate::records::{Actor, RecordType};

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

    /// Parse the stored value; the column is CHECK-constrained.
    pub fn parse(s: &str) -> NamespaceKind {
        if s == "org" {
            NamespaceKind::Org
        } else {
            NamespaceKind::User
        }
    }
}

/// A token's scope, and an org member's role. Stored as `tokens.scope`
/// and `org_members.role`.
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

/// A token as shown in a user's own listing: everything but the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSummary {
    /// `tokens.token_id`.
    pub token_id: Uuid,
    /// First eight hex characters of the stored hash, for telling tokens
    /// apart in a list.
    pub prefix: String,
    /// The token's scope.
    pub scope: Scope,
    /// Namespaces the token covers.
    pub namespaces: Vec<String>,
    /// When it was issued.
    pub created_at: DateTime<Utc>,
    /// When it was revoked, if it was.
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A namespace as shown by `GET /namespaces/{ns}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceInfo {
    /// The namespace.
    pub ns: String,
    /// User or org.
    pub kind: NamespaceKind,
    /// When it was created.
    pub created_at: DateTime<Utc>,
    /// Cards in it visible to the caller.
    pub cards: i64,
    /// Evals in it visible to the caller.
    pub evals: i64,
}

/// An organisation member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// The user.
    pub user_id: Uuid,
    /// Their login.
    pub login: String,
    /// Their role in the org.
    pub role: Scope,
}

/// Create the `namespaces` row for `ns` (and the `orgs` row for an org) if
/// it does not exist. Idempotent; writes no audit row (it is the
/// bootstrap's and the tests' tool, not an API action).
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

/// The user with this login, as `(user_id, login)`.
pub async fn user_by_login(
    pool: &PgPool,
    login: &str,
) -> Result<Option<(Uuid, String)>, StoreError> {
    let row = sqlx::query!("SELECT user_id, login FROM users WHERE login = $1", login)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| (r.user_id, r.login)))
}

/// Store a token. `token_hash` is the sha256 of the secret the caller
/// generated; the secret itself never reaches this crate. Returns the
/// `token_id`. Audits `token.create` once per namespace, as the user.
pub async fn create_token(
    pool: &PgPool,
    user_id: Uuid,
    scope: Scope,
    namespaces: &[String],
    token_hash: &[u8; 32],
) -> Result<Uuid, StoreError> {
    let token_id = Uuid::new_v4();
    let hash: &[u8] = token_hash;
    let mut tx = pool.begin().await?;
    sqlx::query!(
        "INSERT INTO tokens (token_id, user_id, token_hash, scope, namespaces) VALUES ($1, $2, $3, $4, $5)",
        token_id,
        user_id,
        hash,
        scope.as_str(),
        namespaces,
    )
    .execute(&mut *tx)
    .await?;
    let subject = token_id.to_string();
    let actor = Actor {
        user_id: Some(user_id),
        token_id: None,
    };
    for ns in namespaces {
        append(
            &mut tx,
            NewAudit {
                actor,
                ns: Some(ns),
                action: "token.create",
                subject: Some(&subject),
                detail: Some(
                    serde_json::json!({ "scope": scope.as_str(), "namespaces": namespaces }),
                ),
            },
        )
        .await?;
    }
    tx.commit().await?;
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

/// A user's tokens, oldest first, revoked ones included.
pub async fn list_tokens(pool: &PgPool, user_id: Uuid) -> Result<Vec<TokenSummary>, StoreError> {
    let rows = sqlx::query!(
        "SELECT token_id, token_hash, scope, namespaces, created_at, revoked_at
         FROM tokens WHERE user_id = $1 ORDER BY created_at, token_id",
        user_id,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| TokenSummary {
            token_id: r.token_id,
            prefix: r
                .token_hash
                .iter()
                .take(4)
                .map(|b| format!("{b:02x}"))
                .collect(),
            scope: Scope::parse(&r.scope),
            namespaces: r.namespaces,
            created_at: r.created_at,
            revoked_at: r.revoked_at,
        })
        .collect())
}

/// Revoke one of `user_id`'s tokens. `false` when the token is not theirs
/// or is already revoked. Audits `token.revoke` once per namespace.
pub async fn revoke_token(
    pool: &PgPool,
    user_id: Uuid,
    token_id: Uuid,
    actor: Actor,
) -> Result<bool, StoreError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query!(
        "UPDATE tokens SET revoked_at = now()
         WHERE token_id = $1 AND user_id = $2 AND revoked_at IS NULL
         RETURNING namespaces",
        token_id,
        user_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let subject = token_id.to_string();
    for ns in &row.namespaces {
        append(
            &mut tx,
            NewAudit {
                actor,
                ns: Some(ns),
                action: "token.revoke",
                subject: Some(&subject),
                detail: None,
            },
        )
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// A namespace with the count of records in it visible to the caller.
pub async fn namespace(
    pool: &PgPool,
    ns: &str,
    caller_namespaces: &[String],
) -> Result<Option<NamespaceInfo>, StoreError> {
    let row = sqlx::query!(
        "SELECT ns, kind, created_at FROM namespaces WHERE ns = $1",
        ns
    )
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut counts = [0i64; 2];
    for (i, rt) in [RecordType::Card, RecordType::Eval].iter().enumerate() {
        counts[i] = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "count!" FROM records r
               WHERE r.type = $1 AND r.ns = $2
                 AND (r.visibility = 'public' OR r.ns = ANY($3))
                 AND EXISTS (SELECT 1 FROM versions v WHERE v.record_id = r.id AND v.tombstoned_at IS NULL)"#,
            rt.as_str(),
            ns,
            caller_namespaces,
        )
        .fetch_one(pool)
        .await?;
    }
    Ok(Some(NamespaceInfo {
        ns: row.ns,
        kind: NamespaceKind::parse(&row.kind),
        created_at: row.created_at,
        cards: counts[0],
        evals: counts[1],
    }))
}

/// Create an organisation: a namespace of kind `org` and its `orgs` row.
/// Errors with [`StoreError::NamespaceInUse`] when any namespace of that
/// name exists.
pub async fn create_org(pool: &PgPool, ns: &str, actor: Actor) -> Result<(), StoreError> {
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query!("INSERT INTO namespaces (ns, kind) VALUES ($1, 'org')", ns)
        .execute(&mut *tx)
        .await;
    if let Err(e) = inserted {
        if let Some((code, constraint)) = violated_constraint(&e)
            && code == SQLSTATE_UNIQUE
            && constraint == "namespaces_pkey"
        {
            return Err(StoreError::NamespaceInUse(ns.to_owned()));
        }
        return Err(e.into());
    }
    sqlx::query!("INSERT INTO orgs (ns) VALUES ($1)", ns)
        .execute(&mut *tx)
        .await?;
    append(
        &mut tx,
        NewAudit {
            actor,
            ns: Some(ns),
            action: "org.create",
            subject: Some(ns),
            detail: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The members of organisation `ns`, by login. Empty for a user namespace
/// or an unknown one.
pub async fn org_members(pool: &PgPool, ns: &str) -> Result<Vec<Member>, StoreError> {
    let rows = sqlx::query!(
        "SELECT m.user_id, u.login, m.role
         FROM org_members m JOIN users u ON u.user_id = m.user_id
         WHERE m.ns = $1 ORDER BY u.login",
        ns,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Member {
            user_id: r.user_id,
            login: r.login,
            role: Scope::parse(&r.role),
        })
        .collect())
}

/// Add `user_id` to organisation `ns` with `role`, or change their role.
/// Errors with [`StoreError::NotAnOrganisation`] when `ns` has no `orgs`
/// row and [`StoreError::NamespaceUnknown`] when the user does not exist.
pub async fn add_org_member(
    pool: &PgPool,
    ns: &str,
    user_id: Uuid,
    role: Scope,
    actor: Actor,
) -> Result<(), StoreError> {
    let mut tx = pool.begin().await?;
    let login = sqlx::query_scalar!("SELECT login FROM users WHERE user_id = $1", user_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NamespaceUnknown(user_id.to_string()))?;
    let upserted = sqlx::query!(
        "INSERT INTO org_members (ns, user_id, role) VALUES ($1, $2, $3)
         ON CONFLICT (ns, user_id) DO UPDATE SET role = EXCLUDED.role",
        ns,
        user_id,
        role.as_str(),
    )
    .execute(&mut *tx)
    .await;
    if let Err(e) = upserted {
        if let Some((code, constraint)) = violated_constraint(&e)
            && code == SQLSTATE_FOREIGN_KEY
            && constraint == "org_members_ns_fkey"
        {
            return Err(StoreError::NotAnOrganisation(ns.to_owned()));
        }
        return Err(e.into());
    }
    append(
        &mut tx,
        NewAudit {
            actor,
            ns: Some(ns),
            action: "org.member.add",
            subject: Some(&login),
            detail: Some(serde_json::json!({ "user_id": user_id, "role": role.as_str() })),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Remove `user_id` from organisation `ns`. `false` when they were not a
/// member.
pub async fn remove_org_member(
    pool: &PgPool,
    ns: &str,
    user_id: Uuid,
    actor: Actor,
) -> Result<bool, StoreError> {
    let mut tx = pool.begin().await?;
    let removed = sqlx::query!(
        "DELETE FROM org_members m USING users u
         WHERE m.ns = $1 AND m.user_id = $2 AND u.user_id = m.user_id
         RETURNING u.login",
        ns,
        user_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(removed) = removed else {
        return Ok(false);
    };
    append(
        &mut tx,
        NewAudit {
            actor,
            ns: Some(ns),
            action: "org.member.remove",
            subject: Some(&removed.login),
            detail: Some(serde_json::json!({ "user_id": user_id })),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// `user_id`'s role in organisation `ns`, if they are a member.
pub async fn org_role(pool: &PgPool, ns: &str, user_id: Uuid) -> Result<Option<Scope>, StoreError> {
    let role = sqlx::query_scalar!(
        "SELECT role FROM org_members WHERE ns = $1 AND user_id = $2",
        ns,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(role.map(|r| Scope::parse(&r)))
}
