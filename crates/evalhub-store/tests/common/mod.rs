#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

//! A fresh Postgres per test: start the container, apply the embedded
//! migrations, hand back the pool. The container guard must outlive the
//! test; dropping it stops the container.
//!
//! [`s3`] does the same for an S3-compatible store (MinIO) with the bucket
//! created.

use std::time::Duration;

use evalhub_store::PgPool;
use evalhub_store::objects::{ObjectConfig, Objects};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::core::{ContainerPort, ExecCommand, Mount, WaitFor};
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// A running Postgres with the schema applied.
pub struct Db {
    /// Keeps the container alive.
    pub container: ContainerAsync<Postgres>,
    /// Pool against it.
    pub pool: PgPool,
}

/// Postgres major the tests run against. The module's default image is
/// `11-alpine`, which predates generated columns (Postgres 12); the
/// migration needs them.
pub const POSTGRES_TAG: &str = "16";

/// Start a container, migrate, connect.
pub async fn db() -> Db {
    let db = db_unmigrated().await;
    evalhub_store::pool::migrate(&db.pool)
        .await
        .expect("migrate");
    db
}

/// Start a container and connect, with nothing applied: for the tests of
/// the migration runner itself, which apply the SQL half on its own
/// (`evalhub_store::MIGRATOR.run`) to stand where a 0.1.x database stands
/// after the 0.2.0 DDL and before the data step.
pub async fn db_unmigrated() -> Db {
    let container = Postgres::default()
        .with_tag(POSTGRES_TAG)
        .start()
        .await
        .expect("start postgres container");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("mapped port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = evalhub_store::pool::connect(&url, 4)
        .await
        .expect("connect");
    Db { container, pool }
}

/// A deterministic 32-byte digest of `bytes` standing in for the content
/// hash. The store never inspects the hash, it only compares and stores
/// it, so any injective-enough function of the input works here.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, b) in bytes.iter().enumerate() {
        out[i % 32] = out[i % 32].wrapping_mul(31).wrapping_add(*b);
    }
    out
}

/// A running MinIO with the bucket created.
pub struct S3 {
    /// Keeps the container alive.
    pub container: ContainerAsync<GenericImage>,
    /// `http://127.0.0.1:<port>`.
    pub endpoint: String,
    /// Client pair against it.
    pub objects: Objects,
}

/// MinIO image. Neither `minio/minio` on Docker Hub nor
/// `quay.io/minio/minio` can be pulled anonymously any more (quay.io
/// started answering `401` in September 2026), so the Chainguard build is
/// used. It ships `mc` and a shell, which the bucket setup below needs.
/// `testcontainers_modules::minio` hard-codes the Docker Hub name, so a
/// `GenericImage` is used instead.
pub const MINIO_IMAGE: &str = "cgr.dev/chainguard/minio";
/// Pinned by digest: Chainguard's free tier publishes `latest` only.
pub const MINIO_TAG: &str =
    "latest@sha256:bd014394a80898e68c149f2311fdf8d5a2c2f3bb2c33b9327ae6d02b4b065ae1";
/// Bucket the tests use.
pub const BUCKET: &str = "evalhub";
const MINIO_USER: &str = "minioadmin";
const MINIO_PASSWORD: &str = "minioadmin";

/// Start MinIO, create the bucket with the bundled `mc`, build the clients.
pub async fn s3() -> S3 {
    let container = GenericImage::new(MINIO_IMAGE, MINIO_TAG)
        .with_exposed_port(ContainerPort::Tcp(9000))
        // `/data` is not a volume in this image; on the container's overlay
        // root MinIO logs a rename error, itself starting with "API:", before
        // it is up. A tmpfs avoids the error, and the wait names the banner.
        .with_wait_for(WaitFor::message_on_stderr("API: http"))
        .with_mount(Mount::tmpfs_mount("/data"))
        .with_env_var("MINIO_ROOT_USER", MINIO_USER)
        .with_env_var("MINIO_ROOT_PASSWORD", MINIO_PASSWORD)
        .with_cmd(["server", "/data", "--console-address", ":9001"])
        .start()
        .await
        .expect("start minio container");

    // `mc` ships in the image. The exit code is only known once the
    // command has finished, which `stdout_to_vec` / `stderr_to_vec` wait
    // for. The alias step can race the API coming up, so it is retried.
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
    let endpoint = format!("http://127.0.0.1:{port}");
    let objects = Objects::new(ObjectConfig {
        endpoint: endpoint.clone(),
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
    S3 {
        container,
        endpoint,
        objects,
    }
}

/// Insert a namespace, a record and one live version with raw SQL, for
/// tests that need rows without going through `records::create_or_append`.
/// Returns `(record_id, version_id)`.
pub async fn seed_version(
    pool: &PgPool,
    record_type: &str,
    ns: &str,
    name: &str,
    seq: i32,
    visibility: &str,
    title: &str,
) -> (uuid::Uuid, uuid::Uuid) {
    sqlx::query("INSERT INTO namespaces (ns, kind) VALUES ($1, 'user') ON CONFLICT DO NOTHING")
        .bind(ns)
        .execute(pool)
        .await
        .unwrap();
    let record_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM records WHERE type = $1 AND ns = $2 AND name = $3")
            .bind(record_type)
            .bind(ns)
            .bind(name)
            .fetch_optional(pool)
            .await
            .unwrap();
    let record_id = match record_id {
        Some(id) => {
            sqlx::query("UPDATE records SET visibility = $2 WHERE id = $1")
                .bind(id)
                .bind(visibility)
                .execute(pool)
                .await
                .unwrap();
            id
        }
        None => {
            let id = uuid::Uuid::new_v4();
            sqlx::query(
                "INSERT INTO records (id, type, ns, name, visibility) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(record_type)
            .bind(ns)
            .bind(name)
            .bind(visibility)
            .execute(pool)
            .await
            .unwrap();
            id
        }
    };
    let version_id = uuid::Uuid::new_v4();
    let body = serde_json::json!({ "title": title });
    let hash = sha256(format!("{ns}/{name}@{seq}:{title}").as_bytes());
    sqlx::query(
        "INSERT INTO versions (version_id, record_id, seq, content_hash, body) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(version_id)
    .bind(record_id)
    .bind(seq)
    .bind(hash.as_slice())
    .bind(body)
    .execute(pool)
    .await
    .unwrap();
    (record_id, version_id)
}
