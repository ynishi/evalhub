#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Cards joined to runs against a real Postgres: the used set and
//! `run_results` fixed at Card ingest, the run projection
//! (`runs::project`), the Evals withheld from a Card's judgements
//! (`relations::hidden_evals`), and the comparison view over used runs.
//! Eval headers, runs and Cards go through the store's own write paths.
//! Needs Docker.

mod common;

use serde_json::{Value, json};
use uuid::Uuid;

use evalhub_core::canonical::hash_value;
use evalhub_query::{PathTable, ir};
use evalhub_schema::RecordKind;
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_schema::query::QueryRequest;
use evalhub_store::PgPool;
use evalhub_store::auth::{self, NamespaceKind};
use evalhub_store::error::StoreError;
use evalhub_store::records::{
    self, Actor, CreateOutcome, IngestFacts, NewRelation, NewVersion, RecordType, RelationTarget,
    TombstoneReason, TombstoneRequest, Visibility,
};
use evalhub_store::relations::{self, USES_EVAL};
use evalhub_store::runs::{self, RunInclude, RunPage, RunState};

fn no_badges(_: &IngestFacts) -> Vec<String> {
    Vec::new()
}

async fn ns(pool: &PgPool, name: &str) {
    auth::ensure_namespace(pool, name, NamespaceKind::User)
        .await
        .unwrap();
}

/// Post a version of an Eval header `{ns}/{name}` with model `model`,
/// harness `h@1`, and set its visibility. Returns the record id.
async fn eval_header(
    pool: &PgPool,
    ns: &str,
    name: &str,
    title: &str,
    model: &str,
    visibility: Visibility,
) -> Uuid {
    let body = json!({
        "schema": "evalhub.eval/2.0",
        "title": title,
        "producer": {"name": "p", "version": "1"},
        "eval_kind": "run_set",
        "origin": "live",
        "model": {"id": model},
        "harness": {"name": "h", "version": "1"},
    });
    let (bytes, hash) = hash_value(&body).unwrap();
    let canonical: Value = serde_json::from_slice(&bytes).unwrap();
    let fps = evalhub_core::fingerprints(RecordKind::Eval, &canonical).unwrap();
    let fp_rows: Vec<(&str, [u8; 32])> = fps.iter().map(|(f, b)| (f.as_str(), *b)).collect();
    let out = records::create_or_append(
        pool,
        NewVersion {
            record_type: RecordType::Eval,
            ns,
            name,
            body: &canonical,
            content_hash: hash.as_bytes(),
            label: None,
            actor: Actor::default(),
            readable_ns: &[],
            fingerprints: &fp_rows,
            results: &[],
            relations: &[],
            attachments: &[],
        },
        || (Uuid::new_v4(), Uuid::new_v4()),
        &no_badges,
    )
    .await
    .unwrap();
    records::set_visibility(
        pool,
        RecordType::Eval,
        ns,
        name,
        visibility,
        Actor::default(),
    )
    .await
    .unwrap();
    out.meta().record_id
}

/// Write run `run_id` with `body` (its `run_id` is set here).
async fn put(pool: &PgPool, ns: &str, name: &str, run_id: &str, mut body: Value) {
    body["run_id"] = json!(run_id);
    runs::put(pool, ns, name, run_id, &body, Actor::default())
        .await
        .unwrap();
}

/// One `core/uses_eval` edge of a Card: `{ns}/{name}`, `@seq`, `attrs`.
type Uses<'a> = (&'a str, i32, Option<Value>);

/// Post a Card `{ns}/{name}` (model `model`, harness `h@1`) with the given
/// `core/uses_eval` edges and `run_results`, written by a party that may
/// read `readable`. Returns the outcome or the store's refusal.
async fn card(
    pool: &PgPool,
    ns: &str,
    name: &str,
    model: &str,
    uses: &[Uses<'_>],
    run_results: Value,
    readable: &[String],
) -> Result<CreateOutcome, StoreError> {
    let relations_json: Vec<Value> = uses
        .iter()
        .map(|(to, seq, attrs)| {
            let mut rel = json!({"type": USES_EVAL, "to": format!("{to}@{seq}")});
            if let Some(attrs) = attrs {
                rel["attrs"] = attrs.clone();
            }
            rel
        })
        .collect();
    let body = json!({
        "schema": "evalhub.card/1.1",
        "title": name,
        "producer": {"name": "p", "version": "1"},
        "model": {"id": model},
        "harness": {"name": "h", "version": "1"},
        "relations": relations_json,
        "run_results": run_results,
    });
    let (bytes, hash) = hash_value(&body).unwrap();
    let canonical: Value = serde_json::from_slice(&bytes).unwrap();
    let fps = evalhub_core::fingerprints(RecordKind::Card, &canonical).unwrap();
    let fp_rows: Vec<(&str, [u8; 32])> = fps.iter().map(|(f, b)| (f.as_str(), *b)).collect();
    let targets: Vec<(String, String)> = uses
        .iter()
        .map(|(to, _, _)| {
            let (n, m) = to.split_once('/').unwrap();
            (n.to_string(), m.to_string())
        })
        .collect();
    let rels: Vec<Value> = canonical["relations"].as_array().unwrap().clone();
    let relations: Vec<NewRelation<'_>> = uses
        .iter()
        .zip(&targets)
        .zip(&rels)
        .map(|(((_, seq, _), (n, m)), rel)| NewRelation {
            relation_type: USES_EVAL,
            target: RelationTarget::Version {
                ns: n,
                name: m,
                seq: *seq,
                record_type: Some(RecordType::Eval),
            },
            attrs: rel.get("attrs"),
        })
        .collect();
    records::create_or_append(
        pool,
        NewVersion {
            record_type: RecordType::Card,
            ns,
            name,
            body: &canonical,
            content_hash: hash.as_bytes(),
            label: None,
            actor: Actor::default(),
            readable_ns: readable,
            fingerprints: &fp_rows,
            results: &[],
            relations: &relations,
            attachments: &[],
        },
        || (Uuid::new_v4(), Uuid::new_v4()),
        &no_badges,
    )
    .await
}

fn judged(eval: &str, run_id: &str, value: Option<f64>, label: Option<&str>) -> Value {
    let mut v = json!({"eval": eval, "run_id": run_id, "metric": "core/pass"});
    if let Some(x) = value {
        v["value"] = json!(x);
    }
    if let Some(l) = label {
        v["label"] = json!(l);
    }
    v
}

/// `card_eval_runs` of a Card version: `(run_id, content_hash)` by run id.
async fn used(pool: &PgPool, card_version: Uuid) -> Vec<(String, Vec<u8>)> {
    sqlx::query_as(
        "SELECT run_id, content_hash FROM card_eval_runs WHERE card_version_id = $1 ORDER BY run_id",
    )
    .bind(card_version)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn run_hash(pool: &PgPool, record_id: Uuid, run_id: &str) -> Vec<u8> {
    sqlx::query_scalar("SELECT content_hash FROM runs WHERE record_id = $1 AND run_id = $2")
        .bind(record_id)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn rejected(err: StoreError) -> Vec<ErrorEntry> {
    match err {
        StoreError::CardRunsRejected(e) => e,
        other => panic!("expected CardRunsRejected, got {other}"),
    }
}

fn version_of(out: &CreateOutcome) -> Uuid {
    out.meta().version_id
}

fn alice() -> Vec<String> {
    vec!["alice".to_string()]
}

#[tokio::test]
async fn the_used_set_with_and_without_attrs_runs_and_as_a_union() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    let e = eval_header(pool, "alice", "e", "e", "m1", Visibility::Public).await;
    for id in ["r1", "r2", "r3", "r4"] {
        put(pool, "alice", "e", id, json!({"status": "ok"})).await;
    }
    runs::archive(pool, "alice", "e", "r3", Actor::default())
        .await
        .unwrap();
    runs::tombstone(
        pool,
        "alice",
        "e",
        "r4",
        TombstoneRequest {
            reason: TombstoneReason::Withdrawn,
            note: None,
        },
        Actor::default(),
    )
    .await
    .unwrap();

    // No attrs.runs: every run neither archived nor deleted, with the
    // hash it has now.
    let all = card(
        pool,
        "alice",
        "all",
        "m1",
        &[("alice/e", 1, None)],
        json!([]),
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        used(pool, version_of(&all)).await,
        vec![
            ("r1".to_string(), run_hash(pool, e, "r1").await),
            ("r2".to_string(), run_hash(pool, e, "r2").await),
        ]
    );

    // Two edges to different seqs of the one Eval: the union of their
    // attrs.runs. An archived and a deleted run are rows, so they count.
    eval_header(pool, "alice", "e", "e v2", "m1", Visibility::Public).await;
    let union = card(
        pool,
        "alice",
        "union",
        "m1",
        &[
            ("alice/e", 1, Some(json!({"runs": ["r1"]}))),
            ("alice/e", 2, Some(json!({"runs": ["r3", "r4"]}))),
        ],
        json!([
            judged("alice/e", "r3", Some(1.0), None),
            judged("alice/e", "r4", None, Some("fail")),
        ]),
        &[],
    )
    .await
    .unwrap();
    let ids: Vec<String> = used(pool, version_of(&union))
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, vec!["r1", "r3", "r4"]);
    let rows: Vec<(i32, String, Option<f64>, Option<String>)> = sqlx::query_as(
        "SELECT ordinal, run_id, value, label FROM run_results WHERE version_id = $1 ORDER BY ordinal",
    )
    .bind(version_of(&union))
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (0, "r3".to_string(), Some(1.0), None),
            (1, "r4".to_string(), None, Some("fail".to_string())),
        ]
    );

    // When only one of the edges carries attrs.runs, that one decides.
    let some = card(
        pool,
        "alice",
        "some",
        "m1",
        &[
            ("alice/e", 1, Some(json!({"runs": ["r2"]}))),
            ("alice/e", 2, None),
        ],
        json!([]),
        &[],
    )
    .await
    .unwrap();
    let ids: Vec<String> = used(pool, version_of(&some))
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ids, vec!["r2"]);

    // An edge that does not resolve has no used set.
    let ext = card(
        pool,
        "alice",
        "ext",
        "m1",
        &[("alice/later", 1, None)],
        json!([]),
        &[],
    )
    .await
    .unwrap();
    assert!(used(pool, version_of(&ext)).await.is_empty());
}

#[tokio::test]
async fn run_unknown_reads_the_same_for_a_missing_run_and_an_invisible_evals_run() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    ns(pool, "bob").await;
    eval_header(pool, "bob", "secret", "s", "m1", Visibility::Private).await;
    put(pool, "bob", "secret", "r1", json!({"status": "ok"})).await;

    // Alice may not see bob/secret: its run r1 exists, and the answer is
    // the answer for a run that does not.
    let invisible = card(
        pool,
        "alice",
        "probe",
        "m1",
        &[("bob/secret", 1, None)],
        json!([judged("bob/secret", "r1", Some(1.0), None)]),
        &alice(),
    )
    .await
    .unwrap_err();
    let invisible = rejected(invisible);
    assert_eq!(invisible.len(), 1);
    assert_eq!(invisible[0].path, "/run_results/0/run_id");
    assert_eq!(invisible[0].code, ErrorCode::RunUnknown);
    // Nothing was written, the name included.
    assert!(
        records::get_latest(pool, RecordType::Card, "alice", "probe", &alice())
            .await
            .unwrap()
            .is_none()
    );

    // An Eval that does not exist at all.
    let absent = rejected(
        card(
            pool,
            "alice",
            "probe",
            "m1",
            &[("bob/nothing", 1, None)],
            json!([judged("bob/nothing", "r1", Some(1.0), None)]),
            &alice(),
        )
        .await
        .unwrap_err(),
    );

    // Now visible, and the run is missing from it.
    records::set_visibility(
        pool,
        RecordType::Eval,
        "bob",
        "secret",
        Visibility::Public,
        Actor::default(),
    )
    .await
    .unwrap();
    let missing = rejected(
        card(
            pool,
            "alice",
            "probe",
            "m1",
            &[("bob/secret", 1, None)],
            json!([judged("bob/secret", "r9", Some(1.0), None)]),
            &alice(),
        )
        .await
        .unwrap_err(),
    );
    let bytes = |e: &Vec<ErrorEntry>| serde_json::to_vec(e).unwrap();
    assert_eq!(bytes(&invisible), bytes(&missing));
    assert_eq!(bytes(&absent), bytes(&missing));

    // A used set naming a run with no row is refused at its position.
    let attrs = rejected(
        card(
            pool,
            "alice",
            "probe",
            "m1",
            &[("bob/secret", 1, Some(json!({"runs": ["r1", "r9"]})))],
            json!([]),
            &alice(),
        )
        .await
        .unwrap_err(),
    );
    assert_eq!(attrs.len(), 1, "{attrs:?}");
    assert_eq!(attrs[0].path, "/relations/0/attrs/runs/1");
    assert_eq!(attrs[0].code, ErrorCode::RunUnknown);
    assert_eq!(attrs[0].hint, missing[0].hint);
}

#[tokio::test]
async fn a_judgement_must_lie_inside_the_used_set() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    eval_header(pool, "alice", "e", "e", "m1", Visibility::Public).await;
    for id in ["r1", "r2", "r3"] {
        put(pool, "alice", "e", id, json!({"status": "ok"})).await;
    }
    runs::archive(pool, "alice", "e", "r3", Actor::default())
        .await
        .unwrap();

    // r2 is a row, but not one the Card said it used.
    let named = rejected(
        card(
            pool,
            "alice",
            "c",
            "m1",
            &[("alice/e", 1, Some(json!({"runs": ["r1"]})))],
            json!([
                judged("alice/e", "r1", Some(1.0), None),
                judged("alice/e", "r2", Some(0.0), None),
                judged("alice/e", "nope", Some(0.0), None),
            ]),
            &[],
        )
        .await
        .unwrap_err(),
    );
    let got: Vec<(&str, ErrorCode)> = named.iter().map(|e| (e.path.as_str(), e.code)).collect();
    assert_eq!(
        got,
        vec![
            ("/run_results/1/run_id", ErrorCode::RunNotInUsedSet),
            ("/run_results/2/run_id", ErrorCode::RunUnknown),
        ]
    );

    // Without attrs.runs the used set is the live runs: the archived r3 is
    // a row outside it.
    let live = rejected(
        card(
            pool,
            "alice",
            "c",
            "m1",
            &[("alice/e", 1, None)],
            json!([judged("alice/e", "r3", Some(1.0), None)]),
            &[],
        )
        .await
        .unwrap_err(),
    );
    assert_eq!(live[0].code, ErrorCode::RunNotInUsedSet);

    // Named explicitly, the archived run is inside, and the Card lands.
    card(
        pool,
        "alice",
        "c",
        "m1",
        &[("alice/e", 1, Some(json!({"runs": ["r3"]})))],
        json!([judged("alice/e", "r3", Some(1.0), None)]),
        &[],
    )
    .await
    .unwrap();
}

/// A query request compiled the way the server compiles it.
fn run_query(cards: &[&str], request: Value, cursor: Option<ir::RunCursor>) -> ir::RunQuery {
    let cards: Vec<ir::CardRef> = cards.iter().map(|c| c.parse().unwrap()).collect();
    let request: QueryRequest = serde_json::from_value(request).unwrap();
    let limit = request.limit.unwrap_or(50);
    evalhub_query::compile_runs(&request, &PathTable::for_runs(&cards), limit, cursor).unwrap()
}

async fn project(
    pool: &PgPool,
    record_id: Uuid,
    cards: &[&str],
    request: Value,
    include: RunInclude,
    caller: &[String],
) -> RunPage {
    runs::project(
        pool,
        record_id,
        &run_query(cards, request, None),
        include,
        caller,
    )
    .await
    .unwrap()
    .unwrap()
}

fn ids(page: &RunPage) -> Vec<&str> {
    page.runs.iter().map(|r| r.run_id.as_str()).collect()
}

/// Every run id the request returns, following cursors page by page.
async fn all_pages(
    pool: &PgPool,
    record_id: Uuid,
    cards: &[&str],
    request: Value,
) -> Vec<Vec<String>> {
    let mut pages = Vec::new();
    let mut cursor = None;
    loop {
        let q = run_query(cards, request.clone(), cursor);
        let page = runs::project(pool, record_id, &q, RunInclude::default(), &alice())
            .await
            .unwrap()
            .unwrap();
        pages.push(page.runs.iter().map(|r| r.run_id.clone()).collect());
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
        assert!(pages.len() < 10, "paging does not end: {pages:?}");
    }
    pages
}

#[tokio::test]
async fn the_projection_filters_sorts_and_pages() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    let e = eval_header(pool, "alice", "e", "e", "m1", Visibility::Public).await;
    put(
        pool,
        "alice",
        "e",
        "r1",
        json!({"status": "ok", "started_at": "2026-09-20T10:00:00Z",
               "metrics": {"core/tokens": 10.0}}),
    )
    .await;
    put(
        pool,
        "alice",
        "e",
        "r2",
        json!({"status": "error", "error": {"kind": "timeout"}, "metrics": {"core/tokens": 30.0}}),
    )
    .await;
    put(
        pool,
        "alice",
        "e",
        "r3",
        json!({"status": "ok", "model": {"id": "m2"}, "started_at": "2026-09-19T10:00:00Z"}),
    )
    .await;
    put(
        pool,
        "alice",
        "e",
        "r4",
        json!({"status": "skipped", "skip_reason": "budget", "metrics": {"core/tokens": 20.0}}),
    )
    .await;
    put(
        pool,
        "alice",
        "e",
        "r5",
        json!({"status": "error", "error": {"kind": "crash"}, "metrics": {"core/tokens": 5.0}}),
    )
    .await;
    card(
        pool,
        "alice",
        "judge",
        "m1",
        &[("alice/e", 1, None)],
        json!([
            judged("alice/e", "r1", Some(0.9), Some("pass")),
            judged("alice/e", "r2", Some(0.1), Some("fail")),
            judged("alice/e", "r4", Some(0.5), None),
            judged("alice/e", "r5", None, Some("fail")),
        ]),
        &[],
    )
    .await
    .unwrap();
    let judge = ["alice/judge"];
    let filter = |w: Value| json!({"where": w});
    let none = RunInclude::default();

    let page = project(pool, e, &judge, json!({}), none, &alice()).await;
    assert_eq!(ids(&page), vec!["r1", "r2", "r3", "r4", "r5"]);
    let r1 = &page.runs[0];
    assert_eq!(r1.state, RunState::Live);
    let detail = r1.detail.as_ref().unwrap();
    assert_eq!(detail.status, "ok");
    assert_eq!(detail.metrics.get("core/tokens"), Some(&10.0));
    assert_eq!(detail.fingerprints.len(), 6);
    assert_eq!(r1.cards.len(), 1);
    assert!(r1.cards[0].used);
    assert_eq!(r1.cards[0].results[0].value, Some(0.9));
    assert_eq!(r1.cards[0].results[0].label.as_deref(), Some("pass"));
    assert!(
        page.runs[2].cards[0].results.is_empty(),
        "r3 was not judged"
    );
    assert_eq!(page.cards[0].runs_used, 5);

    for (w, want) in [
        (
            json!({"path": "status", "op": "eq", "value": "ok"}),
            vec!["r1", "r3"],
        ),
        (
            json!({"path": "error.kind", "op": "eq", "value": "timeout"}),
            vec!["r2"],
        ),
        (
            json!({"path": "model.id", "op": "eq", "value": "m2"}),
            vec!["r3"],
        ),
        (
            json!({"path": "model.id", "op": "eq", "value": "m1"}),
            vec!["r1", "r2", "r4", "r5"],
        ),
        (
            json!({"path": "metrics[core/tokens]", "op": "gt", "value": 9}),
            vec!["r1", "r2", "r4"],
        ),
        (
            json!({"path": "results[alice/judge][core/pass].value", "op": "gte", "value": 0.5}),
            vec!["r1", "r4"],
        ),
        (
            json!({"path": "results[alice/judge][core/pass].label", "op": "eq", "value": "fail"}),
            vec!["r2", "r5"],
        ),
        (
            json!({"path": "started_at", "op": "gte", "value": "2026-09-20T00:00:00Z"}),
            vec!["r1"],
        ),
        (
            json!({"not": {"path": "status", "op": "in", "value": ["ok", "skipped"]}}),
            vec!["r2", "r5"],
        ),
    ] {
        let page = project(pool, e, &judge, filter(w.clone()), none, &alice()).await;
        assert_eq!(ids(&page), want, "{w}");
    }

    // Sorted on a metric, descending: the run without it first (NULLS
    // FIRST), and paging over it loses no row.
    let pages = all_pages(
        pool,
        e,
        &judge,
        json!({"sort": [{"path": "metrics[core/tokens]", "dir": "desc"}], "limit": 2}),
    )
    .await;
    assert_eq!(
        pages,
        vec![vec!["r3", "r2"], vec!["r4", "r1"], vec!["r5"]],
        "{pages:?}"
    );
    // Ascending on a judgement: runs without one last, by run_id.
    let pages = all_pages(
        pool,
        e,
        &judge,
        json!({"sort": [{"path": "results[alice/judge][core/pass].value", "dir": "asc"}], "limit": 2}),
    )
    .await;
    assert_eq!(
        pages,
        vec![vec!["r2", "r4"], vec!["r1", "r3"], vec!["r5"]],
        "{pages:?}"
    );
    // Two keys: status, then error.kind descending.
    let pages = all_pages(
        pool,
        e,
        &judge,
        json!({"sort": [{"path": "status", "dir": "asc"}, {"path": "error.kind", "dir": "desc"}],
               "limit": 3}),
    )
    .await;
    // Within `ok` both kinds are NULL, and the run_id tie-break follows
    // the last key's direction (descending).
    assert_eq!(
        pages,
        vec![vec!["r2", "r5", "r3"], vec!["r1", "r4"]],
        "{pages:?}"
    );
    // A facet key as a sort key.
    let pages = all_pages(
        pool,
        e,
        &judge,
        json!({"sort": [{"path": "model.id", "dir": "desc"}], "limit": 4}),
    )
    .await;
    assert_eq!(pages, vec![vec!["r3", "r5", "r4", "r2"], vec!["r1"]]);
}

#[tokio::test]
async fn an_overwrite_shows_in_changed_since_card_and_moves_the_used_set_hash() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    let e = eval_header(pool, "alice", "e", "e", "m1", Visibility::Public).await;
    put(pool, "alice", "e", "r1", json!({"status": "ok"})).await;
    put(pool, "alice", "e", "r2", json!({"status": "ok"})).await;
    let posted = card(
        pool,
        "alice",
        "judge",
        "m1",
        &[("alice/e", 1, None)],
        json!([judged("alice/e", "r1", Some(1.0), None)]),
        &[],
    )
    .await
    .unwrap();
    let before_r1 = run_hash(pool, e, "r1").await;

    let page = project(
        pool,
        e,
        &["alice/judge"],
        json!({}),
        RunInclude::default(),
        &alice(),
    )
    .await;
    let c = &page.cards[0];
    assert!(c.changed_since_card.is_empty());
    assert_eq!(c.used_set_hash, c.posted_used_set_hash);
    let before = c.used_set_hash.clone();
    // The used-set hash is the core formula over the used runs.
    let expect = evalhub_core::run::runs_hash(&[
        (
            "r1",
            evalhub_core::ContentHash::from_bytes(before_r1.clone().try_into().unwrap()),
        ),
        (
            "r2",
            evalhub_core::ContentHash::from_bytes(
                run_hash(pool, e, "r2").await.try_into().unwrap(),
            ),
        ),
    ])
    .unwrap();
    assert_eq!(before, expect.as_bytes().to_vec());

    // Overwrite r1.
    put(
        pool,
        "alice",
        "e",
        "r1",
        json!({"status": "ok", "meta": {"again": true}}),
    )
    .await;
    assert_eq!(
        used(pool, version_of(&posted)).await[0],
        ("r1".to_string(), before_r1),
        "card_eval_runs keeps the hash at posting time"
    );
    let page = project(
        pool,
        e,
        &["alice/judge"],
        json!({}),
        RunInclude::default(),
        &alice(),
    )
    .await;
    let c = &page.cards[0];
    assert_eq!(c.changed_since_card, vec!["r1"]);
    assert_ne!(c.used_set_hash, before);
    assert_eq!(c.posted_used_set_hash, before);
    assert!(page.runs[0].cards[0].changed);
    assert!(!page.runs[1].cards[0].changed);

    // The comparison view says the same.
    let eval_version = records::get_latest(pool, RecordType::Eval, "alice", "e", &[])
        .await
        .unwrap()
        .unwrap()
        .meta
        .version_id;
    let rows = relations::cards_using_eval(pool, eval_version, &alice())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].runs_used, 2);
    assert_eq!(rows[0].changed_since_card, vec!["r1"]);
    assert_eq!(rows[0].used_set_hash, c.used_set_hash);
}

#[tokio::test]
async fn archived_rows_are_for_members_and_used_runs_are_marked() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    let e = eval_header(pool, "alice", "e", "e", "m1", Visibility::Public).await;
    for (id, tokens) in [("r1", 1.0), ("r2", 2.0), ("r3", 3.0), ("r4", 4.0)] {
        put(
            pool,
            "alice",
            "e",
            id,
            json!({"status": "ok", "metrics": {"core/tokens": tokens}}),
        )
        .await;
    }
    card(
        pool,
        "alice",
        "judge",
        "m1",
        &[("alice/e", 1, Some(json!({"runs": ["r1", "r2", "r3"]})))],
        json!([
            judged("alice/e", "r2", Some(1.0), None),
            judged("alice/e", "r3", Some(0.0), None),
        ]),
        &[],
    )
    .await
    .unwrap();
    // Public, so that an outsider may name it.
    records::set_visibility(
        pool,
        RecordType::Card,
        "alice",
        "judge",
        Visibility::Public,
        Actor::default(),
    )
    .await
    .unwrap();
    runs::archive(pool, "alice", "e", "r2", Actor::default())
        .await
        .unwrap();
    runs::tombstone(
        pool,
        "alice",
        "e",
        "r3",
        TombstoneRequest {
            reason: TombstoneReason::Other,
            note: None,
        },
        Actor::default(),
    )
    .await
    .unwrap();
    runs::archive(pool, "alice", "e", "r4", Actor::default())
        .await
        .unwrap();
    let both = RunInclude {
        archived: true,
        deleted: true,
    };
    let archived = RunInclude {
        archived: true,
        deleted: false,
    };

    // No Card: only live rows, unless a member asks.
    let page = project(pool, e, &[], json!({}), RunInclude::default(), &alice()).await;
    assert_eq!(ids(&page), vec!["r1"]);
    let page = project(pool, e, &[], json!({}), archived, &alice()).await;
    assert_eq!(ids(&page), vec!["r1", "r2", "r4"]);
    assert_eq!(page.runs[1].state, RunState::Archived);
    assert!(page.runs[1].detail.is_some());
    let page = project(pool, e, &[], json!({}), both, &alice()).await;
    assert_eq!(ids(&page), vec!["r1", "r2", "r3", "r4"]);
    assert_eq!(page.runs[2].state, RunState::Deleted);
    assert!(page.runs[2].detail.is_none());
    let page = project(pool, e, &[], json!({}), both, &[]).await;
    assert_eq!(ids(&page), vec!["r1"], "include is a member's option");

    // With the Card, its used runs appear whatever their state. An outsider
    // sees the archived r2 exactly as the deleted r3.
    let page = project(pool, e, &["alice/judge"], json!({}), both, &[]).await;
    assert_eq!(ids(&page), vec!["r1", "r2", "r3"]);
    let (r2, r3) = (&page.runs[1], &page.runs[2]);
    assert_eq!(r2.state, RunState::Deleted);
    assert_eq!(r3.state, RunState::Deleted);
    assert!(r2.detail.is_none() && r3.detail.is_none());
    assert_eq!(
        r2.cards[0].results[0].value,
        Some(1.0),
        "judgements follow the Card"
    );
    let page = project(
        pool,
        e,
        &["alice/judge"],
        json!({}),
        RunInclude::default(),
        &alice(),
    )
    .await;
    assert_eq!(ids(&page), vec!["r1", "r2", "r3"]);
    assert_eq!(page.runs[1].state, RunState::Archived);
    assert_eq!(page.runs[2].state, RunState::Deleted);

    // A masked row cannot be found by what it hides.
    let page = project(
        pool,
        e,
        &["alice/judge"],
        json!({"where": {"path": "metrics[core/tokens]", "op": "eq", "value": 2}}),
        both,
        &[],
    )
    .await;
    assert!(page.runs.is_empty(), "{:?}", ids(&page));
    let page = project(
        pool,
        e,
        &["alice/judge"],
        json!({"where": {"path": "metrics[core/tokens]", "op": "eq", "value": 2}}),
        both,
        &alice(),
    )
    .await;
    assert_eq!(ids(&page), vec!["r2"]);
}

#[tokio::test]
async fn a_card_the_caller_may_not_use_here_is_unknown() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    let e = eval_header(pool, "alice", "e", "e", "m1", Visibility::Public).await;
    eval_header(pool, "alice", "other", "o", "m1", Visibility::Public).await;
    put(pool, "alice", "e", "r1", json!({"status": "ok"})).await;
    card(
        pool,
        "alice",
        "private",
        "m1",
        &[("alice/e", 1, None)],
        json!([]),
        &[],
    )
    .await
    .unwrap();
    card(
        pool,
        "alice",
        "elsewhere",
        "m1",
        &[("alice/other", 1, None)],
        json!([]),
        &[],
    )
    .await
    .unwrap();
    // Cards are private by default; publish "elsewhere" so only its edge
    // makes it unknown.
    records::set_visibility(
        pool,
        RecordType::Card,
        "alice",
        "elsewhere",
        Visibility::Public,
        Actor::default(),
    )
    .await
    .unwrap();

    let unknown = |cards: &'static [&'static str], caller: Vec<String>| async move {
        match runs::project(
            pool,
            e,
            &run_query(cards, json!({}), None),
            RunInclude::default(),
            &caller,
        )
        .await
        {
            Err(StoreError::RunCardsUnknown(names)) => names,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(
        unknown(&["alice/private"], Vec::new()).await,
        vec!["alice/private"]
    );
    assert_eq!(
        unknown(&["alice/nobody"], Vec::new()).await,
        vec!["alice/nobody"]
    );
    assert_eq!(
        unknown(&["alice/elsewhere"], Vec::new()).await,
        vec!["alice/elsewhere"]
    );
    // The owner may name the private Card.
    let page = project(
        pool,
        e,
        &["alice/private"],
        json!({}),
        RunInclude::default(),
        &alice(),
    )
    .await;
    assert_eq!(page.cards[0].card.as_str(), "alice/private");

    // An Eval the caller may not see reads as absent.
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
        runs::project(
            pool,
            e,
            &run_query(&[], json!({}), None),
            RunInclude::default(),
            &[]
        )
        .await
        .unwrap()
        .is_none()
    );
}

#[tokio::test]
async fn hidden_evals_of_a_public_card_citing_a_private_eval() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    let secret = eval_header(pool, "alice", "secret", "s", "m1", Visibility::Private).await;
    eval_header(pool, "alice", "open", "o", "m1", Visibility::Public).await;
    put(pool, "alice", "secret", "r1", json!({"status": "ok"})).await;
    put(pool, "alice", "open", "r1", json!({"status": "ok"})).await;
    let c = card(
        pool,
        "alice",
        "pub",
        "m1",
        &[("alice/secret", 1, None), ("alice/open", 1, None)],
        json!([
            judged("alice/secret", "r1", Some(1.0), None),
            judged("alice/open", "r1", Some(1.0), None),
            {"eval": "alice/secret", "run_id": "r1", "metric": "core/other", "value": 2.0},
        ]),
        &[],
    )
    .await
    .unwrap();
    records::set_visibility(
        pool,
        RecordType::Card,
        "alice",
        "pub",
        Visibility::Public,
        Actor::default(),
    )
    .await
    .unwrap();
    let v = version_of(&c);

    let hidden = relations::hidden_evals(pool, &[v], &[]).await.unwrap();
    assert_eq!(hidden.len(), 1);
    let h = &hidden[&v];
    assert_eq!(h.len(), 1, "{h:?}");
    assert_eq!(
        (
            h[0].eval_record_id,
            h[0].ns.as_str(),
            h[0].name.as_str(),
            h[0].run_results
        ),
        (secret, "alice", "secret", 2)
    );
    assert!(
        relations::hidden_evals(pool, &[v], &alice())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn same_model_compares_the_card_with_every_used_run() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    eval_header(pool, "alice", "e", "e", "m1", Visibility::Public).await;
    // r1 takes the header's model; r2 names another.
    put(pool, "alice", "e", "r1", json!({"status": "ok"})).await;
    put(
        pool,
        "alice",
        "e",
        "r2",
        json!({"status": "ok", "model": {"id": "m2"}}),
    )
    .await;
    for (name, uses) in [("only-r1", Some(json!({"runs": ["r1"]}))), ("both", None)] {
        card(
            pool,
            "alice",
            name,
            "m1",
            &[("alice/e", 1, uses)],
            json!([]),
            &[],
        )
        .await
        .unwrap();
    }
    // An Eval without runs: the header is compared, as before runs were rows.
    eval_header(pool, "alice", "bare", "b", "m1", Visibility::Public).await;
    card(
        pool,
        "alice",
        "on-bare",
        "m1",
        &[("alice/bare", 1, None)],
        json!([]),
        &[],
    )
    .await
    .unwrap();

    let latest = |name: &'static str| async move {
        records::get_latest(pool, RecordType::Eval, "alice", name, &[])
            .await
            .unwrap()
            .unwrap()
            .meta
            .version_id
    };
    let rows = relations::cards_using_eval(pool, latest("e").await, &alice())
        .await
        .unwrap();
    let row = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
    assert!(row("only-r1").same_model);
    assert!(row("only-r1").same_harness);
    assert_eq!(row("only-r1").runs_used, 1);
    assert!(!row("both").same_model, "r2's model differs");
    assert!(row("both").same_harness);
    assert_eq!(row("both").runs_used, 2);
    assert!(row("both").changed_since_card.is_empty());

    let rows = relations::cards_using_eval(pool, latest("bare").await, &alice())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].same_model && rows[0].same_harness);
    assert_eq!(rows[0].runs_used, 0);
}

#[tokio::test]
async fn a_later_uses_eval_edge_only_grows_the_used_set() {
    let db = common::db().await;
    let pool = &db.pool;
    ns(pool, "alice").await;
    let e = eval_header(pool, "alice", "e", "e", "m1", Visibility::Public).await;
    put(pool, "alice", "e", "r1", json!({"status": "ok"})).await;
    put(pool, "alice", "e", "r2", json!({"status": "ok"})).await;
    // No attrs.runs: the used set is every live run, r1 and r2.
    let posted = card(
        pool,
        "alice",
        "c",
        "m1",
        &[("alice/e", 1, None)],
        json!([]),
        &[],
    )
    .await
    .unwrap();
    let v = version_of(&posted);
    let before = used(pool, v).await;
    assert_eq!(
        before.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        vec!["r1", "r2"]
    );

    // r1 is overwritten and rX written after the Card was posted.
    put(
        pool,
        "alice",
        "e",
        "r1",
        json!({"status": "ok", "meta": {"again": true}}),
    )
    .await;
    put(pool, "alice", "e", "rX", json!({"status": "ok"})).await;
    eval_header(pool, "alice", "e", "e v2", "m1", Visibility::Public).await;

    // A second edge, to another seq, naming rX only. Under the ingest's
    // rule from scratch the set would become {rX}; the recorded runs stay.
    relations::add(
        pool,
        v,
        USES_EVAL,
        relations::RelationTarget::parse("alice/e@2").unwrap(),
        Some(&json!({"runs": ["rX"]})),
        (None, None),
        &[],
    )
    .await
    .unwrap();
    let after = used(pool, v).await;
    assert_eq!(
        after,
        vec![
            before[0].clone(),
            before[1].clone(),
            ("rX".to_string(), run_hash(pool, e, "rX").await),
        ],
        "old rows keep their posting-time hashes; rX is added"
    );
    assert_ne!(
        after[0].1,
        run_hash(pool, e, "r1").await,
        "r1 was overwritten since"
    );
}
