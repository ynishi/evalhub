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
//! Query    { record_type, filter, sort, limit, cursor, expand, version }
//! RunQuery { cards, filter, sort, limit, cursor: RunCursor }
//! Filter := And(Vec) | Or(Vec) | Not(Box) | Cmp { column, op, value }
//!         | Exists { column } | Any { table, conditions: Vec<Cmp> }
//! Column := Generated(name) | Fingerprint(facet) | Ext { path, ty } | Body(json_path)
//!         | Array(column) | Run(RunColumn)
//! RunColumn := RunId | Status | ErrorKind | StartedAt | EndedAt | Facet(json_path)
//!            | Fingerprint(facet) | Metric(id) | Result { card, metric, field }
//! Cursor    := { keys: Vec<Value>, version_id }   // decoded; encoding is the server's
//! RunCursor := { keys: Vec<Value>, run_id }       // the same, for the run projection
//! ```
//!
//! # Two targets
//!
//! [`Query`] searches records (`POST /cards/query`, `POST /evals/query`):
//! one row per version. [`RunQuery`] reads the run projection of one Eval
//! (`GET /evals/{ns}/{name}/runs`): one row per run of that Eval, joined
//! with the `run_results` of the Cards the request names. The filter tree
//! is shared — the grammar does not change between them — but the columns
//! a run query resolves to are [`Column::Run`], plus [`Column::Ext`] and
//! [`Column::Body`] for the run's own `ext`, which in a [`RunQuery`] are
//! paths below the *run's* body, not a version's. A record query never
//! contains [`Column::Run`], and a run query never contains
//! [`Column::Generated`], [`Column::Fingerprint`] or [`Column::Array`];
//! a backend may refuse either stray as unsupported.
//!
//! The run projection has its own cursor, [`RunCursor`], because the row
//! it pages over is keyed by `run_id` (unique within the Eval), not by a
//! `version_id`.
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
    /// In a [`RunQuery`] the path is below the run's body, not a
    /// version's.
    Ext {
        /// JSON path segments below the document root, `ext` included.
        path: Vec<String>,
        /// The registered type, which picks the cast helper.
        ty: ValueType,
    },
    /// Any other path, served by the GIN index over the whole body.
    /// Only equality and existence are allowed here, which is what that
    /// index answers. In a [`RunQuery`] the path is below the run's body
    /// (an unregistered `ext` key of the run).
    Body(Vec<String>),
    /// A column of the side table an `any` condition ranges over.
    Array(ArrayColumn),
    /// A column of the run projection. Only a [`RunQuery`] carries these.
    Run(RunColumn),
}

/// Where a value of the run projection lives. One variant per storage
/// decision of the run tables: a column of `runs`, the run's body, the
/// `run_fingerprints` / `run_metrics` side tables, or a Card's
/// `run_results` joined by `run_id`.
///
/// Every column is scoped to the runs of one Eval: the projection is read
/// through `GET /evals/{ns}/{name}/runs`, never across Evals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunColumn {
    /// `runs.run_id`. A string, unique within the Eval.
    RunId,
    /// `runs.status`: `ok`, `error` or `skipped`.
    Status,
    /// `runs.error_kind`, the producer's slug; null unless `status` is
    /// `error`.
    ErrorKind,
    /// `runs.started_at`. The DSL types it as a string (RFC 3339); a
    /// backend compares it as a timestamp.
    StartedAt,
    /// `runs.ended_at`, as [`RunColumn::StartedAt`].
    EndedAt,
    /// A key of one of the run's six facets, as JSON path segments below
    /// the run's body (`["model", "id"]`). The run stores its facets after
    /// the header's defaults were copied onto it, so this is the run's
    /// own condition, never the header's.
    Facet(Vec<String>),
    /// The fingerprint of one of the run's facets (`run_fingerprints`),
    /// hex-encoded as the API returns it.
    Fingerprint(String),
    /// One of the run's own `metrics`, by registry id `{ns}/{name}`
    /// (`run_metrics`). A run without that metric has no value: a
    /// comparison is false for it, and it sorts as the backend sorts nulls.
    Metric(String),
    /// A judgement from one of the Cards the request names: that Card's
    /// latest live version's `run_results` entry for this run and
    /// `metric`. A run the Card did not judge on that metric has no value.
    Result {
        /// The Card, as named in the request.
        card: CardRef,
        /// The `run_results[].metric`, `{ns}/{name}`.
        metric: String,
        /// Which field of the entry.
        field: ResultField,
    },
}

/// Which field of a Card's `run_results` entry a [`RunColumn::Result`]
/// reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultField {
    /// `run_results[].value`, a number.
    Value,
    /// `run_results[].label`, a string.
    Label,
}

/// A Card named by a run projection request: `{ns}/{name}`, joined at its
/// latest live version.
///
/// `GET /evals/{ns}/{name}/runs?cards=…` names the Cards whose judgements
/// the projection carries, and each is joined at the version a reader
/// would get from `GET /cards/{ns}/{name}`. There is no `@seq` form: the
/// projection answers "what do these graders say about these runs now",
/// and a Card's earlier versions remain readable on their own. Whether the
/// caller may see each Card is the server's decision, made before the
/// path table is built; this crate only knows the names.
///
/// Both parts follow the registry id rule (`[a-z0-9][a-z0-9._-]*`), so a
/// Card reference never contains `[`, `]` or a second `/`, which is what
/// lets `results[{card}][{metric}]` be split without escaping.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CardRef(String);

impl CardRef {
    /// Parse `{ns}/{name}`, or `None` when it is not of that form.
    pub fn parse(s: &str) -> Option<Self> {
        evalhub_core::validate::is_valid_id(s).then(|| Self(s.to_string()))
    }

    /// The text form, `{ns}/{name}`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The namespace part.
    pub fn ns(&self) -> &str {
        self.0.split_once('/').map_or("", |(ns, _)| ns)
    }

    /// The name part.
    pub fn name(&self) -> &str {
        self.0.split_once('/').map_or("", |(_, name)| name)
    }
}

impl std::fmt::Display for CardRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for CardRef {
    type Err = InvalidCardRef;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| InvalidCardRef(s.to_string()))
    }
}

impl TryFrom<String> for CardRef {
    type Error = InvalidCardRef;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<CardRef> for String {
    fn from(card: CardRef) -> Self {
        card.0
    }
}

/// The text was not a Card reference of the form `{ns}/{name}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a Card reference of the form {{ns}}/{{name}}: {0:?}")]
pub struct InvalidCardRef(pub String);

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
    /// A column of the run projection. Only a [`RunQuery`] carries these;
    /// a run query sorts on a registered `ext` path of the run with
    /// [`SortKey::Column`] and [`Column::Ext`].
    Run(RunColumn),
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

/// A checked, resolved read of one Eval's run projection, ready for a
/// backend to compile.
///
/// Which Eval, and whether archived and deleted runs are included
/// (`include=archived,deleted`, members only), are the server's to decide
/// and pass beside this; the DSL knows neither. With no sort the backend
/// orders by `run_id` ascending.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunQuery {
    /// The Cards whose `run_results` are joined, in the order the request
    /// named them. Every [`RunColumn::Result`] in the filter and the sort
    /// names one of these; the projection carries all of them whether or
    /// not a condition mentions them.
    pub cards: Vec<CardRef>,
    /// The predicate. `None` matches every run the server lets through.
    pub filter: Option<Filter>,
    /// Sort keys, most significant first. `run_id` is always the final,
    /// implicit tie-breaker.
    pub sort: Vec<Sort>,
    /// Page size the caller asked for, already clamped by the server.
    pub limit: u32,
    /// Where the previous page stopped, decoded and verified by the server.
    pub cursor: Option<RunCursor>,
}

/// Where the previous page of a run projection stopped: the sort keys of
/// its last row and that row's `run_id`, which breaks ties and makes the
/// tuple unique within the Eval.
///
/// [`Cursor`] cannot serve here, because its tie-breaker is a
/// `version_id` and a run is not a version. The server treats this exactly
/// as it treats a [`Cursor`]: serialises it as JSON, signs the bytes, and
/// hands the client an opaque string; on the next request it verifies the
/// signature and deserialises. The store only compares against it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunCursor {
    /// One value per entry of [`RunQuery::sort`], in the same order. A run
    /// with no value for a key (a metric it lacks) carries `null`.
    pub keys: Vec<Value>,
    /// The last row's `run_id`.
    pub run_id: String,
}
