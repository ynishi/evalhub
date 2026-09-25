//! Compilation of `evalhub_query::ir::RunQuery` to SQL: the run projection
//! of one Eval (`GET /evals/{ns}/{name}/runs`).
//!
//! [`crate::query_sql`] is bound to `records JOIN versions` and one row per
//! version; a row here is a run of one Eval record, so this is a renderer
//! of its own, with the same rules: the IR arrives with every path
//! resolved, every literal is a bind parameter, and only names this module
//! chooses reach the SQL text. [`crate::runs::project`] is the caller: it
//! decides the Eval, the reader's membership and the Cards (the
//! [`RunScope`]) and reads the side rows of the page.
//!
//! # The statement
//!
//! ```text
//! WITH p0 AS (                       one Eval's runs, masked for the reader
//!   SELECT run_id, content_hash, state, shown,
//!          CASE WHEN shown THEN status/error_kind/started_at/ended_at/body END
//!   FROM runs WHERE record_id = $ AND <rows the reader may list>
//! ), p AS (SELECT p.*, <sort key i> AS k{i} FROM p0 p)
//! SELECT … FROM p WHERE <filter> AND <keyset> ORDER BY k…, run_id LIMIT n+1
//! ```
//!
//! | `RunColumn` / `Column`  | Renders to                                                            |
//! | ----------------------- | --------------------------------------------------------------------- |
//! | `RunId`, `Status`, `ErrorKind` | `p.run_id`, `p.status`, `p.error_kind`                         |
//! | `StartedAt`, `EndedAt`  | `p.started_at`, `p.ended_at` (`timestamptz`; literals are parsed)     |
//! | `Facet(path)`           | `evalhub_{num,str,bool}(p.body #> '{…}')`, typed by the run table     |
//! | `Fingerprint(facet)`    | `(SELECT encode(fingerprint, 'hex') FROM run_fingerprints …)`         |
//! | `Metric(id)`            | `(SELECT value FROM run_metrics … AND metric = $)`                    |
//! | `Result { card, … }`    | `EXISTS (SELECT 1 FROM run_results … AND <cmp>)`; sorted by the best value in the sort's direction |
//! | `Ext { path, ty }`      | `evalhub_{num,str,bool}(p.body #> '{…}')`, below the run's body       |
//! | `Body(path)` eq / exists | `p.body @> $` / `p.body @? $`                                        |
//!
//! `Column::Generated`, `Column::Fingerprint`, `Column::Array` and `any`
//! are record columns; the type check never puts them in a run query, and
//! meeting one is [`StoreError::QueryUnsupported`].
//!
//! # Which rows, and what a row shows
//!
//! Rows are the runs of one Eval record that are neither archived nor
//! deleted; plus the archived ones when `include.archived` and the
//! deleted ones when `include.deleted`, both only for a member of the
//! Eval's namespace; plus every run in the used set of a Card of the
//! request, whatever its state, so a Card's judgement of a run that has
//! since been archived or deleted still has a row to sit on.
//!
//! A row is *shown* (`shown`) when it is not deleted, and, if archived,
//! the reader is a member. A row that is not shown is in the deleted
//! form: `state` is `deleted` (for a non-member an archived run reads
//! exactly like a deleted one), and every run column is NULL already in
//! `p0`. Because the mask is applied before the filter and the sort see
//! the row, a filter or sort cannot reveal what the reader may not see:
//! `metrics[x] > 5` is false for a masked row, as it is for a run without
//! that metric. The Card's judgements are not masked; they are the Card's
//! claims and follow the Card.
//!
//! # Sorting and keyset pagination
//!
//! Every sort key is computed once, as `k{i}` of `p`, and the keyset
//! predicate and `ORDER BY` refer to that column, so a bound metric id is
//! bound once per key. `ORDER BY` is `k{i} ASC NULLS LAST` or
//! `k{i} DESC NULLS FIRST` (Postgres's defaults, spelled out) and ends
//! with `p.run_id` in the last key's direction (ascending with no key),
//! which makes the tuple unique within the Eval. The cursor is
//! `ir::RunCursor { keys, run_id }`.
//!
//! Unlike [`crate::query_sql`], the keyset predicate is NULL-aware, since a
//! run without a metric, a judgement or a timestamp is ordinary here: the
//! lexicographic expansion is always used, and each key's "after" and
//! "equal" follow its NULL placement.
//!
//! ```text
//! ASC  NULLS LAST,  cursor c:  after = k > c OR k IS NULL   equal = k = c
//!                   cursor NULL: after = FALSE              equal = k IS NULL
//! DESC NULLS FIRST, cursor c:  after = k < c                equal = k = c
//!                   cursor NULL: after = k IS NOT NULL      equal = k IS NULL
//! ```
//!
//! A row-value comparison would serve from an index, but the scan is
//! always one Eval's runs (`runs` is keyed by `(record_id, run_id)`),
//! bounded by the page limit and the batch limits, and correctness over a
//! NULL key matters more here than the range scan.
//!
//! # Timestamps
//!
//! The DSL types `started_at` / `ended_at` as strings (RFC 3339); the
//! columns are `timestamptz`, so a literal is parsed and bound as a
//! timestamp, and compared as one. A literal that is not RFC 3339 exactly
//! (`DateTime::parse_from_rfc3339`; a space for the `T` is accepted, as
//! RFC 3339 §5.6 allows) is
//! [`StoreError::QueryUnsupported`], naming the literal and whether it
//! came from the filter or the cursor. `prefix` / `contains` are refused on
//! them: a timestamp has no text form to match against.

use std::sync::OnceLock;

use evalhub_query::PathTable;
use evalhub_query::ir;
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::error::StoreError;
use crate::query_sql::{
    KeyType, array_literal, as_bool, as_f64, as_string, escape_like, jsonpath, nest,
    parse_timestamp, push_key_value, read_key,
};

/// The facets a run has, as `run_fingerprints` constrains them.
pub const RUN_FACETS: &[&str] = &["model", "task", "harness", "generation", "trial", "env"];

/// Which archived and deleted runs a reader asked for
/// (`include=archived,deleted`). Honoured only for a member of the Eval's
/// namespace; see the module doc.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunInclude {
    /// List archived runs.
    pub archived: bool,
    /// List deleted (tombstoned) runs.
    pub deleted: bool,
}

/// What a run projection statement is scoped to, decided by
/// [`crate::runs::project`] before the SQL is built.
#[derive(Debug, Clone)]
pub struct RunScope<'a> {
    /// The Eval record whose runs are read.
    pub record_id: Uuid,
    /// The reader is a member of the Eval's namespace.
    pub member: bool,
    /// Archived / deleted runs asked for.
    pub include: RunInclude,
    /// The Cards of the request, each with the version whose
    /// `run_results` and `card_eval_runs` are joined (its latest live
    /// version). Every `RunColumn::Result` names one of these.
    pub cards: &'a [(ir::CardRef, Uuid)],
}

/// One row of the statement's result: the run's columns as masked for
/// the reader. Side rows (metrics, fingerprints, judgements) are read by
/// the caller.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RawRun {
    pub run_id: String,
    pub content_hash: Vec<u8>,
    /// `live`, `archived` or `deleted`, as the reader may see it.
    pub state: String,
    pub shown: bool,
    pub status: Option<String>,
    pub error_kind: Option<String>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A sort key resolved to what renders it.
struct SortPlan {
    expr: Expr,
    dir: ir::Dir,
    ty: KeyType,
}

/// A scalar expression over the row `p`. `Plain` needs no bind;
/// the others carry the values they bind.
enum Expr {
    Plain(String),
    Metric(String),
    /// The best judgement in the sort's direction, for a sort key.
    Result {
        version_id: Uuid,
        metric: String,
        field: ir::ResultField,
        dir: ir::Dir,
    },
}

/// The run projection's path table, for the types of facet keys (the IR
/// carries a facet key's path, not its type).
fn run_table() -> &'static PathTable {
    static TABLE: OnceLock<PathTable> = OnceLock::new();
    TABLE.get_or_init(|| PathTable::for_runs(&[]))
}

/// The type of a facet key of a run, from the run table.
fn facet_type(path: &[String]) -> Result<ir::ValueType, StoreError> {
    match run_table().lookup(&path.join(".")) {
        Some(evalhub_query::PathInfo::Scalar { ty, .. }) => Ok(ty),
        _ => Err(StoreError::QueryUnsupported(format!(
            "{path:?} is not a facet key of a run"
        ))),
    }
}

fn cast(ty: ir::ValueType) -> &'static str {
    match ty {
        ir::ValueType::Number => "evalhub_num",
        ir::ValueType::String => "evalhub_str",
        ir::ValueType::Boolean => "evalhub_bool",
    }
}

/// A typed read of a path below the run's body.
fn body_expr(path: &[String], ty: ir::ValueType) -> Result<String, StoreError> {
    Ok(format!("{}(p.body #> {})", cast(ty), array_literal(path)?))
}

fn fingerprint_expr(facet: &str) -> Result<String, StoreError> {
    if !RUN_FACETS.contains(&facet) {
        return Err(StoreError::QueryUnsupported(format!(
            "unknown facet {facet:?}"
        )));
    }
    // `facet` is one of the constants above, so it may be spelled out.
    Ok(format!(
        "(SELECT encode(f.fingerprint, 'hex') FROM run_fingerprints f \
         WHERE f.record_id = p.record_id AND f.run_id = p.run_id AND f.facet = '{facet}' \
         AND p.shown)"
    ))
}

/// The version joined for `card`, or unsupported when the scope lacks it.
fn card_version(scope: &RunScope<'_>, card: &ir::CardRef) -> Result<Uuid, StoreError> {
    scope
        .cards
        .iter()
        .find(|(c, _)| c == card)
        .map(|(_, v)| *v)
        .ok_or_else(|| StoreError::QueryUnsupported(format!("card {card} is not joined")))
}

/// A run column as a scalar expression and its key type. `dir` is the
/// sort direction when the expression is a sort key (it picks the best
/// judgement); a filter never asks for `Result` here.
fn column_expr(
    column: &ir::Column,
    scope: &RunScope<'_>,
    dir: Option<ir::Dir>,
) -> Result<(Expr, KeyType), StoreError> {
    use ir::RunColumn as R;
    Ok(match column {
        ir::Column::Run(R::RunId) => (Expr::Plain("p.run_id".into()), KeyType::Text),
        ir::Column::Run(R::Status) => (Expr::Plain("p.status".into()), KeyType::Text),
        ir::Column::Run(R::ErrorKind) => (Expr::Plain("p.error_kind".into()), KeyType::Text),
        ir::Column::Run(R::StartedAt) => (Expr::Plain("p.started_at".into()), KeyType::Timestamp),
        ir::Column::Run(R::EndedAt) => (Expr::Plain("p.ended_at".into()), KeyType::Timestamp),
        ir::Column::Run(R::Facet(path)) => {
            let ty = facet_type(path)?;
            (Expr::Plain(body_expr(path, ty)?), ty.into())
        }
        ir::Column::Run(R::Fingerprint(facet)) => {
            (Expr::Plain(fingerprint_expr(facet)?), KeyType::Text)
        }
        ir::Column::Run(R::Metric(metric)) => (Expr::Metric(metric.clone()), KeyType::Number),
        ir::Column::Run(R::Result {
            card,
            metric,
            field,
        }) => {
            let dir = dir.ok_or_else(|| {
                StoreError::QueryUnsupported("a judgement is compared with EXISTS".into())
            })?;
            let ty = match field {
                ir::ResultField::Value => KeyType::Number,
                ir::ResultField::Label => KeyType::Text,
            };
            (
                Expr::Result {
                    version_id: card_version(scope, card)?,
                    metric: metric.clone(),
                    field: *field,
                    dir,
                },
                ty,
            )
        }
        ir::Column::Ext { path, ty } => (Expr::Plain(body_expr(path, *ty)?), (*ty).into()),
        other => {
            return Err(StoreError::QueryUnsupported(format!(
                "{other:?} is not a column of the run projection"
            )));
        }
    })
}

fn push_expr(q: &mut QueryBuilder<Postgres>, expr: &Expr) {
    match expr {
        Expr::Plain(s) => {
            q.push(s.as_str());
        }
        Expr::Metric(metric) => {
            q.push(
                "(SELECT m.value FROM run_metrics m WHERE m.record_id = p.record_id \
                 AND m.run_id = p.run_id AND p.shown AND m.metric = ",
            );
            q.push_bind(metric.clone());
            q.push(")");
        }
        Expr::Result {
            version_id,
            metric,
            field,
            dir,
        } => {
            let col = match field {
                ir::ResultField::Value => "value",
                ir::ResultField::Label => "label",
            };
            q.push(format!(
                "(SELECT rr.{col} FROM run_results rr WHERE rr.version_id = "
            ));
            q.push_bind(*version_id);
            q.push(
                " AND rr.eval_record_id = p.record_id AND rr.run_id = p.run_id AND rr.metric = ",
            );
            q.push_bind(metric.clone());
            q.push(format!(
                " AND rr.{col} IS NOT NULL ORDER BY rr.{col} {}, rr.ordinal LIMIT 1)",
                dir_sql(*dir)
            ));
        }
    }
}

fn dir_sql(dir: ir::Dir) -> &'static str {
    match dir {
        ir::Dir::Asc => "ASC",
        ir::Dir::Desc => "DESC",
    }
}

fn plan_sorts(query: &ir::RunQuery, scope: &RunScope<'_>) -> Result<Vec<SortPlan>, StoreError> {
    query
        .sort
        .iter()
        .map(|s| {
            let column = match &s.key {
                ir::SortKey::Run(c) => ir::Column::Run(c.clone()),
                ir::SortKey::Column(c @ ir::Column::Ext { .. }) => c.clone(),
                other => {
                    return Err(StoreError::QueryUnsupported(format!(
                        "{other:?} is not a sort key of the run projection"
                    )));
                }
            };
            let (expr, ty) = column_expr(&column, scope, Some(s.dir))?;
            Ok(SortPlan {
                expr,
                dir: s.dir,
                ty,
            })
        })
        .collect()
}

/// Build the statement. Separated from [`fetch`] so that [`render`] can
/// show the SQL without a database.
fn build(query: &ir::RunQuery, scope: &RunScope<'_>) -> Result<QueryBuilder<Postgres>, StoreError> {
    let sorts = plan_sorts(query, scope)?;
    // Whether the row is shown to the reader. `member` is decided by the
    // caller, not by input, so it is spelled as a constant.
    let shown = if scope.member {
        "(u.tombstoned_at IS NULL)"
    } else {
        "(u.tombstoned_at IS NULL AND u.archived_at IS NULL)"
    };

    let mut q: QueryBuilder<Postgres> = QueryBuilder::new(format!(
        "WITH p0 AS (SELECT u.record_id, u.run_id, u.content_hash, \
         CASE WHEN NOT {shown} THEN 'deleted' WHEN u.archived_at IS NOT NULL THEN 'archived' \
         ELSE 'live' END AS state, \
         {shown} AS shown, \
         CASE WHEN {shown} THEN u.status END AS status, \
         CASE WHEN {shown} THEN u.error_kind END AS error_kind, \
         CASE WHEN {shown} THEN u.started_at END AS started_at, \
         CASE WHEN {shown} THEN u.ended_at END AS ended_at, \
         CASE WHEN {shown} THEN u.body END AS body \
         FROM runs u WHERE u.record_id = "
    ));
    q.push_bind(scope.record_id);
    q.push(" AND ((u.archived_at IS NULL AND u.tombstoned_at IS NULL)");
    if scope.member && scope.include.archived {
        q.push(" OR (u.archived_at IS NOT NULL AND u.tombstoned_at IS NULL)");
    }
    if scope.member && scope.include.deleted {
        q.push(" OR u.tombstoned_at IS NOT NULL");
    }
    if !scope.cards.is_empty() {
        let versions: Vec<Uuid> = scope.cards.iter().map(|(_, v)| *v).collect();
        q.push(
            " OR EXISTS (SELECT 1 FROM card_eval_runs c WHERE c.eval_record_id = u.record_id \
             AND c.run_id = u.run_id AND c.card_version_id = ANY(",
        );
        q.push_bind(versions);
        q.push("))");
    }
    q.push(")), p AS (SELECT p.*");
    for (i, plan) in sorts.iter().enumerate() {
        q.push(", ");
        push_expr(&mut q, &plan.expr);
        q.push(format!(" AS k{i}"));
    }
    q.push(
        " FROM p0 p) SELECT p.run_id, p.content_hash, p.state, p.shown, p.status, \
         p.error_kind, p.started_at, p.ended_at",
    );
    for i in 0..sorts.len() {
        q.push(format!(", p.k{i}"));
    }
    q.push(" FROM p WHERE TRUE");

    if let Some(filter) = &query.filter {
        q.push(" AND ");
        push_filter(&mut q, filter, scope)?;
    }
    if let Some(cursor) = &query.cursor {
        push_keyset(&mut q, &sorts, cursor)?;
    }

    q.push(" ORDER BY ");
    for (i, plan) in sorts.iter().enumerate() {
        q.push(format!("p.k{i} {}, ", order_sql(plan.dir)));
    }
    let tiebreak = sorts.last().map(|p| p.dir).unwrap_or(ir::Dir::Asc);
    q.push(format!("p.run_id {}", dir_sql(tiebreak)));
    q.push(" LIMIT ");
    q.push_bind(i64::from(query.limit.max(1)) + 1);
    Ok(q)
}

fn order_sql(dir: ir::Dir) -> &'static str {
    match dir {
        ir::Dir::Asc => "ASC NULLS LAST",
        ir::Dir::Desc => "DESC NULLS FIRST",
    }
}

/// The NULL-aware keyset predicate; see the module doc.
fn push_keyset(
    q: &mut QueryBuilder<Postgres>,
    sorts: &[SortPlan],
    cursor: &ir::RunCursor,
) -> Result<(), StoreError> {
    if cursor.keys.len() != sorts.len() {
        return Err(StoreError::QueryUnsupported(format!(
            "cursor carries {} key(s) for {} sort key(s)",
            cursor.keys.len(),
            sorts.len()
        )));
    }
    let tiebreak = sorts.last().map(|p| p.dir).unwrap_or(ir::Dir::Asc);
    q.push(" AND (");
    for i in 0..=sorts.len() {
        if i > 0 {
            q.push(" OR ");
        }
        q.push("(TRUE");
        for (j, plan) in sorts.iter().take(i).enumerate() {
            q.push(" AND ");
            push_key_equal(q, j, plan.ty, &cursor.keys[j])?;
        }
        q.push(" AND ");
        match sorts.get(i) {
            Some(plan) => push_key_after(q, i, plan, &cursor.keys[i])?,
            None => {
                q.push(format!(
                    "p.run_id {} ",
                    match tiebreak {
                        ir::Dir::Asc => ">",
                        ir::Dir::Desc => "<",
                    }
                ));
                q.push_bind(cursor.run_id.clone());
            }
        }
        q.push(")");
    }
    q.push(")");
    Ok(())
}

fn push_key_equal(
    q: &mut QueryBuilder<Postgres>,
    i: usize,
    ty: KeyType,
    value: &Value,
) -> Result<(), StoreError> {
    if value.is_null() {
        q.push(format!("p.k{i} IS NULL"));
    } else {
        q.push(format!("p.k{i} = "));
        push_key_value(q, ty, value, "cursor")?;
    }
    Ok(())
}

fn push_key_after(
    q: &mut QueryBuilder<Postgres>,
    i: usize,
    plan: &SortPlan,
    value: &Value,
) -> Result<(), StoreError> {
    match (plan.dir, value.is_null()) {
        (ir::Dir::Asc, true) => {
            q.push("FALSE");
        }
        (ir::Dir::Asc, false) => {
            q.push(format!("(p.k{i} > "));
            push_key_value(q, plan.ty, value, "cursor")?;
            q.push(format!(" OR p.k{i} IS NULL)"));
        }
        (ir::Dir::Desc, true) => {
            q.push(format!("p.k{i} IS NOT NULL"));
        }
        (ir::Dir::Desc, false) => {
            q.push(format!("p.k{i} < "));
            push_key_value(q, plan.ty, value, "cursor")?;
        }
    }
    Ok(())
}

fn push_filter(
    q: &mut QueryBuilder<Postgres>,
    filter: &ir::Filter,
    scope: &RunScope<'_>,
) -> Result<(), StoreError> {
    match filter {
        ir::Filter::And(children) => push_junction(q, children, "AND", "TRUE", scope),
        ir::Filter::Or(children) => push_junction(q, children, "OR", "FALSE", scope),
        ir::Filter::Not(inner) => {
            q.push("NOT (");
            push_filter(q, inner, scope)?;
            q.push(")");
            Ok(())
        }
        ir::Filter::Cmp(cmp) => push_cmp(q, cmp, scope),
        ir::Filter::Exists { column } => push_exists(q, column, scope),
        ir::Filter::Any { .. } => Err(StoreError::QueryUnsupported(
            "`any` has nothing to range over in the run projection".into(),
        )),
    }
}

fn push_junction(
    q: &mut QueryBuilder<Postgres>,
    children: &[ir::Filter],
    joiner: &str,
    empty: &str,
    scope: &RunScope<'_>,
) -> Result<(), StoreError> {
    if children.is_empty() {
        q.push(empty);
        return Ok(());
    }
    q.push("(");
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            q.push(format!(" {joiner} "));
        }
        push_filter(q, child, scope)?;
    }
    q.push(")");
    Ok(())
}

/// The start of an `EXISTS` over the judgements of one Card on one run and
/// metric; the caller appends its condition and the closing parenthesis.
fn push_result_exists(
    q: &mut QueryBuilder<Postgres>,
    scope: &RunScope<'_>,
    card: &ir::CardRef,
    metric: &str,
) -> Result<(), StoreError> {
    q.push("EXISTS (SELECT 1 FROM run_results rr WHERE rr.version_id = ");
    q.push_bind(card_version(scope, card)?);
    q.push(" AND rr.eval_record_id = p.record_id AND rr.run_id = p.run_id AND rr.metric = ");
    q.push_bind(metric.to_owned());
    Ok(())
}

fn push_cmp(
    q: &mut QueryBuilder<Postgres>,
    cmp: &ir::Cmp,
    scope: &RunScope<'_>,
) -> Result<(), StoreError> {
    match &cmp.column {
        // A judgement matches when any of the Card's entries for this run
        // and metric does (several may exist, partitioned by `by`).
        ir::Column::Run(ir::RunColumn::Result {
            card,
            metric,
            field,
        }) => {
            push_result_exists(q, scope, card, metric)?;
            q.push(" AND ");
            let (col, ty) = match field {
                ir::ResultField::Value => ("rr.value", KeyType::Number),
                ir::ResultField::Label => ("rr.label", KeyType::Text),
            };
            push_operator(q, &Expr::Plain(col.into()), cmp.op, ty, &cmp.value)?;
            q.push(")");
            Ok(())
        }
        ir::Column::Body(path) => match cmp.op {
            ir::Op::Eq => {
                q.push("p.body @> ");
                q.push_bind(nest(path, cmp.value.clone()));
                Ok(())
            }
            other => Err(StoreError::QueryUnsupported(format!(
                "{other:?} on an unregistered path; register an ext_schema to index it"
            ))),
        },
        column => {
            let (expr, ty) = column_expr(column, scope, None)?;
            push_operator(q, &expr, cmp.op, ty, &cmp.value)
        }
    }
}

fn push_exists(
    q: &mut QueryBuilder<Postgres>,
    column: &ir::Column,
    scope: &RunScope<'_>,
) -> Result<(), StoreError> {
    match column {
        ir::Column::Run(ir::RunColumn::Result {
            card,
            metric,
            field,
        }) => {
            push_result_exists(q, scope, card, metric)?;
            q.push(match field {
                ir::ResultField::Value => " AND rr.value IS NOT NULL)",
                ir::ResultField::Label => " AND rr.label IS NOT NULL)",
            });
        }
        ir::Column::Body(path) => {
            q.push("p.body @? ");
            q.push_bind(jsonpath(path)?);
            q.push("::jsonpath");
        }
        column => {
            let (expr, _) = column_expr(column, scope, None)?;
            q.push("(");
            push_expr(q, &expr);
            q.push(") IS NOT NULL");
        }
    }
    Ok(())
}

/// Apply one operator to one expression with one bound value, binding by
/// key type (timestamps are parsed from their RFC 3339 text).
fn push_operator(
    q: &mut QueryBuilder<Postgres>,
    expr: &Expr,
    op: ir::Op,
    ty: KeyType,
    value: &Value,
) -> Result<(), StoreError> {
    let symbol = match op {
        ir::Op::Eq => "=",
        ir::Op::Ne => "<>",
        ir::Op::Gt => ">",
        ir::Op::Gte => ">=",
        ir::Op::Lt => "<",
        ir::Op::Lte => "<=",
        ir::Op::In => {
            push_expr(q, expr);
            q.push(" = ANY(");
            push_array(q, ty, value)?;
            q.push(")");
            return Ok(());
        }
        ir::Op::Prefix | ir::Op::Contains => {
            if ty != KeyType::Text {
                return Err(StoreError::QueryUnsupported(format!(
                    "{op:?} on a non-string column"
                )));
            }
            let text = as_string(value)?;
            let pattern = match op {
                ir::Op::Prefix => format!("{}%", escape_like(&text)),
                _ => format!("%{}%", escape_like(&text)),
            };
            push_expr(q, expr);
            q.push(" LIKE ");
            q.push_bind(pattern);
            q.push(" ESCAPE '\\'");
            return Ok(());
        }
    };
    push_expr(q, expr);
    q.push(format!(" {symbol} "));
    push_key_value(q, ty, value, "filter")
}

fn push_array(
    q: &mut QueryBuilder<Postgres>,
    ty: KeyType,
    value: &Value,
) -> Result<(), StoreError> {
    let items = value
        .as_array()
        .ok_or_else(|| StoreError::QueryUnsupported("`in` needs an array".into()))?;
    match ty {
        KeyType::Number => {
            let v: Result<Vec<f64>, _> = items.iter().map(as_f64).collect();
            q.push_bind(v?);
        }
        KeyType::Text => {
            let v: Result<Vec<String>, _> = items.iter().map(as_string).collect();
            q.push_bind(v?);
        }
        KeyType::Boolean => {
            let v: Result<Vec<bool>, _> = items.iter().map(as_bool).collect();
            q.push_bind(v?);
        }
        KeyType::Timestamp => {
            let v: Result<Vec<chrono::DateTime<chrono::Utc>>, _> = items
                .iter()
                .map(|item| parse_timestamp(item, "filter"))
                .collect();
            q.push_bind(v?);
        }
    }
    Ok(())
}

/// The SQL the run projection compiles to, with binds as `$1`, `$2`, … in
/// the order they are pushed. For tests and for `EXPLAIN` by hand;
/// [`crate::runs::project`] executes the same statement.
pub fn render(query: &ir::RunQuery, scope: &RunScope<'_>) -> Result<String, StoreError> {
    Ok(build(query, scope)?.sql().as_str().to_owned())
}

/// Execute the statement and return one page of rows with the cursor that
/// continues it, or `None` when the page is the last.
pub(crate) async fn fetch(
    conn: &mut sqlx::PgConnection,
    query: &ir::RunQuery,
    scope: &RunScope<'_>,
) -> Result<(Vec<RawRun>, Option<ir::RunCursor>), StoreError> {
    let sorts = plan_sorts(query, scope)?;
    let limit = usize::try_from(query.limit.max(1)).unwrap_or(usize::MAX);
    let mut q = build(query, scope)?;
    let rows = q.build().fetch_all(&mut *conn).await?;

    let mut out = Vec::with_capacity(rows.len().min(limit));
    let mut last_keys = Vec::new();
    for row in rows.iter().take(limit) {
        out.push(RawRun {
            run_id: row.try_get("run_id")?,
            content_hash: row.try_get("content_hash")?,
            state: row.try_get("state")?,
            shown: row.try_get("shown")?,
            status: row.try_get("status")?,
            error_kind: row.try_get("error_kind")?,
            started_at: row.try_get("started_at")?,
            ended_at: row.try_get("ended_at")?,
        });
        last_keys = sorts
            .iter()
            .enumerate()
            .map(|(i, plan)| read_key(row, i, plan.ty))
            .collect::<Result<_, _>>()?;
    }
    let next = if rows.len() > limit {
        out.last().map(|r| ir::RunCursor {
            keys: last_keys,
            run_id: r.run_id.clone(),
        })
    } else {
        None
    };
    Ok((out, next))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn query(sort: Vec<ir::Sort>, filter: Option<ir::Filter>) -> ir::RunQuery {
        ir::RunQuery {
            cards: Vec::new(),
            filter,
            sort,
            limit: 10,
            cursor: None,
        }
    }

    fn scope(member: bool) -> RunScope<'static> {
        RunScope {
            record_id: Uuid::nil(),
            member,
            include: RunInclude {
                archived: true,
                deleted: true,
            },
            cards: &[],
        }
    }

    #[test]
    fn a_non_member_never_lists_archived_or_deleted_runs_it_did_not_judge() {
        let sql = render(&query(Vec::new(), None), &scope(false)).unwrap();
        assert!(!sql.contains("OR (u.archived_at IS NOT NULL"), "{sql}");
        assert!(!sql.contains("OR u.tombstoned_at IS NOT NULL"), "{sql}");
        assert!(
            sql.contains("(u.tombstoned_at IS NULL AND u.archived_at IS NULL) AS shown"),
            "{sql}"
        );
        let sql = render(&query(Vec::new(), None), &scope(true)).unwrap();
        assert!(sql.contains("OR (u.archived_at IS NOT NULL"), "{sql}");
        assert!(sql.ends_with("ORDER BY p.run_id ASC LIMIT $2"), "{sql}");
    }

    #[test]
    fn facet_keys_are_typed_by_the_run_table() {
        assert_eq!(
            facet_type(&["model".into(), "id".into()]).unwrap(),
            ir::ValueType::String
        );
        assert_eq!(
            facet_type(&["generation".into(), "temperature".into()]).unwrap(),
            ir::ValueType::Number
        );
        assert!(facet_type(&["model".into(), "nope".into()]).is_err());
    }

    #[test]
    fn a_record_column_is_refused() {
        let filter = ir::Filter::Cmp(ir::Cmp {
            column: ir::Column::Generated("title".into()),
            op: ir::Op::Eq,
            value: Value::String("x".into()),
        });
        assert!(matches!(
            render(&query(Vec::new(), Some(filter)), &scope(true)),
            Err(StoreError::QueryUnsupported(_))
        ));
        assert!(fingerprint_expr("grading").is_err());
    }

    #[test]
    fn a_timestamp_literal_must_be_rfc_3339() {
        for literal in ["yesterday", "2026-09-20"] {
            let filter = ir::Filter::Cmp(ir::Cmp {
                column: ir::Column::Run(ir::RunColumn::StartedAt),
                op: ir::Op::Gte,
                value: Value::String(literal.into()),
            });
            match render(&query(Vec::new(), Some(filter)), &scope(true)) {
                Err(StoreError::QueryUnsupported(message)) => {
                    assert!(message.starts_with("filter timestamp"), "{message}");
                    assert!(message.contains(literal), "{message}");
                }
                other => panic!("{literal}: {other:?}"),
            }
        }
    }

    #[test]
    fn the_keyset_is_null_aware() {
        let mut q = query(
            vec![ir::Sort {
                key: ir::SortKey::Run(ir::RunColumn::Metric("core/tokens".into())),
                dir: ir::Dir::Asc,
            }],
            None,
        );
        q.cursor = Some(ir::RunCursor {
            keys: vec![Value::Null],
            run_id: "r3".into(),
        });
        let sql = render(&q, &scope(true)).unwrap();
        assert!(
            sql.contains("AND ((TRUE AND FALSE) OR (TRUE AND p.k0 IS NULL AND p.run_id > "),
            "{sql}"
        );
    }
}
