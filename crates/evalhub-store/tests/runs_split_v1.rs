#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The 1.0 → 2.0 conversion `0003_runs_split` applies to stored 0.1.x
//! bodies ([`evalhub_store::data_migrations::v1`]). Pure: no database,
//! no Docker.
//!
//! These tests moved here from `evalhub-core` with the conversion, when
//! release 0.4.0 stopped accepting `evalhub.eval/1.0` bodies on `POST` and
//! the migration became the conversion's only caller.

use std::path::PathBuf;

use evalhub_core::canonical::hash_value;
use evalhub_core::fingerprint::{fingerprints, for_run};
use evalhub_core::run::materialise;
use evalhub_core::validate;
use evalhub_schema::RecordKind;
use evalhub_store::data_migrations::v1::{
    SplitV1, UNRECORDED, V1_MIGRATION_META_KEY, declares_v1, split_v1,
};
use serde_json::{Value, json};

fn v1_body() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../evalhub-schema/fixtures/eval-run-set-v1.json");
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

const FACETS: [&str; 6] = ["model", "task", "harness", "generation", "trial", "env"];

#[test]
fn declares_v1_reads_only_the_identifier() {
    assert!(declares_v1(&v1_body()));
    assert!(declares_v1(&json!({"schema": "evalhub.eval/1.0"})));
    assert!(!declares_v1(
        &json!({"schema": "evalhub.eval/2.0", "runs": []})
    ));
    assert!(!declares_v1(&json!({"schema": 1})));
    assert!(!declares_v1(&json!({})));
    assert!(!declares_v1(&json!([1])));
}

fn one_run_body(outcome: &str) -> Value {
    let mut body = v1_body();
    body["runs"][0]["outcome"] = json!(outcome);
    body
}

#[test]
fn split_maps_every_outcome() {
    let cases = [
        ("pass", json!("ok"), None, None),
        ("fail", json!("ok"), None, None),
        (
            "error",
            json!("error"),
            Some(json!({"kind": UNRECORDED})),
            None,
        ),
        ("skipped", json!("skipped"), None, Some(json!(UNRECORDED))),
    ];
    for (outcome, status, error, skip_reason) in cases {
        let SplitV1 { runs, .. } = split_v1(&one_run_body(outcome));
        assert_eq!(runs.len(), 1);
        let (id, run) = &runs[0];
        assert_eq!(id, "r1");
        assert_eq!(run["status"], status, "{outcome}");
        assert_eq!(run.get("error").cloned(), error, "{outcome}");
        assert_eq!(run.get("skip_reason").cloned(), skip_reason, "{outcome}");
        assert!(
            run.get("outcome").is_none(),
            "{outcome}: outcome is not a 2.0 key"
        );
        assert_eq!(
            run["meta"],
            json!({ V1_MIGRATION_META_KEY: { "outcome": outcome } }),
            "{outcome}: the dropped outcome is kept verbatim"
        );
        // Every converted run is a valid 2.0 run under its own id.
        assert_eq!(validate::run(id, run), vec![], "{outcome}");
    }
    assert_eq!(V1_MIGRATION_META_KEY, "v0.2.0_migration");
}

#[test]
fn split_produces_the_expected_run_exactly() {
    let SplitV1 {
        runs,
        duplicate_run_ids,
        ..
    } = split_v1(&v1_body());
    assert!(duplicate_run_ids.is_empty());
    assert_eq!(
        runs,
        vec![(
            "r1".to_string(),
            json!({
                "run_id": "r1",
                "status": "ok",
                "started_at": "2026-09-20T10:00:00Z",
                "ended_at": "2026-09-20T10:02:11Z",
                "calls": "calls/r1.jsonl",
                "artifacts": ["artifacts/r1/diff.patch"],
                "attachments": [
                    {"path": "calls/r1.jsonl", "sha256": "a8b6c5d4e3f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a7b6", "size": 90210, "media_type": "application/x-ndjson"},
                    {"path": "artifacts/r1/diff.patch", "sha256": "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0", "size": 3311, "media_type": "text/x-diff"}
                ],
                "meta": {"v0.2.0_migration": {"outcome": "pass"}},
                "harness": {"name": "my-harness", "version": "0.4.1"},
                "model": {"id": "qwen3.6-32b"},
                "task": {"id": "single2", "version": "3", "split": "test", "n": 33},
                "generation": {"temperature": 0.0},
                "trial": {"k": 4},
                "env": {"git": {"origin": "https://github.com/x/y", "commit": "abc123", "dirty": false}}
            })
        )]
    );
}

#[test]
fn split_copies_the_header_facets_onto_each_run() {
    let body = v1_body();
    let SplitV1 { runs, .. } = split_v1(&body);
    let run = &runs[0].1;
    for facet in FACETS {
        assert_eq!(run[facet], body[facet], "{facet}");
    }
    // Materialisation is already done: the write path's call finds nothing.
    let mut again = run.clone();
    materialise(&mut again, &body);
    assert_eq!(&again, run);
    // The run's fingerprints are the header's on every facet it inherited.
    let from_run = for_run(run).unwrap();
    let from_header = fingerprints(RecordKind::Eval, &body).unwrap();
    assert_eq!(from_run, from_header);
}

#[test]
fn split_copies_only_the_attachments_a_run_names_once_each_in_header_order() {
    let mut body = v1_body();
    let s = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    body["attachments"] = json!([
        {"path": "b.txt", "sha256": s, "size": 1},
        {"path": "unrelated.txt", "sha256": s, "size": 1},
        {"path": "a.jsonl", "sha256": s, "size": 1},
    ]);
    body["runs"] = json!([
        {"run_id": "r1", "outcome": "pass", "calls": "a.jsonl", "artifacts": ["b.txt", "a.jsonl"]},
        {"run_id": "r2", "outcome": "fail"},
    ]);
    let SplitV1 { header, runs, .. } = split_v1(&body);
    let paths = |run: &Value| -> Vec<String> {
        run.get("attachments")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|e| e["path"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(paths(&runs[0].1), ["b.txt", "a.jsonl"]);
    assert!(
        runs[1].1.get("attachments").is_none(),
        "names nothing, gets nothing"
    );
    // Copied, not moved: the header keeps all three.
    assert_eq!(header["attachments"], body["attachments"]);
    assert_eq!(validate::run("r1", &runs[0].1), vec![]);
}

#[test]
fn split_duplicate_run_id_last_wins_and_is_reported() {
    let mut body = v1_body();
    body["runs"] = json!([
        {"run_id": "a", "outcome": "pass"},
        {"run_id": "b", "outcome": "pass"},
        {"run_id": "a", "outcome": "error"},
        {"run_id": "a", "outcome": "skipped"},
        {"run_id": "c", "outcome": "fail"},
    ]);
    let SplitV1 {
        runs,
        duplicate_run_ids,
        ..
    } = split_v1(&body);
    let ids: Vec<&str> = runs.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"], "first-appearance order");
    assert_eq!(runs[0].1["status"], json!("skipped"), "the last `a` wins");
    assert_eq!(
        runs[0].1["meta"],
        json!({"v0.2.0_migration": {"outcome": "skipped"}})
    );
    assert_eq!(duplicate_run_ids, ["a"]);
}

#[test]
fn split_header_is_the_body_without_runs_and_hashes_as_the_same_header_posted_as_2_0() {
    let body = v1_body();
    let SplitV1 { header, .. } = split_v1(&body);

    // The same header, as a 2.0 producer would post it.
    let mut posted = body.as_object().unwrap().clone();
    posted.remove("runs");
    posted.insert("schema".into(), json!("evalhub.eval/2.0"));
    let posted = Value::Object(posted);

    assert_eq!(header, posted);
    assert_eq!(
        hash_value(&header).unwrap().1,
        hash_value(&posted).unwrap().1
    );
    assert_eq!(validate::validate(RecordKind::Eval, &header), vec![]);
    assert!(header.get("runs").is_none());
    // The input is not modified.
    assert_eq!(body, v1_body());
}

#[test]
fn split_is_deterministic_and_tolerates_what_validation_would_reject() {
    assert_eq!(split_v1(&v1_body()), split_v1(&v1_body()));

    let not_an_object = split_v1(&json!([1]));
    assert_eq!(not_an_object.header, json!([1]));
    assert!(not_an_object.runs.is_empty());

    let mut body = v1_body();
    body["runs"] = json!([
        {"outcome": "pass"},
        "r9",
        {"run_id": "r1", "outcome": "exploded"},
        {"run_id": "r2"},
    ]);
    let SplitV1 { runs, .. } = split_v1(&body);
    let ids: Vec<&str> = runs.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        ["r1", "r2"],
        "elements without a string run_id are dropped"
    );
    assert_eq!(runs[0].1["status"], json!("error"));
    assert_eq!(
        runs[0].1["meta"],
        json!({"v0.2.0_migration": {"outcome": "exploded"}})
    );
    assert_eq!(runs[1].1["status"], json!("error"));
    assert!(
        runs[1].1.get("meta").is_none(),
        "no outcome, nothing to keep"
    );
}
