#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

//! A fresh Postgres per test: start the container, apply the embedded
//! migrations, hand back the pool. The container guard must outlive the
//! test; dropping it stops the container.

use evalhub_store::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};

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
    evalhub_store::pool::migrate(&pool).await.expect("migrate");
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
