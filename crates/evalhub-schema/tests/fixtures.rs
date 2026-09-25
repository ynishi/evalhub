#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The fixtures under `fixtures/` are the complete examples from the module
//! docs. They must deserialise into the record types and serialise back to
//! the same JSON value, and the core must stay closed.

use evalhub_schema::card::Card;
use evalhub_schema::eval::Eval;
use evalhub_schema::eval::v1;
use evalhub_schema::run::{Run, RunStatus};
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
fn card_run_results_fixture_round_trips() {
    let input = fixture("card-run-results.json");
    let card: Card = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(card.schema, evalhub_schema::CARD_SCHEMA);
    assert_eq!(card.run_results.len(), 2);
    assert_eq!(card.run_results[0].run_id, "r1");
    assert_eq!(card.run_results[1].value, None);
    assert_eq!(card.run_results[1].label.as_deref(), Some("fail"));
    let output = serde_json::to_value(&card).unwrap();
    assert_eq!(output, input);
}

#[test]
fn a_card_declaring_1_0_may_carry_run_results() {
    // One document for both minors: `run_results` is an optional key, and a
    // 1.0 Card that carries it is read the same way.
    let mut input = fixture("card-run-results.json");
    input["schema"] = Value::from("evalhub.card/1.0");
    let card: Card = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(card.run_results.len(), 2);
    assert_eq!(serde_json::to_value(&card).unwrap(), input);
}

#[test]
fn eval_fixture_round_trips() {
    let input = fixture("eval-run-set.json");
    let eval: Eval = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(eval.schema, evalhub_schema::EVAL_SCHEMA);
    let output = serde_json::to_value(&eval).unwrap();
    assert_eq!(output, input);
}

#[test]
fn an_eval_header_with_runs_is_rejected() {
    // Runs are not part of the 2.0 header.
    let mut input = fixture("eval-run-set.json");
    input["runs"] = fixture("eval-run-set-v1.json")["runs"].clone();
    let err = serde_json::from_value::<Eval>(input).unwrap_err();
    assert!(err.to_string().contains("unknown field `runs`"), "{err}");
}

#[test]
fn eval_v1_fixture_round_trips() {
    let input = fixture("eval-run-set-v1.json");
    let eval: v1::Eval = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(eval.schema, "evalhub.eval/1.0");
    assert_eq!(eval.runs.len(), 1);
    assert_eq!(eval.runs[0].run_id, "r1");
    assert_eq!(eval.runs[0].outcome, v1::Outcome::Pass);
    let output = serde_json::to_value(&eval).unwrap();
    assert_eq!(output, input);
}

#[test]
fn run_fixture_round_trips() {
    let input = fixture("run.json");
    let run: Run = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(run.run_id, "r1");
    assert_eq!(run.status, RunStatus::Ok);
    assert_eq!(run.metrics["core/tokens_out"], 1834.0);
    assert_eq!(run.attachments.len(), 2);
    let output = serde_json::to_value(&run).unwrap();
    assert_eq!(output, input);
}

#[test]
fn an_errored_run_round_trips() {
    let input = serde_json::json!({
        "run_id": "r2",
        "status": "error",
        "error": {"kind": "timeout", "message": "no answer after 600 s", "log": "logs/r2.txt"},
        "attachments": [{"path": "logs/r2.txt", "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", "size": 0}],
        "ext": {}
    });
    let run: Run = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(run.status, RunStatus::Error);
    assert_eq!(run.error.as_ref().unwrap().kind, "timeout");
    assert_eq!(serde_json::to_value(&run).unwrap(), input);
}

#[test]
fn run_and_run_error_are_closed() {
    let mut input = fixture("run.json");
    input["outcome"] = Value::from("pass");
    let err = serde_json::from_value::<Run>(input).unwrap_err();
    assert!(err.to_string().contains("unknown field `outcome`"), "{err}");

    let mut input = fixture("run.json");
    input["status"] = Value::from("error");
    input["error"] = serde_json::json!({"kind": "crash", "code": 139});
    let err = serde_json::from_value::<Run>(input).unwrap_err();
    assert!(err.to_string().contains("unknown field `code`"), "{err}");
}

#[test]
fn run_meta_is_kept_verbatim() {
    let mut input = fixture("run.json");
    input["meta"] = serde_json::json!({"anything": [1, {"nested": true}], "v0.2.0_migration": {"outcome": "pass"}});
    let run: Run = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(serde_json::to_value(&run).unwrap(), input);
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
    let cases = [
        (evalhub_schema::schema_for_card(), "card-complete.json"),
        (evalhub_schema::schema_for_card(), "card-run-results.json"),
        (evalhub_schema::schema_for_eval(), "eval-run-set.json"),
        (evalhub_schema::schema_for_eval_v1(), "eval-run-set-v1.json"),
        (evalhub_schema::schema_for_run(), "run.json"),
    ];
    for (schema, name) in cases {
        let required = schema.as_value()["required"].as_array().unwrap().clone();
        let record = fixture(name);
        for key in required {
            assert!(
                record.get(key.as_str().unwrap()).is_some(),
                "{name}: missing {key}"
            );
        }
    }
}
