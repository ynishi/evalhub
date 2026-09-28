#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Runs: facet materialisation, the run hash formulas and the run
//! fingerprints.
//!
//! The expected hashes are literals so that a client implementing the
//! formulas in `evalhub_core::run` can check itself against them. They were
//! computed outside this crate: the canonical bytes are written out in each
//! test (for these inputs RFC 8785 is "keys sorted, no whitespace, integers
//! without a fraction"), and the digest is SHA-256 of those bytes.

use std::path::PathBuf;

use evalhub_core::canonical::{ContentHash, canonicalize};
use evalhub_core::fingerprint::{Facet, fingerprints, for_run};
use evalhub_core::run::{materialise, run_content_hash, runs_hash};
use evalhub_schema::RecordKind;
use serde_json::{Value, json};

fn schema_fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../evalhub-schema/fixtures")
        .join(name);
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

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
