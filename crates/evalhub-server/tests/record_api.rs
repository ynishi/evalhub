#![allow(clippy::unwrap_used, clippy::expect_used)]

//! What M2 added to the record path: validation that collects every error,
//! fingerprints and badges, labels, visibility, tombstones and listing.
//! Needs Docker (Postgres via testcontainers).

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use common::{CARD, EVAL, Hub, with_title};
use evalhub_store::auth::Scope;

/// Every violation in one body comes back in one `422`.
#[tokio::test]
async fn validation_collects_every_error() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;

    let mut v: Value = serde_json::from_str(CARD).unwrap();
    // A metric without a namespace, counts that do not add up, and a
    // samples_ref naming an attachment that is not declared.
    v["results"][0]["metric"] = json!("pass_rate");
    v["results"][0]["samples_ref"] = json!("nowhere.jsonl");
    v["counts"]["attempted"] = json!(1);

    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/bad",
            Some(&alice),
            Some(&v.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    let codes: Vec<&str> = err["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"metric_id_invalid"), "{codes:?}");
    assert!(codes.contains(&"counts_inconsistent"), "{codes:?}");
    assert!(codes.contains(&"attachment_ref_unknown"), "{codes:?}");
    for e in err["errors"].as_array().unwrap() {
        let path = e["path"].as_str().unwrap();
        assert!(
            path.is_empty() || path.starts_with('/'),
            "paths are JSON pointers: {path}"
        );
    }
}

/// A key marked `x-fingerprint: false` does not change the facet's
/// fingerprint; a core key does.
#[tokio::test]
async fn fingerprints_ignore_recorded_but_unfingerprinted_keys() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;

    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/base",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, base) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/base?expand=fingerprints",
            Some(&alice),
            None,
        )
        .await;
    let base_fp = base["fingerprints"].clone();
    assert!(base_fp["model"].is_string(), "{base}");
    assert_eq!(base_fp.as_object().unwrap().len(), 7, "seven facets");

    // context_window is recorded but excluded from the fingerprint.
    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["model"]["context_window"] = json!(32_768);
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/same",
            Some(&alice),
            Some(&v.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, same) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/same?expand=fingerprints",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(same["fingerprints"]["model"], base_fp["model"]);

    // The model id is a core key.
    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["model"]["id"] = json!("other-model");
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/other",
            Some(&alice),
            Some(&v.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, other) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/other?expand=fingerprints",
            Some(&alice),
            None,
        )
        .await;
    assert_ne!(other["fingerprints"]["model"], base_fp["model"]);
    assert_eq!(other["fingerprints"]["task"], base_fp["task"]);
}

/// Badges name facts the hub checked, and `refs_resolved` waits for the
/// Eval the Card cites.
#[tokio::test]
async fn badges_report_what_the_hub_checked() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.ready_attachments(EVAL).await;

    // The fixture Card cites alice/single2-k4@3, which does not exist yet.
    let (status, first) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/early",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    let badges: Vec<String> = serde_json::from_value(first["badges"].clone()).unwrap();
    assert!(badges.contains(&"env_pinned".to_string()), "{badges:?}");
    assert!(badges.contains(&"redacted".to_string()), "{badges:?}");
    assert!(!badges.contains(&"refs_resolved".to_string()), "{badges:?}");
    assert!(
        !badges.contains(&"metric_registered".to_string()),
        "the registry is empty until M3: {badges:?}"
    );

    // Publish the Eval the Card points at, up to seq 3, then post again.
    for n in 1..=3 {
        let (status, _) = hub
            .call(
                Method::POST,
                "/api/v1/evals/alice/single2-k4",
                Some(&alice),
                Some(&with_title(EVAL, &format!("run {n}"))),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let (status, later) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/late",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{later}");
    let badges: Vec<String> = serde_json::from_value(later["badges"].clone()).unwrap();
    assert!(badges.contains(&"refs_resolved".to_string()), "{badges:?}");
}

/// A record whose attachment was never confirmed is a state conflict.
#[tokio::test]
async fn unconfirmed_attachment_is_a_conflict() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    // Deliberately not seeding the attachment rows.
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{err}");
    assert_eq!(err["errors"][0]["code"], "attachment_missing");
    assert_eq!(err["errors"][0]["path"], "/attachments/0/sha256");
}

/// Labels address versions and can be re-pointed; numbers cannot be labels.
#[tokio::test]
async fn labels_address_and_move() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;

    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x?label=baseline",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, v2) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(&with_title(CARD, "second")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(v2["seq"], 2);

    let (status, by_label) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/x@baseline",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{by_label}");
    assert_eq!(by_label["seq"], 1);

    // Re-point the label at seq 2.
    let (status, moved) = hub
        .call(
            Method::PATCH,
            "/api/v1/cards/alice/x@2/label",
            Some(&alice),
            Some(r#"{"label":"baseline"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["seq"], 2);
    let (_, by_label) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/x@baseline",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(by_label["seq"], 2);
    let (_, v1) = hub
        .call(Method::GET, "/api/v1/cards/alice/x@1", Some(&alice), None)
        .await;
    assert!(v1["label"].is_null(), "the label left version 1");

    let (status, _) = hub
        .call(
            Method::PATCH,
            "/api/v1/cards/alice/x@2/label",
            Some(&alice),
            Some(r#"{"label":"12"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "a label is never a number");
}

/// Visibility opens a record to everyone, and the list follows.
#[tokio::test]
async fn visibility_opens_a_record_and_the_list() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    hub.call(
        Method::POST,
        "/api/v1/cards/alice/open",
        Some(&alice),
        Some(CARD),
    )
    .await;

    let (status, page) = hub.call(Method::GET, "/api/v1/cards", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        page["items"].as_array().unwrap().len(),
        0,
        "private is unseen"
    );

    let (status, settings) = hub
        .call(
            Method::PATCH,
            "/api/v1/cards/alice/open/settings",
            Some(&alice),
            Some(r#"{"visibility":"public"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["visibility"], "public");

    let (status, got) = hub
        .call(Method::GET, "/api/v1/cards/alice/open", None, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let (_, page) = hub.call(Method::GET, "/api/v1/cards", None, None).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["name"], "open");
    assert_eq!(page["items"][0]["latest"]["seq"], 1);
}

/// A tombstone removes the body and keeps the facts.
#[tokio::test]
async fn tombstone_keeps_the_facts() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    let (_, v1) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(CARD),
        )
        .await;
    hub.call(
        Method::POST,
        "/api/v1/cards/alice/x",
        Some(&alice),
        Some(&with_title(CARD, "second")),
    )
    .await;

    let (status, dead) = hub
        .call(
            Method::DELETE,
            "/api/v1/cards/alice/x@2",
            Some(&alice),
            Some(r#"{"reason":"withdrawn","note":"posted by mistake"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{dead}");
    assert_eq!(dead["tombstone"]["reason"], "withdrawn");
    assert_eq!(dead["tombstone"]["note"], "posted by mistake");
    assert!(dead["record"].is_null(), "the body is gone");
    assert_eq!(
        dead["content_hash"].as_str().unwrap().len(),
        64,
        "the commitment stays"
    );

    // Latest falls back to the live version; the tombstone is still addressable.
    let (status, latest) = hub
        .call(Method::GET, "/api/v1/cards/alice/x", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(latest["version_id"], v1["version_id"]);
    let (status, by_seq) = hub
        .call(Method::GET, "/api/v1/cards/alice/x@2", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(by_seq["tombstone"]["reason"], "withdrawn");

    // The version list shows both.
    let (status, versions) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/x/versions",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{versions}");
    let items = versions["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items[0]["tombstone"].is_null());
    assert_eq!(items[1]["tombstone"]["reason"], "withdrawn");

    let (status, _) = hub
        .call(
            Method::DELETE,
            "/api/v1/cards/alice/x@2",
            Some(&alice),
            Some(r#"{"reason":"other"}"#),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "twice is not a thing");
}

/// Listing pages by cursor, searches, and refuses a forged cursor.
#[tokio::test]
async fn listing_pages_searches_and_verifies_cursors() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    hub.ready_attachments(CARD).await;
    for name in ["alpha", "beta", "gamma"] {
        let (status, _) = hub
            .call(
                Method::POST,
                &format!("/api/v1/cards/alice/{name}"),
                Some(&alice),
                Some(&with_title(CARD, name)),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        hub.call(
            Method::PATCH,
            &format!("/api/v1/cards/alice/{name}/settings"),
            Some(&alice),
            Some(r#"{"visibility":"public"}"#),
        )
        .await;
    }

    let (status, page1) = hub
        .call(
            Method::GET,
            "/api/v1/cards?limit=2&sort=name_asc",
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page1}");
    assert_eq!(page1["items"].as_array().unwrap().len(), 2);
    assert_eq!(page1["items"][0]["name"], "alpha");
    let cursor = page1["next_cursor"].as_str().unwrap().to_string();

    let (status, page2) = hub
        .call(
            Method::GET,
            &format!("/api/v1/cards?limit=2&sort=name_asc&cursor={cursor}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{page2}");
    assert_eq!(page2["items"].as_array().unwrap().len(), 1);
    assert_eq!(page2["items"][0]["name"], "gamma");
    assert!(page2["next_cursor"].is_null());

    let (status, found) = hub
        .call(Method::GET, "/api/v1/cards?search=bet", None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["items"].as_array().unwrap().len(), 1);
    assert_eq!(found["items"][0]["name"], "beta");

    let (status, _) = hub
        .call(Method::GET, "/api/v1/cards?cursor=forged", None, None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
