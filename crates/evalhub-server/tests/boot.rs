#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The server boots and answers its meta endpoints without a database, and
//! the `evalhub` binary refuses to serve a database with a migration
//! pending, the one-shot data step included.

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
    // CI builds without `web-dist/`, so this is the placeholder; when a UI
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

/// Run the built `evalhub` binary with `args`, isolated from the caller's
/// configuration (no `EVALHUB_*` variable, no `evalhub.toml` in its working
/// directory). Waits at most `limit`; a process still running then is
/// killed and reported as `None` exit status.
fn run_evalhub(args: &[&str], limit: std::time::Duration) -> (Option<i32>, String, String) {
    use std::process::{Command, Stdio};
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("boot-evalhub");
    std::fs::create_dir_all(&dir).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_evalhub"));
    cmd.args(args)
        .current_dir(&dir)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, _) in std::env::vars() {
        if key.starts_with("EVALHUB_") {
            cmd.env_remove(key);
        }
    }
    let mut child = cmd.spawn().unwrap();
    let start = std::time::Instant::now();
    let finished = loop {
        if child.try_wait().unwrap().is_some() {
            break true;
        }
        if start.elapsed() > limit {
            child.kill().unwrap();
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let out = child.wait_with_output().unwrap();
    (
        finished.then(|| out.status.code()).flatten(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `serve` against a database whose SQL migrations are all applied but
/// whose data step (`0003_runs_split`) has not run exits non-zero, naming
/// the step, before it binds; `migrate` applies the step and says so, and
/// a second `migrate` applies nothing. Needs Docker.
#[tokio::test]
async fn serve_refuses_a_database_with_the_data_step_pending() {
    use testcontainers_modules::postgres::Postgres;
    use testcontainers_modules::testcontainers::ImageExt;
    use testcontainers_modules::testcontainers::runners::AsyncRunner;

    let container = Postgres::default()
        .with_tag("16")
        .start()
        .await
        .expect("start postgres");
    let port = container.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = evalhub_store::pool::connect(&url, 2)
        .await
        .expect("connect");
    // The DDL only: where a 0.1.x database stands once the 0.2.0 SQL
    // migrations have run and the data step has not.
    evalhub_store::MIGRATOR
        .run(&pool)
        .await
        .expect("sql migrations");

    let limit = std::time::Duration::from_secs(60);
    let serve = {
        let url = url.clone();
        tokio::task::spawn_blocking(move || {
            run_evalhub(
                &["serve", "--database-url", &url, "--bind", "127.0.0.1:0"],
                limit,
            )
        })
    };
    let (code, _, stderr) = serve.await.unwrap();
    assert!(
        code.is_some_and(|c| c != 0),
        "serve must exit non-zero on its own, got {code:?}; stderr: {stderr}"
    );
    assert!(
        stderr.contains("pending migrations [data 0003_runs_split]"),
        "{stderr}"
    );
    assert!(stderr.contains("run `evalhub migrate` first"), "{stderr}");

    let migrate = |url: String| {
        tokio::task::spawn_blocking(move || {
            run_evalhub(&["migrate", "--database-url", &url], limit)
        })
    };
    let (code, stdout, stderr) = migrate(url.clone()).await.unwrap();
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert!(
        stdout.contains("applied data migration 0003_runs_split: 0 Eval(s)"),
        "{stdout}"
    );
    let (code, stdout, stderr) = migrate(url.clone()).await.unwrap();
    assert_eq!(code, Some(0), "stderr: {stderr}");
    assert_eq!(stdout.trim(), "no data migration pending");
    assert!(
        evalhub_store::pool::pending_migrations(&pool)
            .await
            .unwrap()
            .is_empty()
    );
}
