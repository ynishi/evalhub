#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Run rows against a real Postgres (and MinIO for the GC case): put,
//! batch, archive, delete, `runs_hash`, materialisation, the
//! `evalhub.eval/1.0` ingest split, GC and download references, audit.
//! Needs Docker.

mod common;

use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use evalhub_core::canonical::hash_value;
use evalhub_schema::RecordKind;
use evalhub_schema::error::ErrorCode;
use evalhub_store::PgPool;
use evalhub_store::auth::{self, NamespaceKind};
use evalhub_store::error::StoreError;
use evalhub_store::objects;
use evalhub_store::records::{
    self, Actor, CreateOutcome, IngestFacts, Ingested, NewAttachmentRef, NewVersion, RecordType,
    TombstoneReason, TombstoneRequest, Visibility,
};
use evalhub_store::runs::{self, RunChange};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../evalhub-schema/fixtures");

fn fixture(name: &str) -> Value {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn no_badges(_: &IngestFacts) -> Vec<String> {
    Vec::new()
}

fn member() -> Vec<String> {
    vec!["alice".to_string()]
}

/// The two digests the fixtures reference, and the one the header names.
const SHA_CALLS: &str = "a8b6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a7b6";
const SHA_DIFF: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";

fn sha(hex_str: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    hex::decode_to_slice(hex_str, &mut out).unwrap();
    out
}

/// Mark digests `ready` without an object store: the write paths only
/// read the row's state.
async fn ready(pool: &PgPool, shas: &[&str]) {
    for s in shas {
        sqlx::query(
            "INSERT INTO attachments (sha256, size, state) VALUES ($1, 1, 'ready')
             ON CONFLICT (sha256) DO UPDATE SET state = 'ready'",
        )
        .bind(sha(s).as_slice())
        .execute(pool)
        .await
        .unwrap();
    }
}

/// `alice` namespace, a public Eval `alice/{name}` with the 2.0 header
/// fixture, and the fixture digests ready.
async fn setup(pool: &PgPool, name: &str) -> Ingested {
    auth::ensure_namespace(pool, "alice", NamespaceKind::User)
        .await
        .unwrap();
    ready(pool, &[SHA_CALLS, SHA_DIFF]).await;
    let out = post(pool, name, &fixture("eval-run-set.json")).await;
    records::set_visibility(
        pool,
        RecordType::Eval,
        "alice",
        name,
        Visibility::Public,
        Actor::default(),
    )
    .await
    .unwrap();
    out
}

/// POST an Eval body the way the server does: canonical form, its hash,
/// its fingerprints and attachment rows.
async fn post(pool: &PgPool, name: &str, body: &Value) -> Ingested {
    try_post(pool, name, body).await.unwrap()
}

async fn try_post(pool: &PgPool, name: &str, body: &Value) -> Result<Ingested, StoreError> {
    let (bytes, hash) = hash_value(body).unwrap();
    let canonical: Value = serde_json::from_slice(&bytes).unwrap();
    let fps = evalhub_core::fingerprints(RecordKind::Eval, &canonical).unwrap();
    let fp_rows: Vec<(&str, [u8; 32])> = fps.iter().map(|(f, b)| (f.as_str(), *b)).collect();
    let shas: Vec<(String, [u8; 32])> = canonical
        .get("attachments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|e| {
                    (
                        e["path"].as_str().unwrap().to_string(),
                        sha(e["sha256"].as_str().unwrap()),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let attachments: Vec<NewAttachmentRef<'_>> = shas
        .iter()
        .map(|(path, sha256)| NewAttachmentRef {
            path,
            sha256: *sha256,
        })
        .collect();
    records::ingest(
        pool,
        NewVersion {
            record_type: RecordType::Eval,
            ns: "alice",
            name,
            body: &canonical,
            content_hash: hash.as_bytes(),
            label: None,
            actor: Actor::default(),
            readable_ns: &[],
            fingerprints: &fp_rows,
            results: &[],
            relations: &[],
            attachments: &attachments,
        },
        || (Uuid::new_v4(), Uuid::new_v4()),
        &no_badges,
    )
    .await
}

async fn count(pool: &PgPool, sql: &'static str, record_id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(record_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn audit_actions(pool: &PgPool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT action, subject FROM audit WHERE action LIKE 'run.%' ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn runs_hash_column(pool: &PgPool, record_id: Uuid) -> Option<Vec<u8>> {
    sqlx::query_scalar("SELECT runs_hash FROM records WHERE id = $1")
        .bind(record_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn rejections(err: StoreError) -> Vec<evalhub_store::error::RunRejection> {
    match err {
        StoreError::RunsRejected(r) => r,
        other => panic!("expected RunsRejected, got {other:?}"),
    }
}

#[tokio::test]
async fn put_creates_then_is_idempotent_then_updates() {
    let db = common::db().await;
    let pool = &db.pool;
    let record_id = setup(pool, "e").await.outcome.meta().record_id;
    let run = fixture("run.json");

    let first = runs::put(pool, "alice", "e", "r1", &run, Actor::default())
        .await
        .unwrap();
    assert_eq!(first.record_id, record_id);
    assert_eq!(first.runs.len(), 1);
    assert_eq!(first.runs[0].change, RunChange::Created);
    assert_eq!(first.runs[0].status, "ok");
    assert!(!first.runs[0].archived);
    let hash = first.runs[0].content_hash.clone();

    // The formulas, recomputed from the stored body.
    let stored = runs::get(pool, "alice", "e", "r1", &[])
        .await
        .unwrap()
        .unwrap();
    let body = stored.body.clone().unwrap();
    assert_eq!(body["run_id"], "r1");
    let (_, h) = hash_value(&body).unwrap();
    assert_eq!(h.as_bytes().to_vec(), hash);
    let rh = evalhub_core::runs_hash(&[("r1", h)]).unwrap();
    assert_eq!(first.runs_hash, rh.as_bytes().to_vec());
    assert_eq!(
        runs_hash_column(pool, record_id).await,
        Some(first.runs_hash.clone())
    );

    // Side rows.
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM run_metrics WHERE record_id = $1",
            record_id
        )
        .await,
        2
    );
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM run_fingerprints WHERE record_id = $1",
            record_id
        )
        .await,
        6
    );
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM run_attachment_refs WHERE record_id = $1",
            record_id
        )
        .await,
        2
    );
    assert_eq!(
        stored.started_at.map(|t| t.to_rfc3339()).as_deref(),
        Some("2026-09-20T10:00:00+00:00")
    );
    assert_eq!(audit_actions(pool).await.len(), 1);

    // Same body: unchanged, same hash, no audit row.
    let again = runs::put(pool, "alice", "e", "r1", &run, Actor::default())
        .await
        .unwrap();
    assert_eq!(again.runs[0].change, RunChange::Unchanged);
    assert_eq!(again.runs[0].content_hash, hash);
    assert_eq!(again.runs_hash, first.runs_hash);
    assert_eq!(audit_actions(pool).await.len(), 1);

    // Changed body: updated, runs_hash moves.
    let mut changed = run.clone();
    changed["metrics"]["core/tokens_out"] = json!(2000.0);
    let updated = runs::put(pool, "alice", "e", "r1", &changed, Actor::default())
        .await
        .unwrap();
    assert_eq!(updated.runs[0].change, RunChange::Updated);
    assert_ne!(updated.runs[0].content_hash, hash);
    assert_ne!(updated.runs_hash, first.runs_hash);
    let value: f64 = sqlx::query_scalar(
        "SELECT value FROM run_metrics WHERE record_id = $1 AND metric = 'core/tokens_out'",
    )
    .bind(record_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(value, 2000.0);
    assert_eq!(
        audit_actions(pool).await,
        vec![
            ("run.create".to_string(), "eval/alice/e/runs/r1".to_string()),
            ("run.update".to_string(), "eval/alice/e/runs/r1".to_string()),
        ]
    );

    // A body whose run_id disagrees with the path is refused.
    let err = runs::put(pool, "alice", "e", "r2", &run, Actor::default())
        .await
        .unwrap_err();
    let r = rejections(err);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].index, 0);
    assert!(
        r[0].errors
            .iter()
            .any(|e| e.code == ErrorCode::RunIdMismatch)
    );

    // An overwrite that drops a metric drops its row.
    let mut fewer = changed.clone();
    fewer["metrics"]
        .as_object_mut()
        .unwrap()
        .remove("core/duration_ms");
    let out = runs::put(pool, "alice", "e", "r1", &fewer, Actor::default())
        .await
        .unwrap();
    assert_eq!(out.runs[0].change, RunChange::Updated);
    let metrics: Vec<String> =
        sqlx::query_scalar("SELECT metric FROM run_metrics WHERE record_id = $1 ORDER BY metric")
            .bind(record_id)
            .fetch_all(pool)
            .await
            .unwrap();
    assert_eq!(metrics, ["core/tokens_out"]);
}

#[tokio::test]
async fn an_unparseable_timestamp_is_kept_in_the_body_and_null_in_the_column() {
    let db = common::db().await;
    let pool = &db.pool;
    let record_id = setup(pool, "e").await.outcome.meta().record_id;
    let run = json!({"run_id": "r1", "status": "ok", "started_at": "yesterday"});
    let out = runs::put(pool, "alice", "e", "r1", &run, Actor::default())
        .await
        .unwrap();
    assert_eq!(out.runs[0].change, RunChange::Created);
    let (column, body): (Option<chrono::DateTime<chrono::Utc>>, Value) =
        sqlx::query_as("SELECT started_at, body FROM runs WHERE record_id = $1 AND run_id = 'r1'")
            .bind(record_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(column, None);
    assert_eq!(body["started_at"], "yesterday");
}

#[tokio::test]
async fn archive_keeps_the_hash_and_hides_from_non_members() {
    let db = common::db().await;
    let pool = &db.pool;
    let record_id = setup(pool, "e").await.outcome.meta().record_id;
    let run = fixture("run.json");
    let put = runs::put(pool, "alice", "e", "r1", &run, Actor::default())
        .await
        .unwrap();

    let archived = runs::archive(pool, "alice", "e", "r1", Actor::default())
        .await
        .unwrap();
    assert!(archived.archived_at.is_some());
    assert_eq!(
        runs_hash_column(pool, record_id).await,
        Some(put.runs_hash.clone())
    );
    // Archiving again changes nothing and writes no audit row.
    runs::archive(pool, "alice", "e", "r1", Actor::default())
        .await
        .unwrap();
    // The same body again on an archived run: unchanged, still archived.
    let same = runs::put(pool, "alice", "e", "r1", &run, Actor::default())
        .await
        .unwrap();
    assert_eq!(same.runs[0].change, RunChange::Unchanged);
    assert!(same.runs[0].archived);

    // Hidden from a non-member of a public Eval, visible to a member.
    assert!(
        runs::get(pool, "alice", "e", "r1", &[])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        runs::get(pool, "alice", "e", "r1", &member())
            .await
            .unwrap()
            .is_some()
    );

    let summary = records::runs_summary(pool, record_id, false).await.unwrap();
    assert_eq!(summary.count, 0);
    assert_eq!(summary.archived, None);
    assert_eq!(summary.deleted, None);
    assert_eq!(summary.runs_hash, put.runs_hash);
    let summary = records::runs_summary(pool, record_id, true).await.unwrap();
    assert_eq!(summary.archived, Some(1));
    assert_eq!(summary.deleted, Some(0));

    // Overwriting an archived run leaves it archived.
    let mut changed = run.clone();
    changed["meta"] = json!({"worker": "gpu-4"});
    let over = runs::put(pool, "alice", "e", "r1", &changed, Actor::default())
        .await
        .unwrap();
    assert_eq!(over.runs[0].change, RunChange::Updated);
    assert!(over.runs[0].archived);
    let row = runs::get(pool, "alice", "e", "r1", &member())
        .await
        .unwrap()
        .unwrap();
    assert!(row.archived_at.is_some());

    runs::unarchive(pool, "alice", "e", "r1", Actor::default())
        .await
        .unwrap();
    assert!(
        runs::get(pool, "alice", "e", "r1", &[])
            .await
            .unwrap()
            .is_some()
    );
    let summary = records::runs_summary(pool, record_id, false).await.unwrap();
    assert_eq!(summary.count, 1);
    assert_eq!(summary.by_status.ok, 1);

    let actions: Vec<String> = audit_actions(pool).await.into_iter().map(|a| a.0).collect();
    assert_eq!(
        actions,
        ["run.create", "run.archive", "run.update", "run.unarchive"]
    );

    let err = runs::archive(pool, "alice", "e", "nope", Actor::default())
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::RunNotFound), "{err}");
}

#[tokio::test]
async fn tombstone_keeps_hash_and_metrics_and_refuses_the_id() {
    let db = common::db().await;
    let pool = &db.pool;
    let record_id = setup(pool, "e").await.outcome.meta().record_id;
    let run = fixture("run.json");
    let put = runs::put(pool, "alice", "e", "r1", &run, Actor::default())
        .await
        .unwrap();

    let gone = runs::tombstone(
        pool,
        "alice",
        "e",
        "r1",
        TombstoneRequest {
            reason: TombstoneReason::Withdrawn,
            note: Some("bad seed"),
        },
        Actor::default(),
    )
    .await
    .unwrap();
    assert!(gone.body.is_none());
    assert_eq!(gone.content_hash, put.runs[0].content_hash);
    let t = gone.tombstone.unwrap();
    assert_eq!(t.reason, TombstoneReason::Withdrawn);
    assert_eq!(t.note.as_deref(), Some("bad seed"));
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM run_metrics WHERE record_id = $1",
            record_id
        )
        .await,
        2
    );
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM run_attachment_refs WHERE record_id = $1",
            record_id
        )
        .await,
        0
    );
    assert_eq!(
        runs_hash_column(pool, record_id).await,
        Some(put.runs_hash.clone())
    );

    // A reader who sees the Eval sees the tombstone.
    let read = runs::get(pool, "alice", "e", "r1", &[])
        .await
        .unwrap()
        .unwrap();
    assert!(read.tombstone.is_some() && read.body.is_none());
    let summary = records::runs_summary(pool, record_id, true).await.unwrap();
    assert_eq!((summary.count, summary.deleted), (0, Some(1)));

    // Twice is an error; so is archiving it; so is writing the id again.
    let err = runs::tombstone(
        pool,
        "alice",
        "e",
        "r1",
        TombstoneRequest {
            reason: TombstoneReason::Other,
            note: None,
        },
        Actor::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::RunDeleted), "{err}");
    let err = runs::archive(pool, "alice", "e", "r1", Actor::default())
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::RunDeleted), "{err}");
    let err = runs::put(pool, "alice", "e", "r1", &run, Actor::default())
        .await
        .unwrap_err();
    let r = rejections(err);
    assert_eq!(r[0].errors.len(), 1);
    assert_eq!(r[0].errors[0].code, ErrorCode::RunDeleted);

    let actions: Vec<String> = audit_actions(pool).await.into_iter().map(|a| a.0).collect();
    assert_eq!(actions, ["run.create", "run.delete"]);
}

#[tokio::test]
async fn batch_is_all_or_nothing_and_reports_every_failure() {
    let db = common::db().await;
    let pool = &db.pool;
    let record_id = setup(pool, "e").await.outcome.meta().record_id;

    let good = json!({"run_id": "a", "status": "ok", "metrics": {"core/accuracy": 1.0}});
    // `error` status without `error`.
    let invalid = json!({"run_id": "b", "status": "error"});
    // An attachment that was never uploaded.
    let missing = json!({"run_id": "c", "status": "ok", "calls": "calls.jsonl",
        "attachments": [{"path": "calls.jsonl", "sha256": "11".repeat(32), "size": 3}]});
    let err = runs::put_batch(
        pool,
        "alice",
        "e",
        &[good.clone(), invalid, missing],
        Actor::default(),
    )
    .await
    .unwrap_err();
    let r = rejections(err);
    assert_eq!(r.iter().map(|x| x.index).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(r[0].run_id, "b");
    assert!(
        r[0].errors
            .iter()
            .any(|e| e.code == ErrorCode::RunStatusDetail)
    );
    assert_eq!(r[1].errors[0].code, ErrorCode::AttachmentMissing);
    assert_eq!(r[1].errors[0].path, "/attachments/0/sha256");
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM runs WHERE record_id = $1",
            record_id
        )
        .await,
        0
    );
    assert_eq!(runs_hash_column(pool, record_id).await, None);
    assert!(audit_actions(pool).await.is_empty());

    // A duplicate id fails the batch.
    let err = runs::put_batch(
        pool,
        "alice",
        "e",
        &[good.clone(), good.clone()],
        Actor::default(),
    )
    .await
    .unwrap_err();
    let r = rejections(err);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].index, 1);
    assert_eq!(r[0].errors[0].code, ErrorCode::BatchDuplicateRunId);
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM runs WHERE record_id = $1",
            record_id
        )
        .await,
        0
    );

    // A good batch: one audit row per created run, one runs_hash.
    let other = json!({"run_id": "d", "status": "skipped", "skip_reason": "flaky"});
    let out = runs::put_batch(pool, "alice", "e", &[good, other], Actor::default())
        .await
        .unwrap();
    assert_eq!(
        out.runs.iter().map(|w| w.change).collect::<Vec<_>>(),
        [RunChange::Created, RunChange::Created]
    );
    assert_eq!(audit_actions(pool).await.len(), 2);
    let summary = records::runs_summary(pool, record_id, false).await.unwrap();
    assert_eq!(summary.count, 2);
    assert_eq!((summary.by_status.ok, summary.by_status.skipped), (1, 1));
    assert_eq!(summary.runs_hash, out.runs_hash);
}

#[tokio::test]
async fn a_materialised_facet_survives_a_later_header() {
    let db = common::db().await;
    let pool = &db.pool;
    setup(pool, "e").await;

    runs::put(
        pool,
        "alice",
        "e",
        "r1",
        &json!({"run_id": "r1", "status": "ok"}),
        Actor::default(),
    )
    .await
    .unwrap();
    let r1 = runs::get(pool, "alice", "e", "r1", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r1.body.as_ref().unwrap()["model"]["id"], "qwen3.6-32b");

    let mut header = fixture("eval-run-set.json");
    header["model"] = json!({"id": "other-model"});
    let out = post(pool, "e", &header).await;
    assert!(matches!(out.outcome, CreateOutcome::Created { .. }));
    assert!(out.converted.is_none());

    let again = runs::get(pool, "alice", "e", "r1", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.body.as_ref().unwrap()["model"]["id"], "qwen3.6-32b");
    assert_eq!(again.content_hash, r1.content_hash);

    runs::put(
        pool,
        "alice",
        "e",
        "r2",
        &json!({"run_id": "r2", "status": "ok"}),
        Actor::default(),
    )
    .await
    .unwrap();
    let r2 = runs::get(pool, "alice", "e", "r2", &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r2.body.as_ref().unwrap()["model"]["id"], "other-model");
}

#[tokio::test]
async fn a_1_0_body_is_split_into_a_header_and_runs() {
    let db = common::db().await;
    let pool = &db.pool;
    auth::ensure_namespace(pool, "alice", NamespaceKind::User)
        .await
        .unwrap();
    ready(pool, &[SHA_CALLS, SHA_DIFF]).await;

    let mut v1 = fixture("eval-run-set-v1.json");
    let second = json!({"run_id": "r2", "outcome": "error"});
    v1["runs"].as_array_mut().unwrap().push(second);

    let out = post(pool, "legacy", &v1).await;
    let CreateOutcome::Created { meta, .. } = &out.outcome else {
        panic!("expected a new version, got {:?}", out.outcome);
    };
    let record_id = meta.record_id;
    let converted = out.converted.clone().unwrap();
    assert_eq!(converted.converted_from, "evalhub.eval/1.0");
    assert_eq!(
        converted
            .runs
            .runs
            .iter()
            .map(|w| (w.run_id.as_str(), w.change, w.status.as_str()))
            .collect::<Vec<_>>(),
        [
            ("r1", RunChange::Created, "ok"),
            ("r2", RunChange::Created, "error")
        ]
    );
    assert!(converted.duplicate_run_ids.is_empty());

    // The stored body is the 2.0 header and the hash is the header's.
    let split = evalhub_core::split_v1(&v1);
    let (_, header_hash) = hash_value(&split.header).unwrap();
    assert_eq!(meta.content_hash, header_hash.as_bytes().to_vec());
    let latest = records::get_latest(pool, RecordType::Eval, "alice", "legacy", &member())
        .await
        .unwrap()
        .unwrap();
    let body = latest.body.unwrap();
    assert_eq!(body["schema"], "evalhub.eval/2.0");
    assert!(body.get("runs").is_none());
    let r1 = runs::get(pool, "alice", "legacy", "r1", &member())
        .await
        .unwrap()
        .unwrap();
    let r1_body = r1.body.unwrap();
    assert_eq!(r1_body["meta"]["v0.2.0_migration"]["outcome"], "pass");
    assert_eq!(r1_body["model"]["id"], "qwen3.6-32b");
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM run_attachment_refs WHERE record_id = $1",
            record_id
        )
        .await,
        2
    );
    let audit_before = audit_actions(pool).await.len();
    assert_eq!(audit_before, 2);

    // Re-posting the same body: no version, no run row, no audit row.
    let again = post(pool, "legacy", &v1).await;
    assert!(matches!(again.outcome, CreateOutcome::Existing(_)));
    assert!(
        again
            .converted
            .unwrap()
            .runs
            .runs
            .iter()
            .all(|w| w.change == RunChange::Unchanged)
    );
    assert_eq!(audit_actions(pool).await.len(), audit_before);
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM versions WHERE record_id = $1",
            record_id
        )
        .await,
        1
    );

    // One more run: one row, no header version.
    v1["runs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"run_id": "r3", "outcome": "skipped"}));
    let third = post(pool, "legacy", &v1).await;
    assert!(matches!(third.outcome, CreateOutcome::Existing(_)));
    let changes: Vec<RunChange> = third
        .converted
        .unwrap()
        .runs
        .runs
        .iter()
        .map(|w| w.change)
        .collect();
    assert_eq!(
        changes,
        [
            RunChange::Unchanged,
            RunChange::Unchanged,
            RunChange::Created
        ]
    );
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM versions WHERE record_id = $1",
            record_id
        )
        .await,
        1
    );
    assert_eq!(
        count(
            pool,
            "SELECT COUNT(*) FROM runs WHERE record_id = $1",
            record_id
        )
        .await,
        3
    );

    // A refused run refuses the post and names its position in runs[].
    runs::tombstone(
        pool,
        "alice",
        "legacy",
        "r2",
        TombstoneRequest {
            reason: TombstoneReason::Duplicate,
            note: None,
        },
        Actor::default(),
    )
    .await
    .unwrap();
    let err = try_post(pool, "legacy", &v1).await.unwrap_err();
    let r = rejections(err);
    assert_eq!((r[0].index, r[0].run_id.as_str()), (1, "r2"));
    assert_eq!(r[0].errors[0].code, ErrorCode::RunDeleted);
}

#[tokio::test]
async fn gc_and_referencing_count_run_references() {
    let db = common::db().await;
    let s3 = common::s3().await;
    let pool = &db.pool;
    setup(pool, "e").await;

    let bytes = b"run log bytes";
    let sha_bytes: [u8; 32] = Sha256::digest(bytes).into();
    let hex_sha = hex::encode(sha_bytes);
    objects::begin_upload(pool, &sha_bytes, bytes.len() as i64, None)
        .await
        .unwrap();
    s3.objects
        .put_direct(&sha_bytes, bytes.to_vec())
        .await
        .unwrap();
    objects::complete(pool, &s3.objects, &sha_bytes, 1 << 20)
        .await
        .unwrap();

    let run = json!({"run_id": "r1", "status": "error",
        "error": {"kind": "timeout", "log": "log.txt"},
        "attachments": [{"path": "log.txt", "sha256": hex_sha, "size": bytes.len()}]});
    runs::put(pool, "alice", "e", "r1", &run, Actor::default())
        .await
        .unwrap();

    let refs = objects::referencing(pool, &sha_bytes).await.unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].ns, "alice");
    assert_eq!(refs[0].visibility, "public");
    assert_eq!(refs[0].version_id, None);
    assert_eq!(refs[0].run.as_ref().map(|r| r.1.as_str()), Some("r1"));
    assert!(!refs[0].run_archived);
    runs::archive(pool, "alice", "e", "r1", Actor::default())
        .await
        .unwrap();
    let refs = objects::referencing(pool, &sha_bytes).await.unwrap();
    assert!(refs[0].run_archived);

    // Past the grace period and referenced only by the run: kept.
    sqlx::query("UPDATE attachments SET created_at = now() - interval '2 hours' WHERE sha256 = $1")
        .bind(sha_bytes.as_slice())
        .execute(pool)
        .await
        .unwrap();
    let grace = Duration::from_secs(3600);
    assert!(
        !objects::gc_candidates(pool, grace)
            .await
            .unwrap()
            .contains(&sha_bytes)
    );
    assert_eq!(objects::gc(pool, &s3.objects, grace).await.unwrap(), 0);
    assert!(objects::state(pool, &sha_bytes).await.unwrap().is_some());

    // Deleting the run drops the reference; the GC collects the object.
    runs::tombstone(
        pool,
        "alice",
        "e",
        "r1",
        TombstoneRequest {
            reason: TombstoneReason::Takedown,
            note: None,
        },
        Actor::default(),
    )
    .await
    .unwrap();
    assert!(
        objects::referencing(pool, &sha_bytes)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        objects::gc_candidates(pool, grace).await.unwrap(),
        vec![sha_bytes]
    );
    assert_eq!(objects::gc(pool, &s3.objects, grace).await.unwrap(), 1);
    assert!(objects::state(pool, &sha_bytes).await.unwrap().is_none());
    assert!(s3.objects.head(&sha_bytes).await.unwrap().is_none());
}

#[tokio::test]
async fn no_live_header_makes_runs_not_found() {
    let db = common::db().await;
    let pool = &db.pool;
    setup(pool, "e").await;
    // The run holds the only reference to SHA_DIFF (the header names
    // SHA_CALLS only).
    runs::put(
        pool,
        "alice",
        "e",
        "r1",
        &json!({"run_id": "r1", "status": "ok", "calls": "diff.patch",
            "attachments": [{"path": "diff.patch", "sha256": SHA_DIFF, "size": 1}]}),
        Actor::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        objects::referencing(pool, &sha(SHA_DIFF))
            .await
            .unwrap()
            .len(),
        1
    );
    records::tombstone(
        pool,
        RecordType::Eval,
        "alice",
        "e",
        1,
        TombstoneRequest {
            reason: TombstoneReason::Withdrawn,
            note: None,
        },
        Actor::default(),
    )
    .await
    .unwrap();

    assert!(
        runs::get(pool, "alice", "e", "r1", &member())
            .await
            .unwrap()
            .is_none()
    );
    let err = runs::put(
        pool,
        "alice",
        "e",
        "r2",
        &json!({"run_id": "r2", "status": "ok"}),
        Actor::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::RecordNotFound), "{err}");
    let err = runs::archive(pool, "alice", "e", "r1", Actor::default())
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::RecordNotFound), "{err}");
    // A run nobody can read grants no download: with no live header the
    // run's reference is not returned (a public Eval would otherwise let
    // anyone fetch the attachment of a run `get` refuses).
    let refs = objects::referencing(pool, &sha(SHA_DIFF)).await.unwrap();
    assert!(
        refs.is_empty(),
        "run reference returned without a live header: {refs:?}"
    );
    // The row is untouched.
    let body: Option<Value> = sqlx::query_scalar("SELECT body FROM runs WHERE run_id = 'r1'")
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(body.is_some());

    // An Eval that does not exist is the same answer.
    let err = runs::put(
        pool,
        "alice",
        "nope",
        "r1",
        &json!({"run_id": "r1", "status": "ok"}),
        Actor::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, StoreError::RecordNotFound), "{err}");
}

#[tokio::test]
async fn a_private_eval_hides_its_runs() {
    let db = common::db().await;
    let pool = &db.pool;
    setup(pool, "e").await;
    runs::put(
        pool,
        "alice",
        "e",
        "r1",
        &json!({"run_id": "r1", "status": "ok"}),
        Actor::default(),
    )
    .await
    .unwrap();
    records::set_visibility(
        pool,
        RecordType::Eval,
        "alice",
        "e",
        Visibility::Private,
        Actor::default(),
    )
    .await
    .unwrap();
    assert!(
        runs::get(pool, "alice", "e", "r1", &[])
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        runs::get(pool, "alice", "e", "r1", &["bob".to_string()])
            .await
            .unwrap()
            .is_none()
    );
    let got = runs::get(pool, "alice", "e", "r1", &member())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.visibility, Visibility::Private);
}
