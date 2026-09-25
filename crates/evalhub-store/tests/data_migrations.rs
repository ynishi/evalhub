#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The one-shot data migration `0003_runs_split` against a real Postgres:
//! 0.1.x-shaped rows are seeded with plain SQL (the 0.2.0 write paths can
//! no longer produce them), then `pool::migrate` runs the data step, and
//! the result is read back row by row. Needs Docker.

mod common;

use std::collections::BTreeMap;

use serde_json::{Value, json};
use uuid::Uuid;

use evalhub_core::canonical::hash_value;
use evalhub_core::eval::split_v1;
use evalhub_core::run::{run_content_hash, runs_hash};
use evalhub_store::PgPool;
use evalhub_store::data_migrations::{ALL, RUNS_SPLIT};
use evalhub_store::error::StoreError;
use evalhub_store::pool::{self, PendingMigration};
use evalhub_store::relations;

const NS: &str = "alice";
const SHA_CALLS_V1: &str = "a8b6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a7b6";
const SHA_DIFF_V1: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";
const SHA_CALLS_V2: &str = "1111111111111111111111111111111111111111111111111111111111111111";

/// A 1.0 Eval body in the shape 0.1.x stored.
fn eval_v1_body(title: &str, model: &str, runs: Value, attachments: Value) -> Value {
    json!({
        "schema": "evalhub.eval/1.0",
        "title": title,
        "producer": {"name": "my-harness", "version": "0.4.1"},
        "eval_kind": "run_set",
        "origin": "live",
        "harness": {"name": "my-harness", "version": "0.4.1"},
        "model": {"id": model},
        "task": {"id": "single2", "version": "3", "split": "test", "n": 33},
        "runs": runs,
        "attachments": attachments,
        "ext": {},
    })
}

/// Canonical form and content hash, as the 0.1.x ingest stored them.
fn canonical(body: &Value) -> (Value, [u8; 32]) {
    let (bytes, hash) = hash_value(body).unwrap();
    (serde_json::from_slice(&bytes).unwrap(), *hash.as_bytes())
}

async fn exec(pool: &PgPool, sql: &'static str) {
    sqlx::query(sql).execute(pool).await.unwrap();
}

async fn record(pool: &PgPool, kind: &str, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO records (id, type, ns, name) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(kind)
        .bind(NS)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Insert a live version; returns its id and stored content hash.
async fn version(pool: &PgPool, record_id: Uuid, seq: i32, body: &Value) -> (Uuid, [u8; 32]) {
    let (body, hash) = canonical(body);
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO versions (version_id, record_id, seq, content_hash, body)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(record_id)
    .bind(seq)
    .bind(hash.as_slice())
    .bind(&body)
    .execute(pool)
    .await
    .unwrap();
    (id, hash)
}

async fn tombstoned_version(pool: &PgPool, record_id: Uuid, seq: i32, was: &Value) -> Uuid {
    let (_, hash) = canonical(was);
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO versions (version_id, record_id, seq, content_hash, body,
                               tombstoned_at, tombstone_reason)
         VALUES ($1, $2, $3, $4, NULL, now(), 'withdrawn')",
    )
    .bind(id)
    .bind(record_id)
    .bind(seq)
    .bind(hash.as_slice())
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn attachment(pool: &PgPool, sha: &str) {
    sqlx::query("INSERT INTO attachments (sha256, size, state) VALUES ($1, 1, 'ready')")
        .bind(hex::decode(sha).unwrap())
        .execute(pool)
        .await
        .unwrap();
}

async fn attachment_ref(pool: &PgPool, version_id: Uuid, sha: &str, path: &str) {
    sqlx::query("INSERT INTO attachment_refs (version_id, sha256, path) VALUES ($1, $2, $3)")
        .bind(version_id)
        .bind(hex::decode(sha).unwrap())
        .bind(path)
        .execute(pool)
        .await
        .unwrap();
}

async fn uses_eval(pool: &PgPool, from: Uuid, to: Uuid, attrs: Option<Value>) {
    sqlx::query(
        "INSERT INTO relations (from_version_id, type, to_version_id, attrs)
         VALUES ($1, 'core/uses_eval', $2, $3)",
    )
    .bind(from)
    .bind(to)
    .bind(attrs)
    .execute(pool)
    .await
    .unwrap();
}

/// What the seed wrote, for the assertions.
struct Seed {
    eval_id: Uuid,
    v1: Uuid,
    v1_body: Value,
    v1_hash: [u8; 32],
    v2: Uuid,
    v2_body: Value,
    v2_hash: [u8; 32],
    v3: Uuid,
    c1: Uuid,
    c2: Uuid,
    c3: Uuid,
}

/// The 0.1.x database of the Acceptance: one Eval `alice/e` with two live
/// versions whose `runs[]` overlap (`r1`, `r2`) with different bodies and
/// different header facets (`model`), a tombstoned third version whose
/// run `r5` exists nowhere else, and three Cards: `c1` on `@1` with
/// `attrs.runs` naming a dangling `ghost` and `r4` (a run of the Eval
/// that only `@2` has), `c2` on `@1` without
/// `attrs.runs`, `c3` on the tombstoned `@3`.
async fn seed(pool: &PgPool) -> Seed {
    exec(
        pool,
        "INSERT INTO namespaces (ns, kind) VALUES ('alice', 'user')",
    )
    .await;
    for sha in [SHA_CALLS_V1, SHA_DIFF_V1, SHA_CALLS_V2] {
        attachment(pool, sha).await;
    }

    let eval_id = record(pool, "eval", "e").await;
    let v1_body = eval_v1_body(
        "e, first",
        "m1",
        json!([
            {"run_id": "r1", "outcome": "pass",
             "started_at": "2026-09-20T10:00:00Z", "ended_at": "2026-09-20T10:02:11Z",
             "calls": "calls/r1.jsonl", "artifacts": ["artifacts/r1/diff.patch"]},
            {"run_id": "r2", "outcome": "error"},
            {"run_id": "r3", "outcome": "skipped"},
        ]),
        json!([
            {"path": "calls/r1.jsonl", "sha256": SHA_CALLS_V1, "size": 1},
            {"path": "artifacts/r1/diff.patch", "sha256": SHA_DIFF_V1, "size": 1},
        ]),
    );
    let (v1, v1_hash) = version(pool, eval_id, 1, &v1_body).await;
    attachment_ref(pool, v1, SHA_CALLS_V1, "calls/r1.jsonl").await;
    attachment_ref(pool, v1, SHA_DIFF_V1, "artifacts/r1/diff.patch").await;

    let v2_body = eval_v1_body(
        "e, second",
        "m2",
        json!([
            {"run_id": "r1", "outcome": "fail",
             "started_at": "2026-09-21T10:00:00Z", "calls": "calls/r1.jsonl"},
            {"run_id": "r2", "outcome": "error"},
            {"run_id": "r4", "outcome": "pass"},
        ]),
        json!([{"path": "calls/r1.jsonl", "sha256": SHA_CALLS_V2, "size": 1}]),
    );
    let (v2, v2_hash) = version(pool, eval_id, 2, &v2_body).await;
    attachment_ref(pool, v2, SHA_CALLS_V2, "calls/r1.jsonl").await;

    let v3_was = eval_v1_body(
        "e, third",
        "m3",
        json!([{"run_id": "r5", "outcome": "pass"}]),
        json!([]),
    );
    let v3 = tombstoned_version(pool, eval_id, 3, &v3_was).await;

    let card = json!({"schema": "evalhub.card/1.0", "title": "a card"});
    let (c1, _) = version(pool, record(pool, "card", "c1").await, 1, &card).await;
    uses_eval(
        pool,
        c1,
        v1,
        Some(json!({"runs": ["r1", "r2", "ghost", "r4"]})),
    )
    .await;
    let (c2, _) = version(pool, record(pool, "card", "c2").await, 1, &card).await;
    uses_eval(pool, c2, v1, None).await;
    let (c3, _) = version(pool, record(pool, "card", "c3").await, 1, &card).await;
    uses_eval(pool, c3, v3, Some(json!({"runs": ["r5"]}))).await;

    Seed {
        eval_id,
        v1,
        v1_body,
        v1_hash,
        v2,
        v2_body,
        v2_hash,
        v3,
        c1,
        c2,
        c3,
    }
}

/// `run_id` → content hash of each run of `body` as `split_v1` converts
/// it with that body's own header.
fn converted_hashes(body: &Value) -> BTreeMap<String, Vec<u8>> {
    split_v1(body)
        .runs
        .iter()
        .map(|(id, run)| {
            (
                id.clone(),
                run_content_hash(id, run).unwrap().as_bytes().to_vec(),
            )
        })
        .collect()
}

async fn card_eval_runs(pool: &PgPool, card_version_id: Uuid) -> BTreeMap<String, Vec<u8>> {
    sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT run_id, content_hash FROM card_eval_runs WHERE card_version_id = $1",
    )
    .bind(card_version_id)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .collect()
}

/// Everything the data step writes, as text, for the "second run changes
/// nothing" comparison.
async fn snapshot(pool: &PgPool) -> Vec<String> {
    let mut out = Vec::new();
    for sql in [
        "SELECT version_id::text || ' ' || encode(content_hash, 'hex') || ' ' || coalesce(body::text, '-')
         FROM versions ORDER BY version_id",
        "SELECT run_id || ' ' || encode(content_hash, 'hex') || ' ' || body::text || ' ' || updated_at::text
         FROM runs ORDER BY run_id",
        "SELECT run_id || ' ' || path || ' ' || encode(sha256, 'hex') FROM run_attachment_refs ORDER BY 1",
        "SELECT run_id || ' ' || facet || ' ' || encode(fingerprint, 'hex') FROM run_fingerprints ORDER BY 1",
        "SELECT card_version_id::text || ' ' || run_id || ' ' || encode(content_hash, 'hex')
         FROM card_eval_runs ORDER BY 1",
        "SELECT id::text || ' ' || action || ' ' || coalesce(detail::text, '') FROM audit ORDER BY id",
        "SELECT name || ' ' || applied_at::text FROM data_migrations ORDER BY name",
        "SELECT id::text || ' ' || coalesce(encode(runs_hash, 'hex'), '-') FROM records ORDER BY id",
    ] {
        let rows: Vec<String> = sqlx::query_scalar(sql).fetch_all(pool).await.unwrap();
        out.extend(rows);
    }
    out
}

#[tokio::test]
async fn runs_split_moves_0_1_x_runs_into_rows_and_rebuilds_card_used_sets() {
    let db = common::db_unmigrated().await;
    let pool = &db.pool;
    // The 0.2.0 DDL, then the 0.1.x data, then the data step: the order a
    // real upgrade meets them in (0003 adds tables and a column only).
    evalhub_store::MIGRATOR.run(pool).await.unwrap();
    let s = seed(pool).await;

    let applied = pool::migrate(pool).await.unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].name, RUNS_SPLIT);
    assert!(
        applied[0]
            .summary
            .starts_with("1 Eval(s), 2 version body(ies) rewritten, 4 run row(s)"),
        "{}",
        applied[0].summary
    );

    // Live bodies: no runs, schema 2.0, the header split_v1 returns, and
    // the stored hash is the hash of the stored body.
    let live: Vec<(Uuid, Vec<u8>, Value)> = sqlx::query_as(
        "SELECT version_id, content_hash, body FROM versions
         WHERE record_id = $1 AND tombstoned_at IS NULL ORDER BY seq",
    )
    .bind(s.eval_id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(live.len(), 2);
    for ((version_id, hash, body), before) in live.iter().zip([&s.v1_body, &s.v2_body]) {
        assert!(body.get("runs").is_none(), "{version_id}: {body}");
        assert_eq!(body["schema"], "evalhub.eval/2.0");
        assert_eq!(body, &split_v1(before).header);
        assert_eq!(hash.as_slice(), hash_value(body).unwrap().1.as_bytes());
    }
    let none_with_runs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM versions v JOIN records r ON r.id = v.record_id
         WHERE r.type = 'eval' AND v.body ? 'runs'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(none_with_runs, 0);
    // The tombstone is untouched.
    let v3_body: Option<Value> =
        sqlx::query_scalar("SELECT body FROM versions WHERE version_id = $1")
            .bind(s.v3)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(v3_body.is_none());

    // Run rows: the highest seq wins, converted with that version's header.
    type RunRow = (String, String, Option<String>, Value, Vec<u8>);
    let runs: BTreeMap<String, RunRow> = sqlx::query_as::<_, RunRow>(
        "SELECT run_id, status, error_kind, body, content_hash FROM runs
         WHERE record_id = $1",
    )
    .bind(s.eval_id)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|r| (r.0.clone(), r))
    .collect();
    assert_eq!(
        runs.keys().collect::<Vec<_>>(),
        ["r1", "r2", "r3", "r4"],
        "r5 lived only in the tombstoned version"
    );
    let expect = |id: &str, status: &str, kind: Option<&str>, outcome: &str, model: &str| {
        let (_, st, ek, body, _) = &runs[id];
        assert_eq!(st, status, "{id}");
        assert_eq!(ek.as_deref(), kind, "{id}");
        assert_eq!(body["status"], status, "{id}");
        assert_eq!(
            body["meta"],
            json!({"v0.2.0_migration": {"outcome": outcome}}),
            "{id}"
        );
        assert_eq!(
            body["model"],
            json!({"id": model}),
            "{id}: facets of its source"
        );
        assert_eq!(body["harness"]["name"], "my-harness", "{id}");
        assert_eq!(body["task"]["id"], "single2", "{id}");
    };
    expect("r1", "ok", None, "fail", "m2");
    expect("r2", "error", Some("unrecorded"), "error", "m2");
    expect("r3", "skipped", None, "skipped", "m1");
    expect("r4", "ok", None, "pass", "m2");
    assert_eq!(runs["r2"].3["error"], json!({"kind": "unrecorded"}));
    assert_eq!(runs["r3"].3["skip_reason"], "unrecorded");
    assert_eq!(runs["r1"].3["started_at"], "2026-09-21T10:00:00Z");

    let v1_runs = converted_hashes(&s.v1_body);
    let v2_runs = converted_hashes(&s.v2_body);
    for id in ["r1", "r2", "r4"] {
        assert_eq!(runs[id].4, v2_runs[id], "{id} from @2");
    }
    assert_eq!(runs["r3"].4, v1_runs["r3"], "r3 from @1");
    // The two versions disagree on r1 and r2 (outcome, and the model facet).
    assert_ne!(v1_runs["r1"], v2_runs["r1"]);
    assert_ne!(v1_runs["r2"], v2_runs["r2"]);

    // runs_hash over the rows.
    let stored: Vec<u8> = sqlx::query_scalar("SELECT runs_hash FROM records WHERE id = $1")
        .bind(s.eval_id)
        .fetch_one(pool)
        .await
        .unwrap();
    let pairs: Vec<(&str, evalhub_core::ContentHash)> = runs
        .values()
        .map(|r| {
            (
                r.0.as_str(),
                evalhub_core::ContentHash::from_bytes(r.4.clone().try_into().unwrap()),
            )
        })
        .collect();
    assert_eq!(stored, runs_hash(&pairs).unwrap().as_bytes().to_vec());

    // run_attachment_refs from calls, with @2's digest (r1 came from @2).
    let refs: Vec<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT run_id, path, sha256 FROM run_attachment_refs WHERE record_id = $1 ORDER BY 1, 2",
    )
    .bind(s.eval_id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        refs,
        vec![(
            "r1".to_string(),
            "calls/r1.jsonl".to_string(),
            hex::decode(SHA_CALLS_V2).unwrap()
        )]
    );
    assert_eq!(runs["r1"].3["attachments"][0]["sha256"], SHA_CALLS_V2);

    // run_fingerprints: the six facets of each run, from its stored body.
    let fps: Vec<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT run_id, facet, fingerprint FROM run_fingerprints WHERE record_id = $1",
    )
    .bind(s.eval_id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(fps.len(), 4 * 6);
    for (run_id, facet, fp) in &fps {
        let want = evalhub_core::fingerprint::for_run(&runs[run_id].3).unwrap();
        let facet: evalhub_core::fingerprint::Facet = facet.parse().unwrap();
        assert_eq!(want.get(facet).unwrap().as_slice(), fp.as_slice());
    }

    // Audit: one runs_split row per rewritten version, no actor, old and
    // new hash; the new one is the stored one.
    type AuditRow = (Option<Uuid>, Option<Uuid>, Option<String>, String, Value);
    let audit: Vec<AuditRow> = sqlx::query_as(
        "SELECT actor_user_id, actor_token_id, ns, subject, detail FROM audit
         WHERE action = 'migration.runs_split' ORDER BY subject",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(audit.len(), 2);
    for ((user, token, ns, subject, detail), (seq, version_id, old)) in audit
        .iter()
        .zip([(1, s.v1, s.v1_hash), (2, s.v2, s.v2_hash)])
    {
        assert!(user.is_none() && token.is_none());
        assert_eq!(ns.as_deref(), Some(NS));
        assert_eq!(subject, &format!("eval/alice/e@{seq}"));
        assert_eq!(detail["version_id"], version_id.to_string());
        assert_eq!(detail["old_content_hash"], hex::encode(old));
        let new: Vec<u8> =
            sqlx::query_scalar("SELECT content_hash FROM versions WHERE version_id = $1")
                .bind(version_id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(detail["new_content_hash"], hex::encode(&new));
        assert_ne!(new.as_slice(), old.as_slice());
    }

    // Cards: the used set from the pointed-at version, with its hashes.
    // r4 is in no version c1 points at but has a row: it takes the row's
    // current hash (the stage 5 rule), so it is used and never "changed".
    let c1 = card_eval_runs(pool, s.c1).await;
    assert_eq!(
        c1,
        [
            ("r1", &v1_runs["r1"]),
            ("r2", &v1_runs["r2"]),
            ("r4", &runs["r4"].4)
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect::<BTreeMap<_, _>>(),
        "attrs.runs of c1, hashed as @1 had them, r4 as its row; ghost skipped"
    );
    assert!(!v1_runs.contains_key("r4"));
    let c2 = card_eval_runs(pool, s.c2).await;
    assert_eq!(c2, v1_runs, "every run of @1, not of the record");
    assert!(
        card_eval_runs(pool, s.c3).await.is_empty(),
        "c3 is on a tombstone"
    );

    let unmatched: Vec<(Option<Uuid>, Option<String>, String, Value)> = sqlx::query_as(
        "SELECT actor_user_id, ns, subject, detail FROM audit
         WHERE action = 'migration.card_runs_unmatched'",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(unmatched.len(), 1);
    let (actor, ns, subject, detail) = &unmatched[0];
    assert!(actor.is_none());
    assert_eq!(ns.as_deref(), Some(NS));
    assert_eq!(subject, "card/alice/c1@1");
    assert_eq!(
        detail,
        &json!({"card_version_id": s.c1.to_string(), "eval": "alice/e", "run_ids": ["ghost"]})
    );
    let names_r4: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit
         WHERE action = 'migration.card_runs_unmatched' AND detail->'run_ids' ? 'r4'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(
        names_r4, 0,
        "r4 matched a row; no unmatched audit row names it"
    );

    // The comparison view still lists every migrated Card, and the runs @2
    // overwrote show as changed since the Card judged them.
    let caller = vec![NS.to_string()];
    let on_v1 = relations::cards_using_eval(pool, s.v1, &caller)
        .await
        .unwrap();
    let names: Vec<&str> = on_v1.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["c1", "c2"]);
    assert_eq!(
        on_v1[0].changed_since_card,
        ["r1", "r2"],
        "r4 took its row's hash"
    );
    assert_eq!(on_v1[0].runs_used, 3);
    assert_eq!(on_v1[1].changed_since_card, ["r1", "r2"], "r3 is unchanged");
    assert_eq!(on_v1[1].runs_used, 3);
    let on_v3 = relations::cards_using_eval(pool, s.v3, &caller)
        .await
        .unwrap();
    assert_eq!(on_v3.len(), 1);
    assert_eq!(on_v3[0].name, "c3");
    assert_eq!(on_v3[0].runs_used, 0);
    let on_v2 = relations::cards_using_eval(pool, s.v2, &caller)
        .await
        .unwrap();
    assert!(on_v2.is_empty());

    // Bookkeeping, and a second run changes nothing.
    let done: Vec<String> = sqlx::query_scalar("SELECT name FROM data_migrations")
        .fetch_all(pool)
        .await
        .unwrap();
    assert_eq!(done, [RUNS_SPLIT]);
    assert!(pool::pending_migrations(pool).await.unwrap().is_empty());
    let before = snapshot(pool).await;
    assert!(pool::migrate(pool).await.unwrap().is_empty());
    assert_eq!(snapshot(pool).await, before);
}

#[tokio::test]
async fn pending_migrations_names_the_data_step_until_it_has_run() {
    let db = common::db_unmigrated().await;
    let pool = &db.pool;

    // Nothing applied: every SQL migration, then the data step.
    let pending = pool::pending_migrations(pool).await.unwrap();
    assert_eq!(
        pending.last(),
        Some(&PendingMigration::Data { name: RUNS_SPLIT })
    );
    assert!(pending.iter().any(|p| matches!(
        p,
        PendingMigration::Sql { version: 3, description } if description == "runs"
    )));

    // Only the DDL applied: the data step alone.
    evalhub_store::MIGRATOR.run(pool).await.unwrap();
    let pending = pool::pending_migrations(pool).await.unwrap();
    assert_eq!(pending, [PendingMigration::Data { name: RUNS_SPLIT }]);
    assert_eq!(pending[0].to_string(), "data 0003_runs_split");

    // The runner applies it, once, on an empty database too.
    let applied = pool::migrate(pool).await.unwrap();
    assert_eq!(
        applied.iter().map(|a| a.name).collect::<Vec<_>>(),
        ALL.to_vec()
    );
    assert!(pool::pending_migrations(pool).await.unwrap().is_empty());
    assert!(pool::migrate(pool).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_invalid_run_id_stops_the_migration_and_leaves_the_database_untouched() {
    let db = common::db_unmigrated().await;
    let pool = &db.pool;
    evalhub_store::MIGRATOR.run(pool).await.unwrap();
    let s = seed(pool).await;
    // A second Eval carrying a run_id the stage 1 format refuses, and one
    // element without any run_id.
    let bad_id = record(pool, "eval", "bad").await;
    let bad_body = eval_v1_body(
        "bad",
        "m1",
        json!([{"run_id": "a/b", "outcome": "pass"}, {"outcome": "pass"}]),
        json!([]),
    );
    let (bad_v1, _) = version(pool, bad_id, 1, &bad_body).await;
    let before = snapshot(pool).await;

    let err = pool::migrate(pool).await.unwrap_err();
    let StoreError::DataMigrationRefused { name, problems } = &err else {
        panic!("expected DataMigrationRefused, got {err:?}");
    };
    assert_eq!(*name, RUNS_SPLIT);
    assert_eq!(problems.len(), 2, "{problems:#?}");
    assert!(problems[0].starts_with("eval/alice/bad@1: runs[0].run_id \"a/b\""));
    assert!(problems[1].starts_with("eval/alice/bad@1: runs[1] has no string run_id"));
    assert!(err.to_string().contains("nothing was changed"));

    // Nothing moved: not the bad Eval, not the good one processed before
    // or after it, and the step is still pending.
    assert_eq!(snapshot(pool).await, before);
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM runs")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(runs, 0);
    let body: Value = sqlx::query_scalar("SELECT body FROM versions WHERE version_id = $1")
        .bind(bad_v1)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(body["schema"], "evalhub.eval/1.0");
    let good: Value = sqlx::query_scalar("SELECT body FROM versions WHERE version_id = $1")
        .bind(s.v2)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(good.get("runs").is_some());
    assert_eq!(
        pool::pending_migrations(pool).await.unwrap(),
        [PendingMigration::Data { name: RUNS_SPLIT }]
    );
}
