#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The privacy boundary across a relation, one test per path: a public
//! Card that cites a private Eval never gives an outsider the Eval's name,
//! and no write tells an outsider whether a private version exists. Uses
//! only the HTTP API. Needs Docker (Postgres and MinIO via testcontainers).

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{CARD, EVAL, Hub};
use evalhub_store::auth::Scope;

/// A Card whose only relation is `core/uses_eval` to `to`.
fn card_citing(title: &str, to: &str) -> String {
    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["title"] = json!(title);
    v["relations"] = json!([{ "type": "core/uses_eval", "to": to }]);
    v.to_string()
}

/// The situation every test starts from: alice's private Eval
/// `alice/secret`, alice's public Card `alice/public-card` citing it, and
/// bob, who holds no access to alice's namespace.
struct Scene {
    hub: Hub,
    alice: String,
    bob: String,
    /// `GET /evals/alice/secret` as alice sees it.
    eval: Value,
}

async fn scene() -> Scene {
    let hub = Hub::start_with_storage().await;
    let alice = hub.user("alice", Scope::Write).await;
    let bob = hub.user("bob", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.ready_attachments(EVAL).await;

    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/evals/alice/secret",
            Some(&alice),
            Some(EVAL),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/public-card",
            Some(&alice),
            Some(&card_citing("public", "alice/secret@1")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = hub
        .call(
            Method::PATCH,
            "/api/v1/cards/alice/public-card/settings",
            Some(&alice),
            Some(r#"{"visibility":"public"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, eval) = hub
        .call(
            Method::GET,
            "/api/v1/evals/alice/secret",
            Some(&alice),
            None,
        )
        .await;
    Scene {
        hub,
        alice,
        bob,
        eval,
    }
}

/// `GET` of the public Card: the element citing the private Eval is
/// removed for bob and for an anonymous reader, and reported as the
/// commitment; alice gets the stored body.
#[tokio::test]
async fn get_withholds_the_private_target_from_the_body() {
    let s = scene().await;
    for reader in [Some(s.bob.as_str()), None] {
        let (status, env) = s
            .hub
            .call(Method::GET, "/api/v1/cards/alice/public-card", reader, None)
            .await;
        assert_eq!(status, StatusCode::OK, "{env}");
        assert!(!env.to_string().contains("alice/secret"), "{env}");
        assert_eq!(env["record"]["relations"], json!([]), "{env}");
        let withheld = &env["withheld"]["relations"];
        assert_eq!(withheld.as_array().map(Vec::len), Some(1), "{env}");
        assert_eq!(withheld[0]["type"], "core/uses_eval");
        assert_eq!(withheld[0]["content_hash"], s.eval["content_hash"]);
    }

    let (_, env) = s
        .hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/public-card",
            Some(&s.alice),
            None,
        )
        .await;
    assert!(env.get("withheld").is_none(), "{env}");
    assert_eq!(env["record"]["relations"][0]["to"], "alice/secret@1");
}

/// `POST /cards/query`: the hit's body is withheld the same way.
#[tokio::test]
async fn query_withholds_the_private_target_from_the_body() {
    let s = scene().await;
    let (status, page) = s
        .hub
        .call(
            Method::POST,
            "/api/v1/cards/query",
            Some(&s.bob),
            Some(r#"{"limit": 10}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().map(Vec::len), Some(1), "{page}");
    assert!(!page.to_string().contains("alice/secret"), "{page}");
    let hit = &page["items"][0];
    assert_eq!(hit["record"]["relations"], json!([]), "{hit}");
    assert_eq!(hit["withheld"]["relations"][0]["type"], "core/uses_eval");
}

/// `POST /cards/query` filtering on `relations[].to`: a filter over an
/// edge bob may not see matches nothing, exactly or by prefix, so the
/// query cannot say that `alice/secret@1` exists or who cites it. alice's
/// same filter finds the Card.
#[tokio::test]
async fn query_filter_on_a_hidden_target_matches_nothing() {
    let s = scene().await;
    for to in [
        json!("alice/secret@1"),
        json!({"op": "prefix", "value": "alice/"}),
    ] {
        let body =
            json!({"where": {"path": "relations", "op": "any", "match": {"to": to}}}).to_string();
        let (status, page) = s
            .hub
            .call(
                Method::POST,
                "/api/v1/cards/query",
                Some(&s.bob),
                Some(&body),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["items"], json!([]), "{page}");

        let (_, page) = s
            .hub
            .call(
                Method::POST,
                "/api/v1/cards/query",
                Some(&s.alice),
                Some(&body),
            )
            .await;
        assert_eq!(page["items"].as_array().map(Vec::len), Some(1), "{page}");
    }
}

/// `POST /cards/bob/…` citing alice's private version gives bob the same
/// badges as citing a version that does not exist.
#[tokio::test]
async fn posting_a_reference_to_a_private_version_looks_like_a_missing_one() {
    let s = scene().await;
    let (status, probe) = s
        .hub
        .call(
            Method::POST,
            "/api/v1/cards/bob/probe",
            Some(&s.bob),
            Some(&card_citing("probe", "alice/secret@1")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{probe}");
    let (status, control) = s
        .hub
        .call(
            Method::POST,
            "/api/v1/cards/bob/control",
            Some(&s.bob),
            Some(&card_citing("control", "alice/nothing@1")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{control}");
    assert_eq!(probe["badges"], control["badges"], "{probe} / {control}");
    assert!(
        !probe["badges"]
            .as_array()
            .unwrap()
            .contains(&json!("refs_resolved")),
        "{probe}"
    );
}

/// `POST …/relations` from bob's Card to alice's private version answers
/// `unresolved`, as for a missing version, and reads back that way; once
/// alice makes the Eval public the edge resolves.
#[tokio::test]
async fn adding_an_edge_to_a_private_version_looks_like_a_missing_one() {
    let s = scene().await;
    let (status, _) = s
        .hub
        .call(
            Method::POST,
            "/api/v1/cards/bob/probe",
            Some(&s.bob),
            Some(&card_citing("probe", "external:https://example.org/x")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    for to in ["alice/secret@1", "alice/nothing@1"] {
        let (status, edge) = s
            .hub
            .call(
                Method::POST,
                "/api/v1/cards/bob/probe@1/relations",
                Some(&s.bob),
                Some(&json!({"type": "core/uses_eval", "to": to}).to_string()),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{edge}");
        assert_eq!(edge["to"], json!({ "unresolved": to }), "{edge}");
    }
    let (_, env) = s
        .hub
        .call(
            Method::GET,
            "/api/v1/cards/bob/probe?expand=relations",
            Some(&s.bob),
            None,
        )
        .await;
    let secret_edge = |env: &Value| {
        env["relations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["to"]["unresolved"] == "alice/secret@1" || r["to"]["name"] == "secret")
            .cloned()
            .unwrap_or_else(|| panic!("no edge to alice/secret: {env}"))
    };
    assert_eq!(
        secret_edge(&env)["to"],
        json!({"unresolved": "alice/secret@1"}),
        "{env}"
    );

    let (status, _) = s
        .hub
        .call(
            Method::PATCH,
            "/api/v1/evals/alice/secret/settings",
            Some(&s.alice),
            Some(r#"{"visibility":"public"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, env) = s
        .hub
        .call(
            Method::GET,
            "/api/v1/cards/bob/probe?expand=relations",
            Some(&s.bob),
            None,
        )
        .await;
    assert_eq!(secret_edge(&env)["to"]["name"], "secret", "{env}");
}
