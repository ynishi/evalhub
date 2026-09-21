//! Persistence for evalhub: Postgres as the source of truth, an S3-compatible
//! object store for attachments, and the compilation of query IR to SQL.
//!
//! # Source of truth
//!
//! Postgres holds everything the hub knows, including the canonical bytes of
//! every record version (`versions.body`, `jsonb`). There is no file-system
//! backend and no "files are truth, the index is derived" mode: a hub that
//! could be rebuilt from files would need a `reindex` command, and a
//! `reindex` command is where two sources of truth start to disagree.
//! Portability is `pg_dump` plus a bucket sync, or per-record
//! `export?format=bundle`.
//!
//! Object storage holds attachment bytes only, keyed by sha256. The hub
//! hands out presigned URLs and never proxies bytes; this is what keeps the
//! server stateless and cheap to run on egress-free storage.
//!
//! # Tables
//!
//! ```text
//! users / orgs / org_members / tokens        identity and authorisation
//! namespaces (ns, kind)                       one per user, one per org
//! records    (id, type, ns, name, visibility) the named thing; UNIQUE(type, ns, name)
//! versions   (version_id, record_id, seq, label, content_hash, body jsonb,
//!             changed[], badges[], tombstoned_at, tombstone_reason, tombstone_note)
//!            + generated columns for every core facet key
//! fingerprints (version_id, facet, fingerprint, registry_version)
//! results    (version_id, metric, aggregation, value, n, by jsonb)
//! relations  (from_version_id, type, to_version_id NULL, to_external, attrs jsonb)
//! attachments (sha256, size, media_type, state pending|ready)
//! attachment_refs (version_id, sha256, path)
//! registry   (kind, ns, id, version, body jsonb, state)
//! audit      append-only
//! ```
//!
//! # No EAV: the index lives on the JSON body
//!
//! An earlier design flattened every parameter into an
//! entity-attribute-value table. It was replaced by indexing `versions.body`
//! directly, in three layers:
//!
//! 1. **Core facet keys** are `GENERATED ALWAYS AS (body #>> '{model,id}')
//!    STORED` columns with B-tree indexes. The key set is fixed by the
//!    schema, so the columns are fixed by the migration, and the query
//!    compiler emits plain column references.
//! 2. **Unregistered `ext`** is served by one GIN index (`jsonb_path_ops`)
//!    over `body`. That index answers containment (`@>`) and existence
//!    (`@?`), which is exactly the `eq` / `exists` the query language allows
//!    for unregistered paths.
//! 3. **Registered `ext` paths** each get an expression index,
//!    `CREATE INDEX CONCURRENTLY ... ((evalhub_num(body #> '{ext,ns,key}')))`,
//!    created when the `ext_schema` is registered. Postgres does the backfill;
//!    there is no re-flatten job. The registry entry's `applying → applied`
//!    transition is read from `pg_index.indisvalid`.
//!
//! The cast helpers (`evalhub_num`, `evalhub_str`, `evalhub_bool`) are
//! `IMMUTABLE` SQL functions that return `NULL` unless `jsonb_typeof`
//! matches, so an old version with a wrongly typed value cannot make index
//! creation fail. The query compiler emits the *same* expression text the
//! index was created with — both come from one function in
//! [`query_sql`] — or the planner will not use the index.
//!
//! # One transaction per write
//!
//! A `POST` of a record is a single transaction touching `versions`,
//! `fingerprints`, `results`, `relations`, `attachment_refs` and `audit`.
//! Either the whole version exists or none of it does; there is no state in
//! which a version is visible without its relations.
//!
//! # Tombstones
//!
//! Deleting a version sets `tombstoned_at`, a `tombstone_reason`
//! (`withdrawn` / `duplicate` / `takedown` / `other`), an optional note, and
//! nulls `body`. `version_id`, `content_hash` and `changed[]` remain, so a
//! relation pointing at the version still resolves and shows *that* it
//! points at a tombstone and why. "Latest" means the highest non-tombstoned
//! `seq`. When a tombstone drops an attachment's reference count to zero,
//! the GC job deletes the object.
//!
//! # Modules
//!
//! - [`pool`] — connection pool and migration runner.
//! - [`records`] — names, versions, labels, tombstones, idempotent create.
//! - [`objects`] — attachment lifecycle: presign, confirm, reference count, GC.
//! - [`relations`] — edges, resolution, traversal, the comparison view.
//! - [`registry`] — registry entries and the `applying` transition.
//! - [`index`] — expression-index creation for registered `ext` paths.
//! - [`query_sql`] — IR to SQL, including cursor predicates.
//! - [`audit`] — append-only audit rows.
//!
//! Migrations are embedded from `migrations/` with `sqlx::migrate!`; they
//! are the only way the schema changes.

pub mod audit;
pub mod error;
pub mod index;
pub mod objects;
pub mod pool;
pub mod query_sql;
pub mod records;
pub mod registry;
pub mod relations;

/// The connection pool type handed to the server. Re-exported so that no
/// other crate needs a direct `sqlx` dependency.
pub use sqlx::PgPool;

/// The embedded migrations. Applied by `evalhub migrate` and by the
/// integration tests against a fresh container.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
