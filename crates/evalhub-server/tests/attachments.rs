#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The attachment flow end to end: announce, upload to the presigned URL,
//! confirm, attach to a record, download. Needs Docker (Postgres and
//! MinIO via testcontainers).

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{CARD, Hub, sha256_hex};
use evalhub_store::auth::Scope;

/// A Card body whose single attachment is `bytes` under `path`.
fn card_attaching(path: &str, bytes: &[u8]) -> String {
    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["attachments"] = json!([{
        "path": path,
        "sha256": sha256_hex(bytes),
        "size": bytes.len(),
        "media_type": "application/x-ndjson",
    }]);
    v["results"][0]["samples_ref"] = json!(path);
    v.to_string()
}

async fn put(url: &str, bytes: Vec<u8>) -> StatusCode {
    let res = reqwest::Client::new()
        .put(url)
        .body(bytes)
        .send()
        .await
        .expect("upload");
    StatusCode::from_u16(res.status().as_u16()).unwrap()
}

/// Announce, upload, confirm, then post a record that references the
/// object — the `409` the record path gives without it is gone.
#[tokio::test]
async fn upload_confirm_and_attach() {
    let hub = Hub::start_with_storage().await;
    let alice = hub.user("alice", Scope::Write).await;
    let bytes = b"{\"id\":1}\n{\"id\":2}\n".to_vec();
    let sha = sha256_hex(&bytes);

    // Without the object, a record referencing it is a conflict.
    let body = card_attaching("samples.jsonl", &bytes);
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(&body),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");

    let announce =
        json!({"sha256": sha, "size": bytes.len(), "media_type": "application/x-ndjson"});
    let (status, announced) = hub
        .call(
            Method::POST,
            "/api/v1/attachments",
            Some(&alice),
            Some(&announce.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{announced}");
    assert_eq!(announced["state"], "pending");
    let url = announced["upload_url"].as_str().unwrap().to_string();
    assert!(announced["expires_in_secs"].as_u64().unwrap() > 0);

    assert_eq!(put(&url, bytes.clone()).await, StatusCode::OK);

    let (status, done) = hub
        .call(
            Method::POST,
            &format!("/api/v1/attachments/{sha}/complete"),
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["size"], bytes.len());
    assert_eq!(done["hashed_by_hub"], true, "below the verify cap");

    // Announcing a confirmed object says so and hands out no URL.
    let (status, again) = hub
        .call(
            Method::POST,
            "/api/v1/attachments",
            Some(&alice),
            Some(&announce.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["state"], "ready");
    assert!(again.get("upload_url").is_none());

    // Now the record goes in.
    let (status, created) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(&body),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
}

/// `complete` refuses bytes that are not what was announced.
#[tokio::test]
async fn complete_checks_size_and_hash() {
    let hub = Hub::start_with_storage().await;
    let alice = hub.user("alice", Scope::Write).await;

    // Announce ten bytes, upload five.
    let bytes = b"0123456789".to_vec();
    let sha = sha256_hex(&bytes);
    let (_, announced) = hub
        .call(
            Method::POST,
            "/api/v1/attachments",
            Some(&alice),
            Some(&json!({"sha256": sha, "size": 10}).to_string()),
        )
        .await;
    let url = announced["upload_url"].as_str().unwrap().to_string();
    assert_eq!(put(&url, b"01234".to_vec()).await, StatusCode::OK);
    let (status, err) = hub
        .call(
            Method::POST,
            &format!("/api/v1/attachments/{sha}/complete"),
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["path"], "/size");

    // Right size, wrong bytes.
    let (status, err) = {
        assert_eq!(put(&url, b"9876543210".to_vec()).await, StatusCode::OK);
        hub.call(
            Method::POST,
            &format!("/api/v1/attachments/{sha}/complete"),
            Some(&alice),
            None,
        )
        .await
    };
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["path"], "/sha256");
    assert_eq!(err["errors"][0]["code"], "schema");

    // Announced, never uploaded.
    let missing = sha256_hex(b"nothing was ever put here");
    hub.call(
        Method::POST,
        "/api/v1/attachments",
        Some(&alice),
        Some(&json!({"sha256": missing, "size": 1}).to_string()),
    )
    .await;
    let (status, err) = hub
        .call(
            Method::POST,
            &format!("/api/v1/attachments/{missing}/complete"),
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["errors"][0]["code"], "attachment_missing");
}

/// Downloading follows the visibility of the records that reference the
/// object.
#[tokio::test]
async fn download_follows_the_referencing_records() {
    let hub = Hub::start_with_storage().await;
    let alice = hub.user("alice", Scope::Write).await;
    let bob = hub.user("bob", Scope::Write).await;
    let bytes = b"{\"sample\":true}\n".to_vec();
    let sha = sha256_hex(&bytes);

    let (_, announced) = hub
        .call(
            Method::POST,
            "/api/v1/attachments",
            Some(&alice),
            Some(&json!({"sha256": sha, "size": bytes.len()}).to_string()),
        )
        .await;
    let url = announced["upload_url"].as_str().unwrap().to_string();
    assert_eq!(put(&url, bytes.clone()).await, StatusCode::OK);
    hub.call(
        Method::POST,
        &format!("/api/v1/attachments/{sha}/complete"),
        Some(&alice),
        None,
    )
    .await;

    // Unreferenced: any token, but not an anonymous caller.
    let path = format!("/api/v1/attachments/{sha}");
    assert_eq!(
        hub.call(Method::GET, &path, Some(&bob), None).await.0,
        StatusCode::FOUND,
        "an upload in flight is visible to any token"
    );
    assert_eq!(
        hub.call(Method::GET, &path, None, None).await.0,
        StatusCode::NOT_FOUND
    );

    // Referenced by a private record: the owner only.
    let body = card_attaching("samples.jsonl", &bytes);
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(&body),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        hub.call(Method::GET, &path, Some(&alice), None).await.0,
        StatusCode::FOUND
    );
    assert_eq!(
        hub.call(Method::GET, &path, Some(&bob), None).await.0,
        StatusCode::NOT_FOUND,
        "a stranger cannot fetch a private record's attachment"
    );
    assert_eq!(
        hub.call(Method::GET, &path, None, None).await.0,
        StatusCode::NOT_FOUND
    );

    // Public record: everyone, and the redirect really serves the bytes.
    hub.call(
        Method::PATCH,
        "/api/v1/cards/alice/x/settings",
        Some(&alice),
        Some(r#"{"visibility":"public"}"#),
    )
    .await;
    let res = hub
        .app
        .clone()
        .oneshot(
            axum::http::Request::get(&path)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FOUND);
    let location = res.headers()["location"].to_str().unwrap().to_string();
    let fetched = reqwest::get(&location).await.expect("download");
    assert_eq!(fetched.status().as_u16(), 200);
    assert_eq!(fetched.bytes().await.unwrap().to_vec(), bytes);

    // HEAD reports the size without a body.
    let res = hub
        .app
        .clone()
        .oneshot(
            axum::http::Request::head(&path)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()["content-length"].to_str().unwrap(),
        bytes.len().to_string()
    );
}

/// A path segment that is not a sha256, and a hub with no object store.
#[tokio::test]
async fn bad_sha_and_missing_storage() {
    let hub = Hub::start_with_storage().await;
    let alice = hub.user("alice", Scope::Write).await;
    for bad in ["not-hex", "abc", &"z".repeat(64)] {
        let (status, _) = hub
            .call(
                Method::GET,
                &format!("/api/v1/attachments/{bad}"),
                Some(&alice),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/attachments",
            Some(&alice),
            Some(&json!({"sha256": "nope", "size": 1}).to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["path"], "/sha256");

    // A hub without storage says so rather than pretending.
    let plain = Hub::start().await;
    let carol = plain.user("carol", Scope::Write).await;
    let (status, _) = plain
        .call(
            Method::POST,
            "/api/v1/attachments",
            Some(&carol),
            Some(&json!({"sha256": sha256_hex(b"x"), "size": 1}).to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

use tower::ServiceExt;
