//! Runs: the rows of an Eval record.
//!
//! An Eval is a header (its versions, [`crate::records`]) and a set of
//! runs. A run is not a version. It is written under a `run_id` the
//! producer chooses, overwritten in place by a later write of the same id,
//! archived, and deleted, and none of that appends a header version. The
//! header and the runs meet in three places: a run copies the facets it
//! omits from the latest live header when it is written, the record row is
//! the lock both take, and `records.runs_hash` is the one digest over every
//! run of the record.
//!
//! # Tables
//!
//! ```text
//! runs                (record_id, run_id)  status, error_kind, started_at, ended_at,
//!                                          body, content_hash, archived_at, tombstone
//! run_metrics         (record_id, run_id, metric)   value
//! run_attachment_refs (record_id, run_id, path)     sha256   ← counted by GC and downloads
//! run_fingerprints    (record_id, run_id, facet)    fingerprint
//! records.runs_hash                                 digest over every run of the record
//! ```
//!
//! # Write ordering
//!
//! [`put`] and [`put_batch`] share one implementation; the ingest of an
//! `evalhub.eval/1.0` body ([`crate::records::ingest`]) calls the same one
//! for the runs it split off. In one transaction:
//!
//! ```text
//! 1. lock the record row                       SELECT … FOR UPDATE (as the header ingest)
//!    and read the latest live header           none → RecordNotFound
//! 2. per element:
//!    a. run_id seen earlier in the batch?      batch_duplicate_run_id
//!    b. materialise omitted facets             evalhub_core::run::materialise(run, header)
//!    c. validate                               evalhub_core::validate::run (incl. run_id_invalid,
//!                                              run_id_mismatch, run_status_detail, …)
//!    d. decode attachments[].sha256            schema (not 64 hex characters)
//!    e. the id's row is tombstoned?            run_deleted
//!    f. every attachments[].sha256 `ready`?    attachment_missing (elements that passed a–e)
//!    any failure in any element → RunsRejected(every failing element), nothing written
//! 3. content hash                              evalhub_core::run::run_content_hash
//!    equal to the stored row's?                unchanged: no row written, no audit row
//! 4. upsert runs; replace run_metrics, run_attachment_refs, run_fingerprints
//! 5. recompute records.runs_hash               over every row, archived and tombstoned included
//! 6. one audit row per run created or updated  run.create / run.update
//! 7. commit
//! ```
//!
//! Step 1 is what keeps `runs_hash` equal to the rows: a header post and a
//! run write to the same Eval are serialised on the record row, and two
//! run writes are too. Different Evals do not contend. Because every
//! element is checked before anything is written (step 2) and the writes
//! share the transaction, a batch is all or nothing; the batch reports
//! every failing element rather than the first, so a producer fixes a batch
//! in one round trip, as it fixes a record.
//!
//! The run is stored as hashed: the body after materialisation, with
//! `run_id` set, in canonical form. `sha256(JCS(body))` of a stored run is
//! therefore its `content_hash`, and a client can check it without knowing
//! the formula's `run_id` rule.
//!
//! # Timestamps
//!
//! `started_at` and `ended_at` are copied from the body into
//! `timestamptz` columns for filtering and sorting. A body value that is
//! not an RFC 3339 date-time is accepted (the JSON Schema declares
//! `format: date-time`, but Draft 2020-12 formats are annotations, not
//! assertions, in the core validator), kept verbatim in the body, and
//! leaves the column NULL, so the run sorts and filters as one with no
//! timestamp. The content hash covers the string as sent.
//!
//! # Materialisation
//!
//! The facets a run omits are copied from the latest live header when the
//! run is written, and the copy is what is stored and hashed. A later
//! header post changes no run already written. See `evalhub_core::run`.
//!
//! # Outcomes
//!
//! Each element is `created` (no row), `updated` (a live row with another
//! content hash; `updated_at` moves, `created_at` stays) or `unchanged`
//! (same content hash; nothing is written). Overwriting an archived run
//! leaves it archived: archiving is the owner's decision about showing the
//! run, not a property of its content.
//!
//! # Archive and delete
//!
//! [`archive`] sets `archived_at`, [`unarchive`] clears it. An archived run
//! is kept whole and counted in `runs_hash`; it is left out of reads by
//! anyone who is not a member of the namespace ([`get`]) and out of the
//! summary counts ([`crate::records::runs_summary`]).
//!
//! [`tombstone`] deletes a run the way a version is tombstoned: `body` is
//! nulled, the reason and note are recorded, and `run_attachment_refs` are
//! deleted so the object's reference count drops and the GC may collect
//! it. `content_hash`, `run_metrics` and `run_fingerprints` stay, so
//! `runs_hash` does not change and a Card that used the run still has what
//! it judged against. A deleted `run_id` is never written again
//! (`run_deleted`): writing it would make the Cards that judged the old run
//! point at different content under the same name. Deleting twice is
//! [`StoreError::RunDeleted`].
//!
//! Neither archive, unarchive nor delete changes a hash; they change how a
//! run is shown and kept, not what it is.
//!
//! # No live header
//!
//! When every header version of the Eval is tombstoned the record has no
//! latest, and the runs are unreachable: every function here answers
//! [`StoreError::RecordNotFound`] (writes) or `None` (reads). No data is
//! changed; posting a new header version makes them reachable again.
//!
//! # Visibility
//!
//! Runs follow the record's `visibility`; they have none of their own.
//! [`get`] applies the same predicate as every read in
//! [`crate::records`], plus one rule of its own: an archived run is visible
//! to members of the namespace only. Writes do not filter (the server has
//! checked `write` on the namespace).
//!
//! # Audit
//!
//! `run.create` / `run.update` / `run.archive` / `run.unarchive` /
//! `run.delete`, subject `eval/{ns}/{name}/runs/{run_id}`, one row per run
//! that changed; an unchanged element or an archive of an archived run
//! writes none.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use evalhub_core::canonical::canonicalize;
use evalhub_core::run::{materialise, run_content_hash, runs_hash};
use evalhub_schema::error::{ErrorCode, ErrorEntry};

use crate::audit::{NewAudit, append};
use crate::error::{RunRejection, StoreError};
use crate::records::{
    Actor, RecordType, Tombstone, TombstoneReason, TombstoneRequest, Visibility, lock_record,
};

/// What a write did to one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunChange {
    /// There was no row for the id; one was inserted (`201` for a `PUT`).
    Created,
    /// A live row with another content hash was overwritten.
    Updated,
    /// The row already had this content hash; nothing was written.
    Unchanged,
}

impl RunChange {
    /// `created` / `updated` / `unchanged`, as the batch response spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            RunChange::Created => "created",
            RunChange::Updated => "updated",
            RunChange::Unchanged => "unchanged",
        }
    }
}

/// One run as a write left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunWritten {
    /// The id it is stored under.
    pub run_id: String,
    /// What the write did.
    pub change: RunChange,
    /// sha256 of the stored run (`evalhub_core::run::run_content_hash`).
    pub content_hash: Vec<u8>,
    /// `ok` / `error` / `skipped`.
    pub status: String,
    /// The row is archived. Only an overwrite of an archived run reports
    /// `true`; a write never archives or unarchives.
    pub archived: bool,
}

/// The result of [`put`] or [`put_batch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunsWritten {
    /// The Eval's `records.id`.
    pub record_id: Uuid,
    /// One entry per input element, in input order (exactly one for
    /// [`put`]).
    pub runs: Vec<RunWritten>,
    /// `records.runs_hash` after the write.
    pub runs_hash: Vec<u8>,
}

/// A run as read back.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredRun {
    /// The Eval's `records.id`.
    pub record_id: Uuid,
    /// The run's id within the Eval.
    pub run_id: String,
    /// `ok` / `error` / `skipped`.
    pub status: String,
    /// `error.kind` when `status` is `error`.
    pub error_kind: Option<String>,
    /// `started_at`, when the body carried a parseable one.
    pub started_at: Option<DateTime<Utc>>,
    /// `ended_at`, when the body carried a parseable one.
    pub ended_at: Option<DateTime<Utc>>,
    /// The stored run; `None` exactly when `tombstone` is `Some`.
    pub body: Option<Value>,
    /// sha256 of the stored run. Kept on a tombstone.
    pub content_hash: Vec<u8>,
    /// When the id was first written.
    pub created_at: DateTime<Utc>,
    /// When the content last changed (or the row was archived, unarchived
    /// or deleted).
    pub updated_at: DateTime<Utc>,
    /// When it was archived, if it is.
    pub archived_at: Option<DateTime<Utc>>,
    /// The tombstone, if the run was deleted.
    pub tombstone: Option<Tombstone>,
    /// The Eval's visibility at read time.
    pub visibility: Visibility,
}

/// One element to write: the `run_id` it goes under and the body as sent.
pub(crate) struct RunInput<'a> {
    pub run_id: &'a str,
    pub body: &'a Value,
}

/// Write one run of `eval/{ns}/{name}` under `run_id`, in one transaction.
/// See the module doc for the ordering. Returns the outcome with one entry.
///
/// Errors: [`StoreError::RecordNotFound`] when the Eval does not exist or
/// has no live header; [`StoreError::RunsRejected`] (one element, index 0)
/// for validation failures, `run_deleted` and `attachment_missing`;
/// [`StoreError::Query`] otherwise. Nothing is written on any error.
///
/// Cost: a record lock, one read of the latest header, a handful of
/// statements for the run and its side rows, and a read of every
/// `(run_id, content_hash)` of the record to recompute `runs_hash` when
/// the run changed.
pub async fn put(
    pool: &PgPool,
    ns: &str,
    name: &str,
    run_id: &str,
    body: &Value,
    actor: Actor,
) -> Result<RunsWritten, StoreError> {
    let mut tx = pool.begin().await?;
    let (record_id, header) = lock_with_header(&mut tx, ns, name).await?;
    let out = write_runs(
        &mut tx,
        record_id,
        ns,
        name,
        &header,
        &[RunInput { run_id, body }],
        actor,
    )
    .await?;
    tx.commit().await?;
    Ok(out)
}

/// Write several runs of `eval/{ns}/{name}`, all or nothing, in one
/// transaction under one record lock. Each element is a run body carrying
/// its own `run_id` (an element without a string `run_id` is checked under
/// the empty id, which fails `run_id_invalid`). The same checks and rules
/// as [`put`] apply to every element; in addition an id that appears more
/// than once is `batch_duplicate_run_id` on each later occurrence.
///
/// Errors: as [`put`]; [`StoreError::RunsRejected`] lists every failing
/// element with its index. The batch size limit is the server's.
pub async fn put_batch(
    pool: &PgPool,
    ns: &str,
    name: &str,
    runs: &[Value],
    actor: Actor,
) -> Result<RunsWritten, StoreError> {
    let inputs: Vec<RunInput<'_>> = runs
        .iter()
        .map(|body| RunInput {
            run_id: body.get("run_id").and_then(Value::as_str).unwrap_or(""),
            body,
        })
        .collect();
    let mut tx = pool.begin().await?;
    let (record_id, header) = lock_with_header(&mut tx, ns, name).await?;
    let out = write_runs(&mut tx, record_id, ns, name, &header, &inputs, actor).await?;
    tx.commit().await?;
    Ok(out)
}

/// Run `run_id` of `eval/{ns}/{name}` as `caller_namespaces` may see it.
///
/// `None` when the Eval does not exist, is private and `ns` is not among
/// `caller_namespaces`, has no live header, has no such run, or the run is
/// archived and the caller is not a member of `ns`. The cases are
/// deliberately indistinguishable. A deleted run is returned, with its
/// tombstone and `body: None`, to anyone who may see the Eval, unless it
/// is also archived (then to members only, as any archived run).
pub async fn get(
    pool: &PgPool,
    ns: &str,
    name: &str,
    run_id: &str,
    caller_namespaces: &[String],
) -> Result<Option<StoredRun>, StoreError> {
    let row = sqlx::query_as!(
        RunRow,
        "SELECT u.record_id, u.run_id, u.status, u.error_kind, u.started_at, u.ended_at,
                u.body, u.content_hash, u.created_at, u.updated_at, u.archived_at,
                u.tombstoned_at, u.tombstone_reason, u.tombstone_note, r.visibility
         FROM records r
         JOIN runs u ON u.record_id = r.id
         WHERE r.type = 'eval' AND r.ns = $1 AND r.name = $2 AND u.run_id = $3
           AND (r.visibility = 'public' OR r.ns = ANY($4))
           AND EXISTS (SELECT 1 FROM versions v
                       WHERE v.record_id = r.id AND v.tombstoned_at IS NULL)",
        ns,
        name,
        run_id,
        caller_namespaces,
    )
    .fetch_optional(pool)
    .await?;
    let member = caller_namespaces.iter().any(|n| n == ns);
    Ok(row
        .filter(|r| member || r.archived_at.is_none())
        .map(RunRow::into_stored))
}

/// Archive run `run_id` of `eval/{ns}/{name}`: keep it whole, hide it from
/// non-members and from the summary counts. Changes no hash. Archiving an
/// archived run changes nothing and writes no audit row. Returns the run.
///
/// Errors: [`StoreError::RecordNotFound`] (no Eval, or no live header),
/// [`StoreError::RunNotFound`], [`StoreError::RunDeleted`].
pub async fn archive(
    pool: &PgPool,
    ns: &str,
    name: &str,
    run_id: &str,
    actor: Actor,
) -> Result<StoredRun, StoreError> {
    set_archived(pool, ns, name, run_id, true, actor).await
}

/// Undo [`archive`]. Unarchiving a run that is not archived changes
/// nothing and writes no audit row. Errors as [`archive`].
pub async fn unarchive(
    pool: &PgPool,
    ns: &str,
    name: &str,
    run_id: &str,
    actor: Actor,
) -> Result<StoredRun, StoreError> {
    set_archived(pool, ns, name, run_id, false, actor).await
}

/// Delete run `run_id` of `eval/{ns}/{name}`: null the body, record the
/// reason and note, drop its `run_attachment_refs`. `content_hash`,
/// `run_metrics`, `run_fingerprints` and `archived_at` stay; `runs_hash`
/// does not change. Returns the run as it now reads.
///
/// Errors: [`StoreError::RecordNotFound`] (no Eval, or no live header),
/// [`StoreError::RunNotFound`], [`StoreError::RunDeleted`] when it already
/// is deleted.
pub async fn tombstone(
    pool: &PgPool,
    ns: &str,
    name: &str,
    run_id: &str,
    request: TombstoneRequest<'_>,
    actor: Actor,
) -> Result<StoredRun, StoreError> {
    let TombstoneRequest { reason, note } = request;
    let mut tx = pool.begin().await?;
    let (record_id, _) = lock_with_header(&mut tx, ns, name).await?;
    let current = load(&mut tx, record_id, run_id)
        .await?
        .ok_or(StoreError::RunNotFound)?;
    if current.tombstoned_at.is_some() {
        return Err(StoreError::RunDeleted);
    }
    sqlx::query!(
        "UPDATE runs
         SET tombstoned_at = now(), tombstone_reason = $3, tombstone_note = $4,
             body = NULL, updated_at = now()
         WHERE record_id = $1 AND run_id = $2",
        record_id,
        run_id,
        reason.as_str(),
        note,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "DELETE FROM run_attachment_refs WHERE record_id = $1 AND run_id = $2",
        record_id,
        run_id,
    )
    .execute(&mut *tx)
    .await?;
    let subject = subject(ns, name, run_id);
    append(
        &mut tx,
        NewAudit {
            actor,
            ns: Some(ns),
            action: "run.delete",
            subject: Some(&subject),
            detail: Some(serde_json::json!({
                "content_hash": hex::encode(&current.content_hash),
                "reason": reason.as_str(),
                "note": note,
            })),
        },
    )
    .await?;
    let run = load(&mut tx, record_id, run_id)
        .await?
        .ok_or(StoreError::RunNotFound)?;
    tx.commit().await?;
    Ok(run.into_stored())
}

async fn set_archived(
    pool: &PgPool,
    ns: &str,
    name: &str,
    run_id: &str,
    archived: bool,
    actor: Actor,
) -> Result<StoredRun, StoreError> {
    let mut tx = pool.begin().await?;
    let (record_id, _) = lock_with_header(&mut tx, ns, name).await?;
    let current = load(&mut tx, record_id, run_id)
        .await?
        .ok_or(StoreError::RunNotFound)?;
    if current.tombstoned_at.is_some() {
        return Err(StoreError::RunDeleted);
    }
    if current.archived_at.is_some() == archived {
        tx.commit().await?;
        return Ok(current.into_stored());
    }
    sqlx::query!(
        "UPDATE runs
         SET archived_at = CASE WHEN $3 THEN now() ELSE NULL END, updated_at = now()
         WHERE record_id = $1 AND run_id = $2",
        record_id,
        run_id,
        archived,
    )
    .execute(&mut *tx)
    .await?;
    let subject = subject(ns, name, run_id);
    append(
        &mut tx,
        NewAudit {
            actor,
            ns: Some(ns),
            action: if archived {
                "run.archive"
            } else {
                "run.unarchive"
            },
            subject: Some(&subject),
            detail: Some(serde_json::json!({
                "content_hash": hex::encode(&current.content_hash),
            })),
        },
    )
    .await?;
    let run = load(&mut tx, record_id, run_id)
        .await?
        .ok_or(StoreError::RunNotFound)?;
    tx.commit().await?;
    Ok(run.into_stored())
}

/// Lock `eval/{ns}/{name}` and read its latest live header body.
/// [`StoreError::RecordNotFound`] when either is missing.
async fn lock_with_header(
    tx: &mut Transaction<'_, Postgres>,
    ns: &str,
    name: &str,
) -> Result<(Uuid, Value), StoreError> {
    let (record_id, _) = lock_record(tx, RecordType::Eval, ns, name).await?;
    let header = sqlx::query_scalar!(
        "SELECT body FROM versions
         WHERE record_id = $1 AND tombstoned_at IS NULL
         ORDER BY seq DESC LIMIT 1",
        record_id,
    )
    .fetch_optional(&mut **tx)
    .await?
    .flatten()
    .ok_or(StoreError::RecordNotFound)?;
    Ok((record_id, header))
}

/// One element after the checks: everything the write needs.
struct Checked {
    run_id: String,
    /// Stored body: materialised, `run_id` set, canonical form.
    body: Value,
    content_hash: [u8; 32],
    status: String,
    error_kind: Option<String>,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    metrics: Vec<(String, f64)>,
    attachments: Vec<(String, [u8; 32])>,
    fingerprints: Vec<(&'static str, [u8; 32])>,
}

/// Steps 2–6 of the module doc for `inputs`, inside the caller's
/// transaction, which already holds the record lock. `header` is the body
/// facets are materialised from. Shared by [`put`], [`put_batch`] and the
/// `evalhub.eval/1.0` ingest in [`crate::records`].
pub(crate) async fn write_runs(
    tx: &mut Transaction<'_, Postgres>,
    record_id: Uuid,
    ns: &str,
    name: &str,
    header: &Value,
    inputs: &[RunInput<'_>],
    actor: Actor,
) -> Result<RunsWritten, StoreError> {
    // Existing rows for the ids in the batch, in one read.
    let ids: Vec<String> = inputs.iter().map(|i| i.run_id.to_owned()).collect();
    let existing: HashMap<String, (Vec<u8>, bool, bool)> = sqlx::query!(
        "SELECT run_id, content_hash, tombstoned_at IS NOT NULL AS \"tombstoned!\",
                archived_at IS NOT NULL AS \"archived!\"
         FROM runs WHERE record_id = $1 AND run_id = ANY($2)",
        record_id,
        &ids,
    )
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|r| (r.run_id, (r.content_hash, r.tombstoned, r.archived)))
    .collect();

    // Step 2: check every element, collecting every failure.
    let mut seen: HashSet<&str> = HashSet::new();
    let mut rejections: Vec<RunRejection> = Vec::new();
    let mut checked: Vec<Option<Checked>> = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        let mut errors: Vec<ErrorEntry> = Vec::new();
        let first = seen.insert(input.run_id);
        if !first && !input.run_id.is_empty() {
            errors.push(entry(
                "/run_id",
                ErrorCode::BatchDuplicateRunId,
                format!("run_id {:?} appears earlier in this batch", input.run_id),
            ));
        }
        let mut body = input.body.clone();
        materialise(&mut body, header);
        errors.extend(evalhub_core::validate::run(input.run_id, &body));
        let attachments = decode_attachments(&body, &mut errors);
        if existing
            .get(input.run_id)
            .is_some_and(|(_, tombstoned, _)| *tombstoned)
        {
            errors.push(entry(
                "/run_id",
                ErrorCode::RunDeleted,
                format!(
                    "run {:?} was deleted; a deleted run_id is not written again",
                    input.run_id
                ),
            ));
        }
        if errors.is_empty() {
            checked.push(Some(prepare(input.run_id, body, attachments)?));
        } else {
            checked.push(None);
            rejections.push(RunRejection {
                index,
                run_id: input.run_id.to_owned(),
                errors,
            });
        }
    }

    // Step 2f: attachments, in one read over every element that passed.
    let wanted: Vec<Vec<u8>> = checked
        .iter()
        .flatten()
        .flat_map(|c| c.attachments.iter().map(|(_, sha)| sha.to_vec()))
        .collect();
    if !wanted.is_empty() {
        let ready: HashSet<Vec<u8>> = sqlx::query_scalar!(
            "SELECT sha256 FROM attachments WHERE sha256 = ANY($1) AND state = 'ready'",
            &wanted,
        )
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .collect();
        for (index, slot) in checked.iter_mut().enumerate() {
            let Some(c) = slot else { continue };
            let errors: Vec<ErrorEntry> = c
                .attachments
                .iter()
                .enumerate()
                .filter(|(_, (_, sha))| !ready.contains(sha.as_slice()))
                .map(|(i, _)| {
                    entry(
                        &format!("/attachments/{i}/sha256"),
                        ErrorCode::AttachmentMissing,
                        "upload the object and confirm it with POST /attachments/{sha256}/complete first",
                    )
                })
                .collect();
            if !errors.is_empty() {
                rejections.push(RunRejection {
                    index,
                    run_id: c.run_id.clone(),
                    errors,
                });
                *slot = None;
            }
        }
    }
    if !rejections.is_empty() {
        rejections.sort_by_key(|r| r.index);
        for r in &mut rejections {
            r.errors.sort_by(|a, b| {
                a.path
                    .cmp(&b.path)
                    .then_with(|| (a.code as u8).cmp(&(b.code as u8)))
            });
        }
        return Err(StoreError::RunsRejected(rejections));
    }

    // Steps 3–4: upsert what changed.
    let mut written = Vec::with_capacity(checked.len());
    for c in checked.into_iter().flatten() {
        let prior = existing.get(&c.run_id);
        let archived = prior.is_some_and(|(_, _, archived)| *archived);
        let change = match prior {
            None => RunChange::Created,
            Some((hash, _, _)) if hash.as_slice() == c.content_hash => RunChange::Unchanged,
            Some(_) => RunChange::Updated,
        };
        if change != RunChange::Unchanged {
            upsert(tx, record_id, &c).await?;
        }
        written.push(RunWritten {
            run_id: c.run_id,
            change,
            content_hash: c.content_hash.to_vec(),
            status: c.status,
            archived,
        });
    }

    // Step 5.
    let changed_any = written.iter().any(|w| w.change != RunChange::Unchanged);
    let digest = if changed_any {
        recompute_runs_hash(tx, record_id).await?
    } else {
        current_runs_hash(tx, record_id).await?
    };

    // Step 6: one audit row per run created or updated, in input order.
    for w in written.iter().filter(|w| w.change != RunChange::Unchanged) {
        let subject = subject(ns, name, &w.run_id);
        append(
            tx,
            NewAudit {
                actor,
                ns: Some(ns),
                action: if w.change == RunChange::Created {
                    "run.create"
                } else {
                    "run.update"
                },
                subject: Some(&subject),
                detail: Some(serde_json::json!({
                    "content_hash": hex::encode(&w.content_hash),
                })),
            },
        )
        .await?;
    }

    Ok(RunsWritten {
        record_id,
        runs: written,
        runs_hash: digest,
    })
}

/// Canonicalise and hash a run that passed validation, and extract the
/// columns and side rows from it.
fn prepare(
    run_id: &str,
    mut body: Value,
    attachments: Vec<(String, [u8; 32])>,
) -> Result<Checked, StoreError> {
    if let Some(obj) = body.as_object_mut() {
        obj.insert("run_id".to_string(), Value::String(run_id.to_owned()));
    }
    let content_hash = *run_content_hash(run_id, &body)?.as_bytes();
    // Store the canonical form, re-parsed, as the header ingest does: the
    // stored bytes are then exactly what the hash was computed over.
    let bytes = canonicalize(&body)?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|e| {
        StoreError::Canonical(evalhub_core::canonical::CanonicalError::Serialize(e))
    })?;

    let status = body
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let error_kind = body
        .get("error")
        .and_then(|e| e.get("kind"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let time = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc))
    };
    let started_at = time("started_at");
    let ended_at = time("ended_at");
    let metrics = body
        .get("metrics")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()?)))
                .collect()
        })
        .unwrap_or_default();
    let fingerprints = evalhub_core::fingerprint::for_run(&body)?
        .iter()
        .map(|(facet, fp)| (facet.as_str(), *fp))
        .collect();
    Ok(Checked {
        run_id: run_id.to_owned(),
        body,
        content_hash,
        status,
        error_kind,
        started_at,
        ended_at,
        metrics,
        attachments,
        fingerprints,
    })
}

/// `attachments[]` as `(path, sha256)`, decoding each digest; a digest
/// that is not 64 hex characters is a `schema` error at its sha256.
fn decode_attachments(body: &Value, errors: &mut Vec<ErrorEntry>) -> Vec<(String, [u8; 32])> {
    let mut out = Vec::new();
    let Some(list) = body.get("attachments").and_then(Value::as_array) else {
        return out;
    };
    for (i, a) in list.iter().enumerate() {
        let (Some(path), Some(hex_str)) = (
            a.get("path").and_then(Value::as_str),
            a.get("sha256").and_then(Value::as_str),
        ) else {
            continue;
        };
        let mut sha = [0u8; 32];
        if hex_str.len() == 64 && hex::decode_to_slice(hex_str, &mut sha).is_ok() {
            out.push((path.to_owned(), sha));
        } else {
            errors.push(entry(
                &format!("/attachments/{i}/sha256"),
                ErrorCode::Schema,
                "sha256 must be 64 hexadecimal characters",
            ));
        }
    }
    out
}

/// Insert or overwrite the run row and replace its side rows.
async fn upsert(
    tx: &mut Transaction<'_, Postgres>,
    record_id: Uuid,
    c: &Checked,
) -> Result<(), StoreError> {
    let hash: &[u8] = &c.content_hash;
    sqlx::query!(
        "INSERT INTO runs (record_id, run_id, status, error_kind, started_at, ended_at, body, content_hash)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (record_id, run_id) DO UPDATE
         SET status = EXCLUDED.status, error_kind = EXCLUDED.error_kind,
             started_at = EXCLUDED.started_at, ended_at = EXCLUDED.ended_at,
             body = EXCLUDED.body, content_hash = EXCLUDED.content_hash,
             updated_at = now()",
        record_id,
        c.run_id,
        c.status,
        c.error_kind,
        c.started_at,
        c.ended_at,
        c.body,
        hash,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "DELETE FROM run_metrics WHERE record_id = $1 AND run_id = $2",
        record_id,
        c.run_id,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "DELETE FROM run_attachment_refs WHERE record_id = $1 AND run_id = $2",
        record_id,
        c.run_id,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "DELETE FROM run_fingerprints WHERE record_id = $1 AND run_id = $2",
        record_id,
        c.run_id,
    )
    .execute(&mut **tx)
    .await?;

    let (metrics, values): (Vec<String>, Vec<f64>) = c.metrics.iter().cloned().unzip();
    if !metrics.is_empty() {
        sqlx::query!(
            "INSERT INTO run_metrics (record_id, run_id, metric, value)
             SELECT $1, $2, m, v FROM UNNEST($3::text[], $4::float8[]) AS t (m, v)",
            record_id,
            c.run_id,
            &metrics,
            &values,
        )
        .execute(&mut **tx)
        .await?;
    }
    let (paths, shas): (Vec<String>, Vec<Vec<u8>>) = c
        .attachments
        .iter()
        .map(|(p, s)| (p.clone(), s.to_vec()))
        .unzip();
    if !paths.is_empty() {
        sqlx::query!(
            "INSERT INTO run_attachment_refs (record_id, run_id, path, sha256)
             SELECT $1, $2, p, s FROM UNNEST($3::text[], $4::bytea[]) AS t (p, s)",
            record_id,
            c.run_id,
            &paths,
            &shas,
        )
        .execute(&mut **tx)
        .await?;
    }
    let (facets, fps): (Vec<String>, Vec<Vec<u8>>) = c
        .fingerprints
        .iter()
        .map(|(f, fp)| ((*f).to_owned(), fp.to_vec()))
        .unzip();
    if !facets.is_empty() {
        sqlx::query!(
            "INSERT INTO run_fingerprints (record_id, run_id, facet, fingerprint)
             SELECT $1, $2, f, p FROM UNNEST($3::text[], $4::bytea[]) AS t (f, p)",
            record_id,
            c.run_id,
            &facets,
            &fps,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Recompute `records.runs_hash` from every run row of the record,
/// archived and tombstoned included, store it and return it.
async fn recompute_runs_hash(
    tx: &mut Transaction<'_, Postgres>,
    record_id: Uuid,
) -> Result<Vec<u8>, StoreError> {
    let rows = sqlx::query!(
        "SELECT run_id, content_hash FROM runs WHERE record_id = $1",
        record_id,
    )
    .fetch_all(&mut **tx)
    .await?;
    let pairs: Vec<(String, evalhub_core::ContentHash)> = rows
        .into_iter()
        .map(|r| {
            let mut bytes = [0u8; 32];
            // The column is always a 32-byte digest written above; a
            // shorter value would be a bug, and hashes as zero padding
            // rather than failing the write.
            let n = r.content_hash.len().min(32);
            bytes[..n].copy_from_slice(&r.content_hash[..n]);
            (r.run_id, evalhub_core::ContentHash::from_bytes(bytes))
        })
        .collect();
    let digest = runs_hash(&pairs)?.as_bytes().to_vec();
    sqlx::query!(
        "UPDATE records SET runs_hash = $2 WHERE id = $1",
        record_id,
        &digest,
    )
    .execute(&mut **tx)
    .await?;
    Ok(digest)
}

/// `records.runs_hash` as stored, or the hash of the empty set when no run
/// was ever written.
async fn current_runs_hash(
    tx: &mut Transaction<'_, Postgres>,
    record_id: Uuid,
) -> Result<Vec<u8>, StoreError> {
    let stored = sqlx::query_scalar!("SELECT runs_hash FROM records WHERE id = $1", record_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok(match stored {
        Some(h) => h,
        None => empty_runs_hash()?,
    })
}

/// `runs_hash` of a record with no runs: the formula over `[]`.
pub(crate) fn empty_runs_hash() -> Result<Vec<u8>, StoreError> {
    Ok(runs_hash::<&str>(&[])?.as_bytes().to_vec())
}

/// The row shape every run read maps from.
struct RunRow {
    record_id: Uuid,
    run_id: String,
    status: String,
    error_kind: Option<String>,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    body: Option<Value>,
    content_hash: Vec<u8>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    archived_at: Option<DateTime<Utc>>,
    tombstoned_at: Option<DateTime<Utc>>,
    tombstone_reason: Option<String>,
    tombstone_note: Option<String>,
    visibility: String,
}

impl RunRow {
    fn into_stored(self) -> StoredRun {
        let tombstone = self.tombstoned_at.map(|at| Tombstone {
            at,
            reason: self
                .tombstone_reason
                .as_deref()
                .and_then(TombstoneReason::parse)
                .unwrap_or(TombstoneReason::Other),
            note: self.tombstone_note.clone(),
        });
        StoredRun {
            record_id: self.record_id,
            run_id: self.run_id,
            status: self.status,
            error_kind: self.error_kind,
            started_at: self.started_at,
            ended_at: self.ended_at,
            body: self.body,
            content_hash: self.content_hash,
            created_at: self.created_at,
            updated_at: self.updated_at,
            archived_at: self.archived_at,
            tombstone,
            visibility: Visibility::parse(&self.visibility),
        }
    }
}

/// One run row inside the transaction, without a visibility filter.
async fn load(
    tx: &mut Transaction<'_, Postgres>,
    record_id: Uuid,
    run_id: &str,
) -> Result<Option<RunRow>, StoreError> {
    Ok(sqlx::query_as!(
        RunRow,
        "SELECT u.record_id, u.run_id, u.status, u.error_kind, u.started_at, u.ended_at,
                u.body, u.content_hash, u.created_at, u.updated_at, u.archived_at,
                u.tombstoned_at, u.tombstone_reason, u.tombstone_note, r.visibility
         FROM runs u JOIN records r ON r.id = u.record_id
         WHERE u.record_id = $1 AND u.run_id = $2",
        record_id,
        run_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// The audit subject of a run.
fn subject(ns: &str, name: &str, run_id: &str) -> String {
    format!("eval/{ns}/{name}/runs/{run_id}")
}

fn entry(path: &str, code: ErrorCode, hint: impl Into<String>) -> ErrorEntry {
    ErrorEntry {
        path: path.to_owned(),
        code,
        hint: Some(hint.into()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn decode_attachments_reports_bad_digests() {
        let body = serde_json::json!({"attachments": [
            {"path": "a", "sha256": "00".repeat(32), "size": 1},
            {"path": "b", "sha256": "zz", "size": 1},
        ]});
        let mut errors = Vec::new();
        let out = decode_attachments(&body, &mut errors);
        assert_eq!(out, vec![("a".to_string(), [0u8; 32])]);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "/attachments/1/sha256");
        assert_eq!(errors[0].code, ErrorCode::Schema);
    }

    #[test]
    fn prepare_stores_the_hashed_object() {
        let body = serde_json::json!({"status": "ok", "metrics": {"core/x": 1.0}});
        let c = prepare("r1", body, Vec::new()).unwrap();
        assert_eq!(c.body["run_id"], "r1");
        let (_, h) = evalhub_core::canonical::hash_value(&c.body).unwrap();
        assert_eq!(*h.as_bytes(), c.content_hash);
        assert_eq!(c.metrics, vec![("core/x".to_string(), 1.0)]);
        assert_eq!(c.fingerprints.len(), 6);
    }

    #[test]
    fn empty_runs_hash_is_the_formula_over_nothing() {
        let h = empty_runs_hash().unwrap();
        let expected = evalhub_core::canonical::content_hash(b"[]");
        assert_eq!(h, expected.as_bytes().to_vec());
    }
}
