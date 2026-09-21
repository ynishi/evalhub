#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The UI session: a token exchanged for a private cookie, the cookie
//! accepted wherever a header would be, and both ends of its life. Needs
//! Docker (Postgres via testcontainers).

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::Value;
use tower::ServiceExt;

use common::{CARD, Hub};
use evalhub_store::auth::Scope;

/// A response, reduced to what these tests ask about.
struct Answer {
    status: StatusCode,
    body: Value,
    set_cookie: Vec<String>,
}

impl Answer {
    /// The value the `Set-Cookie` header assigns to the session, if it
    /// assigns one. An empty string means the header clears it.
    fn session(&self) -> Option<String> {
        let header = self
            .set_cookie
            .iter()
            .find(|c| c.starts_with("evalhub_session="))?;
        let value = header
            .trim_start_matches("evalhub_session=")
            .split(';')
            .next()
            .unwrap_or_default();
        Some(value.to_string())
    }
}

/// Call the hub with any combination of a bearer header and a cookie.
async fn call(
    hub: &Hub,
    method: Method,
    path: &str,
    token: Option<&str>,
    cookie: Option<&str>,
    body: Option<&str>,
) -> Answer {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, format!("evalhub_session={c}"));
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = hub.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let set_cookie = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answer {
        status,
        body,
        set_cookie,
    }
}

/// Open a session for `token` and return the cookie's encrypted value.
async fn open_session(hub: &Hub, token: &str) -> String {
    let body = format!(r#"{{"token":"{token}"}}"#);
    let answer = call(
        hub,
        Method::POST,
        "/api/v1/session",
        None,
        None,
        Some(&body),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    answer.session().expect("a session cookie")
}

#[tokio::test]
async fn a_token_opens_a_session_that_authorises_on_its_own() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;

    let body = format!(r#"{{"token":"{alice}"}}"#);
    let answer = call(
        &hub,
        Method::POST,
        "/api/v1/session",
        None,
        None,
        Some(&body),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(answer.body["user"], "alice");
    assert_eq!(answer.body["scope"], "write");
    assert_eq!(answer.body["namespaces"], serde_json::json!(["alice"]));

    let raw = answer
        .set_cookie
        .iter()
        .find(|c| c.starts_with("evalhub_session="))
        .expect("session cookie");
    assert!(raw.contains("HttpOnly"), "{raw}");
    assert!(raw.contains("SameSite=Strict"), "{raw}");
    assert!(raw.contains("Path=/"), "{raw}");
    // The default bind is 127.0.0.1, so the cookie is usable over plain
    // http; a hub bound to a routable address gets `Secure` instead.
    assert!(!raw.contains("Secure"), "{raw}");

    let cookie = answer.session().unwrap();
    // The cookie is not the token that opened the session, and it is not
    // the token it carries either: it is encrypted.
    assert_ne!(cookie, alice);

    let who = call(
        &hub,
        Method::GET,
        "/api/v1/whoami",
        None,
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(who.status, StatusCode::OK);
    assert_eq!(who.body["user"], "alice");

    // A write, which needs a real token behind it.
    let created = call(
        &hub,
        Method::POST,
        "/api/v1/cards/alice/x",
        None,
        Some(&cookie),
        Some(CARD),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);

    // The token that opened the session still works: the session minted
    // its own, and did not move this one.
    let (status, _) = hub
        .call(Method::GET, "/api/v1/whoami", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_header_wins_over_a_cookie() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    let bob = hub.user("bob", Scope::Write).await;
    let alice_session = open_session(&hub, &alice).await;

    let who = call(
        &hub,
        Method::GET,
        "/api/v1/whoami",
        Some(&bob),
        Some(&alice_session),
        None,
    )
    .await;
    assert_eq!(who.status, StatusCode::OK);
    assert_eq!(
        who.body["user"], "bob",
        "the deliberate header, not the ambient cookie"
    );
}

#[tokio::test]
async fn ending_a_session_revokes_its_token_and_clears_the_cookie() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    let cookie = open_session(&hub, &alice).await;

    let bye = call(
        &hub,
        Method::DELETE,
        "/api/v1/session",
        None,
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(bye.status, StatusCode::NO_CONTENT);
    assert_eq!(bye.session().as_deref(), Some(""), "the cookie is cleared");

    // The browser would have dropped it, but a client that kept it gets
    // nowhere: the token behind it is revoked.
    let who = call(
        &hub,
        Method::GET,
        "/api/v1/whoami",
        None,
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(who.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        who.session().as_deref(),
        Some(""),
        "a refused cookie clears itself"
    );

    // The token that opened it is untouched.
    let (status, _) = hub
        .call(Method::GET, "/api/v1/whoami", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    // Ending a session that is not open is still the caller's wish.
    let again = call(&hub, Method::DELETE, "/api/v1/session", None, None, None).await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_bad_credential_opens_nothing() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;

    let answer = call(
        &hub,
        Method::POST,
        "/api/v1/session",
        None,
        None,
        Some(r#"{"token":"not-a-token"}"#),
    )
    .await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert!(answer.session().is_none(), "no cookie is set");

    // A cookie this key cannot decrypt reads as no cookie at all: it was
    // signed by another process, so the caller is anonymous rather than
    // refused.
    let forged = call(
        &hub,
        Method::GET,
        "/api/v1/whoami",
        None,
        Some("bm90LWEtcmVhbC1jb29raWU"),
        None,
    )
    .await;
    assert_eq!(forged.status, StatusCode::OK);
    assert!(forged.body["user"].is_null(), "{}", forged.body);

    // Writing needs a credential, and a cookie that does not decrypt is
    // not one.
    let refused = call(
        &hub,
        Method::POST,
        "/api/v1/cards/alice/x",
        None,
        Some("bm90LWEtcmVhbC1jb29raWU"),
        Some(CARD),
    )
    .await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);

    // A session whose underlying token is revoked by hand, as the token
    // settings page would: the cookie decrypts, the token does not pass.
    let cookie = open_session(&hub, &alice).await;
    let tokens = hub
        .call(Method::GET, "/api/v1/tokens", Some(&alice), None)
        .await
        .1;
    // The bootstrap token first, then the one the session minted.
    let session_token_id = tokens["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["token_id"].as_str().unwrap())
        .next_back()
        .unwrap()
        .to_string();
    let (status, _) = hub
        .call(
            Method::DELETE,
            &format!("/api/v1/tokens/{session_token_id}"),
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let who = call(
        &hub,
        Method::GET,
        "/api/v1/whoami",
        None,
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(who.status, StatusCode::UNAUTHORIZED);
    assert_eq!(who.session().as_deref(), Some(""));
}

#[tokio::test]
async fn the_session_inherits_scope_and_namespaces_but_not_more() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Read).await;
    hub.ready_attachments(CARD).await;
    let cookie = open_session(&hub, &alice).await;

    let who = call(
        &hub,
        Method::GET,
        "/api/v1/whoami",
        None,
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(who.body["scope"], "read");

    let refused = call(
        &hub,
        Method::POST,
        "/api/v1/cards/alice/x",
        None,
        Some(&cookie),
        Some(CARD),
    )
    .await;
    assert_eq!(
        refused.status,
        StatusCode::FORBIDDEN,
        "a read session cannot write"
    );
}
