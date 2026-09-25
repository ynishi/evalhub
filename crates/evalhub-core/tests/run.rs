#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Runs: facet materialisation, the 1.0 → 2.0 split, the run hash formulas
//! and the run fingerprints.
//!
//! The expected hashes are literals so that a client implementing the
//! formulas in `evalhub_core::run` can check itself against them. They were
//! computed outside this crate: the canonical bytes are written out in each
//! test (for these inputs RFC 8785 is "keys sorted, no whitespace, integers
//! without a fraction"), and the digest is SHA-256 of those bytes.

use std::path::PathBuf;

use evalhub_core::canonical::{ContentHash, canonicalize, hash_value};
use evalhub_core::eval::{SplitV1, UNRECORDED, V1_MIGRATION_META_KEY, split_v1};
use evalhub_core::fingerprint::{Facet, fingerprints, for_run};
use evalhub_core::run::{materialise, run_content_hash, runs_hash};
use evalhub_core::validate;
use evalhub_schema::RecordKind;
use serde_json::{Value, json};

fn schema_fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../evalhub-schema/fixtures")
        .join(name);
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

fn v1_body() -> Value {
    schema_fixture("eval-run-set-v1.json")
}

const FACETS: [&str; 6] = ["model", "task", "harness", "generation", "trial", "env"];

// ---------------------------------------------------------------- materialise

#[test]
fn materialise_copies_an_omitted_facet_and_keeps_the_runs_own() {
    let header = json!({
        "model": {"id": "header-model"},
        "task": {"id": "single2"},
        "generation": {"temperature": 0.0},
    });
    let mut run = json!({"run_id": "r1", "status": "ok", "model": {"id": "run-model"}});
    materialise(&mut run, &header);
    assert_eq!(
        run["model"],
        json!({"id": "run-model"}),
        "the run's own facet wins"
    );
    assert_eq!(
        run["task"],
        json!({"id": "single2"}),
        "an omitted facet is copied"
    );
    assert_eq!(run["generation"], json!({"temperature": 0.0}));
    for absent in ["harness", "trial", "env"] {
        assert!(
            run.get(absent).is_none(),
            "{absent}: the header has none to copy"
        );
    }
}

#[test]
fn materialise_keeps_a_null_facet_and_does_not_copy_a_null_default() {
    let header = json!({"model": {"id": "m"}, "task": null});
    let mut run = json!({"run_id": "r1", "status": "ok", "model": null});
    materialise(&mut run, &header);
    assert_eq!(
        run["model"],
        Value::Null,
        "null is the run's claim: recorded, unknown"
    );
    assert!(run.get("task").is_none());
}

#[test]
fn materialise_is_idempotent_and_a_later_header_change_does_not_touch_the_result() {
    let header = json!({"model": {"id": "m1"}, "trial": {"k": 4}});
    let mut run = json!({"run_id": "r1", "status": "ok"});
    materialise(&mut run, &header);
    let once = run.clone();
    materialise(&mut run, &header);
    assert_eq!(
        run, once,
        "a second call with the same header changes nothing"
    );

    let hash_before = run_content_hash("r1", &run).unwrap();
    let changed = json!({"model": {"id": "m2"}, "trial": {"k": 8}});
    materialise(&mut run, &changed);
    assert_eq!(run, once, "the run holds its own copy of the old defaults");
    assert_eq!(run_content_hash("r1", &run).unwrap(), hash_before);
}

#[test]
fn materialise_touches_only_the_six_eval_facets() {
    let header = json!({
        "schema": "evalhub.eval/2.0", "title": "t", "attachments": [], "ext": {"a/b": {}},
        "grading": {"graders": []},
    });
    let mut run = json!({"run_id": "r1", "status": "ok"});
    materialise(&mut run, &header);
    assert_eq!(run, json!({"run_id": "r1", "status": "ok"}));

    let mut not_an_object = json!([1]);
    materialise(&mut not_an_object, &json!({"model": {"id": "m"}}));
    assert_eq!(not_an_object, json!([1]));
}

// ---------------------------------------------------------------- split_v1

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

// ---------------------------------------------------------------- hashes

const R1_HASH: &str = "557d9e8829145c72b6f4737ec40e10ff20a82acce34caee06097511b730f390c";
const R2_HASH: &str = "cc70ad5aab5e6007d83632b9fec2dbcd942781366d9dc82796cd1eee827a7de0";

fn r1() -> Value {
    json!({"run_id": "r1", "status": "ok", "model": {"id": "qwen3.6-32b"}, "metrics": {"core/tokens_out": 1834}})
}

fn r2() -> Value {
    json!({"run_id": "r2", "status": "error", "error": {"kind": "timeout"}, "model": {"id": "qwen3.6-32b"}})
}

#[test]
fn run_content_hash_literal() {
    // sha256 of
    // {"metrics":{"core/tokens_out":1834},"model":{"id":"qwen3.6-32b"},"run_id":"r1","status":"ok"}
    let expected_bytes = br#"{"metrics":{"core/tokens_out":1834},"model":{"id":"qwen3.6-32b"},"run_id":"r1","status":"ok"}"#;
    assert_eq!(canonicalize(&r1()).unwrap(), expected_bytes);
    assert_eq!(run_content_hash("r1", &r1()).unwrap().as_hex(), R1_HASH);
    // {"error":{"kind":"timeout"},"model":{"id":"qwen3.6-32b"},"run_id":"r2","status":"error"}
    assert_eq!(run_content_hash("r2", &r2()).unwrap().as_hex(), R2_HASH);
}

#[test]
fn run_content_hash_takes_the_run_id_it_is_written_under() {
    // Without `run_id` in the body, the hash is the same: the id is added.
    let mut body = r1();
    body.as_object_mut().unwrap().remove("run_id");
    assert_eq!(run_content_hash("r1", &body).unwrap().as_hex(), R1_HASH);
    // A different id is a different run.
    assert_ne!(run_content_hash("r1b", &body).unwrap().as_hex(), R1_HASH);
    // Key order does not matter.
    let reordered: Value = serde_json::from_str(
        r#"{"metrics":{"core/tokens_out":1834},"status":"ok","model":{"id":"qwen3.6-32b"},"run_id":"r1"}"#,
    )
    .unwrap();
    assert_eq!(
        run_content_hash("r1", &reordered).unwrap().as_hex(),
        R1_HASH
    );
}

#[test]
fn run_content_hash_covers_the_materialised_facets() {
    let mut bare = json!({"run_id": "r1", "status": "ok", "metrics": {"core/tokens_out": 1834}});
    let before = run_content_hash("r1", &bare).unwrap();
    materialise(&mut bare, &json!({"model": {"id": "qwen3.6-32b"}}));
    let after = run_content_hash("r1", &bare).unwrap();
    assert_ne!(before, after);
    assert_eq!(
        after.as_hex(),
        R1_HASH,
        "equal to the run that carried the facet itself"
    );
}

#[test]
fn runs_hash_literal() {
    let h1: ContentHash = R1_HASH.parse().unwrap();
    let h2: ContentHash = R2_HASH.parse().unwrap();
    // sha256 of [["r1","557d…390c"],["r2","cc70…7de0"]]
    let expected = "0e43546a37cc88b39715ef05aea877e3075a5c20bb36364cbcdfef5b26d7437b";
    assert_eq!(
        runs_hash(&[("r1", h1), ("r2", h2)]).unwrap().as_hex(),
        expected
    );
    // Input order does not matter; owned strings work as well.
    assert_eq!(
        runs_hash(&[("r2".to_string(), h2), ("r1".to_string(), h1)])
            .unwrap()
            .as_hex(),
        expected
    );
}

#[test]
fn runs_hash_sorts_by_bytes_not_by_number() {
    let h1: ContentHash = R1_HASH.parse().unwrap();
    let h2: ContentHash = R2_HASH.parse().unwrap();
    // "r10" < "r2" byte-wise: sha256 of [["r10","557d…390c"],["r2","cc70…7de0"]]
    assert_eq!(
        runs_hash(&[("r2", h2), ("r10", h1)]).unwrap().as_hex(),
        "0089bf2d35be0aea01fff2b79564f41216e181b04594461e0560fc216e03c8a7"
    );
}

#[test]
fn runs_hash_of_no_runs_is_the_hash_of_an_empty_list() {
    let none: [(&str, ContentHash); 0] = [];
    // sha256 of []
    assert_eq!(
        runs_hash(&none).unwrap().as_hex(),
        "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
    );
}

#[test]
fn runs_hash_changes_when_a_run_is_added_or_overwritten() {
    let h1: ContentHash = R1_HASH.parse().unwrap();
    let h2: ContentHash = R2_HASH.parse().unwrap();
    let base = runs_hash(&[("r1", h1)]).unwrap();
    assert_ne!(runs_hash(&[("r1", h1), ("r2", h2)]).unwrap(), base, "added");
    assert_ne!(runs_hash(&[("r1", h2)]).unwrap(), base, "overwritten");
}

// ---------------------------------------------------------------- fingerprints

#[test]
fn for_run_fingerprints_the_six_eval_facets_of_the_run() {
    let run = schema_fixture("run.json");
    let fp = for_run(&run).unwrap();
    assert_eq!(fp.len(), 6);
    assert!(fp.get(Facet::Grading).is_none());
    // The same rule as a record: a run and an Eval header with equal facets
    // have equal fingerprints.
    let header = json!({"model": run["model"], "generation": run["generation"]});
    assert_eq!(fp, fingerprints(RecordKind::Eval, &header).unwrap());
    // A run's own facet makes the difference.
    let mut other = run.clone();
    other["model"] = json!({"id": "another-model"});
    let other_fp = for_run(&other).unwrap();
    assert_ne!(other_fp.get(Facet::Model), fp.get(Facet::Model));
    assert_eq!(other_fp.get(Facet::Generation), fp.get(Facet::Generation));
}
