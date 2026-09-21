//! The store's error type.
//!
//! One enum, one variant per failure class the server has to distinguish.
//! Database driver errors and migration errors are wrapped rather than
//! exposed so that callers match on *what went wrong* (`MigrationsPending`,
//! `LabelInUse`) instead of on driver internals.

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

    /// A write named a namespace that has no `namespaces` row. Namespaces
    /// are created by user or organisation creation, never implicitly by a
    /// record write.
    #[error("namespace {0:?} does not exist")]
    NamespaceUnknown(String),

    /// The requested `label` already names another version of the same
    /// record. Maps to `409 label_in_use`.
    #[error("label already in use on this record")]
    LabelInUse,

    /// `create_user` was given a login that already exists.
    #[error("login {0:?} already exists")]
    LoginInUse(String),
}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        StoreError::Query(e)
    }
}

/// Postgres SQLSTATE for a foreign-key violation.
pub(crate) const SQLSTATE_FOREIGN_KEY: &str = "23503";
/// Postgres SQLSTATE for a unique violation.
pub(crate) const SQLSTATE_UNIQUE: &str = "23505";

/// The constraint a database error violated, if it was a database error
/// with a named constraint. Used to turn FK / unique failures into the
/// typed variants above.
pub(crate) fn violated_constraint(e: &sqlx::Error) -> Option<(String, String)> {
    let db = e.as_database_error()?;
    let code = db.code()?.into_owned();
    let constraint = db.constraint()?.to_owned();
    Some((code, constraint))
}
