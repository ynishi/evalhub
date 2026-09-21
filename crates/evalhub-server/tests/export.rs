#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `GET …/export?format=` — the Hugging Face `model-index` projection.
//! Needs Docker (Postgres via testcontainers).

mod common;

use axum::http::{Method, StatusCode};

use common::{CARD, EVAL, Hub};
use evalhub_store::auth::Scope;

#[tokio::test]
async fn a_card_exports_as_model_index_yaml() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    let (status, created) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/single2",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");

    let (status, yaml) = hub
        .call_text(
            Method::GET,
            "/api/v1/cards/alice/single2/export?format=hf-model-index",
            Some(&alice),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{yaml}");

    // The document a model card embeds: the model, the task, the score.
    let parsed: serde_json::Value = serde_norway::from_str(&yaml).expect("valid yaml");
    let result = &parsed["model-index"][0]["results"][0];
    assert_eq!(parsed["model-index"][0]["name"], "qwen3.6-32b");
    assert_eq!(result["task"]["type"], "single2");
    assert_eq!(result["dataset"]["split"], "test");
    assert_eq!(result["metrics"][0]["type"], "core/pass_rate");
    assert_eq!(result["metrics"][0]["value"], 0.62);
    // The link back to the version the numbers came from.
    assert_eq!(result["source"]["name"], "evalhub alice/single2@1");

    // A version that does not exist is not an empty document.
    let (status, _) = hub
        .call_text(
            Method::GET,
            "/api/v1/cards/alice/single2@9/export?format=hf-model-index",
            Some(&alice),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A private record stays private through the export.
    let (status, _) = hub
        .call_text(
            Method::GET,
            "/api/v1/cards/alice/single2/export?format=hf-model-index",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_eval_has_no_model_index_and_bundle_is_undecided() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.ready_attachments(EVAL).await;
    hub.call(
        Method::POST,
        "/api/v1/evals/alice/single2-k4",
        Some(&alice),
        Some(EVAL),
    )
    .await;
    hub.call(
        Method::POST,
        "/api/v1/cards/alice/single2",
        Some(&alice),
        Some(CARD),
    )
    .await;

    // An Eval carries material, not scores: there is nothing to project.
    let (status, _) = hub
        .call_text(
            Method::GET,
            "/api/v1/evals/alice/single2-k4/export?format=hf-model-index",
            Some(&alice),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The bundle format is not decided; the hub says so rather than
    // pretending.
    let (status, _) = hub
        .call_text(
            Method::GET,
            "/api/v1/cards/alice/single2/export?format=bundle",
            Some(&alice),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);

    let (status, _) = hub
        .call_text(
            Method::GET,
            "/api/v1/cards/alice/single2/export?format=parquet",
            Some(&alice),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
