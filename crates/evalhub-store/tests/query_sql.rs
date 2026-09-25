#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The query compiler: what it renders, and what those statements return
//! from a real Postgres.
//!
//! The rendering tests are snapshots, because the exact text is the
//! contract with the planner — an `ext` expression that drifts from the
//! one the index was built with is an index nobody uses, and the snapshot
//! is what notices.

mod common;

use evalhub_query::ir;
use evalhub_store::query_sql::{self, QueryHit};
use evalhub_store::records::Actor;
use evalhub_store::registry::{self, Kind};
use evalhub_store::{PgPool, index};
use serde_json::{Value, json};
use uuid::Uuid;

/// A query over Cards with nothing asked of it.
fn base() -> ir::Query {
    ir::Query {
        record_type: ir::RecordType::Card,
        filter: None,
        sort: Vec::new(),
        limit: 50,
        cursor: None,
        expand: Vec::new(),
        version: ir::VersionSelector::Latest,
    }
}

fn cmp(column: ir::Column, op: ir::Op, value: Value) -> ir::Filter {
    ir::Filter::Cmp(ir::Cmp { column, op, value })
}

fn generated(name: &str) -> ir::Column {
    ir::Column::Generated(name.to_string())
}

fn ext_path() -> Vec<String> {
    vec![
        "ext".to_string(),
        "alice/qwen-loop".to_string(),
        "rung".to_string(),
    ]
}

fn render(query: &ir::Query) -> String {
    query_sql::render(query, &["alice".to_string()]).unwrap()
}

// ---------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------

#[test]
fn renders_the_plain_query() {
    insta::assert_snapshot!("plain", render(&base()));
}

#[test]
fn renders_every_column_kind() {
    let query = ir::Query {
        filter: Some(ir::Filter::And(vec![
            cmp(generated("model_id"), ir::Op::Eq, json!("qwen3.6-32b")),
            cmp(generated("gen_temperature"), ir::Op::Lte, json!(0.2)),
            cmp(
                ir::Column::Fingerprint("model".into()),
                ir::Op::Eq,
                json!("00ff"),
            ),
            cmp(
                ir::Column::Ext {
                    path: ext_path(),
                    ty: ir::ValueType::Number,
                },
                ir::Op::Gte,
                json!(4),
            ),
            cmp(
                ir::Column::Body(vec!["ext".into(), "bob/other".into()]),
                ir::Op::Eq,
                json!({"k": 1}),
            ),
            ir::Filter::Exists {
                column: generated("task_split"),
            },
            ir::Filter::Any {
                table: ir::ArrayTable::Results,
                conditions: vec![
                    ir::Cmp {
                        column: ir::Column::Array(ir::ArrayColumn::ResultMetric),
                        op: ir::Op::Eq,
                        value: json!("core/pass_rate"),
                    },
                    ir::Cmp {
                        column: ir::Column::Array(ir::ArrayColumn::ResultValue),
                        op: ir::Op::Gte,
                        value: json!(0.5),
                    },
                ],
            },
            ir::Filter::Any {
                table: ir::ArrayTable::Relations,
                conditions: vec![ir::Cmp {
                    column: ir::Column::Array(ir::ArrayColumn::RelationTo),
                    op: ir::Op::Prefix,
                    value: json!("alice/single2-k4"),
                }],
            },
            ir::Filter::Not(Box::new(cmp(
                generated("harness_name"),
                ir::Op::In,
                json!(["a", "b"]),
            ))),
        ])),
        ..base()
    };
    insta::assert_snapshot!("every_column_kind", render(&query));
}

#[test]
fn renders_a_metric_sort_with_a_cursor() {
    let query = ir::Query {
        sort: vec![
            ir::Sort {
                key: ir::SortKey::Metric("core/pass_rate".into()),
                dir: ir::Dir::Desc,
            },
            ir::Sort {
                key: ir::SortKey::CreatedAt,
                dir: ir::Dir::Desc,
            },
        ],
        cursor: Some(ir::Cursor {
            keys: vec![json!(0.62), json!("2026-09-20T10:00:00Z")],
            version_id: Uuid::nil(),
        }),
        limit: 10,
        ..base()
    };
    insta::assert_snapshot!("metric_sort_cursor", render(&query));
}

#[test]
fn renders_mixed_directions_as_a_lexicographic_disjunction() {
    let query = ir::Query {
        sort: vec![
            ir::Sort {
                key: ir::SortKey::Column(generated("model_id")),
                dir: ir::Dir::Asc,
            },
            ir::Sort {
                key: ir::SortKey::Column(generated("gen_temperature")),
                dir: ir::Dir::Desc,
            },
        ],
        cursor: Some(ir::Cursor {
            keys: vec![json!("qwen"), json!(0.2)],
            version_id: Uuid::nil(),
        }),
        ..base()
    };
    let sql = render(&query);
    assert!(
        !sql.contains("(v.model_id, v.gen_temperature, v.version_id) >"),
        "a row-value comparison carries one direction only: {sql}"
    );
    insta::assert_snapshot!("mixed_directions", sql);
}

#[test]
fn version_all_drops_the_latest_clause() {
    let all = render(&ir::Query {
        version: ir::VersionSelector::All,
        ..base()
    });
    assert!(!all.contains("SELECT max(seq)"));
    assert!(
        all.contains("v.tombstoned_at IS NULL"),
        "a tombstone has no body to match"
    );
}

#[test]
fn unrenderable_ir_is_refused_rather_than_spliced() {
    // A column name that is not in the migration.
    let sneaky = ir::Query {
        filter: Some(cmp(generated("body) --"), ir::Op::Eq, json!("x"))),
        ..base()
    };
    assert!(query_sql::render(&sneaky, &[]).is_err());

    // An array column in the wrong table.
    let crossed = ir::Query {
        filter: Some(ir::Filter::Any {
            table: ir::ArrayTable::Relations,
            conditions: vec![ir::Cmp {
                column: ir::Column::Array(ir::ArrayColumn::ResultValue),
                op: ir::Op::Gt,
                value: json!(1),
            }],
        }),
        ..base()
    };
    assert!(query_sql::render(&crossed, &[]).is_err());

    // A cursor that does not match the sort.
    let mismatched = ir::Query {
        sort: vec![ir::Sort {
            key: ir::SortKey::CreatedAt,
            dir: ir::Dir::Desc,
        }],
        cursor: Some(ir::Cursor {
            keys: vec![],
            version_id: Uuid::nil(),
        }),
        ..base()
    };
    assert!(query_sql::render(&mismatched, &[]).is_err());

    // A range on an unregistered path, which the GIN index cannot serve.
    let unindexed = ir::Query {
        filter: Some(cmp(
            ir::Column::Body(vec!["ext".into(), "bob/x".into()]),
            ir::Op::Gt,
            json!(1),
        )),
        ..base()
    };
    assert!(query_sql::render(&unindexed, &[]).is_err());
}

// ---------------------------------------------------------------------
// execution
// ---------------------------------------------------------------------

/// Insert a namespace, a record and one live version with the given body.
async fn seed(pool: &PgPool, ns: &str, name: &str, visibility: &str, body: Value) -> Uuid {
    sqlx::query("INSERT INTO namespaces (ns, kind) VALUES ($1, 'user') ON CONFLICT DO NOTHING")
        .bind(ns)
        .execute(pool)
        .await
        .unwrap();
    let record_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO records (id, type, ns, name, visibility) VALUES ($1, 'card', $2, $3, $4)",
    )
    .bind(record_id)
    .bind(ns)
    .bind(name)
    .bind(visibility)
    .execute(pool)
    .await
    .unwrap();
    let version_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO versions (version_id, record_id, seq, content_hash, body) VALUES ($1, $2, 1, $3, $4)",
    )
    .bind(version_id)
    .bind(record_id)
    .bind(common::sha256(name.as_bytes()).as_slice())
    .bind(&body)
    .execute(pool)
    .await
    .unwrap();
    version_id
}

async fn add_result(pool: &PgPool, version_id: Uuid, metric: &str, value: f64) {
    sqlx::query(
        "INSERT INTO results (version_id, ordinal, metric, aggregation, value, n)
         VALUES ($1, (SELECT coalesce(max(ordinal) + 1, 0) FROM results WHERE version_id = $1), $2, 'mean', $3, 33)",
    )
    .bind(version_id)
    .bind(metric)
    .bind(value)
    .execute(pool)
    .await
    .unwrap();
}

async fn add_fingerprint(pool: &PgPool, version_id: Uuid, facet: &str, fingerprint: &[u8]) {
    sqlx::query("INSERT INTO fingerprints (version_id, facet, fingerprint) VALUES ($1, $2, $3)")
        .bind(version_id)
        .bind(facet)
        .bind(fingerprint)
        .execute(pool)
        .await
        .unwrap();
}

fn names(hits: &[QueryHit]) -> Vec<&str> {
    hits.iter().map(|h| h.name.as_str()).collect()
}

async fn run(pool: &PgPool, query: &ir::Query) -> Vec<QueryHit> {
    query_sql::run(pool, query, &["alice".to_string()])
        .await
        .unwrap()
        .0
}

/// A body with the facets the generated columns read.
fn card(model: &str, temperature: f64, rung: i64) -> Value {
    json!({
        "schema": "evalhub.card/1.0",
        "title": format!("{model} at {temperature}"),
        "producer": {"name": "my-harness", "version": "0.4.1"},
        "model": {"id": model},
        "task": {"id": "single2", "split": "test"},
        "harness": {"name": "my-harness", "version": "0.4.1"},
        "generation": {"temperature": temperature},
        "ext": {"alice/qwen-loop": {"rung": rung}}
    })
}

#[tokio::test]
async fn relation_filters_see_only_targets_the_caller_may_see() {
    let db = common::db().await;
    let pool = &db.pool;

    // bob's public Card cites bob's private Eval; the edge resolved.
    let (_, secret) = common::seed_version(pool, "eval", "bob", "secret", 1, "private", "s").await;
    let cites = seed(pool, "bob", "cites", "public", card("qwen3.6-32b", 0.0, 4)).await;
    sqlx::query(
        "INSERT INTO relations (from_version_id, type, to_version_id) VALUES ($1, 'core/uses_eval', $2)",
    )
    .bind(cites)
    .bind(secret)
    .execute(pool)
    .await
    .unwrap();

    let by = |column, op, value: Value| ir::Query {
        filter: Some(ir::Filter::Any {
            table: ir::ArrayTable::Relations,
            conditions: vec![ir::Cmp {
                column: ir::Column::Array(column),
                op,
                value,
            }],
        }),
        ..base()
    };
    let exact = by(
        ir::ArrayColumn::RelationTo,
        ir::Op::Eq,
        json!("bob/secret@1"),
    );
    let prefix = by(ir::ArrayColumn::RelationTo, ir::Op::Prefix, json!("bob/"));
    let typed = by(
        ir::ArrayColumn::RelationType,
        ir::Op::Eq,
        json!("core/uses_eval"),
    );

    // alice sees the Card, but not the edge: no filter over it matches,
    // so the query cannot say that bob/secret@1 exists or who cites it.
    assert_eq!(names(&run(pool, &base()).await), vec!["cites"]);
    for q in [&exact, &prefix, &typed] {
        assert!(run(pool, q).await.is_empty(), "{q:?}");
    }
    // Negated, the edge is equally absent.
    let not = ir::Query {
        filter: Some(ir::Filter::Not(Box::new(exact.filter.clone().unwrap()))),
        ..base()
    };
    assert_eq!(names(&run(pool, &not).await), vec!["cites"]);

    // bob sees both.
    let bob = ["bob".to_string()];
    for q in [&exact, &prefix, &typed] {
        let (hits, _) = query_sql::run(pool, q, &bob).await.unwrap();
        assert_eq!(names(&hits), vec!["cites"], "{q:?}");
    }
}

#[tokio::test]
async fn filters_over_columns_fingerprints_results_and_relations() {
    let db = common::db().await;
    let pool = &db.pool;

    let a = seed(pool, "alice", "a", "private", card("qwen3.6-32b", 0.0, 4)).await;
    let b = seed(pool, "alice", "b", "private", card("qwen3.6-32b", 0.9, 2)).await;
    let c = seed(pool, "alice", "c", "private", card("other-model", 0.1, 7)).await;
    seed(
        pool,
        "bob",
        "hidden",
        "private",
        card("qwen3.6-32b", 0.0, 4),
    )
    .await;
    seed(pool, "bob", "open", "public", card("qwen3.6-32b", 0.0, 4)).await;

    add_result(pool, a, "core/pass_rate", 0.62).await;
    add_result(pool, b, "core/pass_rate", 0.10).await;
    add_result(pool, c, "core/accuracy", 0.99).await;
    add_fingerprint(pool, a, "model", &[0xaa, 0xbb]).await;
    add_fingerprint(pool, b, "model", &[0xaa, 0xbb]).await;
    add_fingerprint(pool, c, "model", &[0xcc]).await;
    sqlx::query("INSERT INTO relations (from_version_id, type, to_external) VALUES ($1, 'core/uses_eval', 'alice/single2-k4@3')")
        .bind(a)
        .execute(pool)
        .await
        .unwrap();

    // The caller sees their own private records and anyone's public ones.
    let all = run(pool, &base()).await;
    let mut visible = names(&all);
    visible.sort_unstable();
    assert_eq!(visible, vec!["a", "b", "c", "open"]);

    // Equality on a generated column.
    let hits = run(
        pool,
        &ir::Query {
            filter: Some(cmp(generated("model_id"), ir::Op::Eq, json!("other-model"))),
            ..base()
        },
    )
    .await;
    assert_eq!(names(&hits), vec!["c"]);

    // A range on a numeric generated column.
    let hits = run(
        pool,
        &ir::Query {
            filter: Some(cmp(generated("gen_temperature"), ir::Op::Lte, json!(0.2))),
            ..base()
        },
    )
    .await;
    let mut got = names(&hits);
    got.sort_unstable();
    assert_eq!(got, vec!["a", "c", "open"]);

    // A fingerprint.
    let hits = run(
        pool,
        &ir::Query {
            filter: Some(cmp(
                ir::Column::Fingerprint("model".into()),
                ir::Op::Eq,
                json!("aabb"),
            )),
            ..base()
        },
    )
    .await;
    let mut got = names(&hits);
    got.sort_unstable();
    assert_eq!(got, vec!["a", "b"]);

    // `any` over results: metric and value on the same row.
    let hits = run(
        pool,
        &ir::Query {
            filter: Some(ir::Filter::Any {
                table: ir::ArrayTable::Results,
                conditions: vec![
                    ir::Cmp {
                        column: ir::Column::Array(ir::ArrayColumn::ResultMetric),
                        op: ir::Op::Eq,
                        value: json!("core/pass_rate"),
                    },
                    ir::Cmp {
                        column: ir::Column::Array(ir::ArrayColumn::ResultValue),
                        op: ir::Op::Gte,
                        value: json!(0.5),
                    },
                ],
            }),
            ..base()
        },
    )
    .await;
    assert_eq!(
        names(&hits),
        vec!["a"],
        "b reports 0.10, c a different metric"
    );

    // `any` over relations, matching the textual target by prefix.
    let hits = run(
        pool,
        &ir::Query {
            filter: Some(ir::Filter::Any {
                table: ir::ArrayTable::Relations,
                conditions: vec![ir::Cmp {
                    column: ir::Column::Array(ir::ArrayColumn::RelationTo),
                    op: ir::Op::Prefix,
                    value: json!("alice/single2-k4"),
                }],
            }),
            ..base()
        },
    )
    .await;
    assert_eq!(names(&hits), vec!["a"]);

    // An unregistered ext path, by containment.
    let hits = run(
        pool,
        &ir::Query {
            filter: Some(cmp(
                ir::Column::Body(vec!["ext".into(), "alice/qwen-loop".into()]),
                ir::Op::Eq,
                json!({"rung": 7}),
            )),
            ..base()
        },
    )
    .await;
    assert_eq!(names(&hits), vec!["c"]);

    // Existence, and its negation.
    let hits = run(
        pool,
        &ir::Query {
            filter: Some(ir::Filter::Not(Box::new(ir::Filter::Exists {
                column: generated("model_revision"),
            }))),
            ..base()
        },
    )
    .await;
    assert_eq!(hits.len(), 4, "no fixture records a revision");
}

#[tokio::test]
async fn a_registered_ext_path_uses_its_expression_index() {
    let db = common::db().await;
    let pool = &db.pool;

    // Enough rows that the planner has something to choose between.
    seed(pool, "alice", "seed", "private", card("qwen", 0.0, 1)).await;
    for i in 0..500 {
        let body = card("qwen", 0.0, i);
        sqlx::query(
            "INSERT INTO versions (version_id, record_id, seq, content_hash, body)
             VALUES ($1, (SELECT id FROM records WHERE ns = 'alice' AND name = 'seed'), $2, $3, $4)",
        )
        .bind(Uuid::new_v4())
        .bind(i32::try_from(i).unwrap() + 2)
        .bind(common::sha256(&i.to_le_bytes()).as_slice())
        .bind(&body)
        .execute(pool)
        .await
        .unwrap();
    }

    let body = json!({
        "ns": "alice",
        "fingerprint": false,
        "schema": {"type": "object", "properties": {"rung": {"type": "integer"}}}
    });
    registry::put(
        pool,
        Kind::ExtSchemas,
        "alice",
        "qwen-loop",
        "1",
        &body,
        Actor::default(),
    )
    .await
    .unwrap();
    assert_eq!(index::apply_pending(pool).await.unwrap(), Some(1));

    let path = ext_path();
    let name = index::index_name(&path);
    assert!(index::is_valid(pool, &name).await.unwrap());

    sqlx::query("ANALYZE versions").execute(pool).await.unwrap();

    // The claim under test is textual: the expression the compiler emits
    // is the one the index was built with, so the planner can match them.
    // Sequential scans are disabled to make the planner say so on a table
    // this small — this is a matching test, not a performance one.
    let expression = query_sql::ext_expression(&path, ir::ValueType::Number).unwrap();
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *conn)
        .await
        .ok();
    let plan: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN SELECT 1 FROM versions v WHERE {expression} > 3 AND v.tombstoned_at IS NULL"
    )))
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    let plan = plan.join("\n");
    assert!(
        plan.contains(&name),
        "the planner did not match the expression index {name}:\n{plan}"
    );

    // And the compiled query returns the right rows.
    let hits = run(
        pool,
        &ir::Query {
            filter: Some(cmp(
                ir::Column::Ext {
                    path,
                    ty: ir::ValueType::Number,
                },
                ir::Op::Gte,
                json!(400),
            )),
            version: ir::VersionSelector::All,
            limit: 200,
            ..base()
        },
    )
    .await;
    assert_eq!(hits.len(), 100, "rungs 400..=499");
}

#[tokio::test]
async fn keyset_pagination_walks_a_metric_sort() {
    let db = common::db().await;
    let pool = &db.pool;

    for (name, value) in [("low", 0.10), ("mid", 0.50), ("high", 0.90)] {
        let v = seed(pool, "alice", name, "private", card("qwen", 0.0, 1)).await;
        add_result(pool, v, "core/pass_rate", value).await;
    }

    let query = ir::Query {
        sort: vec![ir::Sort {
            key: ir::SortKey::Metric("core/pass_rate".into()),
            dir: ir::Dir::Desc,
        }],
        limit: 2,
        ..base()
    };
    let (page1, cursor) = query_sql::run(pool, &query, &["alice".to_string()])
        .await
        .unwrap();
    assert_eq!(names(&page1), vec!["high", "mid"]);
    let cursor = cursor.expect("a third record remains");
    assert_eq!(cursor.keys, vec![json!(0.5)]);

    let (page2, next) = query_sql::run(
        pool,
        &ir::Query {
            cursor: Some(cursor),
            ..query.clone()
        },
        &["alice".to_string()],
    )
    .await
    .unwrap();
    assert_eq!(names(&page2), vec!["low"]);
    assert!(next.is_none(), "the last page has no cursor");
}

#[tokio::test]
async fn latest_selects_one_version_and_all_selects_every_live_one() {
    let db = common::db().await;
    let pool = &db.pool;

    seed(pool, "alice", "x", "private", card("qwen", 0.0, 1)).await;
    let record_id: Uuid = sqlx::query_scalar("SELECT id FROM records WHERE name = 'x'")
        .fetch_one(pool)
        .await
        .unwrap();
    for seq in 2..=3 {
        sqlx::query(
            "INSERT INTO versions (version_id, record_id, seq, content_hash, body) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(record_id)
        .bind(seq)
        .bind(common::sha256(&[seq as u8]).as_slice())
        .bind(card("qwen", 0.0, i64::from(seq)))
        .execute(pool)
        .await
        .unwrap();
    }
    // Tombstone the newest, so "latest" has to fall back.
    sqlx::query(
        "UPDATE versions SET tombstoned_at = now(), tombstone_reason = 'withdrawn', body = NULL
         WHERE record_id = $1 AND seq = 3",
    )
    .bind(record_id)
    .execute(pool)
    .await
    .unwrap();

    let latest = run(pool, &base()).await;
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].seq, 2, "the tombstone is not the latest");

    let all = run(
        pool,
        &ir::Query {
            version: ir::VersionSelector::All,
            ..base()
        },
    )
    .await;
    let mut seqs: Vec<i32> = all.iter().map(|h| h.seq).collect();
    seqs.sort_unstable();
    assert_eq!(seqs, vec![1, 2], "a tombstone matches nothing");
}
