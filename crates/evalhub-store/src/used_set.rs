//! The used set: which runs of an Eval a Card version used, fixed when the
//! Card version is posted ([`crate::records::ingest`]) or when a
//! `core/uses_eval` edge is added to it later ([`crate::relations::add`]).
//!
//! The rule is `evalhub_schema::card`'s "The used set", applied per Eval
//! *record* (runs belong to the record, not to a version):
//!
//! ```text
//! uses_eval edges of the Card version, resolved to a version of Eval record E
//!   any of them carries attrs.runs?  → used set = ∪ attrs.runs    (each must have a row)
//!   none does                        → used set = every run of E neither archived
//!                                                 nor tombstoned, at this moment
//! edges that did not resolve (external, not yet existing, not visible)
//!                                    → no used set, no card_eval_runs rows
//! ```
//!
//! "Has a row" counts archived and tombstoned runs: a tombstoned run keeps
//! its row and its content hash, so a Card can still say it judged it.
//!
//! # Locking
//!
//! The caller holds the Card record's row `FOR UPDATE` (the ingest's step
//! 1, or [`crate::relations::add`]). [`lock_evals`] then takes `FOR SHARE`
//! on each Eval record row the used sets are read from, in `records.id`
//! order. A run write takes the Eval row `FOR UPDATE` and never locks a
//! Card, so a run write waits for the Card's transaction (and vice versa)
//! but the two never wait on each other in a cycle; two Card writes
//! sharing Evals take only share locks on them, which do not conflict, and
//! the id order makes any future exclusive locker agree on the order. The
//! consequence is that the content hashes recorded in `card_eval_runs` are
//! exactly the ones the rows had when the Card version was committed.
//!
//! The relation was resolved, and the Eval's visibility read, before the
//! lock. `set_visibility` takes the record row `FOR UPDATE`, so the
//! visibility read *under* the share lock is the one that holds until the
//! Card commits; [`lock_evals`] reads it again and returns only the Evals
//! the writer may still see. An edge into one that turned private in
//! between is treated as unresolved (no used set, stored as text, no
//! `refs_resolved`), so the Card learns nothing about its runs.
//!
//! # What is recorded
//!
//! `card_eval_runs (card_version_id, eval_record_id, run_id, content_hash)`,
//! one row per used run with the run's `content_hash` at that moment
//! ([`insert`]). Comparing it with `runs.content_hash` later says which runs
//! were overwritten since the Card judged them.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use evalhub_schema::error::{ErrorCode, ErrorEntry};

use crate::error::StoreError;

/// The hint of every `run_unknown` entry. It names nothing but the
/// position, so an Eval the writer may not see, an Eval that does not
/// exist and a run missing from a visible Eval read the same.
pub(crate) const RUN_UNKNOWN_HINT: &str = "no run with this run_id in that Eval, as far as the writer may see; \
     a run is always in the hub, so name one that has a row (archived and deleted runs count)";

/// One `core/uses_eval` edge of a Card version that resolved to a version
/// of an Eval record.
pub(crate) struct UsesEval {
    /// The Eval record the edge's target version belongs to.
    pub eval_record_id: Uuid,
    /// `attrs.runs`, when it is an array: each element and its position.
    /// A non-string element is kept as `None`, which no run matches.
    /// `None` when the edge carries no `attrs.runs` array.
    pub runs: Option<Vec<Option<String>>>,
    /// JSON pointer of `attrs.runs` for error entries, when this edge's
    /// runs are to be reported (`/relations/{j}/attrs/runs`, or
    /// `/attrs/runs` for an edge being added). `None` for edges already
    /// stored, whose runs were checked when they were written and cannot
    /// have lost their rows since (a run row is never deleted).
    pub runs_path: Option<String>,
}

impl UsesEval {
    /// Read `attrs.runs` out of an edge's attributes.
    pub(crate) fn runs_of(attrs: Option<&Value>) -> Option<Vec<Option<String>>> {
        let list = attrs?.get("runs")?.as_array()?;
        Some(list.iter().map(|v| v.as_str().map(str::to_owned)).collect())
    }
}

/// A fixed used set: `run_id` → the run's `content_hash` now.
pub(crate) type UsedSet = BTreeMap<String, Vec<u8>>;

/// Take `FOR SHARE` on each of `records`' rows, in id order, and return
/// the ones a writer covering `readable_ns` may see *under the lock*. See
/// the module doc for why this order and this strength, and for why the
/// visibility is read again here.
pub(crate) async fn lock_evals(
    tx: &mut Transaction<'_, Postgres>,
    records: &BTreeSet<Uuid>,
    readable_ns: &[String],
) -> Result<BTreeSet<Uuid>, StoreError> {
    if records.is_empty() {
        return Ok(BTreeSet::new());
    }
    let ids: Vec<Uuid> = records.iter().copied().collect();
    let rows = sqlx::query!(
        "SELECT id, ns, visibility FROM records WHERE id = ANY($1) ORDER BY id FOR SHARE",
        &ids,
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| crate::relations::visible_to(&r.visibility, &r.ns, readable_ns))
        .map(|r| r.id)
        .collect())
}

/// Fix the used set of every Eval record among `uses`, per the module
/// doc. A reported `attrs.runs` element with no row is pushed onto
/// `errors` as `run_unknown`; the used set then holds only the runs that
/// have rows. Records with no edge in `uses` are absent from the result.
///
/// Cost: one read per Eval record (the named runs, or its live runs).
pub(crate) async fn fix(
    tx: &mut Transaction<'_, Postgres>,
    uses: &[UsesEval],
    errors: &mut Vec<ErrorEntry>,
) -> Result<BTreeMap<Uuid, UsedSet>, StoreError> {
    let mut by_record: BTreeMap<Uuid, Vec<&UsesEval>> = BTreeMap::new();
    for u in uses {
        by_record.entry(u.eval_record_id).or_default().push(u);
    }
    let mut out = BTreeMap::new();
    for (record_id, edges) in by_record {
        let explicit: Vec<&UsesEval> = edges.iter().copied().filter(|u| u.runs.is_some()).collect();
        let set: UsedSet = if explicit.is_empty() {
            sqlx::query!(
                "SELECT run_id, content_hash FROM runs
                 WHERE record_id = $1 AND archived_at IS NULL AND tombstoned_at IS NULL",
                record_id,
            )
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .map(|r| (r.run_id, r.content_hash))
            .collect()
        } else {
            let named: BTreeSet<String> = explicit
                .iter()
                .flat_map(|u| u.runs.iter().flatten().flatten().cloned())
                .collect();
            let named: Vec<String> = named.into_iter().collect();
            let found: UsedSet = sqlx::query!(
                "SELECT run_id, content_hash FROM runs WHERE record_id = $1 AND run_id = ANY($2)",
                record_id,
                &named,
            )
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .map(|r| (r.run_id, r.content_hash))
            .collect();
            for u in &explicit {
                let (Some(runs), Some(path)) = (&u.runs, &u.runs_path) else {
                    continue;
                };
                for (k, run_id) in runs.iter().enumerate() {
                    if !run_id.as_ref().is_some_and(|id| found.contains_key(id)) {
                        errors.push(run_unknown(format!("{path}/{k}")));
                    }
                }
            }
            found
        };
        out.insert(record_id, set);
    }
    Ok(out)
}

/// One `run_results[]` element, as far as the used-set check needs it.
pub(crate) struct RunResultRef<'a> {
    /// Position in `run_results[]`.
    pub index: usize,
    /// The Eval record `eval` resolved to through the Card's resolved
    /// `core/uses_eval` edges, or `None` when it resolved to nothing.
    pub eval_record_id: Option<Uuid>,
    /// `run_results[].run_id`.
    pub run_id: &'a str,
}

/// Check every `run_results[]` element against the used sets: an `eval`
/// that resolved to nothing, or a run with no row, is `run_unknown`; a run
/// with a row that is not in the used set is `run_not_in_used_set`. Both
/// at `/run_results/{i}/run_id`.
///
/// Cost: one read per Eval record with elements outside its used set.
pub(crate) async fn check_run_results(
    tx: &mut Transaction<'_, Postgres>,
    items: &[RunResultRef<'_>],
    used: &BTreeMap<Uuid, UsedSet>,
    errors: &mut Vec<ErrorEntry>,
) -> Result<(), StoreError> {
    // Elements outside their used set, grouped by record: they are either
    // rows the Card did not use, or not rows at all.
    let mut outside: BTreeMap<Uuid, Vec<&RunResultRef<'_>>> = BTreeMap::new();
    for item in items {
        match item.eval_record_id {
            None => errors.push(run_unknown(run_id_path(item.index))),
            Some(record_id) => {
                let inside = used
                    .get(&record_id)
                    .is_some_and(|set| set.contains_key(item.run_id));
                if !inside {
                    outside.entry(record_id).or_default().push(item);
                }
            }
        }
    }
    for (record_id, items) in outside {
        let ids: Vec<String> = items.iter().map(|i| i.run_id.to_owned()).collect();
        let rows: HashSet<String> = sqlx::query_scalar!(
            "SELECT run_id FROM runs WHERE record_id = $1 AND run_id = ANY($2)",
            record_id,
            &ids,
        )
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .collect();
        for item in items {
            if rows.contains(item.run_id) {
                errors.push(ErrorEntry {
                    path: run_id_path(item.index),
                    code: ErrorCode::RunNotInUsedSet,
                    hint: Some(
                        "the run is not in this Card's used set for that Eval: name it in the \
                         core/uses_eval edge's attrs.runs, or leave attrs.runs out to use every \
                         live run"
                            .to_string(),
                    ),
                });
            } else {
                errors.push(run_unknown(run_id_path(item.index)));
            }
        }
    }
    Ok(())
}

/// Record the used sets of `card_version_id`: one `card_eval_runs` row per
/// used run, with its content hash now. A row that already exists keeps
/// the hash it was recorded with (a used set only grows; see
/// [`crate::relations::add`]).
pub(crate) async fn insert(
    tx: &mut Transaction<'_, Postgres>,
    card_version_id: Uuid,
    used: &BTreeMap<Uuid, UsedSet>,
) -> Result<(), StoreError> {
    for (record_id, set) in used {
        if set.is_empty() {
            continue;
        }
        let (ids, hashes): (Vec<String>, Vec<Vec<u8>>) =
            set.iter().map(|(id, h)| (id.clone(), h.clone())).unzip();
        sqlx::query!(
            "INSERT INTO card_eval_runs (card_version_id, eval_record_id, run_id, content_hash)
             SELECT $1, $2, i, h FROM UNNEST($3::text[], $4::bytea[]) AS t (i, h)
             ON CONFLICT (card_version_id, eval_record_id, run_id) DO NOTHING",
            card_version_id,
            *record_id,
            &ids,
            &hashes,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// One recorded used run, read back with what it is now.
#[derive(Debug, Clone)]
pub(crate) struct UsedRun {
    pub run_id: String,
    /// `card_eval_runs.content_hash`: the hash when the Card used it.
    pub posted: Vec<u8>,
    /// `runs.content_hash` now.
    pub current: Vec<u8>,
    /// The run's `harness` fingerprint, if it has one.
    pub harness: Option<Vec<u8>>,
    /// The run's `model` fingerprint, if it has one.
    pub model: Option<Vec<u8>>,
}

/// The recorded used sets of `card_version_ids` for the Eval record
/// `eval_record_id`, keyed by Card version, each sorted by `run_id`. A
/// version with no rows is absent. One statement, on `conn`, so that a
/// caller reading more than this can put it in one snapshot
/// ([`crate::runs::project`]).
pub(crate) async fn read(
    conn: &mut sqlx::PgConnection,
    card_version_ids: &[Uuid],
    eval_record_id: Uuid,
) -> Result<BTreeMap<Uuid, Vec<UsedRun>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.card_version_id, c.run_id, c.content_hash AS posted, u.content_hash AS current,
                  (SELECT f.fingerprint FROM run_fingerprints f
                   WHERE f.record_id = c.eval_record_id AND f.run_id = c.run_id
                     AND f.facet = 'harness') AS harness,
                  (SELECT f.fingerprint FROM run_fingerprints f
                   WHERE f.record_id = c.eval_record_id AND f.run_id = c.run_id
                     AND f.facet = 'model') AS model
           FROM card_eval_runs c
           JOIN runs u ON u.record_id = c.eval_record_id AND u.run_id = c.run_id
           WHERE c.card_version_id = ANY($1) AND c.eval_record_id = $2
           ORDER BY c.card_version_id, c.run_id"#,
        card_version_ids,
        eval_record_id,
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out: BTreeMap<Uuid, Vec<UsedRun>> = BTreeMap::new();
    for r in rows {
        out.entry(r.card_version_id).or_default().push(UsedRun {
            run_id: r.run_id,
            posted: r.posted,
            current: r.current,
            harness: r.harness,
            model: r.model,
        });
    }
    Ok(out)
}

/// What a recorded used set says now: see [`crate::runs::ProjectedCard`]
/// and [`crate::relations::ComparisonRow`], which carry these fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Summary {
    pub runs_used: i64,
    pub used_set_hash: Vec<u8>,
    pub posted_used_set_hash: Vec<u8>,
    pub changed_since_card: Vec<String>,
}

/// Summarise a recorded used set. `used_set_hash` is
/// `evalhub_core::run::runs_hash` over `(run_id, runs.content_hash now)`,
/// `posted_used_set_hash` the same over the hashes recorded at posting
/// time; they differ exactly when `changed_since_card` is not empty. The
/// empty set hashes as `[]`.
pub(crate) fn summarise(runs: &[UsedRun]) -> Result<Summary, StoreError> {
    let current: Vec<(&str, evalhub_core::ContentHash)> = runs
        .iter()
        .map(|r| (r.run_id.as_str(), content_hash(&r.current)))
        .collect();
    let posted: Vec<(&str, evalhub_core::ContentHash)> = runs
        .iter()
        .map(|r| (r.run_id.as_str(), content_hash(&r.posted)))
        .collect();
    Ok(Summary {
        runs_used: i64::try_from(runs.len()).unwrap_or(i64::MAX),
        used_set_hash: evalhub_core::run::runs_hash(&current)?.as_bytes().to_vec(),
        posted_used_set_hash: evalhub_core::run::runs_hash(&posted)?.as_bytes().to_vec(),
        changed_since_card: runs
            .iter()
            .filter(|r| r.posted != r.current)
            .map(|r| r.run_id.clone())
            .collect(),
    })
}

/// A stored digest as a [`evalhub_core::ContentHash`]. The columns always
/// hold 32 bytes written by this crate; a shorter value would be a bug and
/// is zero-padded rather than failing a read (as `runs_hash` is recomputed
/// in [`crate::runs`]).
fn content_hash(bytes: &[u8]) -> evalhub_core::ContentHash {
    let mut out = [0u8; 32];
    let n = bytes.len().min(32);
    out[..n].copy_from_slice(&bytes[..n]);
    evalhub_core::ContentHash::from_bytes(out)
}

/// Sort entries the way every rejection in this crate is sorted.
pub(crate) fn sort_entries(errors: &mut [ErrorEntry]) {
    errors.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| (a.code as u8).cmp(&(b.code as u8)))
    });
}

fn run_id_path(index: usize) -> String {
    format!("/run_results/{index}/run_id")
}

fn run_unknown(path: String) -> ErrorEntry {
    ErrorEntry {
        path,
        code: ErrorCode::RunUnknown,
        hint: Some(RUN_UNKNOWN_HINT.to_string()),
    }
}
