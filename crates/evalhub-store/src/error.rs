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

    /// A one-shot data migration ([`crate::data_migrations`]) found data it
    /// will not convert, and stopped. Its transaction was rolled back, so
    /// the database is exactly as before the attempt and the migration is
    /// still pending. `problems` names every offending item found (one
    /// line each, with the record and version it is in), not only the
    /// first, so the operator fixes the data in one round.
    #[error("data migration {name} stopped, nothing was changed: {}", .problems.join("; "))]
    DataMigrationRefused {
        /// The data migration, e.g. `0003_runs_split`.
        name: &'static str,
        /// One line per offending item.
        problems: Vec<String>,
    },

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

    /// The query IR asked for something this backend cannot render: a
    /// column it does not know, an operator the column does not support,
    /// a cursor that does not match the sort. The type check in
    /// `evalhub_query` rejects these first; reaching here means the two
    /// disagree, so the message names what could not be rendered.
    #[error("query cannot be compiled: {0}")]
    QueryUnsupported(String),

    /// A registry entry already exists at that address. Entries are
    /// immutable; a change is a new `@{version}`.
    #[error("registry entry {0} already exists")]
    RegistryEntryExists(String),

    /// A write addressed the `core/` namespace, which ships with the hub
    /// and is read-only over the API.
    #[error("the core registry is read-only")]
    RegistryCoreReadOnly,

    /// A read or a state change addressed a registry entry that does not
    /// exist.
    #[error("registry entry {0} does not exist")]
    RegistryEntryNotFound(String),

    /// An `ext_schema` body was not shaped like a JSON Schema this hub can
    /// derive typed paths from.
    #[error("ext_schema {0} cannot be indexed: {1}")]
    ExtSchemaInvalid(String, String),

    /// A run write addressed a `run_id` the Eval has no row for (archive,
    /// unarchive, delete). A `put` of an unknown id creates it instead.
    #[error("run does not exist")]
    RunNotFound,

    /// Archive, unarchive or delete addressed a run that is already
    /// deleted (tombstoned). Maps to `409 run_deleted`. A `put` of a
    /// deleted id is reported per element in [`StoreError::RunsRejected`]
    /// instead, with the same code.
    #[error("run is deleted")]
    RunDeleted,

    /// A `put` or `put_batch` (or the runs of an `evalhub.eval/1.0` body)
    /// was refused, and nothing was written. Carries every failing element,
    /// in input order, each with the reasons it failed: the validation
    /// codes of `evalhub_core::validate::run`, `batch_duplicate_run_id`,
    /// `run_deleted` and `attachment_missing`. The HTTP status is the
    /// server's to choose from the codes (`ErrorCode::status`).
    #[error("{} run(s) rejected", .0.len())]
    RunsRejected(Vec<RunRejection>),

    /// A Card's references to runs were refused, and nothing was written:
    /// the Card ingest ([`crate::records::ingest`]) or a later
    /// `core/uses_eval` edge ([`crate::relations::add`]) named a run that
    /// has no row in an Eval the writer may see (`run_unknown`), or a
    /// `run_results[]` element outside the Card's used set for its Eval
    /// (`run_not_in_used_set`). Carries every offending position, sorted
    /// by `(path, code)`: `/run_results/{i}/run_id`,
    /// `/relations/{j}/attrs/runs/{k}` on ingest, `/attrs/runs/{k}` on
    /// `add`. Both codes are `422`.
    ///
    /// An entry never says whether an Eval exists or may be seen: a run
    /// of an Eval the writer may not see, a run of an Eval that does not
    /// exist and a run missing from a visible Eval produce the same entry,
    /// byte for byte (the hint names nothing but the position).
    #[error("{} run reference(s) rejected", .0.len())]
    CardRunsRejected(Vec<evalhub_schema::error::ErrorEntry>),

    /// A run projection ([`crate::runs::project`]) named Cards that are
    /// unknown to the caller: no such Card, a Card the caller may not see,
    /// a Card with no live version, or one whose latest live version has
    /// no resolved `core/uses_eval` edge into the Eval read. The cases are
    /// deliberately one variant. Carries every such `{ns}/{name}`, in
    /// request order; nothing was read.
    #[error("unknown card(s): {0:?}")]
    RunCardsUnknown(Vec<String>),

    /// Canonicalising a body failed. Not expected for a value parsed from
    /// JSON text; surfaced rather than unwrapped because a hash of a
    /// partially written buffer must never be stored.
    #[error(transparent)]
    Canonical(#[from] evalhub_core::canonical::CanonicalError),
}

/// One refused element of a run write. See [`StoreError::RunsRejected`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRejection {
    /// Position of the element in the input: the batch's `runs[]`, `0` for
    /// a single `put`, or the position in the posted `runs[]` for a
    /// converted `evalhub.eval/1.0` body (the last element carrying that
    /// `run_id`, which is the one the conversion kept).
    pub index: usize,
    /// The `run_id` the element was written under (empty when it had none).
    pub run_id: String,
    /// Every reason, sorted by `(path, code)`. `path` is a JSON pointer
    /// into the run body, as `evalhub_core::validate::run` reports it.
    /// `attachment_missing` is looked up only for an element that passed
    /// every other check (its digests are only known to be well formed
    /// then), so an element can gain it on a second attempt.
    pub errors: Vec<evalhub_schema::error::ErrorEntry>,
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
