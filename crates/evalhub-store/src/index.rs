//! Expression-index creation for registered `ext` paths.
//!
//! When an `ext_schema` is registered, each typed path in it becomes an
//! index on `versions`:
//!
//! ```sql
//! CREATE INDEX CONCURRENTLY IF NOT EXISTS versions_ext_<hash>
//!   ON versions ((evalhub_num(body #> '{"ext","alice/qwen-loop","rung"}')))
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
//! uses ([`crate::query_sql::ext_expression`]), because Postgres only uses
//! an expression index when the query's expression matches it textually
//! after normalisation. Two renderings of "the number at this path" would
//! mean an index nobody uses. [`ext_expression`] here is that function,
//! re-exported so a reader of this module finds it where they look.
//!
//! State is read, not stored: an `ext_schema` is `applied` when every index
//! it implies exists and `pg_index.indisvalid` is true. A failed
//! `CONCURRENTLY` build leaves an invalid index; the job drops and retries
//! it, and the entry stays `applying` meanwhile.
//!
//! The index name is derived from a hash of the path so it is stable,
//! unique, and within Postgres's 63-byte identifier limit.
//!
//! # The lock
//!
//! [`LOCK_KEY`] is the advisory-lock key this work takes, next to the
//! attachment collector's in `evalhub_server::jobs::lock`. It lives here
//! because the statements do; the job that calls [`apply_pending`] on a
//! timer lives in the server.

use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, PgPool};

use crate::error::StoreError;
use crate::pool::AdvisoryLock;
use crate::registry::{self, ExtPath, ExtSchema, Kind, State};

pub use crate::query_sql::{ext_expression, ext_expression_unqualified};

/// Advisory-lock key for expression-index building. Arbitrary but fixed:
/// deployments sharing a database must agree on it.
pub const LOCK_KEY: i64 = 0x4556_4148_0000_0002;

/// The index name for a path: stable, unique, and short enough for
/// Postgres's 63-byte identifier limit.
///
/// The hash is over the segments with a separator that cannot appear in
/// one, so `{a.b}` and `{a,b}` do not collide.
pub fn index_name(path: &[ExtPathSegment]) -> String {
    let mut hasher = Sha256::new();
    for segment in path {
        hasher.update(segment.as_bytes());
        hasher.update([0u8]);
    }
    let digest = hasher.finalize();
    format!("versions_ext_{}", hex::encode(&digest[..16]))
}

/// What [`index_name`] hashes: any string-like path segment.
pub type ExtPathSegment = String;

/// Create the index for one typed path, if it is not there.
///
/// Runs `CREATE INDEX CONCURRENTLY`, which Postgres refuses inside a
/// transaction, so it takes its own connection from the pool. It returns
/// when Postgres has finished the build or has left an invalid index
/// behind; [`is_valid`] is what decides which happened.
pub async fn create_concurrently(pool: &PgPool, path: &ExtPath) -> Result<String, StoreError> {
    let name = index_name(&path.path);
    let expression = ext_expression_unqualified(&path.path, path.ty)?;
    let statement = format!(
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS {name} \
         ON versions (({expression})) WHERE tombstoned_at IS NULL"
    );
    let mut conn = pool.acquire().await.map_err(StoreError::Query)?;
    // Audited: `name` is hex from `index_name` and `expression` comes
    // from `ext_expression_unqualified`, which escapes or refuses every
    // segment. Nothing here is caller text.
    sqlx::query(AssertSqlSafe(statement))
        .execute(&mut *conn)
        .await
        .map_err(StoreError::Query)?;
    Ok(name)
}

/// Whether an index exists and Postgres considers it usable.
///
/// A `CONCURRENTLY` build that failed leaves the index in place and
/// `indisvalid = false`; the planner ignores it and so must the hub.
pub async fn is_valid(pool: &PgPool, name: &str) -> Result<bool, StoreError> {
    let valid: Option<bool> = sqlx::query_scalar(
        "SELECT i.indisvalid FROM pg_class c JOIN pg_index i ON i.indexrelid = c.oid
         WHERE c.relname = $1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(StoreError::Query)?;
    Ok(valid.unwrap_or(false))
}

/// Drop an index that exists but is not valid, so the next attempt can
/// build it again. A valid index is left alone.
pub async fn drop_invalid(pool: &PgPool, name: &str) -> Result<bool, StoreError> {
    if is_valid(pool, name).await? {
        return Ok(false);
    }
    let mut conn = pool.acquire().await.map_err(StoreError::Query)?;
    // Audited: `name` is hex from `index_name`.
    sqlx::query(AssertSqlSafe(format!(
        "DROP INDEX CONCURRENTLY IF EXISTS {name}"
    )))
    .execute(&mut *conn)
    .await
    .map_err(StoreError::Query)?;
    Ok(true)
}

/// Build every index one `ext_schema` implies and flip it to `applied`
/// when they are all valid.
///
/// Returns whether the entry finished. A `false` means at least one index
/// is still invalid — dropped here so the next sweep rebuilds it — and the
/// entry stays `applying`, which is what makes its paths behave as
/// unregistered meanwhile.
///
/// An `ext_schema` with no typed paths applies immediately: there is
/// nothing to index, and leaving it `applying` forever would be a lie.
pub async fn apply_ext_schema(pool: &PgPool, schema: &ExtSchema) -> Result<bool, StoreError> {
    let mut all_valid = true;
    for path in &schema.paths {
        let name = create_concurrently(pool, path).await?;
        if !is_valid(pool, &name).await? {
            drop_invalid(pool, &name).await?;
            all_valid = false;
        }
    }
    if !all_valid {
        return Ok(false);
    }
    let (ns, id) = split_key(&schema.key)?;
    registry::set_state(
        pool,
        Kind::ExtSchemas,
        ns,
        id,
        &schema.version,
        State::Applied,
    )
    .await?;
    Ok(true)
}

/// Apply every `ext_schema` that is still `applying`, under the advisory
/// lock so that only one replica builds.
///
/// Returns the number of entries that reached `applied` on this sweep, or
/// `None` when another replica holds the lock and is doing the same work.
pub async fn apply_pending(pool: &PgPool) -> Result<Option<usize>, StoreError> {
    let Some(lock) = AdvisoryLock::try_acquire(pool, LOCK_KEY).await? else {
        return Ok(None);
    };
    let pending = registry::ext_schemas(pool, State::Applying).await?;
    let mut applied = 0usize;
    for schema in &pending {
        if apply_ext_schema(pool, schema).await? {
            applied += 1;
        }
    }
    lock.release().await?;
    Ok(Some(applied))
}

/// `{ns}/{id}` back into its two halves.
fn split_key(key: &str) -> Result<(&str, &str), StoreError> {
    key.split_once('/')
        .ok_or_else(|| StoreError::ExtSchemaInvalid(key.to_owned(), "key is not {ns}/{id}".into()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use evalhub_query::ir::ValueType;

    #[test]
    fn index_names_are_stable_short_and_distinct() {
        let a = vec!["ext".to_string(), "alice/x".to_string(), "rung".to_string()];
        let b = vec![
            "ext".to_string(),
            "alice/x".to_string(),
            "depth".to_string(),
        ];
        assert_eq!(index_name(&a), index_name(&a));
        assert_ne!(index_name(&a), index_name(&b));
        assert!(index_name(&a).len() <= 63);
        assert!(index_name(&a).starts_with("versions_ext_"));
    }

    #[test]
    fn segment_separator_prevents_collisions() {
        let joined = vec!["ab".to_string()];
        let split = vec!["a".to_string(), "b".to_string()];
        assert_ne!(index_name(&joined), index_name(&split));
    }

    #[test]
    fn the_index_expression_matches_the_query_one_but_for_the_alias() {
        let path = vec!["ext".to_string(), "alice/x".to_string(), "rung".to_string()];
        let query = ext_expression(&path, ValueType::Number).unwrap();
        let index = ext_expression_unqualified(&path, ValueType::Number).unwrap();
        assert_eq!(query.replacen("v.body", "body", 1), index);
    }

    #[test]
    fn a_key_without_a_slash_is_refused() {
        assert!(split_key("alice").is_err());
        assert_eq!(split_key("alice/x").unwrap(), ("alice", "x"));
    }
}
