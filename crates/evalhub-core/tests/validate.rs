#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Every fixture under `fixtures/validate/` is a record; its sibling
//! `<name>.expected.json` lists the `[{path, code}]` the validator must
//! report, in the validator's own (sorted) order. `valid-*.json` expect `[]`.
//! The record kind is read from the fixture's `schema` field.

use std::path::PathBuf;

use evalhub_core::validate;
use evalhub_schema::RecordKind;
use serde_json::{Value, json};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/validate")
}

fn kind_of(v: &Value) -> RecordKind {
    match v["schema"].as_str() {
        Some(s) if s == evalhub_schema::EVAL_SCHEMA => RecordKind::Eval,
        _ => RecordKind::Card,
    }
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

        let got: Vec<Value> = validate(kind_of(&record), &record)
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
    assert!(seen >= 9, "only {seen} fixtures ran");
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
