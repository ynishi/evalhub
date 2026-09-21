//! Connection pool and migration runner.
//!
//! One `PgPool` per process, sized from configuration. `sqlx`'s built-in
//! pool is sufficient; there is no reason for a second pool crate.
//!
//! Migrations run from [`crate::MIGRATOR`] on `evalhub migrate`, and are
//! *checked* (not applied) on `evalhub serve` start-up: a server whose
//! database is behind refuses to start rather than serving requests it
//! cannot honour. Migrations are forward-only; a mistaken migration is
//! corrected by a new one.
//!
//! Long-running DDL that must not run inside a transaction
//! (`CREATE INDEX CONCURRENTLY`) does not go through the migrator; it is
//! issued by [`crate::index`] on a dedicated connection.
//!
//! [`AdvisoryLock`] is how the background jobs make sure that when several
//! replicas run, only one does a given sweep. It lives here because every
//! statement the hub issues lives in this crate; the jobs themselves are
//! `evalhub_server::jobs`.

use sqlx::PgPool;
use sqlx::migrate::Migrate;
use sqlx::postgres::PgPoolOptions;

use crate::MIGRATOR;
use crate::error::StoreError;

/// Open a pool against `url` with at most `max_connections` connections.
///
/// Fails fast: the first connection is established before this returns, so
/// a wrong URL surfaces at start-up rather than on the first request.
pub async fn connect(url: &str, max_connections: u32) -> Result<PgPool, StoreError> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(url)
        .await
        .map_err(StoreError::Connect)
}

/// Apply every embedded migration that has not been applied yet.
pub async fn migrate(pool: &PgPool) -> Result<(), StoreError> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

/// Return the versions of embedded migrations the database has not applied.
///
/// Empty means the database is current. Used by `serve` to refuse to start
/// on a stale schema.
pub async fn pending_migrations(pool: &PgPool) -> Result<Vec<i64>, StoreError> {
    let mut conn = pool.acquire().await.map_err(StoreError::Query)?;
    conn.ensure_migrations_table(&MIGRATOR.table_name).await?;
    let applied = conn.list_applied_migrations(&MIGRATOR.table_name).await?;
    let applied: std::collections::HashSet<i64> = applied.into_iter().map(|m| m.version).collect();
    Ok(MIGRATOR
        .iter()
        .filter(|m| !applied.contains(&m.version))
        .map(|m| m.version)
        .collect())
}

/// A held Postgres advisory lock.
///
/// The lock is tied to the connection that took it, so it is released
/// when [`AdvisoryLock::release`] is called or, failing that, when the
/// connection goes back to the pool and the session ends — which is what
/// makes a replica that dies mid-sweep harmless.
pub struct AdvisoryLock {
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
    key: i64,
}

impl AdvisoryLock {
    /// Take the lock for `key`, or return `None` when another session
    /// holds it. Never waits: a caller that finds the lock held has
    /// nothing to add, because the holder is doing the same work.
    pub async fn try_acquire(pool: &PgPool, key: i64) -> Result<Option<Self>, StoreError> {
        let mut conn = pool.acquire().await.map_err(StoreError::Query)?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut *conn)
            .await
            .map_err(StoreError::Query)?;
        Ok(locked.then_some(Self { conn, key }))
    }

    /// Release the lock. Dropping the value instead also releases it, at
    /// the end of the session rather than now.
    pub async fn release(mut self) -> Result<(), StoreError> {
        sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(self.key)
            .fetch_one(&mut *self.conn)
            .await
            .map_err(StoreError::Query)?;
        Ok(())
    }
}
