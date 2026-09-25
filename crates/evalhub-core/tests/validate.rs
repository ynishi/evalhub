#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Every fixture under `fixtures/validate/` is a record or a run; its
//! sibling `<name>.expected.json` lists the `[{path, code}]` the validator
//! must report, in the validator's own (sorted) order. `valid-*.json`
//! expect `[]`.
//!
//! A fixture whose name starts with `run-` or `valid-run` is a run, stored
//! as `{"run_id": <the id it is written under>, "body": <the run>}` so that
//! the id in the path can differ from the body's; it goes through
//! `validate::run`, and `path`s point into `body`. Any other fixture is a
//! record, and its kind is read from its `schema` field (`evalhub.eval/…`
//! is an Eval, anything else a Card).

use std::path::PathBuf;

use evalhub_core::eval::{EvalSchema, declared_schema};
use evalhub_core::validate;
use evalhub_schema::RecordKind;
use evalhub_schema::error::ErrorCode;
use serde_json::{Value, json};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/validate")
}

fn kind_of(v: &Value) -> RecordKind {
    match v["schema"].as_str() {
        Some(s) if s.starts_with("evalhub.eval/") => RecordKind::Eval,
        _ => RecordKind::Card,
    }
}

fn is_run_fixture(name: &str) -> bool {
    name.starts_with("run-") || name.starts_with("valid-run")
}

fn read(name: &str) -> Value {
    serde_json::from_slice(&std::fs::read(dir().join(name)).unwrap()).unwrap()
}

#[test]
fn fixtures_report_exactly_the_expected_errors() {
    let mut seen = 0;
    let mut entries: Vec<_> = std::fs::read_dir(dir()).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".json") || name.ends_with(".expected.json") {
            continue;
        }
        let record: Value = serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap();
        let expected_path = dir().join(name.replace(".json", ".expected.json"));
        let expected: Value = serde_json::from_slice(
            &std::fs::read(&expected_path)
                .unwrap_or_else(|e| panic!("{}: {e}", expected_path.display())),
        )
        .unwrap();

        let errors = if is_run_fixture(&name) {
            let run_id = record["run_id"].as_str().expect("run fixture has run_id");
            validate::run(run_id, &record["body"])
        } else {
            validate(kind_of(&record), &record)
        };
        let got: Vec<Value> = errors
            .into_iter()
            .map(|e| json!({"path": e.path, "code": e.code}))
            .collect();
        assert_eq!(
            Value::Array(got.clone()),
            expected,
            "{name}: got {}",
            serde_json::to_string_pretty(&got).unwrap()
        );
        seen += 1;
    }
    assert!(seen >= 30, "only {seen} fixtures ran");
}

#[test]
fn every_error_has_a_hint_and_paths_are_pointers() {
    let record: Value =
        serde_json::from_slice(&std::fs::read(dir().join("attachment-refs.json")).unwrap())
            .unwrap();
    let errors = validate(RecordKind::Card, &record);
    assert!(!errors.is_empty());
    for e in &errors {
        assert!(e.hint.as_deref().is_some_and(|h| !h.is_empty()), "{e:?}");
        assert!(e.path.is_empty() || e.path.starts_with('/'), "{e:?}");
    }
}

#[test]
fn wrong_kind_is_a_schema_error_at_the_root() {
    let card: Value =
        serde_json::from_slice(&std::fs::read(dir().join("valid-card.json")).unwrap()).unwrap();
    let errors = validate(RecordKind::Eval, &card);
    assert!(!errors.is_empty());
    assert!(
        errors
            .iter()
            .all(|e| e.code == evalhub_schema::error::ErrorCode::Schema)
    );
}

#[test]
fn not_an_object_is_one_schema_error() {
    let errors = validate(RecordKind::Card, &json!([1, 2]));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].path, "");
}

#[test]
fn a_1_0_body_is_declared_v1_and_still_valid() {
    let body = read("eval-run-set-v1.json");
    assert_eq!(declared_schema(&body), Some(EvalSchema::V1));
    assert_eq!(validate(RecordKind::Eval, &body), vec![]);
}

#[test]
fn declared_schema_reads_only_the_identifier() {
    assert_eq!(
        declared_schema(&read("valid-eval.json")),
        Some(EvalSchema::V2)
    );
    assert_eq!(declared_schema(&read("eval-schema-unknown.json")), None);
    assert_eq!(declared_schema(&read("valid-card.json")), None);
    assert_eq!(declared_schema(&json!({})), None);
    assert_eq!(declared_schema(&json!({"schema": 2})), None);
    assert_eq!(declared_schema(&json!([1])), None);
    // Every accepted Eval identifier is one of the two arms, and back.
    for id in RecordKind::Eval.schema_ids() {
        let arm = declared_schema(&json!({ "schema": id })).unwrap();
        assert_eq!(arm.id(), *id);
    }
}

#[test]
fn runs_moved_is_the_only_error_and_names_the_run_endpoints() {
    let body = read("eval-runs-moved.json");
    let errors = validate(RecordKind::Eval, &body);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, ErrorCode::RunsMoved);
    let hint = errors[0].hint.as_deref().unwrap();
    assert!(
        hint.contains("PUT /evals/{ns}/{name}/runs/{run_id}"),
        "{hint}"
    );
    assert!(
        hint.contains("POST /evals/{ns}/{name}/runs:batch"),
        "{hint}"
    );
    // Any `runs` key, even an empty or null one.
    for runs in [json!([]), Value::Null] {
        let mut b = read("valid-eval.json");
        b["runs"] = runs;
        let errors = validate(RecordKind::Eval, &b);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].code, ErrorCode::RunsMoved);
    }
}

#[test]
fn a_1_0_body_under_the_2_0_identifier_is_runs_moved_and_the_reverse_is_a_schema_error() {
    let mut body = read("eval-run-set-v1.json");
    body["schema"] = json!("evalhub.eval/2.0");
    let errors = validate(RecordKind::Eval, &body);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].code, ErrorCode::RunsMoved);

    // A 2.0 header declaring 1.0 is checked against the 1.0 document, which
    // it satisfies (1.0 had every header key), so it is accepted as a 1.0
    // body with no runs.
    let mut header = read("valid-eval.json");
    header["schema"] = json!("evalhub.eval/1.0");
    assert_eq!(validate(RecordKind::Eval, &header), vec![]);
}

#[test]
fn every_run_error_has_a_hint() {
    for name in [
        "run-refs-unknown.json",
        "run-status-detail-ok.json",
        "run-id-mismatch.json",
    ] {
        let f = read(name);
        let errors = validate::run(f["run_id"].as_str().unwrap(), &f["body"]);
        assert!(!errors.is_empty(), "{name}");
        for e in &errors {
            assert!(e.hint.as_deref().is_some_and(|h| !h.is_empty()), "{e:?}");
            assert!(e.path.starts_with('/'), "{e:?}");
        }
    }
}

#[test]
fn a_run_body_without_run_id_is_a_schema_error_not_a_mismatch() {
    let errors = validate::run("r1", &json!({"status": "ok"}));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code, ErrorCode::Schema);
}

#[test]
fn a_run_that_is_not_an_object_is_schema_errors_only() {
    let errors = validate::run("r1", &json!([1]));
    assert!(!errors.is_empty());
    assert!(
        errors.iter().all(|e| e.code == ErrorCode::Schema),
        "{errors:?}"
    );
}

#[test]
fn a_run_integer_outside_2_53_is_number_too_large() {
    let errors = validate::run(
        "r1",
        &json!({"run_id": "r1", "status": "ok", "metrics": {"core/tokens_out": 9007199254740993u64}}),
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code, ErrorCode::NumberTooLarge);
    assert_eq!(errors[0].path, "/metrics/core~1tokens_out");
}
