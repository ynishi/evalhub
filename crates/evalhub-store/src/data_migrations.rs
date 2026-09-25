//! One-shot data migrations: steps that must run once, after the SQL
//! migrations, and that need Rust because what they write is derived by
//! `evalhub_core` (content hashes, fingerprints, the 1.0 → 2.0 split).
//!
//! # Where they run
//!
//! [`crate::pool::migrate`] applies the SQL migrations ([`crate::MIGRATOR`])
//! and then every data migration in [`ALL`] whose name has no row in
//! `data_migrations (name, applied_at)`, in the order of [`ALL`]:
//!
//! ```text
//! evalhub migrate ──▶ MIGRATOR.run            one transaction per SQL file (sqlx)
//!                 └─▶ per data migration:     one transaction each
//!                       INSERT data_migrations (name) ON CONFLICT DO NOTHING
//!                         no row inserted  → already applied: roll back, next
//!                       the step itself
//!                       COMMIT                 the row and the data land together
//! ```
//!
//! sqlx runs every SQL file in a transaction of its own, so a DDL file and
//! a data step cannot share one; each data step is atomic on its own
//! instead, and its bookkeeping row is written in the same transaction as
//! its data. A step that fails, or that refuses what it finds
//! ([`StoreError::DataMigrationRefused`]), rolls back whole: no data and no
//! row, so the database is as it was and the step is still pending.
//!
//! Inserting the row *first* is also the lock: a second `evalhub migrate`
//! started while the first is inside the step blocks on the primary key
//! until the first commits, then finds the row and applies nothing. Two
//! runs never apply a step twice. A second run after the first has
//! finished applies nothing either; that is what the row is for.
//!
//! [`crate::pool::pending_migrations`] reports a data migration without a
//! row as pending, next to the SQL migrations the database has not
//! applied. `evalhub serve` refuses to start while anything is pending, so
//! a database with the `0003` DDL applied and the data step not yet run is
//! never served.
//!
//! Test databases are made by the same [`crate::pool::migrate`], so every
//! data step runs in every test database; on an empty database it finds
//! nothing and only writes its row.
//!
//! # `0003_runs_split` (release 0.2.0)
//!
//! Release 0.1.x stored an Eval as one `evalhub.eval/1.0` body with its
//! runs inside (`runs[]`). Release 0.2.0 stores the header as the version
//! body (`evalhub.eval/2.0`) and each run as a row of `runs`
//! ([`crate::runs`]). This step moves stored 1.0 bodies to that shape.
//! Every conversion goes through `evalhub_core::eval::split_v1`, the one
//! implementation that the 1.0 ingest ([`crate::records::ingest`]) uses
//! too, so a run migrated here and the same run posted as 1.0 to 0.2.0 are
//! the same bytes with the same hash.
//!
//! Per Eval record, in `records.id` order, under the record row's
//! `FOR UPDATE` (the lock every header and run write takes):
//!
//! ```text
//! 1. collect   runs[] of every live version declaring 1.0, by run_id;
//!              the highest seq wins. A tombstoned version has no body and
//!              contributes nothing.
//! 2. convert   each winner as split_v1 converted it with the header of the
//!              version it was taken from (status from outcome, unrecorded
//!              error.kind / skip_reason, meta.v0.2.0_migration.outcome,
//!              that header's facets materialised, the header attachments
//!              its calls / artifacts name copied into it).
//!              Every run_id of every such version must pass the stage 1
//!              format (evalhub_core::validate::is_valid_run_id); one that
//!              does not, or an element with no string run_id, stops the
//!              whole migration with every offender listed.
//! 3. write     runs, run_metrics, run_attachment_refs, run_fingerprints
//!              (the rows crate::runs writes, by the same helpers);
//!              records.runs_hash over them.
//! 4. rewrite   every live 1.0 version body := split_v1's header (no runs,
//!              schema evalhub.eval/2.0, canonical form); content_hash
//!              recomputed; audit migration.runs_split
//!              { version_id, old_content_hash, new_content_hash },
//!              subject eval/{ns}/{name}@{seq}, no actor.
//! 5. Cards     for every Card version with core/uses_eval edges resolved
//!              into this Eval: card_eval_runs by the stage 5 rule
//!              (crate::used_set), applied to the versions the edges point
//!              at (below).
//! ```
//!
//! Nothing else about a version changes. Its `version_id`, `seq`, label,
//! `changed[]`, badges, fingerprints, results, relations and attachment
//! references stay: the facets are the same keys with the same values, the
//! header keeps its `attachments[]` (the runs' entries are copied, not
//! moved), and `changed[]` records what differed when the version was
//! posted, which is still true. Generated facet columns follow the body.
//! Two live 1.0 versions that differed only in `runs[]` have the same
//! header, and so the same `content_hash`, after the split; nothing
//! constrains `(record_id, content_hash)` to be unique (`0001_init.sql`),
//! so both are rewritten and the migration does not fail on it.
//! A live version that already declares 2.0 is left alone and writes no
//! audit row; 0.1.x never stored one, so on a real 0.1.x database every
//! live Eval version is rewritten.
//!
//! Rewriting a stored body is the one exception to the rule that a
//! version is write-once (see the crate doc). The new `content_hash` is
//! the hash of the new body, so the invariant "`content_hash` is the
//! sha256 of the stored canonical body" holds after the step as before
//! it, and the audit row maps each old hash to its new one for anyone who
//! recorded the old value outside the hub.
//!
//! ## The Cards (step 5)
//!
//! A 0.1.x Card judged the runs inside the Eval *version* its
//! `core/uses_eval` edge points at. Its used set is rebuilt from that
//! version, so that a run which has changed since shows as changed from
//! the moment the migration commits:
//!
//! ```text
//! edges of Card version C into versions of this Eval
//!   edge to a tombstoned version          → ignored (contributes nothing,
//!                                            its attrs.runs included)
//!   pointed-at runs                       = runs of the live versions the
//!                                            remaining edges point at, each
//!                                            hashed as converted with *that*
//!                                            version's header; a run_id in
//!                                            two pointed-at versions takes
//!                                            the higher seq's
//!   any remaining edge carries attrs.runs → used = ∪ attrs.runs (strings)
//!       run_id in the pointed-at runs     → that hash
//!       else a row of the Eval (step 3)   → the row's hash
//!       else                              → skipped; one audit row per
//!                                            (Card version, Eval):
//!                                            migration.card_runs_unmatched
//!                                            { card_version_id, eval, run_ids }
//!   none does                             → used = every pointed-at run
//! ```
//!
//! The second branch covers an `attrs.runs` id that is a run of the Eval
//! (some live version had it, so step 3 wrote its row) but of none of the
//! versions the Card points at: 0.1.x let a Card name such a run. Stage 5
//! requires every named run to have a row and records the row's hash, and
//! the migration does the same, so the id is kept in the used set with the
//! *current* row's hash. It therefore never appears in the Card's
//! `changed_since_card` (there is no older content the Card is known to
//! have judged), and it writes no audit row, because it matched.
//!
//! An `attrs.runs` element that is not a string (a number, an object,
//! `null`) names no run and is dropped without an audit row. The 0.2.0
//! ingest and `POST …/relations` refuse such an element (`run_unknown`),
//! so only 0.1.x data can carry one.
//!
//! The hash recorded is what the Card judged, which is not necessarily
//! the row: a run that a later Eval version overwrote has the old hash in
//! `card_eval_runs` and the new one in `runs`, and lands in the Card's
//! `changed_since_card`. That is the point: the Card measured the run as
//! it was, and it is no longer that.
//!
//! 0.1.x never checked `attrs.runs` against the body, so an id with no run
//! is ordinary in stored Cards; it is skipped and recorded, and does not
//! fail the migration. Neither does an edge to a tombstoned version. Every
//! Card version is considered, not only the latest, because each version
//! owns its used set. Card bodies are not rewritten: `evalhub.card/1.0` is
//! still valid in 0.2.0.
//!
//! The audit row for unmatched ids (one per Card version and Eval, listing
//! all of that pair's unmatched ids) is written under the Card's
//! namespace, with subject `card/{ns}/{name}@{seq}`: the ids and the
//! Eval's name are the Card's own text (its `attrs.runs` and its edge), so
//! the row tells the Card's owner nothing the Card does not already say.
//!
//! ## Cost
//!
//! One pass over the Eval records, holding one record's live version
//! bodies (and their converted runs) in memory at a time; the Cards of an
//! Eval are read in one statement per Eval. The whole step is one
//! transaction, so a large database holds the Eval rows locked until it
//! commits, which is why the hosting runbook (`docs/hosting.md`) stops the
//! serving Machine for the 0.2.0 deploy.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use evalhub_core::eval::{EvalSchema, SplitV1, declared_schema, split_v1};
use evalhub_core::run::run_content_hash;
use evalhub_core::validate::{MAX_RUN_ID_BYTES, is_valid_run_id};

use crate::audit::{NewAudit, append};
use crate::error::StoreError;
use crate::records::Actor;
use crate::relations::USES_EVAL;
use crate::used_set::{UsedSet, UsesEval};

/// The name of the 0.1.x → 0.2.0 run split, as recorded in
/// `data_migrations.name`. See the module doc.
pub const RUNS_SPLIT: &str = "0003_runs_split";

/// Every data migration, in the order [`crate::pool::migrate`] applies
/// them. A name is never reused or removed: the row it leaves in
/// `data_migrations` is what keeps it from running again.
pub const ALL: &[&str] = &[RUNS_SPLIT];

/// A data migration that [`crate::pool::migrate`] applied in this call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Its name, one of [`ALL`].
    pub name: &'static str,
    /// What it did, in one line for the operator (counts of what it
    /// converted, wrote and skipped).
    pub summary: String,
}

/// What `0003_runs_split` did. Every count is over the whole database.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunsSplitStats {
    /// Eval records that had at least one live 1.0 version.
    pub evals: u64,
    /// Live version bodies rewritten (one `migration.runs_split` audit row
    /// each).
    pub versions_rewritten: u64,
    /// Run rows written.
    pub runs: u64,
    /// (Card version, Eval) pairs that got at least one `card_eval_runs`
    /// row. A Card version using two migrated Evals counts twice.
    pub card_versions: u64,
    /// `card_eval_runs` rows written.
    pub card_eval_runs: u64,
    /// (Card version, Eval) pairs with used `run_id`s that have no row
    /// (one `migration.card_runs_unmatched` audit row each).
    pub card_versions_unmatched: u64,
}

impl std::fmt::Display for RunsSplitStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} Eval(s), {} version body(ies) rewritten, {} run row(s); \
             {} (Card version, Eval) pair(s) with {} used-run row(s), \
             {} with unmatched run ids",
            self.evals,
            self.versions_rewritten,
            self.runs,
            self.card_versions,
            self.card_eval_runs,
            self.card_versions_unmatched,
        )
    }
}

/// Apply every data migration in [`ALL`] that has no `data_migrations`
/// row, each in its own transaction with its row, and return the ones
/// applied here (empty when all had already run). Called by
/// [`crate::pool::migrate`] after the SQL migrations, which create the
/// table.
///
/// Errors: [`StoreError::DataMigrationRefused`] when a step refuses its
/// data, [`StoreError::Query`] (and the other variants a write can raise)
/// otherwise. A failed step leaves no trace; the ones before it in the
/// same call stay applied.
pub(crate) async fn run_pending(pool: &PgPool) -> Result<Vec<Applied>, StoreError> {
    let mut applied = Vec::new();
    for &name in ALL {
        let mut tx = pool.begin().await?;
        // The row first: it is both the record that the step ran and the
        // lock against a concurrent `migrate` (see the module doc).
        let claimed = sqlx::query!(
            "INSERT INTO data_migrations (name) VALUES ($1) ON CONFLICT (name) DO NOTHING",
            name,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if !claimed {
            tx.rollback().await?;
            continue;
        }
        let summary = apply(&mut tx, name).await?;
        tx.commit().await?;
        tracing::info!(migration = name, %summary, "data migration applied");
        applied.push(Applied { name, summary });
    }
    Ok(applied)
}

/// The names in [`ALL`] that have no `data_migrations` row, in order. All
/// of them when the table does not exist yet (the SQL migration that
/// creates it is itself pending). Read-only.
pub(crate) async fn pending(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<&'static str>, StoreError> {
    let table: bool =
        sqlx::query_scalar("SELECT to_regclass('data_migrations') IS NOT NULL AS present")
            .fetch_one(&mut *conn)
            .await?;
    if !table {
        return Ok(ALL.to_vec());
    }
    let done: BTreeSet<String> = sqlx::query_scalar!("SELECT name FROM data_migrations")
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .collect();
    Ok(ALL.iter().copied().filter(|n| !done.contains(*n)).collect())
}

/// Run the step called `name` inside `tx` and describe what it did.
async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    name: &'static str,
) -> Result<String, StoreError> {
    match name {
        RUNS_SPLIT => Ok(runs_split(tx).await?.to_string()),
        // `ALL` and this match are edited together; a name without an arm
        // is a bug, and failing keeps its row from being written.
        other => Err(StoreError::DataMigrationRefused {
            name: other,
            problems: vec![format!("no implementation for data migration {other}")],
        }),
    }
}

/// A live 1.0 version of the Eval being migrated, split.
struct Source {
    version_id: Uuid,
    seq: i32,
    old_hash: Vec<u8>,
    split: SplitV1,
}

impl Source {
    /// `run_id` → content hash of each run of this version, as converted
    /// with this version's header.
    fn run_hashes(&self) -> Result<BTreeMap<String, [u8; 32]>, StoreError> {
        self.split
            .runs
            .iter()
            .map(|(id, run)| Ok((id.clone(), *run_content_hash(id, run)?.as_bytes())))
            .collect()
    }
}

/// `0003_runs_split`. See the module doc for the steps.
async fn runs_split(tx: &mut Transaction<'_, Postgres>) -> Result<RunsSplitStats, StoreError> {
    let mut stats = RunsSplitStats::default();
    let mut problems: Vec<String> = Vec::new();

    let evals =
        sqlx::query!("SELECT id, ns, name FROM records WHERE type = 'eval' ORDER BY id FOR UPDATE")
            .fetch_all(&mut **tx)
            .await?;

    for eval in evals {
        let versions = sqlx::query!(
            "SELECT version_id, seq, content_hash, body FROM versions
             WHERE record_id = $1 AND tombstoned_at IS NULL
             ORDER BY seq",
            eval.id,
        )
        .fetch_all(&mut **tx)
        .await?;

        // Step 1 and the run_id check. Every live version is a pointed-at
        // candidate for step 5; only the 1.0 ones have runs.
        let mut live: HashMap<Uuid, Option<usize>> = HashMap::new();
        let mut sources: Vec<Source> = Vec::new();
        for v in versions {
            let Some(body) = v.body else {
                continue;
            };
            if declared_schema(&body) != Some(EvalSchema::V1) {
                live.insert(v.version_id, None);
                continue;
            }
            let at = format!("eval/{}/{}@{}", eval.ns, eval.name, v.seq);
            check_run_ids(&at, &body, &mut problems);
            let split = split_v1(&body);
            if !split.duplicate_run_ids.is_empty() {
                // 0.1.x accepted a repeated run_id; the last element wins,
                // as at the 1.0 ingest. Not a reason to stop.
                tracing::warn!(version = %at, run_ids = ?split.duplicate_run_ids,
                    "repeated run_id in runs[]; the last element was kept");
            }
            live.insert(v.version_id, Some(sources.len()));
            sources.push(Source {
                version_id: v.version_id,
                seq: v.seq,
                old_hash: v.content_hash,
                split,
            });
        }
        // Once anything is refused nothing will be committed; keep reading
        // so the report names every offender, but write nothing more.
        if !problems.is_empty() || sources.is_empty() {
            continue;
        }
        stats.evals += 1;

        // Steps 1–3: the winners (highest seq), as rows.
        let mut winners: BTreeMap<&str, &Value> = BTreeMap::new();
        for s in &sources {
            for (id, run) in &s.split.runs {
                winners.insert(id.as_str(), run);
            }
        }
        let mut rows: BTreeMap<String, [u8; 32]> = BTreeMap::new();
        for (&run_id, &run) in &winners {
            let mut errors = Vec::new();
            let attachments = crate::runs::decode_attachments(run, &mut errors);
            if !errors.is_empty() {
                problems.push(format!(
                    "eval/{}/{}: run {run_id:?} names an attachment whose sha256 is not 64 \
                     hexadecimal characters",
                    eval.ns, eval.name
                ));
                continue;
            }
            let checked = crate::runs::prepare(run_id, run.clone(), attachments)?;
            crate::runs::upsert(tx, eval.id, &checked).await?;
            rows.insert(checked.run_id.clone(), checked.content_hash);
            stats.runs += 1;
        }
        if !problems.is_empty() {
            continue;
        }
        if !rows.is_empty() {
            crate::runs::recompute_runs_hash(tx, eval.id).await?;
        }

        // Step 4: the headers.
        for s in &sources {
            let (bytes, hash) = evalhub_core::canonical::hash_value(&s.split.header)?;
            let body: Value = serde_json::from_slice(&bytes).map_err(|e| {
                StoreError::Canonical(evalhub_core::canonical::CanonicalError::Serialize(e))
            })?;
            let new_hash: &[u8] = hash.as_bytes();
            sqlx::query!(
                "UPDATE versions SET body = $2, content_hash = $3 WHERE version_id = $1",
                s.version_id,
                body,
                new_hash,
            )
            .execute(&mut **tx)
            .await?;
            let subject = format!("eval/{}/{}@{}", eval.ns, eval.name, s.seq);
            append(
                tx,
                NewAudit {
                    actor: Actor::default(),
                    ns: Some(&eval.ns),
                    action: "migration.runs_split",
                    subject: Some(&subject),
                    detail: Some(serde_json::json!({
                        "version_id": s.version_id,
                        "old_content_hash": hex::encode(&s.old_hash),
                        "new_content_hash": hash.as_hex(),
                    })),
                },
            )
            .await?;
            stats.versions_rewritten += 1;
        }

        // Step 5: the Cards.
        let edges = sqlx::query!(
            r#"SELECT rel.from_version_id, rel.to_version_id AS "to_version_id!", rel.attrs,
                      cr.ns AS card_ns, cr.name AS card_name, cv.seq AS card_seq
               FROM relations rel
               JOIN versions tv ON tv.version_id = rel.to_version_id
               JOIN versions cv ON cv.version_id = rel.from_version_id
               JOIN records cr ON cr.id = cv.record_id
               WHERE rel.type = $1 AND tv.record_id = $2 AND cr.type = 'card'
               ORDER BY rel.from_version_id"#,
            USES_EVAL,
            eval.id,
        )
        .fetch_all(&mut **tx)
        .await?;
        struct CardEdges {
            subject: String,
            ns: String,
            to: Vec<(Uuid, Option<Value>)>,
        }
        let mut cards: BTreeMap<Uuid, CardEdges> = BTreeMap::new();
        for e in edges {
            cards
                .entry(e.from_version_id)
                .or_insert_with(|| CardEdges {
                    subject: format!("card/{}/{}@{}", e.card_ns, e.card_name, e.card_seq),
                    ns: e.card_ns.clone(),
                    to: Vec::new(),
                })
                .to
                .push((e.to_version_id, e.attrs));
        }

        let mut hashes_of: HashMap<usize, BTreeMap<String, [u8; 32]>> = HashMap::new();
        for (card_version_id, card) in cards {
            // Edges into tombstoned versions contribute nothing.
            let live_edges: Vec<(Option<usize>, Option<&Value>)> = card
                .to
                .iter()
                .filter_map(|(to, attrs)| live.get(to).map(|src| (*src, attrs.as_ref())))
                .collect();
            if live_edges.is_empty() {
                continue;
            }
            // The pointed-at runs, each hashed with its own version's
            // header; the higher seq wins between two pointed-at versions.
            let mut pointed_sources: Vec<usize> =
                live_edges.iter().filter_map(|(src, _)| *src).collect();
            pointed_sources.sort_by_key(|&i| sources[i].seq);
            pointed_sources.dedup();
            let mut pointed: BTreeMap<String, [u8; 32]> = BTreeMap::new();
            for i in pointed_sources {
                let h = match hashes_of.entry(i) {
                    std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(sources[i].run_hashes()?)
                    }
                };
                pointed.extend(h.iter().map(|(k, v)| (k.clone(), *v)));
            }

            let explicit: Vec<Vec<Option<String>>> = live_edges
                .iter()
                .filter_map(|(_, attrs)| UsesEval::runs_of(*attrs))
                .collect();
            let mut used: UsedSet = UsedSet::new();
            let mut unmatched: Vec<String> = Vec::new();
            if explicit.is_empty() {
                used.extend(pointed.into_iter().map(|(k, v)| (k, v.to_vec())));
            } else {
                let named: BTreeSet<String> = explicit.into_iter().flatten().flatten().collect();
                for id in named {
                    if let Some(h) = pointed.get(&id).or_else(|| rows.get(&id)) {
                        used.insert(id, h.to_vec());
                    } else {
                        unmatched.push(id);
                    }
                }
            }
            if !used.is_empty() {
                stats.card_versions += 1;
                stats.card_eval_runs += used.len() as u64;
                let mut by_record = BTreeMap::new();
                by_record.insert(eval.id, used);
                crate::used_set::insert(tx, card_version_id, &by_record).await?;
            }
            if !unmatched.is_empty() {
                stats.card_versions_unmatched += 1;
                append(
                    tx,
                    NewAudit {
                        actor: Actor::default(),
                        ns: Some(&card.ns),
                        action: "migration.card_runs_unmatched",
                        subject: Some(&card.subject),
                        detail: Some(serde_json::json!({
                            "card_version_id": card_version_id,
                            "eval": format!("{}/{}", eval.ns, eval.name),
                            "run_ids": unmatched,
                        })),
                    },
                )
                .await?;
            }
        }
    }

    if !problems.is_empty() {
        return Err(StoreError::DataMigrationRefused {
            name: RUNS_SPLIT,
            problems,
        });
    }
    Ok(stats)
}

/// Push a line onto `problems` for every `runs[]` element of the 1.0 body
/// `body` (of version `at`) whose `run_id` is missing, not a string, or
/// not a valid `run_id` in the stage 1 format. `split_v1` would drop an
/// element without a string `run_id`; the migration refuses instead of
/// losing a run silently.
fn check_run_ids(at: &str, body: &Value, problems: &mut Vec<String>) {
    let Some(runs) = body.get("runs").and_then(Value::as_array) else {
        return;
    };
    for (i, element) in runs.iter().enumerate() {
        match element.get("run_id").and_then(Value::as_str) {
            Some(id) if is_valid_run_id(id) => {}
            Some(id) => problems.push(format!(
                "{at}: runs[{i}].run_id {id:?} is not a valid run_id \
                 (non-empty, no '/', at most {MAX_RUN_ID_BYTES} bytes)"
            )),
            None => problems.push(format!("{at}: runs[{i}] has no string run_id")),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    #[test]
    fn check_run_ids_names_every_offender() {
        let long = "x".repeat(MAX_RUN_ID_BYTES + 1);
        let body = json!({
            "schema": "evalhub.eval/1.0",
            "runs": [
                {"run_id": "ok-1"},
                {"run_id": "a/b"},
                {"run_id": ""},
                {"run_id": long},
                {"run_id": 7},
                {},
            ],
        });
        let mut problems = Vec::new();
        check_run_ids("eval/n/e@1", &body, &mut problems);
        assert_eq!(problems.len(), 5, "{problems:#?}");
        assert!(problems[0].starts_with("eval/n/e@1: runs[1].run_id \"a/b\""));
        assert!(problems[3].contains("runs[4] has no string run_id"));
    }

    #[test]
    fn stats_read_as_one_line() {
        let s = RunsSplitStats {
            evals: 1,
            versions_rewritten: 2,
            runs: 3,
            card_versions: 4,
            card_eval_runs: 5,
            card_versions_unmatched: 6,
        };
        let line = s.to_string();
        assert!(!line.contains('\n'));
        assert!(line.starts_with("1 Eval(s), 2 version body(ies) rewritten, 3 run row(s)"));
    }
}
