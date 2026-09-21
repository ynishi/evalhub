#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The committed expectation for the complete Card, and the properties
//! the fingerprint rule promises.
//!
//! To regenerate `fixtures/fingerprint/card-complete.expected.json` after
//! a deliberate change to the rule or the schema annotations:
//! `EVALHUB_UPDATE_FINGERPRINTS=1 cargo test -p evalhub-core --test fingerprint`.

use std::path::PathBuf;

use evalhub_core::fingerprint::{Facet, fingerprints};
use evalhub_schema::RecordKind;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn card() -> Value {
    serde_json::from_slice(
        &std::fs::read(root().join("fixtures/validate/valid-card.json")).unwrap(),
    )
    .unwrap()
}

fn eval() -> Value {
    serde_json::from_slice(
        &std::fs::read(root().join("fixtures/validate/valid-eval.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn card_complete_matches_committed_expectation() {
    let got = fingerprints(RecordKind::Card, &card())
        .unwrap()
        .to_hex_map();
    let got_json = serde_json::to_string_pretty(&got).unwrap() + "\n";
    let path = root().join("fixtures/fingerprint/card-complete.expected.json");
    if std::env::var_os("EVALHUB_UPDATE_FINGERPRINTS").is_some() {
        std::fs::write(&path, &got_json).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (set EVALHUB_UPDATE_FINGERPRINTS=1)",
            path.display()
        )
    });
    assert_eq!(got_json, expected, "fingerprint rule or schema changed");
    assert_eq!(got.len(), 7);
}

#[test]
fn eval_has_six_facets() {
    let fp = fingerprints(RecordKind::Eval, &eval()).unwrap();
    assert_eq!(fp.len(), 6);
    assert!(fp.get(Facet::Grading).is_none());
    assert!(fp.get(Facet::Model).is_some());
}

#[test]
fn excluded_keys_do_not_change_the_fingerprint() {
    let base = fingerprints(RecordKind::Card, &card()).unwrap();
    let mut v = card();
    v["model"]["context_window"] = json!(131072);
    v["env"]["os"] = json!("linux");
    v["env"]["hardware"] = json!({"gpu": "H100", "gpu_count": 8});
    v["model"]["ext"] = json!({"alice/x": {"anything": 1}});
    v["task"]["ext"] = json!({"alice/x": {"anything": 2}});
    assert_eq!(fingerprints(RecordKind::Card, &v).unwrap(), base);
}

#[test]
fn a_core_key_changes_only_its_facet() {
    let base = fingerprints(RecordKind::Card, &card()).unwrap();
    let mut v = card();
    v["model"]["id"] = json!("other-model");
    let changed = fingerprints(RecordKind::Card, &v).unwrap();
    for f in Facet::ALL {
        if f == Facet::Model {
            assert_ne!(changed.get(f), base.get(f));
        } else {
            assert_eq!(changed.get(f), base.get(f), "{f}");
        }
    }
}

#[test]
fn absent_null_and_empty_facets() {
    let mut with_null = card();
    with_null["task"]["seed"] = json!(null);
    let mut without = card();
    without["task"].as_object_mut().unwrap().remove("seed");
    let a = fingerprints(RecordKind::Card, &with_null).unwrap();
    let b = fingerprints(RecordKind::Card, &without).unwrap();
    assert_ne!(a.get(Facet::Task), b.get(Facet::Task));

    // An absent facet, a null facet and an empty facet all hash as `{}`.
    let mut absent = card();
    absent.as_object_mut().unwrap().remove("grading");
    let mut null = card();
    null["grading"] = json!(null);
    let mut empty = card();
    empty["grading"] = json!({});
    let fa = fingerprints(RecordKind::Card, &absent).unwrap();
    let fn_ = fingerprints(RecordKind::Card, &null).unwrap();
    let fe = fingerprints(RecordKind::Card, &empty).unwrap();
    assert_eq!(fa.get(Facet::Grading), fn_.get(Facet::Grading));
    assert_eq!(fa.get(Facet::Grading), fe.get(Facet::Grading));
    assert_ne!(
        fa.get(Facet::Grading),
        fingerprints(RecordKind::Card, &card())
            .unwrap()
            .get(Facet::Grading)
    );
}

fn reorder(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.reverse();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), reorder(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(reorder).collect()),
        other => other.clone(),
    }
}

#[test]
fn key_order_does_not_matter() {
    let a = fingerprints(RecordKind::Card, &card()).unwrap();
    let b = fingerprints(RecordKind::Card, &reorder(&card())).unwrap();
    assert_eq!(a, b);
}

fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i32>().prop_map(|i| json!(i)),
        "[a-z]{0,8}".prop_map(Value::String),
    ]
}

proptest! {
    #[test]
    fn changing_an_excluded_model_key_never_changes_the_model_fingerprint(
        cw in any::<u32>(), ext in scalar()
    ) {
        let base = fingerprints(RecordKind::Card, &card()).unwrap();
        let mut v = card();
        v["model"]["context_window"] = json!(cw);
        v["model"]["ext"] = json!({"alice/p": ext});
        let got = fingerprints(RecordKind::Card, &v).unwrap();
        prop_assert_eq!(got.get(Facet::Model), base.get(Facet::Model));
    }

    #[test]
    fn changing_a_core_generation_key_changes_the_generation_fingerprint(t in 0.01f64..2.0) {
        let base = fingerprints(RecordKind::Card, &card()).unwrap();
        let mut v = card();
        v["generation"]["temperature"] = json!(t);
        let got = fingerprints(RecordKind::Card, &v).unwrap();
        prop_assert_ne!(got.get(Facet::Generation), base.get(Facet::Generation));
        prop_assert_eq!(got.get(Facet::Model), base.get(Facet::Model));
    }
}
