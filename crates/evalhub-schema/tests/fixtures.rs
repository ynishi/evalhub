#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The fixtures under `fixtures/` are the complete examples from the module
//! docs. They must deserialise into the record types and serialise back to
//! the same JSON value, and the core must stay closed.

use evalhub_schema::card::Card;
use evalhub_schema::eval::Eval;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap()
}

#[test]
fn card_fixture_round_trips() {
    let input = fixture("card-complete.json");
    let card: Card = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(card.schema, evalhub_schema::CARD_SCHEMA);
    assert_eq!(card.results.len(), 1);
    assert_eq!(card.results[0].metric, "core/pass_rate");
    let output = serde_json::to_value(&card).unwrap();
    assert_eq!(output, input);
}

#[test]
fn eval_fixture_round_trips() {
    let input = fixture("eval-run-set.json");
    let eval: Eval = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(eval.schema, evalhub_schema::EVAL_SCHEMA);
    assert_eq!(eval.runs.len(), 1);
    assert_eq!(eval.runs[0].run_id, "r1");
    let output = serde_json::to_value(&eval).unwrap();
    assert_eq!(output, input);
}

#[test]
fn unknown_top_level_key_is_rejected() {
    let mut input = fixture("card-complete.json");
    input["verified"] = Value::Bool(true);
    let err = serde_json::from_value::<Card>(input).unwrap_err();
    assert!(
        err.to_string().contains("unknown field `verified`"),
        "{err}"
    );
}

#[test]
fn unknown_key_inside_a_facet_is_rejected() {
    let mut input = fixture("card-complete.json");
    input["model"]["parameter_count"] = Value::from(32_000_000_000_u64);
    let err = serde_json::from_value::<Card>(input).unwrap_err();
    assert!(
        err.to_string().contains("unknown field `parameter_count`"),
        "{err}"
    );
}

#[test]
fn unknown_key_inside_ext_is_accepted() {
    let mut input = fixture("card-complete.json");
    input["ext"]["alice/qwen-loop"]["anything"] = serde_json::json!({"nested": [1, 2, 3]});
    input["model"]["ext"] = serde_json::json!({"alice/qwen-loop": {"parameter_count": 32e9}});
    let card: Card = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(serde_json::to_value(&card).unwrap(), input);
}

#[test]
fn null_and_absent_both_deserialise() {
    // Through the typed structs both read as `None`; the value-level
    // distinction is preserved by the core (see the facet module doc).
    let mut with_null = fixture("card-complete.json");
    with_null["task"]["seed"] = Value::Null;
    let card: Card = serde_json::from_value(with_null).unwrap();
    assert_eq!(card.task.unwrap().seed, None);
}

#[test]
fn fixtures_validate_against_the_generated_schema_shape() {
    // Not a full validation (that is the core's job); a sanity check that the
    // schema's required list names keys the fixture has.
    let schema = evalhub_schema::schema_for_card();
    let required = schema.as_value()["required"].as_array().unwrap().clone();
    let card = fixture("card-complete.json");
    for key in required {
        assert!(card.get(key.as_str().unwrap()).is_some(), "missing {key}");
    }
}
