//! Expression-index creation for registered `ext` paths.
//!
//! When an `ext_schema` is registered, each typed path in it becomes an
//! index on `versions`:
//!
//! ```sql
//! CREATE INDEX CONCURRENTLY IF NOT EXISTS versions_ext_<hash>
//!   ON versions ((evalhub_num(body #> '{ext,alice/qwen-loop,rung}')))
//!   WHERE tombstoned_at IS NULL;
//! ```
//!
//! `CONCURRENTLY` cannot run inside a transaction and takes as long as the
//! table is large, so it is issued on a dedicated connection by a
//! background task (see `evalhub_server::jobs`), one index at a time, under
//! a Postgres advisory lock so that two replicas do not both try. Postgres
//! backfills the index from existing rows; the hub has no re-flatten job of
//! its own.
//!
//! The expression text is produced by the same function the query compiler
//! uses ([`crate::query_sql`]), because Postgres only uses an expression
//! index when the query's expression matches it textually after
//! normalisation. Two renderings of "the number at this path" would mean an
//! index nobody uses.
//!
//! State is read, not stored: an `ext_schema` is `applied` when every index
//! it implies exists and `pg_index.indisvalid` is true. A failed
//! `CONCURRENTLY` build leaves an invalid index; the job drops and retries
//! it, and the entry stays `applying` meanwhile.
//!
//! The index name is derived from a hash of the path so it is stable,
//! unique, and within Postgres's 63-byte identifier limit.
