#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The run projection target: `PathTable::for_runs` and `compile_runs`.
//!
//! As in `compile.rs`, failures are asserted by the codes and JSON
//! pointers they produce, and the one successful fixture is snapshotted as
//! IR.

use evalhub_query::ir::{self, CardRef, ResultField, RunColumn, RunCursor};
use evalhub_query::typecheck::{ExtSchema, Indexed, PathInfo, PathTable};
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_schema::query::{QueryRequest, Sort};
use serde_json::{Value, json};

fn card(s: &str) -> CardRef {
    CardRef::parse(s).expect("a Card reference")
}

/// The run table for a request naming `c/d` and `alice/judge.v2`, with one
/// registered `ext` path.
fn table() -> PathTable {
    PathTable::for_runs(&[card("c/d"), card("alice/judge.v2")]).with_ext(&[ExtSchema {
        ns: "alice/qwen-loop".to_string(),
        paths: vec![(vec!["rung".to_string()], ir::ValueType::Number)],
        applied: true,
    }])
}

fn request(name: &str) -> QueryRequest {
    let text = std::fs::read_to_string(format!(
        "{}/fixtures/query/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture");
    serde_json::from_str(&text).expect("fixture parses as a request")
}

fn filter(where_: Value) -> QueryRequest {
    QueryRequest {
        where_: Some(where_),
        ..Default::default()
    }
}

fn sorted_by(path: &str) -> QueryRequest {
    QueryRequest {
        sort: vec![serde_json::from_value::<Sort>(json!({"path": path, "dir": "asc"})).unwrap()],
        ..Default::default()
    }
}

fn compile(request: &QueryRequest) -> Result<ir::RunQuery, Vec<ErrorEntry>> {
    evalhub_query::compile_runs(request, &table(), 50, None)
}

fn problems(errors: &[ErrorEntry]) -> Vec<(ErrorCode, String)> {
    let mut out: Vec<(ErrorCode, String)> =
        errors.iter().map(|e| (e.code, e.path.clone())).collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn scalar(path: &str) -> (ir::Column, ir::ValueType, Indexed) {
    match table().lookup(path) {
        Some(PathInfo::Scalar {
            column,
            ty,
            indexed,
        }) => (column, ty, indexed),
        other => panic!("{path}: {other:?}"),
    }
}

#[test]
fn the_ir_of_a_run_query() {
    let query = evalhub_query::compile_runs(
        &request("runs"),
        &PathTable::for_runs(&[card("c/d")]),
        50,
        None,
    )
    .expect("the fixture is legal");
    insta::assert_json_snapshot!("runs", query);
}

/// Every kind of path the table has, with the column and type it resolves
/// to; and each one works in a filter and a sort.
#[test]
fn each_path_kind_typechecks() {
    use RunColumn as R;
    use ir::ValueType as V;
    let run = |c: R| ir::Column::Run(c);
    let cases: Vec<(&str, ir::Column, V, Value)> = vec![
        ("run_id", run(R::RunId), V::String, json!("r1")),
        ("status", run(R::Status), V::String, json!("error")),
        ("error.kind", run(R::ErrorKind), V::String, json!("timeout")),
        (
            "started_at",
            run(R::StartedAt),
            V::String,
            json!("2026-09-20T10:00:00Z"),
        ),
        (
            "ended_at",
            run(R::EndedAt),
            V::String,
            json!("2026-09-20T10:02:11Z"),
        ),
        (
            "model.id",
            run(R::Facet(vec!["model".into(), "id".into()])),
            V::String,
            json!("qwen3.6-32b"),
        ),
        (
            "generation.temperature",
            run(R::Facet(vec!["generation".into(), "temperature".into()])),
            V::Number,
            json!(0.2),
        ),
        (
            "env.git.dirty",
            run(R::Facet(vec!["env".into(), "git".into(), "dirty".into()])),
            V::Boolean,
            json!(false),
        ),
        // A facet key with no generated column on records is still a
        // full column here: the projection is scoped to one Eval.
        (
            "model.availability",
            run(R::Facet(vec!["model".into(), "availability".into()])),
            V::String,
            json!("open-weights"),
        ),
        (
            "fingerprint.model",
            run(R::Fingerprint("model".into())),
            V::String,
            json!("3b1f"),
        ),
        (
            "metrics[core/tokens_out]",
            run(R::Metric("core/tokens_out".into())),
            V::Number,
            json!(4000),
        ),
        // Any well-formed id, registered or not, dots included.
        (
            "metrics[bob.lab/p95.latency_ms]",
            run(R::Metric("bob.lab/p95.latency_ms".into())),
            V::Number,
            json!(12.5),
        ),
        (
            "results[c/d][a/b].value",
            run(R::Result {
                card: card("c/d"),
                metric: "a/b".into(),
                field: ResultField::Value,
            }),
            V::Number,
            json!(1),
        ),
        (
            "results[alice/judge.v2][core/verdict].label",
            run(R::Result {
                card: card("alice/judge.v2"),
                metric: "core/verdict".into(),
                field: ResultField::Label,
            }),
            V::String,
            json!("pass"),
        ),
        (
            "ext.alice/qwen-loop.rung",
            ir::Column::Ext {
                path: vec!["ext".into(), "alice/qwen-loop".into(), "rung".into()],
                ty: V::Number,
            },
            V::Number,
            json!(4),
        ),
    ];
    for (path, column, ty, literal) in cases {
        let (got_column, got_ty, indexed) = scalar(path);
        assert_eq!(got_column, column, "{path}");
        assert_eq!(got_ty, ty, "{path}");
        assert_eq!(indexed, Indexed::Full, "{path}");

        let query = compile(&filter(json!({"path": path, "op": "eq", "value": literal})))
            .unwrap_or_else(|e| panic!("eq on {path}: {e:?}"));
        match query.filter {
            Some(ir::Filter::Cmp(cmp)) => assert_eq!(cmp.column, column, "{path}"),
            other => panic!("{path}: {other:?}"),
        }
        if ty != V::Boolean {
            compile(&filter(
                json!({"path": path, "op": "gte", "value": literal}),
            ))
            .unwrap_or_else(|e| panic!("gte on {path}: {e:?}"));
        }
        compile(&filter(json!({"path": path, "op": "exists"})))
            .unwrap_or_else(|e| panic!("exists on {path}: {e:?}"));

        let query = compile(&sorted_by(path)).unwrap_or_else(|e| panic!("sort {path}: {e:?}"));
        let expected = match column {
            ir::Column::Run(c) => ir::SortKey::Run(c),
            other => ir::SortKey::Column(other),
        };
        assert_eq!(query.sort[0].key, expected, "{path}");
    }

    // An unregistered `ext` key: equality and existence, untyped, as for
    // records; no order, because nothing declares what it holds.
    let (column, _, indexed) = scalar("ext.bob/thing.k");
    assert_eq!(indexed, Indexed::EqExistsUntyped);
    assert_eq!(
        column,
        ir::Column::Body(vec!["ext".into(), "bob/thing".into(), "k".into()])
    );
    compile(&filter(
        json!({"path": "ext.bob/thing.k", "op": "eq", "value": 3}),
    ))
    .unwrap();
    let errors = compile(&sorted_by("ext.bob/thing.k")).unwrap_err();
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::NotIndexed, "/sort/0/path".to_string())]
    );
}

/// The table has exactly the fixed paths the module doc lists: the run's
/// columns, the leaves of the six Eval facets, and their fingerprints. An
/// exact set, so that a header key leaking in (`eval_kind`, `origin.*`,
/// `redaction.*`) or a facet leaf going missing fails here.
#[test]
fn the_fixed_paths_are_the_runs() {
    let table = PathTable::for_runs(&[]);
    let actual: Vec<&str> = table.paths().map(|(p, _)| p).collect();
    let expected = [
        "ended_at",
        "env.git.commit",
        "env.git.dirty",
        "env.git.origin",
        "env.hardware.cpu",
        "env.hardware.gpu",
        "env.hardware.gpu_count",
        "env.hardware.memory_gb",
        "env.os",
        "error.kind",
        "fingerprint.env",
        "fingerprint.generation",
        "fingerprint.harness",
        "fingerprint.model",
        "fingerprint.task",
        "fingerprint.trial",
        "generation.max_tokens",
        "generation.n",
        "generation.reasoning.budget_tokens",
        "generation.reasoning.effort",
        "generation.seed",
        "generation.temperature",
        "generation.top_k",
        "generation.top_p",
        "harness.config_sha256",
        "harness.few_shot",
        "harness.name",
        "harness.prompt_template_sha256",
        "harness.system_prompt_sha256",
        "harness.version",
        "model.availability",
        "model.context_window",
        "model.endpoint",
        "model.engine.name",
        "model.engine.version",
        "model.id",
        "model.provider",
        "model.quantization.dtype",
        "model.quantization.method",
        "model.revision",
        "run_id",
        "started_at",
        "status",
        "task.config",
        "task.id",
        "task.n",
        "task.revision",
        "task.sample_ids_sha256",
        "task.seed",
        "task.shuffled",
        "task.source_data",
        "task.split",
        "task.version",
        "trial.k",
        "trial.max_turns",
        "trial.pass_at_k",
        "trial.retries",
        "trial.sandbox.digest",
        "trial.sandbox.image",
        "trial.timeout_ms",
    ];
    assert_eq!(actual, expected);
    for p in [
        "title",
        "producer.name",
        "schema",
        "eval_kind",
        "grading.rubric_sha256",
        "fingerprint.grading",
        "runs",
        "attachments",
        "relations",
        "created_at",
    ] {
        assert!(
            table.lookup(p).is_none(),
            "{p} is a record path, not a run's"
        );
    }
    // Every fixed path is a run column.
    for (p, info) in table.paths() {
        assert!(
            matches!(
                info,
                PathInfo::Scalar {
                    column: ir::Column::Run(_),
                    indexed: Indexed::Full,
                    ..
                }
            ),
            "{p}: {info:?}"
        );
    }
}

/// A record table knows none of the run paths.
#[test]
fn record_tables_do_not_gain_run_paths() {
    let evals = PathTable::from_schema(evalhub_schema::RecordKind::Eval);
    assert!(!evals.is_runs());
    assert!(evals.cards().is_empty());
    for p in [
        "runs",
        "metrics[a/b]",
        "results[c/d][a/b].value",
        "status",
        "run_id",
    ] {
        assert!(evals.lookup(p).is_none(), "{p}");
    }
}

#[test]
fn a_card_not_in_the_request_is_unknown() {
    let errors = compile(&filter(
        json!({"path": "results[bob/other][a/b].value", "op": "eq", "value": 1}),
    ))
    .unwrap_err();
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::UnknownPath, "/where/path".to_string())]
    );
    assert!(
        errors[0].hint.as_deref().unwrap().contains("cards"),
        "the hint says how to name it: {:?}",
        errors[0].hint
    );

    // The same in a sort.
    let errors = compile(&sorted_by("results[bob/other][a/b].value")).unwrap_err();
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::UnknownPath, "/sort/0/path".to_string())]
    );

    // And with no Cards named at all.
    let errors = evalhub_query::compile_runs(
        &filter(json!({"path": "results[c/d][a/b].value", "op": "eq", "value": 1})),
        &PathTable::for_runs(&[]),
        50,
        None,
    )
    .unwrap_err();
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::UnknownPath, "/where/path".to_string())]
    );
}

#[test]
fn a_malformed_metric_id_is_unknown() {
    for path in [
        "metrics[tokens]",
        "metrics[Core/tokens]",
        "metrics[a/b/c]",
        "metrics[]",
        "metrics[a/b",
        "metrics[a/b].value",
        "results[c/d][pass].value",
        "results[c/d][a/b].score",
        "results[c/d].value",
        "results[not a card][a/b].value",
    ] {
        let errors = compile(&filter(json!({"path": path, "op": "exists"})))
            .expect_err(&format!("{path} is not a path"));
        assert_eq!(
            problems(&errors),
            vec![(ErrorCode::UnknownPath, "/where/path".to_string())],
            "{path}"
        );
    }
}

/// The dotted forms are refused with a hint naming the bracket form,
/// because a slug may contain `.`.
#[test]
fn the_dotted_forms_are_refused() {
    for path in [
        "metrics.x",
        "metrics.core/tokens_out",
        "results.c.m.value",
        "results.c/d.a/b.value",
        "metrics",
        "results",
    ] {
        let errors = compile(&filter(json!({"path": path, "op": "eq", "value": 1})))
            .expect_err(&format!("{path} is not a path"));
        assert_eq!(
            problems(&errors),
            vec![(ErrorCode::UnknownPath, "/where/path".to_string())],
            "{path}"
        );
        let hint = errors[0].hint.as_deref().unwrap();
        assert!(
            hint.contains('['),
            "{path}: the hint names the bracket form: {hint}"
        );
    }
}

/// A metric is a number and a label a string; the ordinary type rules
/// apply to the new paths.
#[test]
fn the_type_rules_apply() {
    let errors = compile(&filter(
        json!({"path": "metrics[a/b]", "op": "eq", "value": "many"}),
    ))
    .unwrap_err();
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::TypeMismatch, "/where/value".to_string())]
    );
    let errors = compile(&filter(
        json!({"path": "results[c/d][a/b].label", "op": "gt", "value": 1}),
    ))
    .unwrap_err();
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::TypeMismatch, "/where/value".to_string())]
    );
    // No array paths, so no `any`.
    let errors = compile(&filter(
        json!({"path": "metrics[a/b]", "op": "any", "match": {"value": 1}}),
    ))
    .unwrap_err();
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::TypeMismatch, "/where/op".to_string())]
    );
}

#[test]
fn a_run_cursor_round_trips() {
    let cursor = RunCursor {
        keys: vec![json!(4000.5), Value::Null, json!("ok")],
        run_id: "task-17.trial-2 ü".to_string(),
    };
    // What the server signs is the JSON bytes; what it reads back is the
    // same value.
    let bytes = serde_json::to_vec(&cursor).unwrap();
    let back: RunCursor = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(back, cursor);

    // A record cursor is not a run cursor: the tie-breakers differ.
    let record = ir::Cursor {
        keys: vec![],
        version_id: uuid::Uuid::nil(),
    };
    assert!(serde_json::from_slice::<RunCursor>(&serde_json::to_vec(&record).unwrap()).is_err());

    // compile_runs carries it through when it matches the sort …
    let request = sorted_by("metrics[a/b]");
    let query = evalhub_query::compile_runs(
        &request,
        &table(),
        50,
        Some(RunCursor {
            keys: vec![json!(3)],
            run_id: "r9".into(),
        }),
    )
    .unwrap();
    assert_eq!(query.cursor.unwrap().run_id, "r9");

    // … and refuses it when it does not.
    let errors = evalhub_query::compile_runs(
        &request,
        &table(),
        50,
        Some(RunCursor {
            keys: vec![],
            run_id: "r9".into(),
        }),
    )
    .unwrap_err();
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::Schema, "/cursor".to_string())]
    );
}

/// The Cards the request names are carried into the IR, once each, in
/// request order.
#[test]
fn the_named_cards_reach_the_ir() {
    let table = PathTable::for_runs(&[card("x/y"), card("c/d"), card("x/y")]);
    assert!(table.is_runs());
    assert_eq!(table.cards(), &[card("x/y"), card("c/d")]);
    let query = evalhub_query::compile_runs(&QueryRequest::default(), &table, 25, None).unwrap();
    assert_eq!(query.cards, vec![card("x/y"), card("c/d")]);
    assert!(query.filter.is_none());
    assert!(query.sort.is_empty());
    assert_eq!(query.limit, 25);
}

#[test]
fn a_card_ref_is_ns_slash_name() {
    let c = card("alice/judge.v2");
    assert_eq!(c.ns(), "alice");
    assert_eq!(c.name(), "judge.v2");
    assert_eq!(c.to_string(), "alice/judge.v2");
    assert_eq!(serde_json::to_value(&c).unwrap(), json!("alice/judge.v2"));
    for bad in ["alice", "alice/judge@3", "Alice/j", "a/b/c", "/j", "a/"] {
        assert!(CardRef::parse(bad).is_none(), "{bad}");
        assert!(
            serde_json::from_value::<CardRef>(json!(bad)).is_err(),
            "{bad}"
        );
    }
}

/// Every hint of the parametric paths reads as one sentence: no run of
/// spaces left over from wrapping the source line.
#[test]
fn the_hints_are_clean_prose() {
    for path in [
        "metrics[a/b].value",
        "metrics[tokens]",
        "metrics.x",
        "metrics",
        "results[c/d].value",
        "results[c/d][a/b",
        "results[not a card][a/b].value",
        "results[bob/other][a/b].value",
        "results[c/d][pass].value",
        "results[c/d][a/b].score",
        "results.c.m.value",
        "results",
        "nope",
    ] {
        let errors = compile(&filter(json!({"path": path, "op": "exists"})))
            .expect_err(&format!("{path} is not a path"));
        for e in &errors {
            let hint = e.hint.as_deref().unwrap_or_default();
            assert!(!hint.contains("  "), "{path}: {hint:?}");
        }
    }
}
