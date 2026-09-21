//! The `dsl-kit` grammar of a query and the parser derived from it.
//!
//! ```text
//! query   := { where?: filter, sort?: [sort], limit?: int, cursor?: string,
//!              expand?: [expand], version?: "latest" | "all" }
//! filter  := { and: [filter+] } | { or: [filter+] } | { not: filter } | leaf
//! leaf    := { path, op: scalar_op, value }
//!          | { path, op: "exists" }
//!          | { path, op: "any", match: { key: (value | { op, value }) ... } }
//! scalar_op := eq | ne | gt | gte | lt | lte | in | prefix | contains
//! sort    := { path, dir: "asc" | "desc" }
//! expand  := relations | badges | fingerprints | changed
//! ```
//!
//! The filter AST is declared once, as the `dsl-kit` enum [`Filter`], and
//! from it come the conformance check, the typed builder, and the JSON
//! Schema of the `where` value that `evalhub_schema::query` leaves open.
//! `path` is a string at this stage; its meaning is assigned in
//! [`crate::typecheck`].
//!
//! `any` with `match` is the one non-trivial construct. It applies to array
//! paths (`results`, `relations`, `attachments`, `runs`) and matches if any
//! element satisfies every key in `match`; a key's value is either a literal
//! (implicit `eq`) or `{ op, value }`. This is how `results` is filtered by
//! metric and value at once without exposing array indices.
//!
//! # Two shapes of vocabulary, and which one is hand-written
//!
//! `dsl-kit`'s JSON front end reads the internally-tagged convention
//! (`{"type": "And", "children": […]}`), while the wire format above is
//! key-tagged (`{"and": […]}`). [`parse`] therefore lowers the wire JSON
//! into the tagged shape before handing it to
//! `dsl_kit_parse::serde_bridge`, and the lowering is the only place in the
//! workspace that names a construct by hand. It distinguishes exactly five
//! shapes — `and`, `or`, `not`, and the two leaf forms that carry no scalar
//! value (`exists`, `any`) — because each is a different *node shape*, and
//! a node shape is what a `dsl-kit` variant is.
//!
//! The **operator vocabulary** is not hand-written anywhere: `op` is a
//! payload field of type [`ScalarOp`], which the derived builder
//! deserialises through `serde`. Adding `matches` or `between` to the
//! language is one variant on that enum plus its row in the type table;
//! no parser arm changes. That is the property the crate doc claims, and
//! it is worth stating precisely rather than loosely, because the looser
//! claim ("no hand-written match anywhere") is not true of the lowering.
//!
//! # Why the payloads are `serde_json::Value`
//!
//! A leaf's `value` is any JSON scalar (or an array, for `in`), and a
//! `match` term's value likewise. `dsl-kit` payload fields convert through
//! `serde`, so [`serde_json::Value`] is a legal field type and keeps the
//! grammar honest about what it does *not* decide: whether `0.2` suits
//! `generation.temperature` is a question for the type check, which has the
//! schema. The grammar's job is shape.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use dsl_kit_core::{IdGen, NodeId};
use dsl_kit_macros::{DslBuild, DslNode, DslSchema};
use dsl_kit_parse::DslBuild as _;
use dsl_kit_parse::serde_bridge;
use dsl_kit_schema::DslSchema as _;
use evalhub_schema::error::{ErrorCode, ErrorEntry};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The filter AST: what a `where` clause means, before any path has been
/// given a meaning.
///
/// Every variant carries the `id: NodeId` that `dsl-kit` mints, which is
/// what lets a diagnostic point at a node rather than at a byte range.
#[derive(Debug, Clone, PartialEq, DslNode, DslSchema, DslBuild)]
pub enum Filter {
    /// Every child must match. `{"and": [...]}`.
    And {
        /// Node identity, minted by the builder.
        id: NodeId,
        /// The conjuncts.
        children: Vec<Filter>,
    },
    /// At least one child must match. `{"or": [...]}`.
    Or {
        /// Node identity, minted by the builder.
        id: NodeId,
        /// The disjuncts.
        children: Vec<Filter>,
    },
    /// The child must not match. `{"not": {...}}`.
    Not {
        /// Node identity, minted by the builder.
        id: NodeId,
        /// The negated filter.
        child: Box<Filter>,
    },
    /// A scalar comparison. `{"path": …, "op": …, "value": …}`.
    Compare {
        /// Node identity, minted by the builder.
        id: NodeId,
        /// The query path, uninterpreted here.
        path: String,
        /// The operator.
        op: ScalarOp,
        /// The literal to compare against; an array for `in`.
        value: Value,
    },
    /// The path has a value, `null` included. `{"path": …, "op": "exists"}`.
    Exists {
        /// Node identity, minted by the builder.
        id: NodeId,
        /// The query path, uninterpreted here.
        path: String,
    },
    /// At least one element of an array path satisfies every term.
    /// `{"path": …, "op": "any", "match": {…}}`.
    Any {
        /// Node identity, minted by the builder.
        id: NodeId,
        /// The array path, uninterpreted here.
        path: String,
        /// The terms, keyed by the element field they constrain.
        terms: BTreeMap<String, MatchTerm>,
    },
}

/// The scalar operators, declared once.
///
/// This enum *is* the operator vocabulary: the builder deserialises `op`
/// into it, [`request_schema`] lists its spellings in the published JSON
/// Schema, and [`crate::typecheck`] decides which types each one suits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScalarOp {
    /// Equal.
    Eq,
    /// Not equal.
    Ne,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// One of a list.
    In,
    /// Starts with.
    Prefix,
    /// Contains.
    Contains,
}

impl ScalarOp {
    /// Every operator, in declaration order. Used to publish the
    /// vocabulary without a second list.
    pub const ALL: [ScalarOp; 9] = [
        ScalarOp::Eq,
        ScalarOp::Ne,
        ScalarOp::Gt,
        ScalarOp::Gte,
        ScalarOp::Lt,
        ScalarOp::Lte,
        ScalarOp::In,
        ScalarOp::Prefix,
        ScalarOp::Contains,
    ];

    /// The wire spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            ScalarOp::Eq => "eq",
            ScalarOp::Ne => "ne",
            ScalarOp::Gt => "gt",
            ScalarOp::Gte => "gte",
            ScalarOp::Lt => "lt",
            ScalarOp::Lte => "lte",
            ScalarOp::In => "in",
            ScalarOp::Prefix => "prefix",
            ScalarOp::Contains => "contains",
        }
    }
}

impl fmt::Display for ScalarOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ScalarOp {
    type Err = UnknownOperator;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ScalarOp::ALL
            .into_iter()
            .find(|op| op.as_str() == s)
            .ok_or_else(|| UnknownOperator(s.to_string()))
    }
}

/// A spelling that names no operator.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown operator `{0}`")]
pub struct UnknownOperator(pub String);

/// One term of an `any` match: an operator (`eq` when the wire form was a
/// bare literal) and the value to compare against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchTerm {
    /// How to compare. Absent on the wire means `eq`.
    #[serde(default = "MatchTerm::default_op")]
    pub op: ScalarOp,
    /// The literal to compare against.
    pub value: Value,
}

impl MatchTerm {
    const fn default_op() -> ScalarOp {
        ScalarOp::Eq
    }
}

impl FromStr for MatchTerm {
    type Err = serde_json::Error;

    /// Present so the term satisfies `dsl-kit`'s payload bound, which asks
    /// for both `serde` and `FromStr` so the JSON and canonical-text front
    /// ends cannot diverge. evalhub only uses the JSON one.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

/// The operators that change a leaf's *shape* rather than its comparison,
/// and therefore name a `dsl-kit` variant instead of a [`ScalarOp`].
const EXISTS_OP: &str = "exists";
/// See [`EXISTS_OP`].
const ANY_OP: &str = "any";

/// Parse a `where` value into the filter AST.
///
/// Returns every structural problem it found, not just the first: a
/// caller fixing a query should see the whole bag. Paths in the returned
/// entries are JSON pointers into the *request* (`/where/and/1/op`), which
/// this function tracks itself — `dsl-kit`'s JSON front end reports no
/// position, because it reads a [`serde_json::Value`] and there is no byte
/// offset left to report by then.
pub fn parse(value: &Value) -> Result<Filter, Vec<ErrorEntry>> {
    let mut errors = Vec::new();
    let tagged = lower(value, "/where", &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }

    let schema = Filter::schema();
    let tree = serde_bridge::from_json_value(&tagged, &schema)
        .map_err(|e| diagnostics_to_entries(&e, "/where"))?;
    Filter::from_parse_tree(&tree, &IdGen::new()).map_err(|e| diagnostics_to_entries(&e, "/where"))
}

/// Turn `dsl-kit` diagnostics into error entries. The location is always
/// the clause as a whole, for the reason given on [`parse`]; the code is
/// `schema`, because every one of them is a shape violation.
fn diagnostics_to_entries(error: &dsl_kit_parse::BuildError, pointer: &str) -> Vec<ErrorEntry> {
    error
        .diagnostics
        .iter()
        .map(|d| ErrorEntry {
            path: pointer.to_string(),
            code: ErrorCode::Schema,
            hint: Some(d.message.clone()),
        })
        .collect()
}

fn shape_error(pointer: &str, hint: impl Into<String>) -> ErrorEntry {
    ErrorEntry {
        path: pointer.to_string(),
        code: ErrorCode::Schema,
        hint: Some(hint.into()),
    }
}

/// Lower one wire-shaped filter into `dsl-kit`'s tagged convention,
/// recording shape problems against their JSON pointer.
///
/// On error the returned value is a placeholder; [`parse`] checks the
/// error list before looking at it.
fn lower(value: &Value, pointer: &str, errors: &mut Vec<ErrorEntry>) -> Value {
    let Some(object) = value.as_object() else {
        errors.push(shape_error(
            pointer,
            "a filter is an object: {\"and\": [...]}, {\"or\": [...]}, {\"not\": {...}} \
             or {\"path\": ..., \"op\": ..., \"value\": ...}",
        ));
        return Value::Null;
    };

    // The three connectives are keyed by their name.
    for (key, variant) in [("and", "And"), ("or", "Or")] {
        if let Some(children) = object.get(key) {
            if object.len() > 1 {
                errors.push(shape_error(
                    pointer,
                    format!("`{key}` is the only key a `{key}` filter may carry"),
                ));
                return Value::Null;
            }
            let Some(items) = children.as_array() else {
                errors.push(shape_error(
                    &format!("{pointer}/{key}"),
                    format!("`{key}` takes an array of filters"),
                ));
                return Value::Null;
            };
            let lowered: Vec<Value> = items
                .iter()
                .enumerate()
                .map(|(i, child)| lower(child, &format!("{pointer}/{key}/{i}"), errors))
                .collect();
            return json!({ "type": variant, "children": lowered });
        }
    }
    if let Some(child) = object.get("not") {
        if object.len() > 1 {
            errors.push(shape_error(
                pointer,
                "`not` is the only key a `not` filter may carry",
            ));
            return Value::Null;
        }
        let lowered = lower(child, &format!("{pointer}/not"), errors);
        return json!({ "type": "Not", "child": lowered });
    }

    lower_leaf(object, pointer, errors)
}

fn lower_leaf(object: &Map<String, Value>, pointer: &str, errors: &mut Vec<ErrorEntry>) -> Value {
    let Some(path) = object.get("path") else {
        errors.push(shape_error(
            pointer,
            "a leaf filter needs a `path`; a connective needs `and`, `or` or `not`",
        ));
        return Value::Null;
    };
    let Some(path) = path.as_str() else {
        errors.push(shape_error(
            &format!("{pointer}/path"),
            "`path` is a string",
        ));
        return Value::Null;
    };
    let Some(op) = object.get("op") else {
        errors.push(shape_error(
            pointer,
            format!(
                "a leaf filter needs an `op`: one of {}, `{EXISTS_OP}` or `{ANY_OP}`",
                operator_list()
            ),
        ));
        return Value::Null;
    };
    let Some(op) = op.as_str() else {
        errors.push(shape_error(&format!("{pointer}/op"), "`op` is a string"));
        return Value::Null;
    };

    match op {
        EXISTS_OP => {
            reject_extra_keys(object, &["path", "op"], pointer, errors);
            json!({ "type": "Exists", "path": path })
        }
        ANY_OP => {
            reject_extra_keys(object, &["path", "op", "match"], pointer, errors);
            let Some(terms) = object.get("match") else {
                errors.push(shape_error(
                    pointer,
                    "`any` needs a `match` object naming the element fields to constrain",
                ));
                return Value::Null;
            };
            let Some(terms) = terms.as_object() else {
                errors.push(shape_error(
                    &format!("{pointer}/match"),
                    "`match` is an object",
                ));
                return Value::Null;
            };
            let lowered: Map<String, Value> = terms
                .iter()
                .map(|(key, term)| {
                    (
                        key.clone(),
                        lower_term(term, &format!("{pointer}/match/{key}"), errors),
                    )
                })
                .collect();
            json!({ "type": "Any", "path": path, "terms": lowered })
        }
        _ => {
            reject_extra_keys(object, &["path", "op", "value"], pointer, errors);
            let Some(value) = object.get("value") else {
                errors.push(shape_error(
                    pointer,
                    format!("`{op}` needs a `value`; only `{EXISTS_OP}` stands alone"),
                ));
                return Value::Null;
            };
            json!({ "type": "Compare", "path": path, "op": op, "value": value })
        }
    }
}

/// A `match` term is either a bare literal (meaning `eq`) or
/// `{ op, value }`. Both are lowered to the explicit form, which is what
/// [`MatchTerm`] deserialises from; `dsl-kit` hands each entry of a keyed
/// scalar slot to `serde` verbatim.
fn lower_term(term: &Value, pointer: &str, errors: &mut Vec<ErrorEntry>) -> Value {
    match term.as_object() {
        Some(object) if object.contains_key("op") || object.contains_key("value") => {
            reject_extra_keys(object, &["op", "value"], pointer, errors);
            if !object.contains_key("value") {
                errors.push(shape_error(
                    pointer,
                    "a match term with an `op` also needs a `value`",
                ));
                return Value::Null;
            }
            json!({
                "op": object.get("op").cloned().unwrap_or_else(|| json!("eq")),
                "value": object.get("value"),
            })
        }
        // Any other value — including an object that is plainly a literal —
        // is the implicit-`eq` form.
        _ => json!({ "op": "eq", "value": term }),
    }
}

fn reject_extra_keys(
    object: &Map<String, Value>,
    allowed: &[&str],
    pointer: &str,
    errors: &mut Vec<ErrorEntry>,
) {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            errors.push(shape_error(
                &format!("{pointer}/{key}"),
                format!("unknown key `{key}`; this filter takes {}", list(allowed)),
            ));
        }
    }
}

fn list(items: &[&str]) -> String {
    items
        .iter()
        .map(|i| format!("`{i}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn operator_list() -> String {
    ScalarOp::ALL
        .iter()
        .map(|op| format!("`{op}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The wire key a connective is spelled with, so that the type check can
/// rebuild the JSON pointer of a nested filter without a second table of
/// shape names. Leaves have no key of their own: they sit where their
/// parent put them.
pub(crate) fn wire_key(filter: &Filter) -> &'static str {
    match filter {
        Filter::And { .. } => "and",
        Filter::Or { .. } => "or",
        Filter::Not { .. } => "not",
        Filter::Compare { .. } | Filter::Exists { .. } | Filter::Any { .. } => "",
    }
}

/// The JSON Schema of a `where` value, for the OpenAPI document.
///
/// `evalhub_schema::query::QueryRequest` leaves `where` open because the
/// filter's shape belongs to this crate; the server splices this schema in
/// so a generated client sees the real thing.
///
/// The schema describes the *wire* shape, which is the inverse of the
/// lowering in [`parse`]: `dsl-kit`'s [`dsl_kit_schema::NodeSchema`]
/// describes the tagged shape, so it is read for the variant and field
/// names rather than copied. What it cannot express is carried here
/// instead: which keys pair with which operator. The operator list comes
/// from [`ScalarOp::ALL`], so the published vocabulary cannot drift from
/// the one the parser accepts.
pub fn request_schema() -> schemars::Schema {
    let node = Filter::schema();
    // Read the leaf field names off the declared schema rather than
    // spelling them again; if a variant is renamed the schema follows.
    let compare = node
        .variant("Compare")
        .map(|v| v.fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>())
        .unwrap_or_default();
    let path_key = compare
        .iter()
        .find(|f| *f == "path")
        .cloned()
        .unwrap_or_else(|| "path".to_string());
    let value_key = compare
        .iter()
        .find(|f| *f == "value")
        .cloned()
        .unwrap_or_else(|| "value".to_string());
    let ops: Vec<Value> = ScalarOp::ALL.iter().map(|op| json!(op.as_str())).collect();

    let filter_ref = json!({ "$ref": "#/$defs/filter" });
    let term = json!({
        "description": "A literal (meaning `eq`), or an operator and a literal.",
        "oneOf": [
            { "type": ["string", "number", "boolean", "null"] },
            {
                "type": "object",
                "properties": {
                    "op": { "enum": ops.clone() },
                    &value_key: true,
                },
                "required": [&value_key],
                "additionalProperties": false,
            },
        ],
    });

    let document = json!({
        "$ref": "#/$defs/filter",
        "$defs": {
            "filter": {
                "description": "A filter: a connective or a leaf.",
                "oneOf": [
                    {
                        "type": "object",
                        "properties": { "and": { "type": "array", "items": filter_ref } },
                        "required": ["and"],
                        "additionalProperties": false,
                    },
                    {
                        "type": "object",
                        "properties": { "or": { "type": "array", "items": filter_ref } },
                        "required": ["or"],
                        "additionalProperties": false,
                    },
                    {
                        "type": "object",
                        "properties": { "not": filter_ref },
                        "required": ["not"],
                        "additionalProperties": false,
                    },
                    {
                        "description": "A scalar comparison.",
                        "type": "object",
                        "properties": {
                            &path_key: { "type": "string" },
                            "op": { "enum": ops },
                            &value_key: true,
                        },
                        "required": [&path_key, "op", &value_key],
                        "additionalProperties": false,
                    },
                    {
                        "description": "The path has a value, `null` included.",
                        "type": "object",
                        "properties": {
                            &path_key: { "type": "string" },
                            "op": { "const": EXISTS_OP },
                        },
                        "required": [&path_key, "op"],
                        "additionalProperties": false,
                    },
                    {
                        "description": "At least one element of an array path satisfies every term.",
                        "type": "object",
                        "properties": {
                            &path_key: { "type": "string" },
                            "op": { "const": ANY_OP },
                            "match": { "type": "object", "additionalProperties": term },
                        },
                        "required": [&path_key, "op", "match"],
                        "additionalProperties": false,
                    },
                ],
            },
        },
    });
    // `json!` of an object literal is always an object, and schemars builds
    // a schema from one directly.
    match document {
        Value::Object(map) => schemars::Schema::from(map),
        _ => schemars::Schema::default(),
    }
}
