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
