#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

//! A running hub for the integration tests: a fresh Postgres, the migrated
//! schema, the real router, and helpers to mint users and call endpoints.
//!
//! [`Hub::start_with_storage`] adds a MinIO container and wires it into
//! the router, for the tests that exercise the attachment flow.

use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::core::{ContainerPort, ExecCommand, WaitFor};
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tower::ServiceExt;

use evalhub_server::api::router;
use evalhub_server::config::{Config, S3};
use evalhub_store::PgPool;
use evalhub_store::auth::Scope;
use evalhub_store::objects::{ObjectConfig, Objects};

pub const CARD: &str = include_str!("../../../evalhub-schema/fixtures/card-complete.json");
pub const EVAL: &str = include_str!("../../../evalhub-schema/fixtures/eval-run-set.json");

pub struct Hub {
    _container: ContainerAsync<Postgres>,
    /// Kept alive for the test's duration when storage was requested.
    _minio: Option<ContainerAsync<GenericImage>>,
    pub pool: PgPool,
    pub app: axum::Router,
    /// The query vocabulary the router reads. Tests that register an
    /// extension schema run the index job and then rebuild it, which is
    /// what `serve` does on its own timer.
    pub path_tables: evalhub_server::state::PathTableHandle,
}

/// MinIO image. `minio/minio` on Docker Hub is no longer pullable; the
/// same builds are published on quay.io, and
/// `testcontainers_modules::minio` hard-codes the Docker Hub name, so a
/// `GenericImage` is used instead.
pub const MINIO_IMAGE: &str = "quay.io/minio/minio";
/// Tag matching the one `testcontainers_modules` 0.15 pins.
pub const MINIO_TAG: &str = "RELEASE.2025-02-28T09-55-16Z";
/// Bucket the tests use.
pub const BUCKET: &str = "evalhub";
const MINIO_USER: &str = "minioadmin";
const MINIO_PASSWORD: &str = "minioadmin";

impl Hub {
    /// A hub with a database and no object store: the attachment
    /// endpoints answer `503`.
    pub async fn start() -> Self {
        Self::build(false).await
    }

    /// A hub with a database and a MinIO behind the attachment endpoints.
    pub async fn start_with_storage() -> Self {
        Self::build(true).await
    }

    async fn build(with_storage: bool) -> Self {
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

        let mut config = Config::default();
        let (minio, objects) = if with_storage {
            let (minio, endpoint) = start_minio().await;
            config.s3 = S3 {
                endpoint: Some(endpoint.clone()),
                bucket: BUCKET.to_string(),
                allow_http: true,
                ..S3::default()
            };
            let objects = Objects::new(ObjectConfig {
                endpoint,
                public_endpoint: None,
                bucket: BUCKET.to_string(),
                access_key: MINIO_USER.to_string(),
                secret_key: MINIO_PASSWORD.to_string(),
                region: ObjectConfig::DEFAULT_REGION.to_string(),
                path_style: true,
                allow_http: true,
                presign_ttl: Duration::from_secs(300),
            })
            .expect("objects");
            (Some(minio), Some(Arc::new(objects)))
        } else {
            (None, None)
        };

        let path_tables = evalhub_server::state::PathTableHandle::from_schema();
        let app = router(
            Arc::new(config),
            Some(pool.clone()),
            objects,
            path_tables.clone(),
        );
        Self {
            _container: container,
            _minio: minio,
            pool,
            app,
            path_tables,
        }
    }

    /// Run the index builder once and republish the query vocabulary, as
    /// the background job does.
    pub async fn apply_ext_schemas(&self) {
        evalhub_store::index::apply_pending(&self.pool)
            .await
            .expect("apply pending ext schemas");
        let entries = evalhub_store::registry::ext_schemas_applied(&self.pool)
            .await
            .expect("load applied ext schemas");
        self.path_tables
            .rebuild(&evalhub_server::jobs::to_query_ext(&entries))
            .await;
    }

    /// Mark every attachment a fixture declares as uploaded and confirmed.
    ///
    /// Posting a record whose attachments are not `ready` is `409
    /// attachment_missing`, which is the attachment flow's business; these
    /// tests are about the record path, so they seed the rows the upload
    /// endpoints would have written.
    pub async fn ready_attachments(&self, record: &str) {
        let value: Value = serde_json::from_str(record).unwrap();
        for a in value["attachments"].as_array().into_iter().flatten() {
            let sha = hex::decode(a["sha256"].as_str().unwrap()).unwrap();
            sqlx::query(
                "INSERT INTO attachments (sha256, size, media_type, state, hashed_by_hub)
                 VALUES ($1, $2, $3, 'ready', true)
                 ON CONFLICT (sha256) DO UPDATE SET state = 'ready'",
            )
            .bind(sha)
            .bind(a["size"].as_i64().unwrap())
            .bind(a["media_type"].as_str())
            .execute(&self.pool)
            .await
            .expect("seed attachment");
        }
    }

    /// A user with a personal namespace and a token of `scope`; returns the secret.
    pub async fn user(&self, login: &str, scope: Scope) -> String {
        let user_id = evalhub_store::auth::create_user(&self.pool, login)
            .await
            .expect("create user");
        let (secret, hash) = evalhub_server::auth::new_secret();
        evalhub_store::auth::create_token(&self.pool, user_id, scope, &[login.to_string()], &hash)
            .await
            .expect("create token");
        secret
    }

    pub async fn call(
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

    /// Like [`Hub::call`] but keeps the body as text, for responses that
    /// are not JSON.
    pub async fn call_text(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
    ) -> (StatusCode, String) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let res = self
            .app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// The same record with a different title, so a second POST is a new version.
pub fn with_title(record: &str, title: &str) -> String {
    let mut v: Value = serde_json::from_str(record).unwrap();
    v["title"] = json!(title);
    v.to_string()
}

/// Start MinIO and create the bucket with the bundled `mc`. Returns the
/// container guard and `http://127.0.0.1:<port>`.
async fn start_minio() -> (ContainerAsync<GenericImage>, String) {
    let container = GenericImage::new(MINIO_IMAGE, MINIO_TAG)
        .with_exposed_port(ContainerPort::Tcp(9000))
        .with_wait_for(WaitFor::message_on_stderr("API:"))
        .with_env_var("MINIO_ROOT_USER", MINIO_USER)
        .with_env_var("MINIO_ROOT_PASSWORD", MINIO_PASSWORD)
        .with_cmd(["server", "/data", "--console-address", ":9001"])
        .start()
        .await
        .expect("start minio container");

    // `mc` ships in the image. The exit code is only known once the
    // output has been drained, and the alias step can race the API coming
    // up, so it is retried.
    async fn mc(container: &ContainerAsync<GenericImage>, args: &[&str]) -> (Option<i64>, String) {
        let mut cmd = vec!["mc"];
        cmd.extend_from_slice(args);
        let mut res = container
            .exec(ExecCommand::new(cmd))
            .await
            .expect("exec mc");
        let out = res.stdout_to_vec().await.unwrap_or_default();
        let err = res.stderr_to_vec().await.unwrap_or_default();
        let code = res.exit_code().await.expect("exit code");
        (
            code,
            format!(
                "{}{}",
                String::from_utf8_lossy(&out),
                String::from_utf8_lossy(&err)
            ),
        )
    }
    let mut last = String::new();
    let mut ok = false;
    for _ in 0..40 {
        let (code, output) = mc(
            &container,
            &[
                "alias",
                "set",
                "local",
                "http://127.0.0.1:9000",
                MINIO_USER,
                MINIO_PASSWORD,
            ],
        )
        .await;
        if code == Some(0) {
            ok = true;
            break;
        }
        last = output;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(ok, "mc alias set never succeeded: {last}");
    let (code, output) = mc(
        &container,
        &["mb", "--ignore-existing", &format!("local/{BUCKET}")],
    )
    .await;
    assert_eq!(code, Some(0), "mc mb failed: {output}");

    let port = container
        .get_host_port_ipv4(9000)
        .await
        .expect("mapped port");
    (container, format!("http://127.0.0.1:{port}"))
}

/// sha256 of some bytes, hex, as a client would compute it before
/// announcing an upload.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
