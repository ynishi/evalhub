#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Tokens, organisations and the audit log over HTTP. Needs Docker
//! (Postgres via testcontainers).

mod common;

use axum::http::{Method, StatusCode};
use serde_json::Value;

use common::{CARD, Hub};
use evalhub_store::auth::Scope;

/// A token can mint narrower tokens, list them without their secrets, and
/// revoke them.
#[tokio::test]
async fn tokens_are_issued_once_listed_by_prefix_and_revocable() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Admin).await;

    let (status, issued) = hub
        .call(
            Method::POST,
            "/api/v1/tokens",
            Some(&alice),
            Some(r#"{"scope":"read","namespaces":["alice"]}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let secret = issued["secret"].as_str().unwrap().to_string();
    let token_id = issued["token_id"].as_str().unwrap().to_string();
    assert_eq!(secret.len(), 43);

    let (status, who) = hub
        .call(Method::GET, "/api/v1/whoami", Some(&secret), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{who}");
    assert_eq!(who["user"], "alice");
    assert_eq!(who["scope"], "read");

    let (status, list) = hub
        .call(Method::GET, "/api/v1/tokens", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "the bootstrap token and the new one");
    for item in items {
        assert!(item.get("secret").is_none(), "secrets are shown once");
        assert_eq!(item["prefix"].as_str().unwrap().len(), 8);
    }

    // A token may not mint one that reaches further than itself.
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/tokens",
            Some(&secret),
            Some(r#"{"scope":"write","namespaces":["alice"]}"#),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no scope escalation");
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/tokens",
            Some(&alice),
            Some(r#"{"scope":"read","namespaces":["bob"]}"#),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "not alice's namespace");

    let (status, _) = hub
        .call(
            Method::DELETE,
            &format!("/api/v1/tokens/{token_id}"),
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = hub
        .call(Method::GET, "/api/v1/whoami", Some(&secret), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revocation is immediate");
}

/// An organisation is a namespace with members, and a member's effective
/// scope is the lesser of their role and their token's.
#[tokio::test]
async fn organisation_membership_decides_what_a_token_may_do() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Admin).await;
    let bob = hub.user("bob", Scope::Admin).await;
    hub.ready_attachments(CARD).await;

    let (status, org) = hub
        .call(
            Method::POST,
            "/api/v1/orgs",
            Some(&alice),
            Some(r#"{"ns":"acme"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{org}");
    assert_eq!(org["ns"], "acme");
    assert_eq!(org["kind"], "org");

    // Alice is its admin, so she may issue tokens that cover it. Her
    // bootstrap token covers only her own namespace: acting as the
    // organisation needs a token that names it.
    let (status, issued) = hub
        .call(
            Method::POST,
            "/api/v1/tokens",
            Some(&alice),
            Some(r#"{"scope":"admin","namespaces":["acme"]}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let acme_admin = issued["secret"].as_str().unwrap().to_string();
    let (status, issued) = hub
        .call(
            Method::POST,
            "/api/v1/tokens",
            Some(&alice),
            Some(r#"{"scope":"write","namespaces":["acme"]}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{issued}");
    let acme_token = issued["secret"].as_str().unwrap().to_string();

    // Her personal token does not reach the organisation.
    let (status, _) = hub
        .call(Method::GET, "/api/v1/orgs/acme/members", Some(&alice), None)
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a token acts only in the namespaces it names"
    );
    let (status, created) = hub
        .call(
            Method::POST,
            "/api/v1/cards/acme/x",
            Some(&acme_token),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    // Bob is not a member: his own token gets nothing there.
    let (status, _) = hub
        .call(Method::POST, "/api/v1/cards/acme/y", Some(&bob), Some(CARD))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = hub
        .call(Method::GET, "/api/v1/orgs/acme/members", Some(&bob), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Alice adds him as a reader.
    let (status, member) = hub
        .call(
            Method::POST,
            "/api/v1/orgs/acme/members",
            Some(&acme_admin),
            Some(r#"{"user":"bob","role":"read"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{member}");
    assert_eq!(member["user"], "bob");
    assert_eq!(member["role"], "read");

    let (status, issued) = hub
        .call(
            Method::POST,
            "/api/v1/tokens",
            Some(&bob),
            Some(r#"{"scope":"write","namespaces":["acme"]}"#),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a reader is not an admin of the org: {issued}"
    );

    // A token bob's admin mints for him, covering acme, is still capped by
    // his role there: he may read, not write.
    let bob_acme = {
        let (user_id, _) = evalhub_store::auth::user_by_login(&hub.pool, "bob")
            .await
            .unwrap()
            .unwrap();
        let (secret, hash) = evalhub_server::auth::new_secret();
        evalhub_store::auth::create_token(
            &hub.pool,
            user_id,
            Scope::Write,
            &["acme".to_string()],
            &hash,
        )
        .await
        .unwrap();
        secret
    };
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/acme/z",
            Some(&bob_acme),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "role caps the token");
    let (status, got) = hub
        .call(Method::GET, "/api/v1/cards/acme/x", Some(&bob_acme), None)
        .await;
    assert_eq!(status, StatusCode::OK, "reading is within his role: {got}");

    let (status, members) = hub
        .call(
            Method::GET,
            "/api/v1/orgs/acme/members",
            Some(&acme_admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(members["items"].as_array().unwrap().len(), 2);

    let (status, _) = hub
        .call(
            Method::DELETE,
            "/api/v1/orgs/acme/members/bob",
            Some(&acme_admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = hub
        .call(Method::GET, "/api/v1/cards/acme/x", Some(&bob_acme), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "membership ended");

    // Taking the namespace twice is a conflict.
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/orgs",
            Some(&bob),
            Some(r#"{"ns":"acme"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["errors"][0]["code"], "namespace_in_use");
}

/// The namespace endpoint counts only what the caller may see.
#[tokio::test]
async fn namespace_counts_follow_visibility() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.call(
        Method::POST,
        "/api/v1/cards/alice/x",
        Some(&alice),
        Some(CARD),
    )
    .await;

    let (status, anon) = hub
        .call(Method::GET, "/api/v1/namespaces/alice", None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{anon}");
    assert_eq!(anon["kind"], "user");
    assert_eq!(anon["cards"], 0, "the record is private");

    let (_, mine) = hub
        .call(Method::GET, "/api/v1/namespaces/alice", Some(&alice), None)
        .await;
    assert_eq!(mine["cards"], 1);
    assert_eq!(mine["evals"], 0);

    let (status, _) = hub
        .call(Method::GET, "/api/v1/namespaces/nobody", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The audit log is admin-only and records what happened.
#[tokio::test]
async fn audit_log_is_admin_only_and_records_writes() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Admin).await;
    let bob = hub.user("bob", Scope::Admin).await;
    hub.ready_attachments(CARD).await;
    hub.call(
        Method::POST,
        "/api/v1/cards/alice/x",
        Some(&alice),
        Some(CARD),
    )
    .await;
    hub.call(
        Method::PATCH,
        "/api/v1/cards/alice/x/settings",
        Some(&alice),
        Some(r#"{"visibility":"public"}"#),
    )
    .await;

    let (status, _) = hub
        .call(Method::GET, "/api/v1/audit?ns=alice", Some(&bob), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "another namespace's log");

    let (status, log) = hub
        .call(Method::GET, "/api/v1/audit?ns=alice", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{log}");
    let actions: Vec<&str> = log["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["action"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"record.create"), "{actions:?}");
    assert!(actions.contains(&"settings.visibility"), "{actions:?}");
    assert_eq!(
        log["items"][0]["action"], "settings.visibility",
        "newest first"
    );
    let subject = log["items"][0]["subject"].as_str().unwrap();
    assert_eq!(subject, "card/alice/x");

    // Paging by cursor.
    let (status, page1) = hub
        .call(
            Method::GET,
            "/api/v1/audit?ns=alice&limit=1",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page1["items"].as_array().unwrap().len(), 1);
    let cursor = page1["next_cursor"].as_str().unwrap().to_string();
    let (status, page2) = hub
        .call(
            Method::GET,
            &format!("/api/v1/audit?ns=alice&limit=1&cursor={cursor}"),
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_ne!(page2["items"][0]["action"], Value::Null);
    assert_ne!(page2["items"][0], page1["items"][0]);

    let (status, _) = hub
        .call(
            Method::GET,
            "/api/v1/audit?ns=alice&cursor=forged",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
