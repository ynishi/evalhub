//! The store's error type.
//!
//! One enum, one variant per failure class the server has to distinguish.
//! Database driver errors and migration errors are wrapped rather than
//! exposed so that callers match on *what went wrong* (`MigrationsPending`)
//! instead of on driver internals.

/// Errors from the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Could not connect to Postgres.
    #[error("database connection failed: {0}")]
    Connect(#[source] sqlx::Error),

    /// A query or transaction failed.
    #[error("database query failed: {0}")]
    Query(#[source] sqlx::Error),

    /// Applying migrations failed.
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    /// The database is behind the embedded migrations. Lists the pending versions.
    #[error("database has pending migrations: {0:?}; run `evalhub migrate`")]
    MigrationsPending(Vec<i64>),
}
