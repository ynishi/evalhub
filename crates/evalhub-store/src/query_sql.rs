//! Compilation of `evalhub_query::ir::Query` to SQL.
//!
//! The IR arrives with every path already resolved to a `Column` kind, so
//! this module is a renderer, not a planner:
//!
//! | `Column`              | Renders to                                                    |
//! | --------------------- | ------------------------------------------------------------- |
//! | `Generated(name)`     | `v.<name>`                                                    |
//! | `Fingerprint(facet)`  | `EXISTS (SELECT 1 FROM fingerprints f WHERE f.version_id = v.version_id AND f.facet = $ AND f.fingerprint = $)` |
//! | `Ext { path, Number }`| `evalhub_num(v.body #> '{…}')` — identical text to the index expression |
//! | `Ext { path, String }`| `evalhub_str(v.body #> '{…}')`                                |
//! | `Body(json_path)` eq  | `v.body @> $` (jsonb containment, served by the GIN index)   |
//! | `Body(json_path)` exists | `v.body @? $` (jsonpath existence)                         |
//! | `Any { results, … }`  | `EXISTS (SELECT 1 FROM results x WHERE x.version_id = v.version_id AND …)` |
//! | `Any { relations, … }`| likewise against `relations`, over the edges the caller may see |
//!
//! Queries are built with `sqlx::QueryBuilder`; every literal is a bind
//! parameter, never interpolated. Visibility is applied unconditionally:
//! `AND (r.visibility = 'public' OR r.ns = ANY($caller_namespaces))`.
//! `version = latest` adds `AND v.seq = (SELECT max(seq) FROM versions
//! WHERE record_id = v.record_id AND tombstoned_at IS NULL)`; `all` does
//! not. Tombstoned versions never match either way: they have no body to
//! match against.
//!
//! A relation whose resolved target the caller may not see is not a row
//! of `relations` as far as a query is concerned: `any` over `relations`
//! adds `AND (x.to_version_id IS NULL OR <target visible>)`, with the same
//! predicate. The read paths remove such an element from the body the
//! caller gets back (`withheld`); if a filter could still match it, asking
//! for Cards whose `relations[].to` is some private `{ns}/{name}@{seq}`
//! would say whether that version exists and who cites it. An edge stored
//! unresolved carries only the text its writer typed and stays matchable.
//!
//! # What may reach the SQL text
//!
//! Only names this module chooses. Operators come from closed enums;
//! generated column names are looked up in [`GENERATED_COLUMNS`] and a name
//! that is not there is [`StoreError::QueryUnsupported`] rather than text
//! spliced into a statement. Every value binds.
//!
//! The one deliberate exception is an `ext` path, which is rendered as a
//! literal array by [`ext_expression`]. It has to be: Postgres matches an
//! expression index by comparing expression trees, and a bind parameter is
//! not the same tree as the constant the index was built with, so a bound
//! path would mean an index nobody uses. [`ext_expression`] is the single
//! renderer — [`crate::index`] creates indexes with it — and it rejects
//! path segments it cannot escape safely.
//!
//! # Sorting and keyset pagination
//!
//! Sort keys are rendered from the same `Column` table, with
//! `results[{metric}].value` becoming a lateral join on `results`. The
//! cursor is a tuple `(sort keys…, version_id)`. When every key sorts the
//! same way the page predicate is the row-value comparison
//! `(k1, k2, version_id) > ($, $, $)` (or `<` for descending), which
//! Postgres serves from the sort index; when directions are mixed a
//! row-value comparison would be wrong, so the equivalent lexicographic
//! disjunction is emitted instead. `ORDER BY` always ends with
//! `v.version_id` so the tuple is unique. `LIMIT` is `limit + 1` so the
//! presence of a next page is known without a count.
//!
//! KNOWN LIMITATION: a sort key can be NULL (a metric the version does not
//! report, an `ext` key it does not carry). SQL comparisons against NULL
//! are unknown, so once a cursor is in play those rows fall out of the
//! page sequence — they appear on the first page, in the position
//! `NULLS FIRST`/`NULLS LAST` gives them, and not after. Sorting on a key
//! every record carries avoids it. The fix is NULL-aware branches in the
//! keyset predicate, which is a change to this module alone.
//!
//! # Expansion
//!
//! [`ir::Query::expand`] does not change the SQL. A hit carries the body,
//! the badges and `changed[]` because they are columns of the row already;
//! relations and fingerprints are separate reads the server makes through
//! [`crate::relations`] and [`crate::records::load_fingerprints`], which is
//! where their visibility rules live.
//!
//! # Tests
//!
//! Snapshot tests render each IR fixture to SQL and assert the text;
//! container tests run the same SQL against a seeded database and assert
//! the rows, including that `EXPLAIN` shows the expression index in use.

use chrono::{DateTime, Utc};
use evalhub_query::ir;
use serde_json::Value;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::error::StoreError;
use crate::records::Visibility;

/// One row a query matched, with everything the row itself holds.
///
/// The server decides what to show; this is what the store has without a
/// second read.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryHit {
    /// The named record's id.
    pub record_id: Uuid,
    /// This version's id.
    pub version_id: Uuid,
    /// Namespace of the record.
    pub ns: String,
    /// Name of the record within the namespace.
    pub name: String,
    /// Sequence number of the version.
    pub seq: i32,
    /// The version's label, if it has one.
    pub label: Option<String>,
    /// sha256 of the canonical body.
    pub content_hash: Vec<u8>,
    /// When the version was stored.
    pub created_at: DateTime<Utc>,
    /// Top-level keys that differ from the previous version.
    pub changed: Vec<String>,
    /// Badges awarded at ingest.
    pub badges: Vec<String>,
    /// Whether the record is public.
    pub visibility: Visibility,
    /// The canonical record. Present for every live version.
    pub body: Option<Value>,
    /// The record's title, from the generated column.
    pub title: Option<String>,
}

/// The generated columns of `versions` a query may name, with the type
/// each one binds as. The list mirrors the migration; a `Column::Generated`
/// naming anything else is refused rather than rendered.
pub const GENERATED_COLUMNS: &[(&str, ir::ValueType)] = &[
    ("title", ir::ValueType::String),
    ("producer_name", ir::ValueType::String),
    ("model_id", ir::ValueType::String),
    ("model_provider", ir::ValueType::String),
    ("model_revision", ir::ValueType::String),
    ("task_id", ir::ValueType::String),
    ("task_version", ir::ValueType::String),
    ("task_split", ir::ValueType::String),
    ("harness_name", ir::ValueType::String),
    ("harness_version", ir::ValueType::String),
    ("gen_temperature", ir::ValueType::Number),
    ("gen_top_p", ir::ValueType::Number),
    ("gen_max_tokens", ir::ValueType::Number),
    ("trial_k", ir::ValueType::Number),
    ("env_git_commit", ir::ValueType::String),
    ("env_git_dirty", ir::ValueType::Boolean),
];

/// The facets a fingerprint condition may name, as the migration's check
/// constraint lists them.
pub const FACETS: &[&str] = &[
    "model",
    "task",
    "harness",
    "generation",
    "trial",
    "grading",
    "env",
];

/// The `versions` column and bind type behind a generated-column name.
fn generated(name: &str) -> Result<(&'static str, ir::ValueType), StoreError> {
    GENERATED_COLUMNS
        .iter()
        .find(|(c, _)| *c == name)
        .map(|(c, t)| (*c, *t))
        .ok_or_else(|| StoreError::QueryUnsupported(format!("unknown column {name:?}")))
}

/// The SQL expression for a registered `ext` path, and the only place it
/// is produced.
///
/// The path is rendered as a Postgres `text[]` literal rather than a bind
/// parameter so that the expression is textually the one
/// [`crate::index::create_concurrently`] built the index with; the planner
/// compares expression trees, and a parameter never equals a constant.
/// Segments are escaped for both the array literal and the enclosing
/// string literal.
///
/// Errors when a segment cannot be rendered safely: empty, or carrying a
/// control character or a newline. Registered paths come from an
/// `ext_schema`, so this refuses malformed registrations rather than
/// trusting them.
pub fn ext_expression(path: &[String], ty: ir::ValueType) -> Result<String, StoreError> {
    let cast = match ty {
        ir::ValueType::Number => "evalhub_num",
        ir::ValueType::String => "evalhub_str",
        ir::ValueType::Boolean => "evalhub_bool",
    };
    Ok(format!("{cast}(v.body #> {})", array_literal(path)?))
}

/// The same expression against an unqualified `body`, for `CREATE INDEX`,
/// where there is no alias to qualify with.
///
/// Postgres normalises the alias away when it stores an index expression,
/// so this and [`ext_expression`] match each other; they differ only in
/// what a human reads in `\d+ versions`.
pub fn ext_expression_unqualified(
    path: &[String],
    ty: ir::ValueType,
) -> Result<String, StoreError> {
    Ok(ext_expression(path, ty)?.replacen("v.body", "body", 1))
}

/// A `text[]` literal, escaped for the array syntax and then for the SQL
/// string that carries it.
pub(crate) fn array_literal(path: &[String]) -> Result<String, StoreError> {
    if path.is_empty() {
        return Err(StoreError::QueryUnsupported("empty ext path".into()));
    }
    let mut out = String::from("'{");
    for (i, segment) in path.iter().enumerate() {
        if segment.is_empty() || segment.chars().any(|c| c.is_control()) {
            return Err(StoreError::QueryUnsupported(format!(
                "ext path segment {segment:?} cannot be rendered"
            )));
        }
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        for c in segment.chars() {
            // Inside an array literal a quote and a backslash are escaped
            // with a backslash; inside the SQL string a quote is doubled.
            match c {
                '"' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '\'' => out.push_str("''"),
                _ => out.push(c),
            }
        }
        out.push('"');
    }
    out.push_str("}'");
    Ok(out)
}

/// What a sort key compares as, which decides how its cursor value is read
/// back and bound again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyType {
    Number,
    Text,
    Boolean,
    Timestamp,
}

impl From<ir::ValueType> for KeyType {
    fn from(t: ir::ValueType) -> Self {
        match t {
            ir::ValueType::Number => Self::Number,
            ir::ValueType::String => Self::Text,
            ir::ValueType::Boolean => Self::Boolean,
        }
    }
}

/// A sort key resolved to the SQL that produces it.
struct SortPlan {
    /// Expression to order by and to select as `k{i}`.
    expr: String,
    /// Direction.
    dir: ir::Dir,
    /// How its cursor value travels.
    ty: KeyType,
    /// The metric a lateral join must supply, for `SortKey::Metric`.
    metric: Option<String>,
}

/// Resolve every sort key, assigning lateral aliases to metric keys.
fn plan_sorts(sort: &[ir::Sort]) -> Result<Vec<SortPlan>, StoreError> {
    let mut plans = Vec::with_capacity(sort.len());
    for (i, s) in sort.iter().enumerate() {
        let plan = match &s.key {
            ir::SortKey::CreatedAt => SortPlan {
                expr: "v.created_at".into(),
                dir: s.dir,
                ty: KeyType::Timestamp,
                metric: None,
            },
            ir::SortKey::Metric(metric) => SortPlan {
                expr: format!("s{i}.value"),
                dir: s.dir,
                ty: KeyType::Number,
                metric: Some(metric.clone()),
            },
            ir::SortKey::Column(ir::Column::Generated(name)) => {
                let (col, ty) = generated(name)?;
                SortPlan {
                    expr: format!("v.{col}"),
                    dir: s.dir,
                    ty: ty.into(),
                    metric: None,
                }
            }
            ir::SortKey::Column(ir::Column::Ext { path, ty }) => SortPlan {
                expr: ext_expression(path, *ty)?,
                dir: s.dir,
                ty: (*ty).into(),
                metric: None,
            },
            ir::SortKey::Column(other) => {
                return Err(StoreError::QueryUnsupported(format!(
                    "{other:?} cannot be sorted on"
                )));
            }
            // Only a run projection query (`ir::RunQuery`) sorts on these.
            ir::SortKey::Run(column) => {
                return Err(StoreError::QueryUnsupported(format!(
                    "{column:?} is a run column, not a record column"
                )));
            }
        };
        plans.push(plan);
    }
    Ok(plans)
}

/// Build the statement for `query`, ready to execute or to read as text.
///
/// Separated from [`run`] so that a test can assert the SQL without a
/// database. Binds appear as `$1`, `$2`, … in the order they were pushed.
fn build(
    query: &ir::Query,
    caller_namespaces: &[String],
) -> Result<QueryBuilder<Postgres>, StoreError> {
    let sorts = plan_sorts(&query.sort)?;

    let mut q: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT r.id AS record_id, r.ns, r.name, r.visibility, \
         v.version_id, v.seq, v.label, v.content_hash, v.created_at, \
         v.changed, v.badges, v.body, v.title",
    );
    for (i, plan) in sorts.iter().enumerate() {
        q.push(format!(", {} AS k{i}", plan.expr));
    }
    q.push(" FROM records r JOIN versions v ON v.record_id = r.id");
    for (i, plan) in sorts.iter().enumerate() {
        if let Some(metric) = &plan.metric {
            // The best value in the sort's own direction, so that a
            // version reporting a metric several ways (different `by`
            // partitions) sorts by the one the caller asked to rank on.
            let order = match plan.dir {
                ir::Dir::Asc => "ASC",
                ir::Dir::Desc => "DESC",
            };
            q.push(" LEFT JOIN LATERAL (SELECT value FROM results WHERE version_id = v.version_id AND metric = ");
            q.push_bind(metric.clone());
            q.push(format!(" ORDER BY value {order} LIMIT 1) s{i} ON true"));
        }
    }

    q.push(" WHERE r.type = ");
    q.push_bind(match query.record_type {
        ir::RecordType::Card => "card",
        ir::RecordType::Eval => "eval",
    });
    q.push(" AND (r.visibility = 'public' OR r.ns = ANY(");
    q.push_bind(caller_namespaces.to_vec());
    q.push(")) AND v.tombstoned_at IS NULL");
    if query.version == ir::VersionSelector::Latest {
        q.push(
            " AND v.seq = (SELECT max(seq) FROM versions v2 \
             WHERE v2.record_id = v.record_id AND v2.tombstoned_at IS NULL)",
        );
    }

    if let Some(filter) = &query.filter {
        q.push(" AND ");
        push_filter(&mut q, filter, caller_namespaces)?;
    }

    if let Some(cursor) = &query.cursor {
        push_keyset(&mut q, &sorts, cursor)?;
    }

    q.push(" ORDER BY ");
    for plan in &sorts {
        q.push(format!(
            "{} {}, ",
            plan.expr,
            match plan.dir {
                ir::Dir::Asc => "ASC",
                ir::Dir::Desc => "DESC",
            }
        ));
    }
    // The tiebreaker follows the last key's direction so that the keyset
    // tuple and the order agree; with no keys the default is newest first.
    let tiebreak = sorts.last().map(|p| p.dir).unwrap_or(ir::Dir::Desc);
    if sorts.is_empty() {
        q.push("v.created_at DESC, ");
    }
    q.push(format!(
        "v.version_id {}",
        match tiebreak {
            ir::Dir::Asc => "ASC",
            ir::Dir::Desc => "DESC",
        }
    ));

    q.push(" LIMIT ");
    q.push_bind(i64::from(query.limit.max(1)) + 1);
    Ok(q)
}

/// The keyset predicate: where the previous page stopped.
fn push_keyset(
    q: &mut QueryBuilder<Postgres>,
    sorts: &[SortPlan],
    cursor: &ir::Cursor,
) -> Result<(), StoreError> {
    if cursor.keys.len() != sorts.len() {
        return Err(StoreError::QueryUnsupported(format!(
            "cursor carries {} key(s) for {} sort key(s)",
            cursor.keys.len(),
            sorts.len()
        )));
    }
    let tiebreak = sorts.last().map(|p| p.dir).unwrap_or(ir::Dir::Desc);
    let uniform = sorts.iter().all(|p| p.dir == tiebreak);

    q.push(" AND ");
    if uniform {
        // Row-value comparison: one index range scan.
        q.push("(");
        for plan in sorts {
            q.push(format!("{}, ", plan.expr));
        }
        q.push("v.version_id) ");
        q.push(cmp_symbol(tiebreak));
        q.push(" (");
        for (plan, value) in sorts.iter().zip(&cursor.keys) {
            push_key_value(q, plan.ty, value, "cursor")?;
            q.push(", ");
        }
        q.push_bind(cursor.version_id);
        q.push(")");
        return Ok(());
    }

    // Mixed directions: the lexicographic expansion, because a row-value
    // comparison can only carry one direction.
    q.push("(");
    for (i, plan) in sorts.iter().enumerate() {
        if i > 0 {
            q.push(" OR ");
        }
        q.push("(");
        for (j, earlier) in sorts.iter().take(i).enumerate() {
            q.push(format!("{} = ", earlier.expr));
            push_key_value(q, earlier.ty, &cursor.keys[j], "cursor")?;
            q.push(" AND ");
        }
        q.push(format!("{} {} ", plan.expr, cmp_symbol(plan.dir)));
        push_key_value(q, plan.ty, &cursor.keys[i], "cursor")?;
        q.push(")");
    }
    if !sorts.is_empty() {
        q.push(" OR ");
    }
    q.push("(");
    for (j, plan) in sorts.iter().enumerate() {
        q.push(format!("{} = ", plan.expr));
        push_key_value(q, plan.ty, &cursor.keys[j], "cursor")?;
        q.push(" AND ");
    }
    q.push(format!("v.version_id {} ", cmp_symbol(tiebreak)));
    q.push_bind(cursor.version_id);
    q.push(")");
    q.push(")");
    Ok(())
}

fn cmp_symbol(dir: ir::Dir) -> &'static str {
    match dir {
        ir::Dir::Asc => ">",
        ir::Dir::Desc => "<",
    }
}

/// Bind one value according to its key's type. `what` says where the
/// value came from (`cursor`, `filter`) for the error message.
///
/// A timestamp is parsed as RFC 3339 (`DateTime::parse_from_rfc3339`),
/// the format the DSL documents and the one a cursor this crate wrote
/// carries, rather than with chrono's general `FromStr`.
pub(crate) fn push_key_value(
    q: &mut QueryBuilder<Postgres>,
    ty: KeyType,
    value: &Value,
    what: &str,
) -> Result<(), StoreError> {
    match ty {
        KeyType::Number => q.push_bind(as_f64(value)?),
        KeyType::Text => q.push_bind(as_string(value)?),
        KeyType::Boolean => q.push_bind(as_bool(value)?),
        KeyType::Timestamp => q.push_bind(parse_timestamp(value, what)?),
    };
    Ok(())
}

/// An RFC 3339 timestamp literal, or [`StoreError::QueryUnsupported`]
/// naming `what` (`cursor`, `filter`) and the literal.
pub(crate) fn parse_timestamp(value: &Value, what: &str) -> Result<DateTime<Utc>, StoreError> {
    let text = as_string(value)?;
    DateTime::parse_from_rfc3339(&text)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| {
            StoreError::QueryUnsupported(format!("{what} timestamp {text:?} is not RFC 3339: {e}"))
        })
}

/// Render a predicate.
fn push_filter(
    q: &mut QueryBuilder<Postgres>,
    filter: &ir::Filter,
    caller_namespaces: &[String],
) -> Result<(), StoreError> {
    match filter {
        ir::Filter::And(children) => push_junction(q, children, "AND", "TRUE", caller_namespaces),
        ir::Filter::Or(children) => push_junction(q, children, "OR", "FALSE", caller_namespaces),
        ir::Filter::Not(inner) => {
            q.push("NOT (");
            push_filter(q, inner, caller_namespaces)?;
            q.push(")");
            Ok(())
        }
        ir::Filter::Cmp(cmp) => push_cmp(q, cmp, None),
        ir::Filter::Exists { column } => push_exists(q, column),
        ir::Filter::Any { table, conditions } => push_any(q, *table, conditions, caller_namespaces),
    }
}

fn push_junction(
    q: &mut QueryBuilder<Postgres>,
    children: &[ir::Filter],
    joiner: &str,
    empty: &str,
    caller_namespaces: &[String],
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
        push_filter(q, child, caller_namespaces)?;
    }
    q.push(")");
    Ok(())
}

/// `alias` is `Some("x")` inside an `any` subquery, where the side table's
/// columns live.
fn push_cmp(
    q: &mut QueryBuilder<Postgres>,
    cmp: &ir::Cmp,
    alias: Option<ir::ArrayTable>,
) -> Result<(), StoreError> {
    match &cmp.column {
        ir::Column::Fingerprint(facet) => push_fingerprint(q, facet, cmp),
        ir::Column::Body(path) => push_body(q, path, cmp),
        ir::Column::Array(col) => {
            let table = alias.ok_or_else(|| {
                StoreError::QueryUnsupported("array column outside an `any` condition".into())
            })?;
            if col.table() != table {
                return Err(StoreError::QueryUnsupported(format!(
                    "{col:?} does not belong to {table:?}"
                )));
            }
            let (expr, ty) = array_column(*col);
            push_operator(q, &expr, cmp.op, ty, &cmp.value)
        }
        ir::Column::Generated(name) => {
            let (col, ty) = generated(name)?;
            push_operator(q, &format!("v.{col}"), cmp.op, ty, &cmp.value)
        }
        ir::Column::Ext { path, ty } => {
            let expr = ext_expression(path, *ty)?;
            push_operator(q, &expr, cmp.op, *ty, &cmp.value)
        }
        // Only a run projection query (`ir::RunQuery`) carries these.
        ir::Column::Run(column) => Err(StoreError::QueryUnsupported(format!(
            "{column:?} is a run column, not a record column"
        ))),
    }
}

/// The SQL for a side-table column and the type its values bind as.
fn array_column(col: ir::ArrayColumn) -> (String, ir::ValueType) {
    match col {
        ir::ArrayColumn::ResultMetric => ("x.metric".into(), ir::ValueType::String),
        ir::ArrayColumn::ResultAggregation => ("x.aggregation".into(), ir::ValueType::String),
        ir::ArrayColumn::ResultValue => ("x.value".into(), ir::ValueType::Number),
        ir::ArrayColumn::ResultN => ("x.n::double precision".into(), ir::ValueType::Number),
        ir::ArrayColumn::RelationType => ("x.type".into(), ir::ValueType::String),
        // A relation's target reads the same whether it resolved or not:
        // the textual reference it was written with, or the address of the
        // version it points at.
        ir::ArrayColumn::RelationTo => (
            "COALESCE(x.to_external, (SELECT r2.ns || '/' || r2.name || '@' || v2.seq \
             FROM versions v2 JOIN records r2 ON r2.id = v2.record_id \
             WHERE v2.version_id = x.to_version_id))"
                .into(),
            ir::ValueType::String,
        ),
        ir::ArrayColumn::AttachmentPath => ("x.path".into(), ir::ValueType::String),
        ir::ArrayColumn::AttachmentSha256 => {
            ("encode(x.sha256, 'hex')".into(), ir::ValueType::String)
        }
    }
}

/// `EXISTS` over a side table, with every condition on the same row. Over
/// `relations`, only the edges whose target the caller may see (see the
/// module doc).
fn push_any(
    q: &mut QueryBuilder<Postgres>,
    table: ir::ArrayTable,
    conditions: &[ir::Cmp],
    caller_namespaces: &[String],
) -> Result<(), StoreError> {
    let (name, key) = match table {
        ir::ArrayTable::Results => ("results", "version_id"),
        ir::ArrayTable::Relations => ("relations", "from_version_id"),
        ir::ArrayTable::Attachments => ("attachment_refs", "version_id"),
    };
    q.push(format!(
        "EXISTS (SELECT 1 FROM {name} x WHERE x.{key} = v.version_id"
    ));
    if table == ir::ArrayTable::Relations {
        q.push(
            " AND (x.to_version_id IS NULL OR EXISTS (SELECT 1 FROM versions tv \
             JOIN records tr ON tr.id = tv.record_id WHERE tv.version_id = x.to_version_id \
             AND (tr.visibility = 'public' OR tr.ns = ANY(",
        );
        q.push_bind(caller_namespaces.to_vec());
        q.push("))))");
    }
    for cmp in conditions {
        q.push(" AND ");
        push_cmp(q, cmp, Some(table))?;
    }
    q.push(")");
    Ok(())
}

/// A fingerprint match, which is a row in `fingerprints` rather than a
/// column of the version.
fn push_fingerprint(
    q: &mut QueryBuilder<Postgres>,
    facet: &str,
    cmp: &ir::Cmp,
) -> Result<(), StoreError> {
    if !FACETS.contains(&facet) {
        return Err(StoreError::QueryUnsupported(format!(
            "unknown facet {facet:?}"
        )));
    }
    let negated = match cmp.op {
        ir::Op::Eq => false,
        ir::Op::Ne => true,
        other => {
            return Err(StoreError::QueryUnsupported(format!(
                "{other:?} on a fingerprint"
            )));
        }
    };
    let hex = as_string(&cmp.value)?;
    let bytes = hex::decode(&hex)
        .map_err(|_| StoreError::QueryUnsupported("fingerprint is not hexadecimal".into()))?;
    if negated {
        q.push("NOT ");
    }
    q.push("EXISTS (SELECT 1 FROM fingerprints f WHERE f.version_id = v.version_id AND f.facet = ");
    q.push_bind(facet.to_owned());
    q.push(" AND f.fingerprint = ");
    q.push_bind(bytes);
    q.push(")");
    Ok(())
}

/// An unregistered path, answered by the GIN index: containment for
/// equality, jsonpath existence for presence.
fn push_body(
    q: &mut QueryBuilder<Postgres>,
    path: &[String],
    cmp: &ir::Cmp,
) -> Result<(), StoreError> {
    match cmp.op {
        ir::Op::Eq => {
            q.push("v.body @> ");
            q.push_bind(nest(path, cmp.value.clone()));
        }
        other => {
            return Err(StoreError::QueryUnsupported(format!(
                "{other:?} on an unregistered path; register an ext_schema to index it"
            )));
        }
    }
    Ok(())
}

/// Existence, by column kind.
fn push_exists(q: &mut QueryBuilder<Postgres>, column: &ir::Column) -> Result<(), StoreError> {
    match column {
        ir::Column::Generated(name) => {
            let (col, _) = generated(name)?;
            q.push(format!("v.{col} IS NOT NULL"));
        }
        ir::Column::Ext { path, ty } => {
            q.push(format!("{} IS NOT NULL", ext_expression(path, *ty)?));
        }
        ir::Column::Body(path) => {
            q.push("v.body @? ");
            q.push_bind(jsonpath(path)?);
            q.push("::jsonpath");
        }
        ir::Column::Fingerprint(facet) => {
            if !FACETS.contains(&facet.as_str()) {
                return Err(StoreError::QueryUnsupported(format!(
                    "unknown facet {facet:?}"
                )));
            }
            q.push(
                "EXISTS (SELECT 1 FROM fingerprints f WHERE f.version_id = v.version_id AND f.facet = ",
            );
            q.push_bind(facet.clone());
            q.push(")");
        }
        ir::Column::Array(col) => {
            return Err(StoreError::QueryUnsupported(format!(
                "{col:?} exists outside an `any` condition"
            )));
        }
        // Only a run projection query (`ir::RunQuery`) carries these.
        ir::Column::Run(column) => {
            return Err(StoreError::QueryUnsupported(format!(
                "{column:?} is a run column, not a record column"
            )));
        }
    }
    Ok(())
}

/// Apply one operator to one expression with one bound value.
fn push_operator(
    q: &mut QueryBuilder<Postgres>,
    expr: &str,
    op: ir::Op,
    ty: ir::ValueType,
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
            q.push(format!("{expr} = ANY("));
            push_array(q, ty, value)?;
            q.push(")");
            return Ok(());
        }
        ir::Op::Prefix | ir::Op::Contains => {
            if ty != ir::ValueType::String {
                return Err(StoreError::QueryUnsupported(format!(
                    "{op:?} on a non-string column"
                )));
            }
            let text = as_string(value)?;
            let pattern = match op {
                ir::Op::Prefix => format!("{}%", escape_like(&text)),
                _ => format!("%{}%", escape_like(&text)),
            };
            q.push(format!("{expr} LIKE "));
            q.push_bind(pattern);
            q.push(" ESCAPE '\\'");
            return Ok(());
        }
    };
    q.push(format!("{expr} {symbol} "));
    push_scalar(q, ty, value)?;
    Ok(())
}

fn push_scalar(
    q: &mut QueryBuilder<Postgres>,
    ty: ir::ValueType,
    value: &Value,
) -> Result<(), StoreError> {
    match ty {
        ir::ValueType::Number => q.push_bind(as_f64(value)?),
        ir::ValueType::String => q.push_bind(as_string(value)?),
        ir::ValueType::Boolean => q.push_bind(as_bool(value)?),
    };
    Ok(())
}

fn push_array(
    q: &mut QueryBuilder<Postgres>,
    ty: ir::ValueType,
    value: &Value,
) -> Result<(), StoreError> {
    let items = value
        .as_array()
        .ok_or_else(|| StoreError::QueryUnsupported("`in` needs an array".into()))?;
    match ty {
        ir::ValueType::Number => {
            let v: Result<Vec<f64>, _> = items.iter().map(as_f64).collect();
            q.push_bind(v?);
        }
        ir::ValueType::String => {
            let v: Result<Vec<String>, _> = items.iter().map(as_string).collect();
            q.push_bind(v?);
        }
        ir::ValueType::Boolean => {
            let v: Result<Vec<bool>, _> = items.iter().map(as_bool).collect();
            q.push_bind(v?);
        }
    }
    Ok(())
}

pub(crate) fn as_f64(value: &Value) -> Result<f64, StoreError> {
    value
        .as_f64()
        .ok_or_else(|| StoreError::QueryUnsupported(format!("{value} is not a number")))
}

pub(crate) fn as_string(value: &Value) -> Result<String, StoreError> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| StoreError::QueryUnsupported(format!("{value} is not a string")))
}

pub(crate) fn as_bool(value: &Value) -> Result<bool, StoreError> {
    value
        .as_bool()
        .ok_or_else(|| StoreError::QueryUnsupported(format!("{value} is not a boolean")))
}

/// `{"a": {"b": v}}` for the path `["a", "b"]`, which is what `@>` wants.
pub(crate) fn nest(path: &[String], value: Value) -> Value {
    let mut out = value;
    for key in path.iter().rev() {
        out = serde_json::json!({ key.clone(): out });
    }
    out
}

/// `$."a"."b"` for the path `["a", "b"]`.
pub(crate) fn jsonpath(path: &[String]) -> Result<String, StoreError> {
    let mut out = String::from("$");
    for segment in path {
        if segment.chars().any(|c| c.is_control()) {
            return Err(StoreError::QueryUnsupported(format!(
                "path segment {segment:?} cannot be rendered"
            )));
        }
        out.push_str(".\"");
        out.push_str(&segment.replace('"', "\\\""));
        out.push('"');
    }
    Ok(out)
}

/// Escape `%`, `_` and `\` so a search string matches literally under
/// `LIKE … ESCAPE '\'`.
pub(crate) fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The SQL this query compiles to, with binds as `$1`, `$2`, … in the
/// order they are pushed. For tests and for `EXPLAIN` by hand; [`run`]
/// executes the same statement.
pub fn render(query: &ir::Query, caller_namespaces: &[String]) -> Result<String, StoreError> {
    Ok(build(query, caller_namespaces)?.sql().as_str().to_owned())
}

/// Execute `query` and return one page of hits with the cursor that
/// continues it, or `None` when the page is the last.
///
/// The caller's namespaces are the ones whose private records they may
/// see; the predicate is applied whether or not the query mentions
/// visibility, so there is no way to ask for someone else's records.
pub async fn run(
    pool: &PgPool,
    query: &ir::Query,
    caller_namespaces: &[String],
) -> Result<(Vec<QueryHit>, Option<ir::Cursor>), StoreError> {
    let sorts = plan_sorts(&query.sort)?;
    let limit = usize::try_from(query.limit.max(1)).unwrap_or(usize::MAX);
    let mut q = build(query, caller_namespaces)?;
    let rows = q.build().fetch_all(pool).await?;

    let mut hits = Vec::with_capacity(rows.len().min(limit));
    let mut keys: Vec<Vec<Value>> = Vec::with_capacity(rows.len().min(limit));
    for row in rows.iter().take(limit) {
        let visibility: String = row.try_get("visibility")?;
        hits.push(QueryHit {
            record_id: row.try_get("record_id")?,
            version_id: row.try_get("version_id")?,
            ns: row.try_get("ns")?,
            name: row.try_get("name")?,
            seq: row.try_get("seq")?,
            label: row.try_get("label")?,
            content_hash: row.try_get("content_hash")?,
            created_at: row.try_get("created_at")?,
            changed: row.try_get("changed")?,
            badges: row.try_get("badges")?,
            visibility: Visibility::parse(&visibility),
            body: row.try_get("body")?,
            title: row.try_get("title")?,
        });
        let mut row_keys = Vec::with_capacity(sorts.len());
        for (i, plan) in sorts.iter().enumerate() {
            row_keys.push(read_key(row, i, plan.ty)?);
        }
        keys.push(row_keys);
    }

    let next = if rows.len() > limit {
        hits.last().map(|h| ir::Cursor {
            keys: keys.pop().unwrap_or_default(),
            version_id: h.version_id,
        })
    } else {
        None
    };
    Ok((hits, next))
}

/// Read the `k{i}` column back as the JSON a cursor carries.
pub(crate) fn read_key(
    row: &sqlx::postgres::PgRow,
    i: usize,
    ty: KeyType,
) -> Result<Value, StoreError> {
    let name = format!("k{i}");
    let value = match ty {
        KeyType::Number => row
            .try_get::<Option<f64>, _>(name.as_str())?
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        KeyType::Text => row
            .try_get::<Option<String>, _>(name.as_str())?
            .map(Value::String)
            .unwrap_or(Value::Null),
        KeyType::Boolean => row
            .try_get::<Option<bool>, _>(name.as_str())?
            .map(Value::Bool)
            .unwrap_or(Value::Null),
        KeyType::Timestamp => row
            .try_get::<Option<DateTime<Utc>>, _>(name.as_str())?
            .map(|t| Value::String(t.to_rfc3339()))
            .unwrap_or(Value::Null),
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn ext_expression_is_a_literal_array_with_the_right_cast() {
        let path = vec!["ext".into(), "alice/qwen-loop".into(), "rung".into()];
        assert_eq!(
            ext_expression(&path, ir::ValueType::Number).unwrap(),
            "evalhub_num(v.body #> '{\"ext\",\"alice/qwen-loop\",\"rung\"}')"
        );
        assert_eq!(
            ext_expression(&path, ir::ValueType::String).unwrap(),
            "evalhub_str(v.body #> '{\"ext\",\"alice/qwen-loop\",\"rung\"}')"
        );
        assert_eq!(
            ext_expression_unqualified(&path, ir::ValueType::Boolean).unwrap(),
            "evalhub_bool(body #> '{\"ext\",\"alice/qwen-loop\",\"rung\"}')"
        );
    }

    #[test]
    fn ext_path_segments_are_escaped_or_refused() {
        let quoted = vec!["ext".into(), "a\"b".into()];
        assert_eq!(
            array_literal(&quoted).unwrap(),
            "'{\"ext\",\"a\\\"b\"}'",
            "a quote is backslash-escaped inside the array literal"
        );
        let apostrophe = vec!["ext".into(), "it's".into()];
        assert_eq!(
            array_literal(&apostrophe).unwrap(),
            "'{\"ext\",\"it''s\"}'",
            "a single quote is doubled for the enclosing SQL string"
        );
        assert!(array_literal(&["ext".into(), "a\nb".into()]).is_err());
        assert!(array_literal(&["ext".into(), String::new()]).is_err());
        assert!(array_literal(&[]).is_err());
    }

    #[test]
    fn unknown_generated_column_is_refused() {
        assert!(generated("model_id").is_ok());
        assert!(matches!(
            generated("body; DROP TABLE versions"),
            Err(StoreError::QueryUnsupported(_))
        ));
    }

    #[test]
    fn nest_builds_a_containment_document() {
        let path = vec!["model".into(), "id".into()];
        assert_eq!(
            nest(&path, Value::String("qwen".into())),
            serde_json::json!({"model": {"id": "qwen"}})
        );
    }

    #[test]
    fn jsonpath_quotes_each_segment() {
        assert_eq!(
            jsonpath(&["ext".into(), "alice/x".into()]).unwrap(),
            "$.\"ext\".\"alice/x\""
        );
    }

    #[test]
    fn array_column_belongs_to_one_table() {
        assert_eq!(
            ir::ArrayColumn::ResultMetric.table(),
            ir::ArrayTable::Results
        );
        let cmp = ir::Cmp {
            column: ir::Column::Array(ir::ArrayColumn::ResultMetric),
            op: ir::Op::Eq,
            value: Value::String("core/pass_rate".into()),
        };
        let mut q: QueryBuilder<Postgres> = QueryBuilder::new("");
        assert!(matches!(
            push_cmp(&mut q, &cmp, Some(ir::ArrayTable::Relations)),
            Err(StoreError::QueryUnsupported(_))
        ));
    }
}
