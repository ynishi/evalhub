#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `POST /{cards|evals}/query` end to end: the grammar, the type check's
//! refusals, paging, visibility and expansion. Needs Docker (Postgres via
//! testcontainers).

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{CARD, EVAL, Hub, with_title};
use evalhub_store::auth::Scope;

/// Post `CARD` with the given patches applied, under `name`.
async fn post_card(hub: &Hub, token: &str, name: &str, patches: &[(&str, Value)]) -> Value {
    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["title"] = json!(name);
    for (pointer, value) in patches {
        // `/model/id` style pointers, creating nothing: every pointer the
        // tests use already exists in the fixture.
        *v.pointer_mut(pointer)
            .unwrap_or_else(|| panic!("fixture has no {pointer}")) = value.clone();
    }
    let (status, body) = hub
        .call(
            Method::POST,
            &format!("/api/v1/cards/alice/{name}"),
            Some(token),
            Some(&v.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body
}

async fn query(hub: &Hub, token: Option<&str>, body: Value) -> (StatusCode, Value) {
    hub.call(
        Method::POST,
        "/api/v1/cards/query",
        token,
        Some(&body.to_string()),
    )
    .await
}

/// The example from the query crate's documentation has to work, or the
/// documentation is a lie.
#[tokio::test]
async fn the_documented_query_runs() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.ready_attachments(EVAL).await;

    // The Eval the fixture Card cites, so the relation resolves.
    for n in 1..=3 {
        hub.call(
            Method::POST,
            "/api/v1/evals/alice/single2-k4",
            Some(&alice),
            Some(&with_title(EVAL, &format!("run {n}"))),
        )
        .await;
    }
    post_card(&hub, &alice, "match", &[]).await;
    post_card(
        &hub,
        &alice,
        "other-model",
        &[("/model/id", json!("some-other-model"))],
    )
    .await;

    let (status, page) = query(
        &hub,
        Some(&alice),
        json!({
            "where": {"and": [
                {"path": "model.id", "op": "eq", "value": "qwen3.6-32b"},
                {"path": "generation.temperature", "op": "lte", "value": 0.2},
                {"path": "results", "op": "any", "match": {
                    "metric": "core/pass_rate",
                    "value": {"op": "gte", "value": 0.5}
                }},
                {"path": "relations", "op": "any", "match": {"type": "core/uses_eval"}},
                {"path": "ext.alice/qwen-loop.rung", "op": "eq", "value": 4}
            ]},
            "limit": 50,
            "expand": ["badges", "fingerprints"],
            "version": "latest"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "only the matching Card: {page}");
    assert_eq!(items[0]["name"], "match");
    assert_eq!(
        items[0]["fingerprints"].as_object().unwrap().len(),
        7,
        "expand=fingerprints"
    );
    assert!(items[0]["record"]["title"].is_string(), "the body is there");
}

/// A typo is an error, never an empty page: an empty page looks like data.
#[tokio::test]
async fn a_bad_path_or_operator_is_refused_not_ignored() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    post_card(&hub, &alice, "x", &[]).await;

    let (status, err) = query(
        &hub,
        Some(&alice),
        json!({"where": {"path": "model.idd", "op": "eq", "value": "x"}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "unknown_path");

    let (status, err) = query(
        &hub,
        Some(&alice),
        json!({"where": {"path": "model.id", "op": "gte", "value": 3}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "type_mismatch");

    // An unregistered extension key answers equality from the GIN index
    // and refuses a range, which would be a sequential scan.
    let (status, page) = query(
        &hub,
        Some(&alice),
        json!({"where": {"path": "ext.alice/qwen-loop.rung", "op": "eq", "value": 4}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);

    let (status, err) = query(
        &hub,
        Some(&alice),
        json!({"where": {"path": "ext.alice/qwen-loop.rung", "op": "gt", "value": 3}}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "not_indexed");
    assert!(
        err["errors"][0]["hint"]
            .as_str()
            .unwrap()
            .contains("registry"),
        "the hint names the way out: {err}"
    );
}

/// Sorting by a metric needs the registry, which knows which way is up.
#[tokio::test]
async fn sorting_by_a_metric_needs_the_registry() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    post_card(&hub, &alice, "a", &[("/results/0/value", json!(0.9))]).await;
    post_card(&hub, &alice, "b", &[("/results/0/value", json!(0.1))]).await;

    // `core/pass_rate` is seeded by migration 0002.
    let (status, page) = query(
        &hub,
        Some(&alice),
        json!({"sort": [{"path": "results[core/pass_rate].value", "dir": "desc"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let names: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["a", "b"], "highest pass rate first");

    let (status, err) = query(
        &hub,
        Some(&alice),
        json!({"sort": [{"path": "results[alice/homegrown].value", "dir": "desc"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "not_indexed");
}

/// Paging is by signed cursor, and a forged one is refused.
#[tokio::test]
async fn paging_round_trips_and_verifies_the_cursor() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    for name in ["a", "b", "c"] {
        post_card(&hub, &alice, name, &[]).await;
    }

    let (status, page1) = query(&hub, Some(&alice), json!({"limit": 2})).await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    let cursor = page1["next_cursor"].as_str().unwrap().to_string();

    let (status, page2) = query(&hub, Some(&alice), json!({"limit": 2, "cursor": cursor})).await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert!(page2["next_cursor"].is_null());

    let seen: Vec<&str> = page1["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(page2["items"].as_array().unwrap())
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    let mut sorted = seen.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 3, "every record once: {seen:?}");

    let (status, _) = query(&hub, Some(&alice), json!({"cursor": "forged"})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

/// A query never reveals a record the caller could not read directly.
#[tokio::test]
async fn visibility_holds_through_the_query() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    let bob = hub.user("bob", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    post_card(&hub, &alice, "secret", &[]).await;

    let (status, page) = query(&hub, None, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 0, "private");

    let (_, page) = query(&hub, Some(&bob), json!({})).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 0, "not bob's");

    let (_, page) = query(&hub, Some(&alice), json!({})).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);

    hub.call(
        Method::PATCH,
        "/api/v1/cards/alice/secret/settings",
        Some(&alice),
        Some(r#"{"visibility":"public"}"#),
    )
    .await;
    let (_, page) = query(&hub, None, json!({})).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "now public");
}

/// `expand=relations` returns the version's own edges.
#[tokio::test]
async fn expand_relations_returns_the_edges() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.ready_attachments(EVAL).await;
    for n in 1..=3 {
        hub.call(
            Method::POST,
            "/api/v1/evals/alice/single2-k4",
            Some(&alice),
            Some(&with_title(EVAL, &format!("run {n}"))),
        )
        .await;
    }
    post_card(&hub, &alice, "linked", &[]).await;

    let (status, page) = query(
        &hub,
        Some(&alice),
        json!({"expand": ["relations"], "limit": 10}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let relations = page["items"][0]["relations"].as_array().unwrap();
    assert_eq!(relations.len(), 1, "{page}");
    assert_eq!(relations[0]["type"], "core/uses_eval");
    assert_eq!(relations[0]["to"]["name"], "single2-k4");

    // The same expansion on a record read.
    let (status, one) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/linked?expand=relations",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{one}");
    assert_eq!(one["relations"][0]["type"], "core/uses_eval");
}

/// Evals answer their own queries, with their own facets.
#[tokio::test]
async fn evals_are_queryable_too() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(EVAL).await;
    hub.call(
        Method::POST,
        "/api/v1/evals/alice/single2-k4",
        Some(&alice),
        Some(EVAL),
    )
    .await;

    let (status, page) = hub
        .call(
            Method::POST,
            "/api/v1/evals/query",
            Some(&alice),
            Some(
                &json!({"where": {"path": "task.id", "op": "eq", "value": "single2"}}).to_string(),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);

    // `grading` is a Card facet; an Eval has none, so the path is unknown
    // rather than empty.
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/evals/query",
            Some(&alice),
            Some(
                &json!({"where": {"path": "grading.aggregation", "op": "eq", "value": "mean"}})
                    .to_string(),
            ),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "unknown_path");
}
