#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The server boots and answers its meta endpoints without a database.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use evalhub_server::api::router;
use evalhub_server::config::Config;

fn app() -> axum::Router {
    router(
        Arc::new(Config::default()),
        None,
        None,
        evalhub_server::state::PathTableHandle::from_schema(),
    )
}

async fn get(path: &str) -> (StatusCode, serde_json::Value) {
    let res = app()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

#[tokio::test]
async fn healthz_answers_without_database() {
    let (status, body) = get("/api/v1/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["database"], "not_configured");
}

#[tokio::test]
async fn whoami_is_anonymous_without_token() {
    let (status, body) = get("/api/v1/whoami").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["user"].is_null());
    assert_eq!(body["namespaces"], serde_json::json!([]));
}

#[tokio::test]
async fn unknown_api_path_is_404_and_browser_path_reaches_the_spa() {
    let (status, _) = get("/api/v1/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A path the SPA routes itself: the server must answer a document, not
    // a 404, or a reload of a deep link would break.
    let res = app()
        .oneshot(Request::get("/cards/alice/x").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let ct = res.headers()["content-type"].to_str().unwrap().to_string();
    assert!(ct.starts_with("text/html"), "content-type was {ct}");

    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let html = String::from_utf8_lossy(&bytes).into_owned();
    // CI builds without `web/build`, so this is the placeholder; when a UI
    // is built in it is `index.html` instead, and either way the document
    // has to say where the contract is or point at a script that does.
    if html.contains("not built into this binary") {
        assert!(html.contains("/openapi.json"), "{html}");
        assert!(html.contains("pnpm build"), "{html}");
    } else {
        assert!(html.contains("<script"), "a built SPA loads its bundle");
    }
}

#[tokio::test]
async fn openapi_is_3_1_and_stable() {
    let (status, body) = get("/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["openapi"].as_str().unwrap().starts_with("3.1"),
        "openapi version was {}",
        body["openapi"]
    );
    assert_eq!(body["info"]["title"], "evalhub");
    assert!(
        body["paths"]["/api/v1/cards/{ns}/{name}"]["post"].is_object(),
        "record routes are documented"
    );
    insta::assert_json_snapshot!("openapi", body);
}

#[tokio::test]
async fn schemas_are_served_with_id() {
    let res = app()
        .oneshot(
            Request::get("/schemas/card")
                .header("host", "hub.example")
                .header("x-forwarded-proto", "https")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["$id"], "https://hub.example/schemas/card");
    assert!(
        body["$schema"].as_str().unwrap().contains("2020-12"),
        "{}",
        body["$schema"]
    );
    assert_eq!(body["additionalProperties"], false);

    let (status, _) = get("/schemas/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
