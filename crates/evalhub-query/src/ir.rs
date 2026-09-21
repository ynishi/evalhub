//! The backend-independent query tree.
//!
//! [`Query`] is what the type check produces and what
//! `evalhub_store::query_sql` consumes. Every path in it has been resolved
//! to a [`Column`] that names *where* the value lives (a generated column, a
//! fingerprint row, a side table, a JSON expression) so the SQL compiler
//! needs no knowledge of the schema — it only needs to know how to render
//! each `Column` kind. That split keeps the SQL compiler small and keeps
//! this crate free of SQL.
//!
//! ```text
//! Query { record_type, filter, sort, limit, cursor, expand, version }
//! Filter := And(Vec) | Or(Vec) | Not(Box) | Cmp { column, op, value }
//!         | Exists { column } | Any { table, conditions: Vec<Cmp> }
//! Column := Generated(name) | Fingerprint(facet) | Ext { path, ty } | Body(json_path)
//! Cursor := { keys: Vec<Value>, version_id }   // decoded; encoding is the server's
//! ```
//!
//! The IR is `serde`-serialisable so a query can be snapshot-tested end to
//! end: JSON in, IR out, without a database.
//!
//! # Why the column kinds are closed
//!
//! Each variant of [`Column`] corresponds to one storage decision made in
//! the migration: a generated column, the `fingerprints` table, an
//! expression index over a registered `ext` path, or the GIN index over the
//! whole body. A path the type check cannot place in one of them is not a
//! query the hub can serve, and saying so (`422 not_indexed`) is better
//! than a sequential scan that looks like a feature until the table grows.
//!
//! # Values
//!
//! Literals travel as `serde_json::Value` and are bound as parameters, never
//! rendered into SQL text. The type check has already confirmed that a
//! literal matches its column's type, so the compiler binds by
//! [`ValueType`] rather than by inspecting the JSON again.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A checked, resolved query ready for a backend to compile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    /// Which record kind is being searched. The compiler adds
    /// `records.type = …`.
    pub record_type: RecordType,
    /// The predicate. `None` matches every visible record.
    pub filter: Option<Filter>,
    /// Sort keys, most significant first. Empty means the backend's
    /// default order (newest version first).
    pub sort: Vec<Sort>,
    /// Page size the caller asked for, already clamped by the server.
    pub limit: u32,
    /// Where the previous page stopped, decoded and verified by the server.
    pub cursor: Option<Cursor>,
    /// What to include alongside each record.
    pub expand: Vec<Expand>,
    /// Whether to consider every version or only the latest live one.
    pub version: VersionSelector,
}

/// The record kind a query searches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordType {
    /// Cards: what was measured, how, and what the score was.
    Card,
    /// Evals: the material a Card was measured from.
    Eval,
}

/// Which versions a query considers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionSelector {
    /// Only the highest non-tombstoned `seq` of each name.
    #[default]
    Latest,
    /// Every non-tombstoned version.
    All,
}

/// What to include alongside each matched record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expand {
    /// The version's outgoing relations.
    Relations,
    /// The badges awarded at ingest.
    Badges,
    /// The seven per-facet fingerprints.
    Fingerprints,
    /// The top-level keys that differ from the previous version.
    Changed,
}

/// The predicate tree. `And`/`Or` with no children are the identity of
/// their operation, which is what an empty `{"and": []}` means.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Filter {
    /// Every child must match.
    And(Vec<Filter>),
    /// At least one child must match.
    Or(Vec<Filter>),
    /// The child must not match.
    Not(Box<Filter>),
    /// A scalar comparison against one column.
    Cmp(Cmp),
    /// The column has a value, `null` included.
    Exists {
        /// Where the value lives.
        column: Column,
    },
    /// At least one row of a side table satisfies every condition.
    Any {
        /// The table to look in.
        table: ArrayTable,
        /// Conditions on that table's columns; all must hold of one row.
        conditions: Vec<Cmp>,
    },
}

/// One scalar comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cmp {
    /// Where the value lives.
    pub column: Column,
    /// How to compare.
    pub op: Op,
    /// What to compare against. `In` carries an array; every other
    /// operator carries a scalar.
    pub value: Value,
}

/// The comparison operators, after the type check has confirmed each one
/// suits its column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
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
    /// Starts with (strings).
    Prefix,
    /// Contains (strings).
    Contains,
}

/// A side table an `any` condition ranges over. Each has its own columns,
/// listed in [`ArrayColumn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrayTable {
    /// `results (metric, aggregation, value, n)`.
    Results,
    /// `relations (type, to_version_id, to_external)`.
    Relations,
    /// `attachment_refs (path, sha256)`.
    Attachments,
}

/// Where a value lives, in terms the storage layer can render directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    /// A generated column on `versions`, named by the migration.
    Generated(String),
    /// A facet's fingerprint, matched against the `fingerprints` table.
    Fingerprint(String),
    /// A registered `ext` path, served by an expression index. The
    /// compiler must render the *same* expression text the index was
    /// created with, which is why the path and the type travel together.
    Ext {
        /// JSON path segments below the document root, `ext` included.
        path: Vec<String>,
        /// The registered type, which picks the cast helper.
        ty: ValueType,
    },
    /// Any other path, served by the GIN index over the whole body.
    /// Only equality and existence are allowed here, which is what that
    /// index answers.
    Body(Vec<String>),
    /// A column of the side table an `any` condition ranges over.
    Array(ArrayColumn),
}

/// The columns an `any` condition may compare, one set per table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrayColumn {
    /// `results.metric`
    ResultMetric,
    /// `results.aggregation`
    ResultAggregation,
    /// `results.value`
    ResultValue,
    /// `results.n`
    ResultN,
    /// `relations.type`
    RelationType,
    /// The relation's target, resolved or textual.
    RelationTo,
    /// `attachment_refs.path`
    AttachmentPath,
    /// `attachment_refs.sha256`
    AttachmentSha256,
}

impl ArrayColumn {
    /// The table this column belongs to, so a compiler can reject a
    /// condition that strayed into another one.
    pub fn table(self) -> ArrayTable {
        match self {
            Self::ResultMetric | Self::ResultAggregation | Self::ResultValue | Self::ResultN => {
                ArrayTable::Results
            }
            Self::RelationType | Self::RelationTo => ArrayTable::Relations,
            Self::AttachmentPath | Self::AttachmentSha256 => ArrayTable::Attachments,
        }
    }
}

/// The scalar types the query language distinguishes. They decide the cast
/// helper for an `ext` path and the binding a literal gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    /// IEEE 754 double, as every JSON number is.
    Number,
    /// UTF-8 text.
    String,
    /// `true` or `false`.
    Boolean,
}

/// One sort key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sort {
    /// What to sort by.
    pub key: SortKey,
    /// Which way.
    pub dir: Dir,
}

/// What a sort orders by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    /// A column of `versions`, or a registered `ext` path.
    Column(Column),
    /// The value of one metric, which is a lateral join on `results`.
    Metric(String),
    /// When the version was stored.
    CreatedAt,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    /// Smallest first.
    Asc,
    /// Largest first.
    Desc,
}

/// Where the previous page stopped: the sort keys of its last row and that
/// row's `version_id`, which breaks ties and makes the tuple unique.
///
/// The server signs and encodes this; the store only compares against it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    /// One value per entry of [`Query::sort`], in the same order.
    pub keys: Vec<Value>,
    /// The last row's version id.
    pub version_id: uuid::Uuid,
}
