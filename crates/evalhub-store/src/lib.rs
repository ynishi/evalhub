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
//!
//! records.runs_hash                           digest over every run of an Eval
//! runs       (record_id, run_id, status, error_kind, started_at, ended_at,
//!             body jsonb, content_hash, archived_at,
//!             tombstoned_at, tombstone_reason, tombstone_note)
//! run_metrics         (record_id, run_id, metric, value)
//! run_attachment_refs (record_id, run_id, path, sha256)
//! run_fingerprints    (record_id, run_id, facet, fingerprint)
//! run_results    (version_id, ordinal, eval_record_id, run_id, metric, value, label, by)
//! card_eval_runs (card_version_id, eval_record_id, run_id, content_hash)
//! data_migrations (name, applied_at)          one-shot data steps done
//! ```
//!
//! # Runs
//!
//! An Eval is a header, whose versions are rows of `versions` like a
//! Card's, and a set of runs, which are rows of `runs` keyed by
//! `(record_id, run_id)` and are *not* versions: a run is overwritten in
//! place, archived and deleted without appending a header version
//! ([`runs`]). Both kinds of write lock the `records` row first, so a
//! header post and a run write to the same Eval are serialised, and
//! `records.runs_hash` (the digest over every run, archived and deleted
//! included; `evalhub_core::run::runs_hash`) is recomputed in the same
//! transaction as the run write that changes it. A run's facets default to
//! the header's: the ones it omits are copied from the latest live header
//! when it is written, and the copy is stored and hashed, so a later header
//! changes no run. `run_results` and `card_eval_runs` are the Card side:
//! a Card version's per-run judgements and the runs it used, each with the
//! content hash it saw.
//!
//! Release 0.2.0 still accepts an `evalhub.eval/1.0` body (header and
//! `runs[]` together); [`records::ingest`] splits it and writes the runs
//! through the same code as a batch, in the one transaction. See
//! [`records`].
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
//! which a version is visible without its relations. A run write, a batch
//! of them included, is likewise one transaction over `runs`, its side
//! tables, `records.runs_hash` and `audit`: a batch lands whole or not at
//! all.
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
//! Deleting a run is the same tombstone on a `runs` row: same four
//! reasons, `body` nulled, `run_attachment_refs` deleted, `content_hash`,
//! `run_metrics` and `run_fingerprints` kept, so `runs_hash` does not move
//! and a Card that used the run still has the digest it judged. A deleted
//! `run_id` is never written again (`run_deleted`). An attachment's
//! reference count is the sum of `attachment_refs` and
//! `run_attachment_refs`; the GC collects an object only when both are
//! zero.
//!
//! When every header version of an Eval is tombstoned the record has no
//! latest, and its runs become unreachable (reads `None`, writes
//! `RecordNotFound`) without any data changing.
//!
//! # Visibility
//!
//! `records.visibility` (`private` / `public`) is the only visibility
//! there is: versions and runs follow their record. Every read filters on
//! `visibility = 'public' OR ns = ANY(caller_namespaces)`, the SQL spelling
//! of [`relations::visible_to`], so a private record is indistinguishable
//! from an absent one. Runs add one rule: an archived run is readable by
//! members of the namespace only, its reference to an attachment counts
//! for a download by a member only ([`objects::Referencing::run_archived`]),
//! and the envelope's archived / deleted counts are shown to members only
//! ([`records::runs_summary`]). Write paths do not filter the record
//! written; the server has checked `write` on the namespace.
//!
//! # Modules
//!
//! - [`pool`] — connection pool and migration runner.
//! - [`auth`] — users, namespaces and tokens: the identity rows.
//! - [`records`] — names, versions, labels, tombstones, idempotent create,
//!   the 1.0 → 2.0 split at ingest, the run summary of an Eval.
//! - [`runs`] — run rows: put, batch, get, archive, delete, `runs_hash`.
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
pub mod auth;
pub mod error;
pub mod index;
pub mod objects;
pub mod pool;
pub mod query_sql;
pub mod records;
pub mod registry;
pub mod relations;
pub mod runs;

/// The connection pool type handed to the server. Re-exported so that no
/// other crate needs a direct `sqlx` dependency.
pub use sqlx::PgPool;

/// The embedded migrations. Applied by `evalhub migrate` and by the
/// integration tests against a fresh container.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
