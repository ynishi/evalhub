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
//! Only the attachment GC exists today; the registry jobs arrive with the
//! registry itself.
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

use std::sync::Arc;
use std::time::Duration;

use evalhub_store::PgPool;
use evalhub_store::objects::{self, Objects};
use evalhub_store::pool::AdvisoryLock;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Advisory-lock keys, one per job. They are arbitrary but fixed: two
/// deployments sharing a database must agree on them, and nothing else
/// in the schema uses `pg_advisory_lock`.
pub mod lock {
    /// Attachment garbage collection.
    pub const ATTACHMENT_GC: i64 = 0x4556_4148_0000_0001;
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
