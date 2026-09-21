#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The walking skeleton, end to end: a token-holding caller posts a Card
//! and an Eval, reads them back, and a re-post of the same body is
//! idempotent. Needs Docker (Postgres via testcontainers).

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tower::ServiceExt;

use evalhub_server::api::router;
use evalhub_server::config::Config;
use evalhub_store::PgPool;
use evalhub_store::auth::Scope;

const CARD: &str = include_str!("../../evalhub-schema/fixtures/card-complete.json");
const EVAL: &str = include_str!("../../evalhub-schema/fixtures/eval-run-set.json");

struct Hub {
    _container: ContainerAsync<Postgres>,
    pool: PgPool,
    app: axum::Router,
}

impl Hub {
    async fn start() -> Self {
        let container = Postgres::default()
            .with_tag("16")
            .start()
            .await
            .expect("start postgres");
        let port = container.get_host_port_ipv4(5432).await.expect("port");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
        let pool = evalhub_store::pool::connect(&url, 4)
            .await
            .expect("connect");
        evalhub_store::pool::migrate(&pool).await.expect("migrate");
        let app = router(Arc::new(Config::default()), Some(pool.clone()));
        Self {
            _container: container,
            pool,
            app,
        }
    }

    /// A user with a personal namespace and a token of `scope`; returns the secret.
    async fn user(&self, login: &str, scope: Scope) -> String {
        let user_id = evalhub_store::auth::create_user(&self.pool, login)
            .await
            .expect("create user");
        let (secret, hash) = evalhub_server::auth::new_secret();
        evalhub_store::auth::create_token(&self.pool, user_id, scope, &[login.to_string()], &hash)
            .await
            .expect("create token");
        secret
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }
}

fn with_title(record: &str, title: &str) -> String {
    let mut v: Value = serde_json::from_str(record).unwrap();
    v["title"] = json!(title);
    v.to_string()
}

#[tokio::test]
async fn card_post_get_and_idempotent_repost() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;

    let (status, created) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/single2",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["seq"], 1);
    assert!(created["record"].is_null(), "POST does not echo the record");
    let version_id = created["version_id"].as_str().unwrap().to_string();
    assert_eq!(version_id.len(), 26, "ULID text form");
    let hash = created["content_hash"].as_str().unwrap().to_string();
    assert_eq!(hash.len(), 64);
    let changed: Vec<String> = serde_json::from_value(created["changed"].clone()).unwrap();
    assert!(
        changed.contains(&"title".to_string()),
        "first version changes every key"
    );

    // The stored record hashes to the content_hash the hub reported.
    let (status, got) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/single2",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["version_id"], version_id);
    assert_eq!(got["record"]["title"], "qwen3.6-32b on single2 K4");
    let (_, rehash) = evalhub_core::canonical::hash_value(&got["record"]).unwrap();
    assert_eq!(rehash.as_hex(), hash);

    // Same body again, in a different key order: 200, same version.
    let mut reordered: serde_json::Map<String, Value> = serde_json::from_str(CARD).unwrap();
    let title = reordered.remove("title").unwrap();
    reordered.insert("title".into(), title);
    let body = Value::Object(reordered).to_string();
    let (status, again) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/single2",
            Some(&alice),
            Some(&body),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["version_id"], version_id);
    assert_eq!(again["seq"], 1);

    // A different body: seq 2, and `changed` names the key.
    let (status, v2) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/single2",
            Some(&alice),
            Some(&with_title(CARD, "second")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v2}");
    assert_eq!(v2["seq"], 2);
    assert_eq!(v2["changed"], json!(["title"]));

    // Addressing by seq.
    let (status, first) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/single2@1",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["version_id"], version_id);
    let (status, _) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/single2@3",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = hub
        .call(
            Method::GET,
            "/api/v1/cards/alice/single2@label",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "labels are not addressable yet"
    );
}

#[tokio::test]
async fn eval_post_and_get() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    let (status, created) = hub
        .call(
            Method::POST,
            "/api/v1/evals/alice/single2-k4",
            Some(&alice),
            Some(EVAL),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, got) = hub
        .call(
            Method::GET,
            "/api/v1/evals/alice/single2-k4",
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got["record"]["eval_kind"], "run_set");
    assert_eq!(got["version_id"], created["version_id"]);

    // A Card body posted as an Eval is rejected by shape.
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/evals/alice/wrong",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "schema");
}

#[tokio::test]
async fn auth_and_visibility() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;
    let bob = hub.user("bob", Scope::Write).await;
    let reader = {
        let user_id = evalhub_store::auth::create_user(&hub.pool, "carol")
            .await
            .unwrap();
        let (secret, hash) = evalhub_server::auth::new_secret();
        evalhub_store::auth::create_token(
            &hub.pool,
            user_id,
            Scope::Read,
            &["alice".to_string()],
            &hash,
        )
        .await
        .unwrap();
        secret
    };

    // No token: 401. Wrong namespace: 403. Read scope: 403.
    let (status, _) = hub
        .call(Method::POST, "/api/v1/cards/alice/x", None, Some(CARD))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some("nope"),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&bob),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&reader),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // A namespace nobody owns: the token does not cover it either.
    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/nobody/x",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);

    // Private by default: 404 to anonymous and to bob, 200 to alice and
    // to a read token covering alice.
    let (status, _) = hub
        .call(Method::GET, "/api/v1/cards/alice/x", None, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = hub
        .call(Method::GET, "/api/v1/cards/alice/x", Some(&bob), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = hub
        .call(Method::GET, "/api/v1/cards/alice/x", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = hub
        .call(Method::GET, "/api/v1/cards/alice/x", Some(&reader), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    // whoami reflects the token.
    let (status, who) = hub
        .call(Method::GET, "/api/v1/whoami", Some(&alice), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(who["user"], "alice");
    assert_eq!(who["scope"], "write");
    assert_eq!(who["namespaces"], json!(["alice"]));
}

#[tokio::test]
async fn shape_errors_and_labels() {
    let hub = Hub::start().await;
    let alice = hub.user("alice", Scope::Write).await;

    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["surprise"] = json!(1);
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(&v.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(err["errors"][0]["code"], "schema");

    let mut v: Value = serde_json::from_str(CARD).unwrap();
    v["schema"] = json!("evalhub.card/9.0");
    let (status, err) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some(&v.to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(err["errors"][0]["path"], "schema");

    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x",
            Some(&alice),
            Some("not json"),
        )
        .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "{status}"
    );

    let (status, labelled) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x?label=baseline",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{labelled}");
    assert_eq!(labelled["label"], "baseline");

    let (status, _) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/y?label=123",
            Some(&alice),
            Some(CARD),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "purely numeric labels are refused"
    );

    let (status, conflict) = hub
        .call(
            Method::POST,
            "/api/v1/cards/alice/x?label=baseline",
            Some(&alice),
            Some(&with_title(CARD, "v2")),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{conflict}");
    assert_eq!(conflict["errors"][0]["code"], "label_in_use");
}
