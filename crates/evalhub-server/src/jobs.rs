//! Background work, in-process.
//!
//! Three jobs, each a `tokio` task started by `serve`, each taking a
//! Postgres advisory lock so that when several replicas run only one does
//! the work:
//!
//! | Job               | Trigger                              | Does                                                           |
//! | ----------------- | ------------------------------------ | -------------------------------------------------------------- |
//! | index build       | `ext_schema` registered (`applying`) | `CREATE INDEX CONCURRENTLY` per typed path, one at a time, on a dedicated connection; flips the entry to `applied` when all valid |
//! | badge recompute   | harness / metric registered          | recomputes `harness_registered` / `metric_registered` on versions that cite the new entry |
//! | attachment GC     | periodic                             | deletes objects with zero `attachment_refs` older than the grace period, and `pending` objects older than it |
//!
//! There is no external queue. The work is idempotent and low-volume; the
//! job state that matters (index validity, badge arrays, ref counts) lives
//! in Postgres already, so a restart simply resumes. A queue would add a
//! dependency to self-hosting for no correctness gain.
//!
//! Jobs log at `info` when they start and finish and at `warn` on any
//! error; they never panic the server.
//!
//! Each one sweeps on a timer rather than waiting on a queue. A sweep is
//! a question the database can answer — "is anything pending?" — so a
//! restart loses nothing and a replica that missed an event still does the
//! work on its next tick. That is the whole reason there is no queue: the
//! state that matters is already in Postgres.
//!
//! # The advisory lock
//!
//! Each job owns one key in the `pg_advisory_lock` space, listed in
//! [`lock`]. A sweep takes it with `evalhub_store::pool::AdvisoryLock`
//! (`pg_try_advisory_lock` on a dedicated connection), works, then
//! releases it: `try` rather than a waiting lock, because a replica that
//! finds the lock held has nothing to add — the holder is doing the same
//! sweep — and should go back to sleep rather than queue up behind it. A
//! replica that dies mid-sweep drops its session, and Postgres releases
//! the lock with it.
//!
//! # Shutdown
//!
//! [`spawn_attachment_gc`] returns the `JoinHandle`, and `serve` aborts it
//! once `axum::serve` has returned. A sweep in flight is cut at its next
//! await point; the work is idempotent, so the next start picks up
//! whatever was left. There is no shutdown channel, because there is
//! nothing to flush.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use evalhub_core::badge::{self, Badge, BadgeInput};
use evalhub_query::typecheck::ExtSchema as QueryExtSchema;
use evalhub_store::PgPool;
use evalhub_store::objects::{self, Objects};
use evalhub_store::pool::AdvisoryLock;
use evalhub_store::{index, records, registry};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::state::PathTableHandle;

/// Advisory-lock keys, one per job. They are arbitrary but fixed: two
/// deployments sharing a database must agree on them, and nothing else
/// in the schema uses `pg_advisory_lock`.
pub mod lock {
    /// Attachment garbage collection.
    pub const ATTACHMENT_GC: i64 = 0x4556_4148_0000_0001;
    /// Expression-index builds for registered `ext_schemas`. Owned by
    /// `evalhub_store::index`, which takes it inside `apply_pending`.
    pub const INDEX_BUILD: i64 = evalhub_store::index::LOCK_KEY;
    /// Badge recomputation after a registry write.
    pub const BADGE_RECOMPUTE: i64 = 0x4556_4148_0000_0003;
}

/// Start the attachment garbage collector: every `every`, delete objects
/// that no version references and that were announced more than `grace`
/// ago.
///
/// The first sweep happens immediately, so a server that has been down
/// past its interval does not wait for one more. Errors are logged and the
/// loop continues: a store that is briefly unreachable must not end the
/// job for the lifetime of the process.
pub fn spawn_attachment_gc(
    pool: PgPool,
    objects: Arc<Objects>,
    grace: Duration,
    every: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match sweep(&pool, &objects, grace).await {
                Ok(Some(collected)) if collected > 0 => {
                    info!(collected, "attachment gc collected objects");
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(e) => warn!(error = %format!("{e:#}"), "attachment gc failed"),
            }
        }
    })
}

/// One sweep under the advisory lock. `Ok(None)` means another replica
/// holds the lock and is doing it.
async fn sweep(
    pool: &PgPool,
    objects: &Objects,
    grace: Duration,
) -> Result<Option<usize>, anyhow::Error> {
    let Some(lock) = AdvisoryLock::try_acquire(pool, lock::ATTACHMENT_GC).await? else {
        return Ok(None);
    };
    let result = objects::gc(pool, objects, grace).await;
    // Release whatever happened: a sweep that failed must not keep the
    // lock for the life of the connection.
    if let Err(e) = lock.release().await {
        warn!(error = %e, "releasing the attachment gc lock failed");
    }
    Ok(Some(result?))
}

/// Start the index builder: every `every`, finish any `ext_schema` whose
/// expression indexes are not built yet, and refresh the query vocabulary
/// when one lands.
///
/// The build itself is `evalhub_store::index::apply_pending`, which takes
/// [`lock::INDEX_BUILD`] and issues `CREATE INDEX CONCURRENTLY` on a
/// dedicated connection. This job is the timer around it plus the one
/// thing the store cannot do: swapping the server's path tables, so that
/// the newly indexed paths become sortable and rangeable to the next
/// query.
pub fn spawn_index_build(
    pool: PgPool,
    path_tables: PathTableHandle,
    every: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match index::apply_pending(&pool).await {
                // Another replica is building them; it will publish its
                // own tables, and this one picks the entries up on a later
                // tick through `ext_schemas_applied`.
                Ok(None) => {}
                Ok(Some(0)) => {}
                Ok(Some(applied)) => {
                    info!(applied, "extension schemas applied");
                    match registry::ext_schemas_applied(&pool).await {
                        Ok(entries) => {
                            path_tables.rebuild(&to_query_ext(&entries)).await;
                            info!("query path tables rebuilt");
                        }
                        Err(e) => {
                            warn!(error = %format!("{e:#}"), "reloading extension schemas failed");
                        }
                    }
                }
                Err(e) => warn!(error = %format!("{e:#}"), "index build failed"),
            }
        }
    })
}

/// Translate the store's applied extension schemas into the shape the
/// query crate's path table wants.
///
/// The store keeps a schema's paths from the document root (`ext`,
/// `{ns}/{id}`, key…); the path table wants the key segments below that,
/// with the namespace beside them.
pub fn to_query_ext(entries: &[registry::ExtSchema]) -> Vec<QueryExtSchema> {
    entries
        .iter()
        .map(|entry| QueryExtSchema {
            ns: entry.key.clone(),
            paths: entry
                .paths
                .iter()
                .map(|p| (p.path.iter().skip(2).cloned().collect::<Vec<_>>(), p.ty))
                .collect(),
            applied: true,
        })
        .collect()
}

/// Start the badge recomputation sweep.
///
/// Two badges depend on the registry rather than on the record:
/// `harness_registered` and `metric_registered`. Registering a harness or
/// a metric therefore changes them on versions that were stored before,
/// and this sweep is what makes that true. It walks every `metrics` and
/// `harnesses` entry, asks the store which live versions cite it, and
/// recomputes those versions' badges.
///
/// `refs_resolved` is deliberately *not* recomputed: it records whether a
/// record's references resolved when it was published, which is a fact
/// about publication order and does not become false later. The sweep
/// carries the stored value through.
pub fn spawn_badge_recompute(pool: PgPool, every: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match recompute_badges(&pool).await {
                Ok(Some(updated)) if updated > 0 => {
                    info!(updated, "badges recomputed");
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(e) => warn!(error = %format!("{e:#}"), "badge recompute failed"),
            }
        }
    })
}

/// One badge sweep under [`lock::BADGE_RECOMPUTE`]. `Ok(None)` means
/// another replica holds the lock.
async fn recompute_badges(pool: &PgPool) -> Result<Option<usize>, anyhow::Error> {
    let Some(lock) = AdvisoryLock::try_acquire(pool, lock::BADGE_RECOMPUTE).await? else {
        return Ok(None);
    };
    let result = sweep_badges(pool).await;
    if let Err(e) = lock.release().await {
        warn!(error = %e, "releasing the badge recompute lock failed");
    }
    Ok(Some(result?))
}

/// One badge sweep without the advisory lock, for a caller that already
/// knows it is alone — a test, or a future `evalhub` subcommand.
pub async fn sweep_badges_once(pool: &PgPool) -> Result<usize, anyhow::Error> {
    sweep_badges(pool).await
}

async fn sweep_badges(pool: &PgPool) -> Result<usize, anyhow::Error> {
    // Every version that cites a registered metric or harness, gathered
    // once so a version citing several is recomputed once.
    let mut affected: BTreeSet<uuid::Uuid> = BTreeSet::new();
    for entry in registry::list(pool, Some(registry::Kind::Metrics), None, 1000, 0).await? {
        let metric = format!("{}/{}", entry.ns, entry.id);
        affected.extend(registry::versions_citing_metric(pool, &metric).await?);
    }
    for entry in registry::list(pool, Some(registry::Kind::Harnesses), None, 1000, 0).await? {
        affected.extend(registry::versions_citing_harness(pool, &entry.id, &entry.version).await?);
    }

    let mut updated = 0usize;
    for version_id in affected {
        let Some((body, current)) = records::body_and_badges(pool, version_id).await? else {
            continue;
        };
        let input = BadgeInput {
            // A fact about ingest; carried through, never re-derived.
            all_refs_resolved: current.iter().any(|b| b == Badge::RefsResolved.as_str()),
            harness_registered: harness_registered(pool, &body).await?,
            all_metrics_registered: metrics_registered(pool, &body).await?,
        };
        let recomputed: Vec<String> = badge::compute(&body, &input)
            .into_iter()
            .map(|b| b.as_str().to_string())
            .collect();
        if recomputed != current {
            registry::set_badges(pool, version_id, &recomputed).await?;
            updated += 1;
        }
    }
    Ok(updated)
}

/// Split a registry id `{ns}/{name}`.
fn split_id(id: &str) -> Option<(&str, &str)> {
    id.split_once('/')
}

/// Whether the record's harness is a registry entry.
async fn harness_registered(
    pool: &PgPool,
    body: &serde_json::Value,
) -> Result<bool, anyhow::Error> {
    let (Some(name), Some(version)) = (
        body.pointer("/harness/name").and_then(|v| v.as_str()),
        body.pointer("/harness/version").and_then(|v| v.as_str()),
    ) else {
        return Ok(false);
    };
    let Some((ns, id)) = split_id(name) else {
        return Ok(false);
    };
    Ok(
        registry::get(pool, registry::Kind::Harnesses, ns, id, version)
            .await?
            .is_some(),
    )
}

/// Whether every metric the record reports is a registry entry. A record
/// with no results is vacuously true, which is what the badge rule says.
async fn metrics_registered(
    pool: &PgPool,
    body: &serde_json::Value,
) -> Result<bool, anyhow::Error> {
    let Some(results) = body.get("results").and_then(|v| v.as_array()) else {
        return Ok(true);
    };
    for result in results {
        let Some(metric) = result.get("metric").and_then(|v| v.as_str()) else {
            return Ok(false);
        };
        let Some((ns, id)) = split_id(metric) else {
            return Ok(false);
        };
        // Any version of the entry counts: the badge says the vocabulary
        // knows this metric, not which revision of it.
        let entries = registry::list(pool, Some(registry::Kind::Metrics), Some(ns), 200, 0).await?;
        if !entries.iter().any(|e| e.id == id) {
            return Ok(false);
        }
    }
    Ok(true)
}
