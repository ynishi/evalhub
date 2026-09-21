#![allow(clippy::unwrap_used, clippy::expect_used)]

//! JSON in, IR out: the whole front end without a database.
//!
//! Each fixture under `fixtures/query/` is a request body. The successful
//! one is snapshotted as IR; the failing ones are asserted by the codes and
//! JSON pointers they produce, because those two are the contract a client
//! reads and the prose hint is not.

use std::collections::BTreeMap;

use evalhub_query::grammar::ScalarOp;
use evalhub_query::ir;
use evalhub_query::typecheck::{ExtSchema, Indexed, PathInfo, PathTable};
use evalhub_schema::RecordKind;
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_schema::query::QueryRequest;
use serde_json::{Value, json};

/// The registry entry the complete fixture's `ext` path relies on.
fn registered() -> Vec<ExtSchema> {
    vec![ExtSchema {
        ns: "alice/qwen-loop".to_string(),
        paths: vec![(vec!["rung".to_string()], ir::ValueType::Number)],
        applied: true,
    }]
}

fn table(kind: RecordKind) -> PathTable {
    PathTable::from_schema(kind).with_ext(&registered())
}

fn request(name: &str) -> QueryRequest {
    let text = std::fs::read_to_string(format!(
        "{}/fixtures/query/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture");
    serde_json::from_str(&text).expect("fixture parses as a request")
}

fn compile(name: &str) -> Result<ir::Query, Vec<ErrorEntry>> {
    evalhub_query::compile(
        &request(name),
        RecordKind::Card,
        &table(RecordKind::Card),
        50,
        None,
    )
}

/// Every code and pointer a fixture produced, sorted so the assertion does
/// not depend on the order errors happen to be collected in.
fn problems(errors: &[ErrorEntry]) -> Vec<(ErrorCode, String)> {
    let mut out: Vec<(ErrorCode, String)> =
        errors.iter().map(|e| (e.code, e.path.clone())).collect();
    out.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| format!("{:?}", a.0).cmp(&format!("{:?}", b.0)))
    });
    out
}

#[test]
fn the_complete_example_compiles() {
    let query = compile("complete").expect("the crate doc's example is legal");
    insta::assert_json_snapshot!("complete", query);
}

#[test]
fn an_unknown_path_is_named_not_ignored() {
    let errors = compile("unknown-path").expect_err("model.nope is not a key");
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::UnknownPath, "/where/path".to_string())]
    );
}

#[test]
fn a_literal_of_the_wrong_type_is_a_mismatch() {
    let errors = compile("type-mismatch").expect_err("three mismatches");
    assert_eq!(
        problems(&errors),
        vec![
            // A string where the schema says number.
            (ErrorCode::TypeMismatch, "/where/and/0/value".to_string()),
            // `gte` on a string is legal (lexical), so the complaint is the
            // literal: `model.id` holds a string and "a" is one — this one
            // is legal, so the only error here is from the third leaf.
            (ErrorCode::TypeMismatch, "/where/and/2/value".to_string()),
        ]
    );
}

#[test]
fn an_unindexed_operator_names_the_registry() {
    let errors = compile("not-indexed").expect_err("gte needs an ordered index");
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::NotIndexed, "/where/op".to_string())]
    );
    assert!(
        errors[0].hint.as_deref().unwrap().contains("registry"),
        "the hint points at the fix: {:?}",
        errors[0].hint
    );
}

#[test]
fn shape_problems_come_back_together() {
    let errors = compile("schema-error").expect_err("three shapes are wrong");
    let codes: Vec<ErrorCode> = errors.iter().map(|e| e.code).collect();
    assert!(codes.iter().all(|c| *c == ErrorCode::Schema), "{codes:?}");
    let pointers: Vec<&str> = errors.iter().map(|e| e.path.as_str()).collect();
    assert!(
        pointers.iter().any(|p| p.starts_with("/where/and/0")),
        "the leaf without an op is located: {pointers:?}"
    );
    assert!(
        pointers.iter().any(|p| p.starts_with("/where/and/1")),
        "the `or` that is not an array is located: {pointers:?}"
    );
}

#[test]
fn sorting_needs_an_ordered_index() {
    let errors = evalhub_query::compile(
        &request("sort-not-indexed"),
        RecordKind::Card,
        &table(RecordKind::Card),
        50,
        None,
    )
    .expect_err("model.availability has no column");
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::NotIndexed, "/sort/0/path".to_string())]
    );
}

#[test]
fn an_empty_request_matches_everything_visible() {
    let query = evalhub_query::compile(
        &QueryRequest::default(),
        RecordKind::Eval,
        &table(RecordKind::Eval),
        25,
        None,
    )
    .expect("an empty query is a legal query");
    assert_eq!(query.record_type, ir::RecordType::Eval);
    assert!(query.filter.is_none());
    assert_eq!(query.limit, 25);
    assert_eq!(query.version, ir::VersionSelector::Latest);
}

#[test]
fn a_cursor_must_match_the_sort_it_came_from() {
    let mut request = request("complete");
    let errors = evalhub_query::compile(
        &request,
        RecordKind::Card,
        &table(RecordKind::Card),
        50,
        Some(ir::Cursor {
            keys: vec![json!(1), json!(2)],
            version_id: uuid::Uuid::nil(),
        }),
    )
    .expect_err("two keys, one sort");
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::Schema, "/cursor".to_string())]
    );

    request.sort.clear();
    evalhub_query::compile(
        &request,
        RecordKind::Card,
        &table(RecordKind::Card),
        50,
        Some(ir::Cursor {
            keys: vec![],
            version_id: uuid::Uuid::nil(),
        }),
    )
    .expect("no sort, no keys");
}

/// The operator/type matrix from the crate doc, checked one cell at a time
/// against paths whose storage can serve every operator.
#[test]
fn the_operator_type_matrix_holds() {
    let table = table(RecordKind::Card);
    // (path, type, a legal literal)
    let cases: [(&str, ir::ValueType, Value); 3] = [
        ("model.id", ir::ValueType::String, json!("q")),
        ("generation.temperature", ir::ValueType::Number, json!(0.2)),
        ("env.git.dirty", ir::ValueType::Boolean, json!(false)),
    ];
    for (path, ty, literal) in cases {
        for op in ScalarOp::ALL {
            let legal = match op {
                ScalarOp::Eq | ScalarOp::Ne | ScalarOp::In => true,
                ScalarOp::Gt | ScalarOp::Gte | ScalarOp::Lt | ScalarOp::Lte => {
                    matches!(ty, ir::ValueType::Number | ir::ValueType::String)
                }
                ScalarOp::Prefix | ScalarOp::Contains => ty == ir::ValueType::String,
            };
            let value = if op == ScalarOp::In {
                json!([literal])
            } else {
                literal.clone()
            };
            let request = QueryRequest {
                where_: Some(json!({"path": path, "op": op.as_str(), "value": value})),
                ..Default::default()
            };
            let result = evalhub_query::compile(&request, RecordKind::Card, &table, 10, None);
            assert_eq!(
                result.is_ok(),
                legal,
                "{path} {op}: expected legal={legal}, got {result:?}"
            );
            if !legal {
                let errors = result.unwrap_err();
                assert_eq!(errors[0].code, ErrorCode::TypeMismatch, "{path} {op}");
            }
        }
    }
}

/// `exists` applies to any scalar path, including the ones only the GIN
/// index serves.
#[test]
fn exists_applies_everywhere_a_scalar_lives() {
    let table = table(RecordKind::Card);
    for path in ["model.id", "model.availability", "ext.bob/thing.k"] {
        let request = QueryRequest {
            where_: Some(json!({"path": path, "op": "exists"})),
            ..Default::default()
        };
        evalhub_query::compile(&request, RecordKind::Card, &table, 10, None)
            .unwrap_or_else(|e| panic!("exists on {path}: {e:?}"));
    }
}

/// An `ext` path the registry has not applied yet behaves as unregistered,
/// so a producer cannot accidentally depend on an index that is still
/// building.
#[test]
fn an_unapplied_ext_schema_grants_nothing() {
    let building = vec![ExtSchema {
        ns: "alice/qwen-loop".to_string(),
        paths: vec![(vec!["rung".to_string()], ir::ValueType::Number)],
        applied: false,
    }];
    let table = PathTable::from_schema(RecordKind::Card).with_ext(&building);
    match table.lookup("ext.alice/qwen-loop.rung") {
        Some(PathInfo::Scalar { indexed, .. }) => {
            // Not `Full`: the declared type buys nothing until its index
            // exists. It is the untyped class, the same one an entirely
            // unregistered key gets.
            assert_eq!(indexed, Indexed::EqExistsUntyped);
        }
        other => panic!("{other:?}"),
    }
    // And the operator set is still the restricted one.
    let request = QueryRequest {
        where_: Some(json!({"path": "ext.alice/qwen-loop.rung", "op": "gt", "value": 3})),
        ..Default::default()
    };
    let errors = evalhub_query::compile(&request, RecordKind::Card, &table, 10, None)
        .expect_err("a range needs the index");
    assert_eq!(errors[0].code, ErrorCode::NotIndexed);
}

/// The array paths, their keys, and what `any` does with them.
#[test]
fn any_ranges_over_the_side_tables() {
    let table = table(RecordKind::Card);
    let request = QueryRequest {
        where_: Some(json!({
            "path": "attachments",
            "op": "any",
            "match": {"path": "samples.jsonl"}
        })),
        ..Default::default()
    };
    let query = evalhub_query::compile(&request, RecordKind::Card, &table, 10, None).unwrap();
    match query.filter {
        Some(ir::Filter::Any { table, conditions }) => {
            assert_eq!(table, ir::ArrayTable::Attachments);
            assert_eq!(conditions.len(), 1);
            assert_eq!(
                conditions[0].column,
                ir::Column::Array(ir::ArrayColumn::AttachmentPath)
            );
            assert_eq!(conditions[0].op, ir::Op::Eq);
        }
        other => panic!("{other:?}"),
    }

    // A key the side table does not have is an unknown path, not silence.
    let request = QueryRequest {
        where_: Some(json!({
            "path": "results",
            "op": "any",
            "match": {"metrik": "core/pass_rate"}
        })),
        ..Default::default()
    };
    let errors = evalhub_query::compile(&request, RecordKind::Card, &table, 10, None).unwrap_err();
    assert_eq!(
        problems(&errors),
        vec![(ErrorCode::UnknownPath, "/where/match/metrik".to_string())]
    );
}

/// An Eval has no `grading` facet, so a Card's paths are not automatically
/// an Eval's.
#[test]
fn the_table_follows_the_record_kind() {
    let cards = table(RecordKind::Card);
    let evals = table(RecordKind::Eval);
    assert!(cards.lookup("grading.rubric_sha256").is_some());
    assert!(evals.lookup("grading.rubric_sha256").is_none());
    assert!(cards.lookup("fingerprint.grading").is_some());
    assert!(evals.lookup("fingerprint.grading").is_none());
    assert!(evals.lookup("model.id").is_some());
}

/// Every generated column the table can emit is a real column of the
/// migration. This is what keeps the one hand-maintained list in the crate
/// from rotting: the store's migration is the truth, and this test reads it.
#[test]
fn every_generated_column_exists_in_the_migration() {
    let sql = std::fs::read_to_string(format!(
        "{}/../evalhub-store/migrations/0001_init.sql",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("the migration is next door");

    // `    name      text GENERATED ALWAYS AS (body #>> '{model,id}') STORED,`
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    for line in sql.lines() {
        let line = line.trim();
        let Some(generated) = line.find(" GENERATED ALWAYS AS (") else {
            continue;
        };
        let name = line.split_whitespace().next().unwrap().to_string();
        let expression = &line[generated..];
        let start = expression.find('{').expect("a json path");
        let end = expression.find('}').expect("a json path");
        declared.insert(name, expression[start + 1..end].to_string());
    }
    assert!(
        declared.len() >= 16,
        "parsed {} generated columns from the migration",
        declared.len()
    );

    for (path, column) in evalhub_query::typecheck::GENERATED_COLUMNS {
        let expected = path.join(",");
        match declared.get(*column) {
            None => panic!("`{column}` is not a generated column of `versions`"),
            Some(actual) => assert_eq!(
                actual, &expected,
                "`{column}` materialises `{actual}`, but the path table says `{expected}`"
            ),
        }
    }

    // And the other way: a column the migration adds should be reachable,
    // or the language silently loses a fast path.
    for (column, path) in &declared {
        assert!(
            evalhub_query::typecheck::GENERATED_COLUMNS
                .iter()
                .any(|(_, c)| c == column),
            "the migration generates `{column}` ({path}) but no query path uses it"
        );
    }
}

/// The published schema of `where` accepts the shapes the parser accepts.
#[test]
fn the_published_schema_lists_the_operators() {
    let schema = evalhub_query::request_schema().to_value();
    let text = serde_json::to_string(&schema).unwrap();
    for op in ScalarOp::ALL {
        assert!(text.contains(op.as_str()), "`{op}` is not in the schema");
    }
    assert!(text.contains("exists"));
    assert!(text.contains("any"));
    insta::assert_json_snapshot!("where_schema", schema);
}
