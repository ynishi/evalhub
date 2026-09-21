#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Relations over HTTP: adding edges, walking the graph, and the
//! comparison view that lines Cards up against the Eval they cite. Needs
//! Docker (Postgres via testcontainers).

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{CARD, EVAL, Hub};
use evalhub_store::auth::Scope;

/// A Card citing `to` through `core/uses_eval`, with `title` and an
/// optional model id override.
///
/// The `model` and `harness` facets are taken from the Eval fixture, so
/// that a Card left alone fingerprints identically to the Eval on those
/// axes and `same_model` / `same_harness` mean what the test says they
/// mean. The two fixtures otherwise describe the same run at different
/// levels of detail, which would make every axis differ.
fn card_citing(title: &str, to: &str, model_id: Option<&str>) -> String {
    let eval: Value = serde_json::from_str(EVAL).unwrap();
    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["title"] = json!(title);
    v["model"] = eval["model"].clone();
    v["harness"] = eval["harness"].clone();
    v["relations"] = json!([{ "type": "core/uses_eval", "to": to }]);
    if let Some(id) = model_id {
        v["model"]["id"] = json!(id);
    }
    v.to_string()
}

fn eval_titled(title: &str) -> String {
    let mut v: Value = serde_json::from_str(EVAL).unwrap();
    v["title"] = json!(title);
    v.to_string()
}

/// Two Cards measured on one Eval line up, and the hub says which axes
/// agree without ranking them.
#[tokio::test]
async fn comparison_view_lines_cards_up() {
    let hub = Hub::start_with_storage().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.ready_attachments(EVAL).await;

    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/evals/alice/single2-k4",
            Some(&alice),
            Some(&eval_titled("the material")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    // Same model as the Eval, and a second Card on another model.
    for (name, title, model) in [
        ("first", "same model", None),
        ("second", "other model", Some("other-model")),
    ] {
        let (status, created) = hub
            .call(
                Method::POST,
                &format!("/api/v1/cards/alice/{name}"),
                Some(&alice),
                Some(&card_citing(title, "alice/single2-k4@1", model)),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let badges: Vec<String> = serde_json::from_value(created["badges"].clone()).unwrap();
        assert!(badges.contains(&"refs_resolved".to_string()), "{badges:?}");
    }

    let (status, view) = hub
        .call(
            Method::GET,
            "/api/v1/evals/alice/single2-k4/cards",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let items = view["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let by_name: std::collections::HashMap<&str, &Value> = items
        .iter()
        .map(|i| (i["name"].as_str().unwrap(), i))
        .collect();
    assert_eq!(by_name["first"]["same_model"], true);
    assert_eq!(by_name["first"]["same_harness"], true);
    assert_eq!(by_name["second"]["same_model"], false);
    assert_eq!(
        by_name["second"]["same_harness"], true,
        "only the model differs"
    );
    assert_eq!(
        by_name["first"]["fingerprints"]["model"]
            .as_str()
            .unwrap()
            .len(),
        64
    );

    // Grouping by the model fingerprint separates them.
    let (status, grouped) = hub
        .call(
            Method::GET,
            "/api/v1/evals/alice/single2-k4/cards?group_by=fingerprint.model",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{grouped}");
    let groups = grouped["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert!(grouped["items"].is_null());
    for g in groups {
        assert_eq!(g["items"].as_array().unwrap().len(), 1);
        assert_eq!(g["key"].as_str().unwrap().len(), 64);
    }

    // Grouping by the harness fingerprint, which both share, gives one.
    let (_, one) = hub
        .call(
            Method::GET,
            "/api/v1/evals/alice/single2-k4/cards?group_by=fingerprint.harness",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(one["groups"].as_array().unwrap().len(), 1);

    // A facet that does not exist is refused rather than collapsing.
    for bad in ["model", "fingerprint.nope", "fingerprint."] {
        let (status, _) = hub
            .call(
                Method::GET,
                &format!("/api/v1/evals/alice/single2-k4/cards?group_by={bad}"),
                Some(&alice),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
}

/// Edges added after ingest, and the graph read from both ends.
#[tokio::test]
async fn edges_are_added_and_walked() {
    let hub = Hub::start_with_storage().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.ready_attachments(EVAL).await;

    hub.call(
        Method::POST,
        "/api/v1/evals/alice/material",
        Some(&alice),
        Some(&eval_titled("material")),
    )
    .await;
    let (status, card) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/measured",
            Some(&alice),
            Some(&card_citing("measured", "alice/material@1", None)),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{card}");

    // The ingest edge is there, pointing at the Eval.
    let (status, graph) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/measured/relations",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{graph}");
    assert_eq!(graph["edges"].as_array().unwrap().len(), 1);
    assert_eq!(graph["edges"][0]["type"], "core/uses_eval");
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);

    // And from the Eval's side.
    let (status, incoming) = hub
        .call(
            Method::GET,
            "/api/v1/evals/alice/material/relations?direction=in",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{incoming}");
    assert_eq!(incoming["edges"].as_array().unwrap().len(), 1);
    let names: Vec<&str> = incoming["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["name"].as_str())
        .collect();
    assert!(names.contains(&"measured"), "{names:?}");

    // A second Card citing the first: depth 2 reaches the Eval.
    let (status, added) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/rerun@1/relations",
            Some(&alice),
            Some(r#"{"type":"core/rerun_of","to":"alice/measured@1"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{added}: no such record yet");

    hub.call(
        Method::POST,
        "/api/v1/cards/alice/rerun",
        Some(&alice),
        Some(&card_citing("a rerun", "alice/material@1", None)),
    )
    .await;
    let (status, added) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/rerun@1/relations",
            Some(&alice),
            Some(r#"{"type":"core/rerun_of","to":"alice/measured@1","attrs":{"why":"flaky"}}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{added}");
    assert_eq!(added["type"], "core/rerun_of");
    assert_eq!(added["to"]["name"], "measured");
    assert_eq!(added["attrs"]["why"], "flaky");

    let (_, deep) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/rerun/relations?depth=2",
            Some(&alice),
            None,
        )
        .await;
    let names: Vec<&str> = deep["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["name"].as_str())
        .collect();
    assert!(names.contains(&"material"), "two hops away: {names:?}");

    // An unparseable target is a validation error, not a 500.
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/rerun@1/relations",
            Some(&alice),
            Some(r#"{"type":"core/rerun_of","to":"just a string"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["path"], "/to");

    // An external target is kept textually.
    let (status, ext) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/rerun@1/relations",
            Some(&alice),
            Some(r#"{"type":"core/baseline_of","to":"external:https://example.org/run/7"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{ext}");
    assert_eq!(ext["to"]["external"], "external:https://example.org/run/7");
}

/// A node the caller may not see is reduced to its commitment.
#[tokio::test]
async fn private_endpoints_are_reduced_to_a_commitment() {
    let hub = Hub::start_with_storage().await;
    let alice = hub.user("alice", Scope::Write).await;
    let bob = hub.user("bob", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.ready_attachments(EVAL).await;

    // A private Eval, and a public Card that cites it.
    hub.call(
        Method::POST,
        "/api/v1/evals/alice/secret",
        Some(&alice),
        Some(&eval_titled("not for you")),
    )
    .await;
    hub.call(
        Method::POST,
        "/api/v1/cards/alice/public-card",
        Some(&alice),
        Some(&card_citing("public", "alice/secret@1", None)),
    )
    .await;
    hub.call(
        Method::PATCH,
        "/api/v1/cards/alice/public-card/settings",
        Some(&alice),
        Some(r#"{"visibility":"public"}"#),
    )
    .await;

    let (status, graph) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/public-card/relations",
            Some(&bob),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{graph}");
    let private: Vec<&Value> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["private"] == json!(true))
        .collect();
    assert_eq!(private.len(), 1, "the Eval is hidden: {graph}");
    let node = private[0];
    assert_eq!(node["content_hash"].as_str().unwrap().len(), 64);
    assert!(node["name"].is_null(), "no name leaks: {node}");
    assert!(node["ns"].is_null());
    assert!(node["seq"].is_null());

    // The owner sees the whole thing.
    let (_, owned) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/public-card/relations",
            Some(&alice),
            None,
        )
        .await;
    let names: Vec<&str> = owned["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["name"].as_str())
        .collect();
    assert!(names.contains(&"secret"), "{names:?}");

    // Writing an edge needs write on the namespace.
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/public-card@1/relations",
            Some(&bob),
            Some(r#"{"type":"core/rerun_of","to":"alice/public-card@1"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
