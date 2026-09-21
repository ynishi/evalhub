#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The registry over HTTP: what it accepts, what it refuses, and what
//! registering something changes about queries and badges. Needs Docker
//! (Postgres via testcontainers).

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{CARD, Hub};
use evalhub_store::auth::Scope;

/// Registering a metric is what makes it sortable, because the entry is
/// where `lower_is_better` lives.
#[tokio::test]
async fn a_registered_metric_becomes_sortable() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;

    // Two Cards reporting a metric the hub has never heard of.
    for (name, value) in [("low", 0.2), ("high", 0.9)] {
        let mut v: Value = serde_json::from_str(CARD).unwrap();
        v["title"] = json!(name);
        v["results"][0]["metric"] = json!("alice/homegrown");
        v["results"][0]["value"] = json!(value);
        let (status, body) = hub
            .call(
                Method::POST,
                &format!("/api/v1/cards/alice/{name}"),
                Some(&alice),
                Some(&v.to_string()),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }

    let sort = json!({"sort": [{"path": "results[alice/homegrown].value", "dir": "desc"}]});
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/cards/query",
            Some(&alice),
            Some(&sort.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "not_indexed");

    let (status, entry) = hub
        .call(
            Method::PUT,
            "/api/v1/registry/metrics/alice/homegrown@1",
            Some(&alice),
            Some(r#"{"id":"alice/homegrown","lower_is_better":false,"description":"mine"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");
    assert_eq!(entry["state"], "applied");

    let (status, page) = hub
        .call(
            Method::POST,
            "/api/v1/cards/query",
            Some(&alice),
            Some(&sort.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let names: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["high", "low"]);
}

/// Entries are immutable and `core/` is the hub's own.
#[tokio::test]
async fn entries_are_immutable_and_core_is_read_only() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    let bob = hub.user("bob", Scope::Write).await;

    let body = r#"{"id":"alice/thing","lower_is_better":true,"description":"x"}"#;
    let (status, _) = hub
        .call(
            Method::PUT,
            "/api/v1/registry/metrics/alice/thing@1",
            Some(&alice),
            Some(body),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, err) = hub
        .call(
            Method::PUT,
            "/api/v1/registry/metrics/alice/thing@1",
            Some(&alice),
            Some(body),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["errors"][0]["code"], "registry_entry_exists");

    // A correction is a new version.
    let (status, _) = hub
        .call(
            Method::PUT,
            "/api/v1/registry/metrics/alice/thing@2",
            Some(&alice),
            Some(body),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    // `core/` is seeded by the migration and refuses writes.
    let (status, _) = hub
        .call(
            Method::PUT,
            "/api/v1/registry/metrics/core/pass_rate@2",
            Some(&alice),
            Some(body),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Another namespace is not bob's to write.
    let (status, _) = hub
        .call(
            Method::PUT,
            "/api/v1/registry/metrics/alice/bobs@1",
            Some(&bob),
            Some(body),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Reading is open, and the seeded vocabulary is there.
    let (status, page) = hub
        .call(Method::GET, "/api/v1/registry/metrics?ns=core", None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let ids: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"pass_rate"), "{ids:?}");

    let (status, one) = hub
        .call(
            Method::GET,
            "/api/v1/registry/metrics/alice/thing@1",
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["version"], "1");

    let (status, _) = hub
        .call(
            Method::GET,
            "/api/v1/registry/metrics/alice/nothing@1",
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// An extension schema is accepted at once and indexed in the background;
/// its paths gain ranges only when the index is valid.
#[tokio::test]
async fn an_ext_schema_is_applied_in_the_background() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;

    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["ext"]["alice/qwen-loop"]["rung"] = json!(4);
    let (status, body) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(&v.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let range = json!({"where": {"path": "ext.alice/qwen-loop.rung", "op": "gt", "value": 3}});
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/cards/query",
            Some(&alice),
            Some(&range.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "not_indexed");

    // A schema with nothing indexable is refused rather than left to
    // sit in `applying` forever.
    let (status, err) = hub
        .call(
            Method::PUT,
            "/api/v1/registry/ext_schemas/alice/qwen-loop@1",
            Some(&alice),
            Some(r#"{"schema":{"type":"object"}}"#),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");

    let (status, entry) = hub
        .call(
            Method::PUT,
            "/api/v1/registry/ext_schemas/alice/qwen-loop@1",
            Some(&alice),
            Some(
                r#"{"schema":{"type":"object","properties":{"rung":{"type":"number"}}},
                    "fingerprint":false}"#,
            ),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{entry}");
    assert_eq!(entry["state"], "applying");

    // Still unindexed until the job has run.
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/query",
            Some(&alice),
            Some(&range.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    hub.apply_ext_schemas().await;

    let (status, entry) = hub
        .call(
            Method::GET,
            "/api/v1/registry/ext_schemas/alice/qwen-loop@1",
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{entry}");
    assert_eq!(entry["state"], "applied");

    let (status, page) = hub
        .call(
            Method::POST,
            "/api/v1/cards/query",
            Some(&alice),
            Some(&range.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "{page}");
    assert_eq!(page["items"][0]["name"], "x");
}

/// Registering the harness a stored record cites earns it the badge, once
/// the sweep has run.
#[tokio::test]
async fn registering_a_harness_earns_the_badge() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;

    // The fixture's harness name has no namespace, so give it one: an
    // unqualified name can never match a registry address.
    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["harness"]["name"] = json!("alice/my-harness");
    let (status, created) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(&v.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let badges: Vec<String> = serde_json::from_value(created["badges"].clone()).unwrap();
    assert!(
        !badges.contains(&"harness_registered".to_string()),
        "{badges:?}"
    );

    let version = v["harness"]["version"].as_str().unwrap().to_string();
    let (status, entry) = hub
        .call(
            Method::PUT,
            &format!("/api/v1/registry/harnesses/alice/my-harness@{version}"),
            Some(&alice),
            Some(r#"{"name":"alice/my-harness","homepage":"https://example.invalid"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{entry}");

    evalhub_server::jobs::sweep_badges_once(&hub.pool)
        .await
        .expect("badge sweep");

    let (status, read) = hub
        .call(Method::GET, "/api/v1/cards/alice/x", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    let badges: Vec<String> = serde_json::from_value(read["badges"].clone()).unwrap();
    assert!(
        badges.contains(&"harness_registered".to_string()),
        "the sweep awarded it: {badges:?}"
    );
    // A fact about publication order, carried through untouched.
    assert!(
        badges.contains(&"env_pinned".to_string()),
        "record-derived badges survive: {badges:?}"
    );
}
