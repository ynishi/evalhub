//! The schema-derived path table and the type check.
//!
//! [`PathTable`] is built once per process from the committed JSON Schema in
//! `evalhub_schema` plus the currently `applied` `ext_schemas` from the
//! registry. It maps each legal query path to where its value lives, what
//! type it has, and which operators the storage can actually serve:
//!
//! | Path form                          | Type                 | Served by                | Operators |
//! | ---------------------------------- | -------------------- | ------------------------ | --------- |
//! | `{facet}.{key}` with a column      | from the schema leaf | generated column         | all that suit the type |
//! | `{facet}.{key}` without one        | from the schema leaf | GIN index on `body`      | `eq`, `exists` |
//! | `fingerprint.{facet}`              | string               | `fingerprints` table     | `eq`, `ne`, `in`, `exists` |
//! | `results`, `relations`, `attachments` | array of object   | own tables               | `any` |
//! | `results[{metric}].value` (sort)   | number               | `results` table          | sort only |
//! | `ext.{ns}.{key}` registered        | from the ext_schema  | expression index         | all that suit the type |
//! | `ext.{ns}.{key}` unregistered      | unknown              | GIN index on `body`      | `eq`, `exists` |
//! | `title`, `producer.name`, …        | string               | generated column         | all that suit the type |
//!
//! The row that is easy to get wrong is the second one. Only sixteen facet
//! keys have a generated column (the migration lists them, and
//! [`GENERATED_COLUMNS`] mirrors that list with a test that fails if the two
//! disagree). Every other key of the record is readable and equality-
//! searchable through the GIN index over `body`, but ordering or ranging on
//! it would be a sequential scan, so the check refuses with `not_indexed`
//! and names the registry. That is the same answer an unregistered `ext`
//! path gets, and for the same reason.
//!
//! `runs` is deliberately absent: an Eval's runs live in the body and have
//! no side table, so there is nothing for `any` to range over. A query that
//! needs them wants `attachments` or a registered `ext` path.
//!
//! The check walks the parsed tree and, for each leaf, looks the path up,
//! confirms the operator is legal for the type, confirms the literal is of
//! that type, and confirms the operator is served by the path's index.
//! Failures are collected into `422 { errors[] }` with codes
//! `unknown_path`, `type_mismatch`, `not_indexed`, each carrying a JSON
//! pointer into the submitted request.
//!
//! The table is rebuilt when an `ext_schema` transitions to `applied`; the
//! server holds it behind an `ArcSwap`-style handle so a query in flight
//! sees a consistent table.

use std::collections::BTreeMap;

use evalhub_schema::RecordKind;
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use evalhub_schema::query::{self as envelope, QueryRequest};
use serde_json::Value;

use crate::grammar::{self, Filter, MatchTerm, ScalarOp};
use crate::ir;

/// The facets, in the order the record types declare them. `grading`
/// belongs to a Card only, which [`RecordKind`] decides.
const CARD_FACETS: [&str; 7] = [
    "model",
    "task",
    "harness",
    "generation",
    "trial",
    "grading",
    "env",
];

/// The generated columns of `versions`, as JSON path and column name.
///
/// This mirrors `crates/evalhub-store/migrations/0001_init.sql`. It is the
/// one list in this crate that is not projected from the schema, because
/// the schema does not know which keys the migration chose to materialise.
/// A test parses the migration and fails if the two ever disagree, so the
/// duplication cannot rot silently.
pub const GENERATED_COLUMNS: &[(&[&str], &str)] = &[
    (&["title"], "title"),
    (&["producer", "name"], "producer_name"),
    (&["model", "id"], "model_id"),
    (&["model", "provider"], "model_provider"),
    (&["model", "revision"], "model_revision"),
    (&["task", "id"], "task_id"),
    (&["task", "version"], "task_version"),
    (&["task", "split"], "task_split"),
    (&["harness", "name"], "harness_name"),
    (&["harness", "version"], "harness_version"),
    (&["generation", "temperature"], "gen_temperature"),
    (&["generation", "top_p"], "gen_top_p"),
    (&["generation", "max_tokens"], "gen_max_tokens"),
    (&["trial", "k"], "trial_k"),
    (&["env", "git", "commit"], "env_git_commit"),
    (&["env", "git", "dirty"], "env_git_dirty"),
];

/// How much of the operator set a path's storage can serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indexed {
    /// A dedicated index: every operator the type allows.
    Full,
    /// The GIN index over `body`, which answers containment and
    /// existence and nothing else. The path's type is known from the
    /// schema, so a literal of the wrong type is still refused.
    EqExistsOnly,
    /// The GIN index over `body`, for a path whose type nobody has
    /// declared: an `ext` key with no registered schema.
    ///
    /// Containment answers equality on any JSON scalar, and the hub has
    /// no ground to call one of them wrong here — there is no schema
    /// saying what the key holds. Registering an `ext_schema` is what
    /// turns the guess into a declaration, and with it both the type
    /// check and the ranges.
    EqExistsUntyped,
}

/// What a legal query path denotes.
///
/// The directive's `{ column, ty, indexed }` shape covers scalars; array
/// paths carry no column of their own, because an `any` condition ranges
/// over a side table whose columns are named by its terms. Keeping the two
/// apart is what lets the check reject `{"path": "results", "op": "eq"}`
/// with a type error rather than a confusing column error.
#[derive(Debug, Clone, PartialEq)]
pub enum PathInfo {
    /// A single value.
    Scalar {
        /// Where it lives.
        column: ir::Column,
        /// What it is.
        ty: ir::ValueType,
        /// What the storage can do with it.
        indexed: Indexed,
    },
    /// An array of objects, searchable with `any`.
    Array {
        /// The side table to range over.
        table: ir::ArrayTable,
    },
}

/// A registered extension schema, as the registry holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtSchema {
    /// The namespace that owns it, `{ns}/{name}`.
    pub ns: String,
    /// Typed paths below `ext.{ns}`, each as its key segments.
    pub paths: Vec<(Vec<String>, ir::ValueType)>,
    /// Whether the store has finished building the expression indexes.
    /// Until it has, the paths behave as unregistered.
    pub applied: bool,
}

/// The set of legal query paths with their types and index status,
/// projected from the record schema and the applied `ext_schemas`.
#[derive(Debug, Default, Clone)]
pub struct PathTable {
    paths: BTreeMap<String, PathInfo>,
}

impl PathTable {
    /// Build the table for one record kind from its committed schema.
    pub fn from_schema(kind: RecordKind) -> Self {
        let mut paths = BTreeMap::new();
        let schema = kind.schema().to_value();

        // Root scalars that are not facets.
        let root_props = schema
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let defs = schema.get("$defs").cloned().unwrap_or(Value::Null);

        for (name, prop) in &root_props {
            match name.as_str() {
                // Facets and other objects are walked below; the open `ext`
                // map is contributed by the registry, not by the schema.
                "ext" => {}
                "results" => {
                    paths.insert(
                        name.clone(),
                        PathInfo::Array {
                            table: ir::ArrayTable::Results,
                        },
                    );
                }
                "relations" => {
                    paths.insert(
                        name.clone(),
                        PathInfo::Array {
                            table: ir::ArrayTable::Relations,
                        },
                    );
                }
                "attachments" => {
                    paths.insert(
                        name.clone(),
                        PathInfo::Array {
                            table: ir::ArrayTable::Attachments,
                        },
                    );
                }
                // `runs` has no side table; see the module doc.
                "runs" => {}
                _ => walk(&mut paths, &defs, prop, std::slice::from_ref(name)),
            }
        }

        // Fingerprints are the hub's own facts, not a key of the record.
        for facet in facets(kind) {
            paths.insert(
                format!("fingerprint.{facet}"),
                PathInfo::Scalar {
                    column: ir::Column::Fingerprint((*facet).to_string()),
                    ty: ir::ValueType::String,
                    indexed: Indexed::Full,
                },
            );
        }

        Self { paths }
    }

    /// The same table with the registry's extension schemas folded in.
    ///
    /// An entry that is not yet `applied` is skipped, so its paths keep
    /// behaving as unregistered until the indexes exist.
    pub fn with_ext(&self, entries: &[ExtSchema]) -> Self {
        let mut paths = self.paths.clone();
        for entry in entries.iter().filter(|e| e.applied) {
            for (segments, ty) in &entry.paths {
                let path = format!("ext.{}.{}", entry.ns, segments.join("."));
                let mut json_path = vec!["ext".to_string(), entry.ns.clone()];
                json_path.extend(segments.iter().cloned());
                paths.insert(
                    path,
                    PathInfo::Scalar {
                        column: ir::Column::Ext {
                            path: json_path,
                            ty: *ty,
                        },
                        ty: *ty,
                        indexed: Indexed::Full,
                    },
                );
            }
        }
        Self { paths }
    }

    /// Look a path up.
    ///
    /// An `ext.` path the registry does not know still resolves — to the
    /// GIN index, with `eq` and `exists` only — because a producer may
    /// search its own extension before registering a schema for it.
    pub fn lookup(&self, path: &str) -> Option<PathInfo> {
        if let Some(info) = self.paths.get(path) {
            return Some(info.clone());
        }
        if let Some(rest) = path.strip_prefix("ext.") {
            // `ext.{ns}/{name}.{key…}`: the namespace carries a slash but
            // never a dot, so the first segment is the namespace.
            let (ns, key) = rest.split_once('.')?;
            if ns.is_empty() || key.is_empty() {
                return None;
            }
            let mut json_path = vec!["ext".to_string(), ns.to_string()];
            json_path.extend(key.split('.').map(str::to_string));
            return Some(PathInfo::Scalar {
                column: ir::Column::Body(json_path),
                // A placeholder: nothing has declared this key's type, so
                // `Indexed::EqExistsUntyped` tells the check to ignore it.
                ty: ir::ValueType::String,
                indexed: Indexed::EqExistsUntyped,
            });
        }
        None
    }

    /// Every path the table knows, for diagnostics and for the UI's
    /// completion list.
    pub fn paths(&self) -> impl Iterator<Item = (&str, &PathInfo)> {
        self.paths.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// The facets a record kind has. An Eval has no `grading`.
fn facets(kind: RecordKind) -> &'static [&'static str] {
    match kind {
        RecordKind::Card => &CARD_FACETS,
        RecordKind::Eval => &["model", "task", "harness", "generation", "trial", "env"],
    }
}

/// Walk one property of the schema, adding every scalar leaf below it.
fn walk(paths: &mut BTreeMap<String, PathInfo>, defs: &Value, prop: &Value, segments: &[String]) {
    let resolved = resolve(defs, prop);
    let Some(resolved) = resolved else { return };

    if let Some(properties) = resolved.get("properties").and_then(Value::as_object) {
        for (name, child) in properties {
            // Each facet's `ext` is the registry's business, and a facet
            // object itself is not a value to compare against.
            if name == "ext" {
                continue;
            }
            let mut next = segments.to_vec();
            next.push(name.clone());
            walk(paths, defs, child, &next);
        }
        return;
    }

    let Some(ty) = scalar_type(&resolved) else {
        // Arrays of objects below a facet (`grading.graders`,
        // `env.packages`) have no side table, so they are not queryable;
        // the same reasoning as `runs`.
        return;
    };
    let path = segments.join(".");
    let column = GENERATED_COLUMNS
        .iter()
        .find(|(json_path, _)| json_path == &segments)
        .map(|(_, name)| ir::Column::Generated((*name).to_string()));
    let (column, indexed) = match column {
        Some(column) => (column, Indexed::Full),
        None => (ir::Column::Body(segments.to_vec()), Indexed::EqExistsOnly),
    };
    paths.insert(
        path,
        PathInfo::Scalar {
            column,
            ty,
            indexed,
        },
    );
}

/// Follow a `$ref` (and the `anyOf [T, null]` an `Option<T>` generates)
/// to the object that carries the properties or the type.
fn resolve(defs: &Value, prop: &Value) -> Option<Value> {
    if let Some(reference) = prop.get("$ref").and_then(Value::as_str) {
        let name = reference.rsplit('/').next()?;
        return defs.get(name).cloned();
    }
    if let Some(any_of) = prop.get("anyOf").and_then(Value::as_array) {
        // `Option<T>` is `anyOf: [T, null]`; the non-null branch is the one
        // that carries the shape.
        for branch in any_of {
            if branch.get("type").and_then(Value::as_str) == Some("null") {
                continue;
            }
            if let Some(resolved) = resolve(defs, branch) {
                return Some(resolved);
            }
        }
        return None;
    }
    Some(prop.clone())
}

/// The scalar type of a leaf, or `None` when it is not a scalar.
fn scalar_type(schema: &Value) -> Option<ir::ValueType> {
    let ty = schema.get("type")?;
    let names: Vec<&str> = match ty {
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => return None,
    };
    for name in names {
        match name {
            "string" => return Some(ir::ValueType::String),
            "number" | "integer" => return Some(ir::ValueType::Number),
            "boolean" => return Some(ir::ValueType::Boolean),
            _ => continue,
        }
    }
    None
}

/// The columns an `any` condition may compare, per side table.
fn array_column(table: ir::ArrayTable, key: &str) -> Option<(ir::ArrayColumn, ir::ValueType)> {
    use ir::{ArrayColumn as C, ArrayTable as T, ValueType as V};
    match (table, key) {
        (T::Results, "metric") => Some((C::ResultMetric, V::String)),
        (T::Results, "aggregation") => Some((C::ResultAggregation, V::String)),
        (T::Results, "value") => Some((C::ResultValue, V::Number)),
        (T::Results, "n") => Some((C::ResultN, V::Number)),
        (T::Relations, "type") => Some((C::RelationType, V::String)),
        (T::Relations, "to") => Some((C::RelationTo, V::String)),
        (T::Attachments, "path") => Some((C::AttachmentPath, V::String)),
        (T::Attachments, "sha256") => Some((C::AttachmentSha256, V::String)),
        _ => None,
    }
}

/// The keys each side table accepts, for the hint on an unknown one.
fn array_keys(table: ir::ArrayTable) -> &'static [&'static str] {
    match table {
        ir::ArrayTable::Results => &["metric", "aggregation", "value", "n"],
        ir::ArrayTable::Relations => &["type", "to"],
        ir::ArrayTable::Attachments => &["path", "sha256"],
    }
}

fn entry(path: &str, code: ErrorCode, hint: impl Into<String>) -> ErrorEntry {
    ErrorEntry {
        path: path.to_string(),
        code,
        hint: Some(hint.into()),
    }
}

/// Whether an operator suits a type, per the table in the crate doc.
fn op_suits(op: ScalarOp, ty: ir::ValueType) -> bool {
    use ir::ValueType as V;
    match op {
        ScalarOp::Eq | ScalarOp::Ne | ScalarOp::In => true,
        ScalarOp::Gt | ScalarOp::Gte | ScalarOp::Lt | ScalarOp::Lte => {
            matches!(ty, V::Number | V::String)
        }
        ScalarOp::Prefix | ScalarOp::Contains => matches!(ty, V::String),
    }
}

/// Whether the storage behind a path can serve an operator.
fn index_serves(indexed: Indexed, op: ScalarOp) -> bool {
    match indexed {
        Indexed::Full => true,
        Indexed::EqExistsOnly | Indexed::EqExistsUntyped => matches!(op, ScalarOp::Eq),
    }
}

/// Whether a literal is of a type.
fn literal_matches(value: &Value, ty: ir::ValueType) -> bool {
    match ty {
        ir::ValueType::Number => value.is_number(),
        ir::ValueType::String => value.is_string(),
        ir::ValueType::Boolean => value.is_boolean(),
    }
}

fn type_name(ty: ir::ValueType) -> &'static str {
    match ty {
        ir::ValueType::Number => "a number",
        ir::ValueType::String => "a string",
        ir::ValueType::Boolean => "a boolean",
    }
}

/// Check one comparison against a path's type and index, producing the IR
/// node or the reasons it cannot be served.
fn check_cmp(
    column: ir::Column,
    ty: ir::ValueType,
    indexed: Indexed,
    op: ScalarOp,
    value: &Value,
    pointer: &str,
    errors: &mut Vec<ErrorEntry>,
) -> Option<ir::Cmp> {
    let mut ok = true;
    // A path with no declared type has nothing to check a literal
    // against; containment matches whatever scalar is given.
    let typed = indexed != Indexed::EqExistsUntyped;
    if typed && !op_suits(op, ty) {
        errors.push(entry(
            &format!("{pointer}/op"),
            ErrorCode::TypeMismatch,
            format!("`{op}` does not apply to {}", type_name(ty)),
        ));
        ok = false;
    } else if !index_serves(indexed, op) {
        errors.push(entry(
            &format!("{pointer}/op"),
            ErrorCode::NotIndexed,
            format!(
                "`{op}` needs an index this path does not have; it answers `eq` and `exists` \
                 from the index over the whole record. Register an `ext_schema` for it in the \
                 registry (`PUT /registry/ext_schemas/{{ns}}/{{id}}@{{version}}`) to make it \
                 sortable and rangeable."
            ),
        ));
        ok = false;
    }

    // `in` takes an array of the path's type; every other operator a scalar.
    if op == ScalarOp::In {
        match value.as_array() {
            None => {
                errors.push(entry(
                    &format!("{pointer}/value"),
                    ErrorCode::TypeMismatch,
                    "`in` takes an array",
                ));
                ok = false;
            }
            Some(items) => {
                if let Some(bad) = items.iter().position(|v| typed && !literal_matches(v, ty)) {
                    errors.push(entry(
                        &format!("{pointer}/value/{bad}"),
                        ErrorCode::TypeMismatch,
                        format!("this path holds {}", type_name(ty)),
                    ));
                    ok = false;
                }
            }
        }
    } else if typed && !literal_matches(value, ty) {
        errors.push(entry(
            &format!("{pointer}/value"),
            ErrorCode::TypeMismatch,
            format!("this path holds {}", type_name(ty)),
        ));
        ok = false;
    }

    ok.then(|| ir::Cmp {
        column,
        op: to_ir_op(op),
        value: value.clone(),
    })
}

fn to_ir_op(op: ScalarOp) -> ir::Op {
    match op {
        ScalarOp::Eq => ir::Op::Eq,
        ScalarOp::Ne => ir::Op::Ne,
        ScalarOp::Gt => ir::Op::Gt,
        ScalarOp::Gte => ir::Op::Gte,
        ScalarOp::Lt => ir::Op::Lt,
        ScalarOp::Lte => ir::Op::Lte,
        ScalarOp::In => ir::Op::In,
        ScalarOp::Prefix => ir::Op::Prefix,
        ScalarOp::Contains => ir::Op::Contains,
    }
}

/// Type-check a parsed filter against a path table.
///
/// Every problem is collected; the returned filter is the IR the store
/// compiles. The `pointer` is where this filter sits in the request, so
/// that an error can say `/where/and/2/value` rather than "somewhere".
pub fn check(ast: &Filter, table: &PathTable) -> Result<ir::Filter, Vec<ErrorEntry>> {
    let mut errors = Vec::new();
    let filter = check_at(ast, table, "/where", &mut errors);
    match (filter, errors.is_empty()) {
        (Some(filter), true) => Ok(filter),
        _ => Err(errors),
    }
}

fn check_at(
    ast: &Filter,
    table: &PathTable,
    pointer: &str,
    errors: &mut Vec<ErrorEntry>,
) -> Option<ir::Filter> {
    match ast {
        Filter::And { children, .. } | Filter::Or { children, .. } => {
            let key = grammar::wire_key(ast);
            let checked: Vec<ir::Filter> = children
                .iter()
                .enumerate()
                .filter_map(|(i, child)| {
                    check_at(child, table, &format!("{pointer}/{key}/{i}"), errors)
                })
                .collect();
            if checked.len() != children.len() {
                return None;
            }
            Some(match ast {
                Filter::And { .. } => ir::Filter::And(checked),
                _ => ir::Filter::Or(checked),
            })
        }
        Filter::Not { child, .. } => {
            let inner = check_at(child, table, &format!("{pointer}/not"), errors)?;
            Some(ir::Filter::Not(Box::new(inner)))
        }
        Filter::Compare {
            path, op, value, ..
        } => {
            let info = lookup_or_report(table, path, pointer, errors)?;
            match info {
                PathInfo::Scalar {
                    column,
                    ty,
                    indexed,
                } => {
                    check_cmp(column, ty, indexed, *op, value, pointer, errors).map(ir::Filter::Cmp)
                }
                PathInfo::Array { .. } => {
                    errors.push(entry(
                        &format!("{pointer}/op"),
                        ErrorCode::TypeMismatch,
                        format!("`{path}` is an array; search it with `any` and a `match`"),
                    ));
                    None
                }
            }
        }
        Filter::Exists { path, .. } => {
            let info = lookup_or_report(table, path, pointer, errors)?;
            match info {
                PathInfo::Scalar { column, .. } => Some(ir::Filter::Exists { column }),
                PathInfo::Array { .. } => {
                    errors.push(entry(
                        &format!("{pointer}/path"),
                        ErrorCode::TypeMismatch,
                        format!("`{path}` is an array; search it with `any` and a `match`"),
                    ));
                    None
                }
            }
        }
        Filter::Any { path, terms, .. } => {
            let info = lookup_or_report(table, path, pointer, errors)?;
            let PathInfo::Array { table: side } = info else {
                errors.push(entry(
                    &format!("{pointer}/op"),
                    ErrorCode::TypeMismatch,
                    format!("`{path}` is not an array; `any` ranges over one"),
                ));
                return None;
            };
            check_terms(side, terms, pointer, errors).map(|conditions| ir::Filter::Any {
                table: side,
                conditions,
            })
        }
    }
}

fn check_terms(
    side: ir::ArrayTable,
    terms: &BTreeMap<String, MatchTerm>,
    pointer: &str,
    errors: &mut Vec<ErrorEntry>,
) -> Option<Vec<ir::Cmp>> {
    if terms.is_empty() {
        errors.push(entry(
            &format!("{pointer}/match"),
            ErrorCode::Schema,
            format!(
                "`match` needs at least one of {}",
                array_keys(side)
                    .iter()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
        return None;
    }
    let mut conditions = Vec::new();
    let mut ok = true;
    for (key, term) in terms {
        let term_pointer = format!("{pointer}/match/{key}");
        let Some((column, ty)) = array_column(side, key) else {
            errors.push(entry(
                &term_pointer,
                ErrorCode::UnknownPath,
                format!(
                    "unknown key `{key}`; this array has {}",
                    array_keys(side)
                        .iter()
                        .map(|k| format!("`{k}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
            ok = false;
            continue;
        };
        match check_cmp(
            ir::Column::Array(column),
            ty,
            Indexed::Full,
            term.op,
            &term.value,
            &term_pointer,
            errors,
        ) {
            Some(cmp) => conditions.push(cmp),
            None => ok = false,
        }
    }
    ok.then_some(conditions)
}

fn lookup_or_report(
    table: &PathTable,
    path: &str,
    pointer: &str,
    errors: &mut Vec<ErrorEntry>,
) -> Option<PathInfo> {
    match table.lookup(path) {
        Some(info) => Some(info),
        None => {
            errors.push(entry(
                &format!("{pointer}/path"),
                ErrorCode::UnknownPath,
                format!("`{path}` is not a queryable path of this record"),
            ));
            None
        }
    }
}

/// Parse, check and assemble a whole request into the IR the store runs.
///
/// This is the one function the server calls. `limit` is the clamped page
/// size, and `cursor` the decoded keyset the server verified: neither is
/// this crate's business to decide.
pub fn compile(
    request: &QueryRequest,
    kind: RecordKind,
    table: &PathTable,
    limit: u32,
    cursor: Option<ir::Cursor>,
) -> Result<ir::Query, Vec<ErrorEntry>> {
    let mut errors = Vec::new();

    let filter = match &request.where_ {
        None => None,
        Some(value) => match grammar::parse(value) {
            Ok(ast) => match check(&ast, table) {
                Ok(filter) => Some(filter),
                Err(mut e) => {
                    errors.append(&mut e);
                    None
                }
            },
            Err(mut e) => {
                errors.append(&mut e);
                None
            }
        },
    };

    let mut sort = Vec::new();
    for (i, key) in request.sort.iter().enumerate() {
        let pointer = format!("/sort/{i}");
        if let Some(checked) = check_sort(&key.path, table, &pointer, &mut errors) {
            sort.push(ir::Sort {
                key: checked,
                dir: match key.dir {
                    envelope::Dir::Asc => ir::Dir::Asc,
                    envelope::Dir::Desc => ir::Dir::Desc,
                },
            });
        }
    }

    if let Some(cursor) = &cursor
        && cursor.keys.len() != sort.len()
    {
        errors.push(entry(
            "/cursor",
            ErrorCode::Schema,
            "the cursor does not match this sort; start the page sequence again",
        ));
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(ir::Query {
        record_type: match kind {
            RecordKind::Card => ir::RecordType::Card,
            RecordKind::Eval => ir::RecordType::Eval,
        },
        filter,
        sort,
        limit,
        cursor,
        expand: request
            .expand
            .iter()
            .map(|e| match e {
                envelope::Expand::Relations => ir::Expand::Relations,
                envelope::Expand::Badges => ir::Expand::Badges,
                envelope::Expand::Fingerprints => ir::Expand::Fingerprints,
                envelope::Expand::Changed => ir::Expand::Changed,
            })
            .collect(),
        version: match request.version {
            None | Some(envelope::VersionSelector::Latest) => ir::VersionSelector::Latest,
            Some(envelope::VersionSelector::All) => ir::VersionSelector::All,
        },
    })
}

/// Resolve one sort path. Sorting needs an order, which the GIN index
/// cannot give, so an `eq`-only path is `not_indexed` here too.
fn check_sort(
    path: &str,
    table: &PathTable,
    pointer: &str,
    errors: &mut Vec<ErrorEntry>,
) -> Option<ir::SortKey> {
    if path == "created_at" {
        return Some(ir::SortKey::CreatedAt);
    }
    // `results[{metric}].value` is the one indexed path with a parameter.
    if let Some(rest) = path.strip_prefix("results[")
        && let Some((metric, tail)) = rest.split_once(']')
    {
        if tail != ".value" {
            errors.push(entry(
                &format!("{pointer}/path"),
                ErrorCode::UnknownPath,
                "a metric sorts by its `value`: `results[{metric}].value`",
            ));
            return None;
        }
        if !metric.contains('/') {
            errors.push(entry(
                &format!("{pointer}/path"),
                ErrorCode::UnknownPath,
                format!("`{metric}` is not a metric id of the form `{{ns}}/{{name}}`"),
            ));
            return None;
        }
        return Some(ir::SortKey::Metric(metric.to_string()));
    }

    match table.lookup(path) {
        Some(PathInfo::Scalar {
            column,
            indexed: Indexed::Full,
            ..
        }) => Some(ir::SortKey::Column(column)),
        Some(PathInfo::Scalar { .. }) => {
            errors.push(entry(
                &format!("{pointer}/path"),
                ErrorCode::NotIndexed,
                format!(
                    "`{path}` has no ordered index, so it cannot be sorted on. Register an \
                     `ext_schema` for it in the registry to make it sortable."
                ),
            ));
            None
        }
        Some(PathInfo::Array { .. }) => {
            errors.push(entry(
                &format!("{pointer}/path"),
                ErrorCode::TypeMismatch,
                format!("`{path}` is an array; sort by `results[{{metric}}].value` instead"),
            ));
            None
        }
        None => {
            errors.push(entry(
                &format!("{pointer}/path"),
                ErrorCode::UnknownPath,
                format!("`{path}` is not a queryable path of this record"),
            ));
            None
        }
    }
}
