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

    /// A namespace of that name already exists (as a user or an org).
    #[error("namespace {0:?} already exists")]
    NamespaceInUse(String),

    /// The namespace exists but is a user's, not an organisation's, so it
    /// has no members.
    #[error("namespace {0:?} is not an organisation")]
    NotAnOrganisation(String),

    /// The requested `label` already names another version of the same
    /// record. Maps to `409 label_in_use`. Raised on ingest only; re-pointing
    /// with [`crate::records::set_label`] moves the label instead.
    #[error("label already in use on this record")]
    LabelInUse,

    /// A label that is purely numeric, which the address syntax reserves
    /// for `seq`.
    #[error("label must not be purely numeric")]
    LabelInvalid,

    /// `create_user` was given a login that already exists.
    #[error("login {0:?} already exists")]
    LoginInUse(String),

    /// The object store refused or failed a request.
    #[error("object store: {0}")]
    ObjectStore(#[from] object_store::Error),

    /// `complete` was called for a sha256 with no `attachments` row; the
    /// upload was never announced with `begin_upload`.
    #[error("attachment {0} was never announced")]
    AttachmentUnknown(String),

    /// `complete` found no object under the key: the client did not
    /// upload, or uploaded elsewhere.
    #[error("object for attachment {0} is not in the store")]
    ObjectNotUploaded(String),

    /// The uploaded object's size differs from the announced size.
    #[error("object size {actual} does not match the announced {declared}")]
    ObjectSizeMismatch {
        /// Size announced with `begin_upload`.
        declared: i64,
        /// Size the store reports.
        actual: u64,
    },

    /// The uploaded object's sha256 differs from the key it was announced
    /// under. Only detected for objects small enough to be re-hashed.
    #[error("object content does not hash to {0}")]
    ObjectHashMismatch(String),

    /// `relations::add` named a source version that does not exist.
    #[error("source version {0} does not exist")]
    SourceVersionUnknown(uuid::Uuid),

    /// A relation target was neither `{ns}/{name}@{seq}`, `external:…` nor `hf:…`.
    #[error("relation target {0:?} is not a valid reference")]
    RelationTargetInvalid(String),

    /// A record write named `{type}/{ns}/{name}` and no such record exists.
    /// Write paths do not apply the visibility rule (the caller holds
    /// `write` on the namespace), so this is a plain absence.
    #[error("record does not exist")]
    RecordNotFound,

    /// A write addressed `@{seq}` and the record has no such version.
    #[error("version does not exist")]
    VersionNotFound,

    /// A tombstone was requested for a version that already is one.
    #[error("version is already tombstoned")]
    AlreadyTombstoned,

    /// A record referenced `attachments[].sha256` values that are not
    /// uploaded and confirmed. Maps to `409 attachment_missing`. Carries
    /// every missing digest, in the order they appeared.
    #[error("{} attachment(s) not ready", .0.len())]
    AttachmentMissing(Vec<[u8; 32]>),
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
