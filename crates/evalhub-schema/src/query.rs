//! The query request and response envelope.
//!
//! The grammar of a query, its type rules, and its compilation live in
//! `evalhub_query`; this module holds only the JSON shape that crosses the
//! wire, so that it appears in the OpenAPI document next to the records.
//!
//! ```text
//! POST /cards/query   (or /evals/query)
//! {
//!   "where":   <filter>,                 // and / or / not / leaf
//!   "sort":    [{ "path": "...", "dir": "asc" | "desc" }],
//!   "limit":   50,
//!   "cursor":  null | "<opaque>",
//!   "expand":  ["relations", "badges", "fingerprints", "changed"],
//!   "version": "latest" | "all"
//! }
//! ```
//!
//! A leaf is `{ path, op, value }` or, for array fields, `{ path, op: "any",
//! match: { ... } }`. The set of legal paths and their types is not written
//! down here or anywhere else by hand: it is projected from the record
//! schema in this crate by `evalhub_query::typecheck`. If the schema gains a
//! key, the query language gains a path.
//!
//! The response is a page: `{ items: [...], next_cursor: string | null }`.
//! Cursors are keyset-based (sort key plus `version_id`), opaque and signed;
//! there is no offset pagination, because offsets over a live index skip and
//! repeat rows.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The body of `POST /cards/query` and `POST /evals/query`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    /// The filter: `and` / `or` / `not` / leaf. Grammar and legal paths are
    /// defined by the query language; see `GET /schemas/query`.
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub where_: Option<serde_json::Value>,
    /// Sort keys, applied in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<Sort>,
    /// Page size. The server clamps it to its maximum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Opaque cursor from a previous page's `next_cursor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Extra fields to include on each item.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expand: Vec<Expand>,
    /// Which versions to search. Defaults to `latest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<VersionSelector>,
}

/// One sort key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sort {
    /// A query path (for example `results[core/pass_rate].value`).
    pub path: String,
    /// Direction.
    pub dir: Dir,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    /// Ascending.
    Asc,
    /// Descending.
    Desc,
}

/// Optional parts of a version that a read can include.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Expand {
    /// The version's edges.
    Relations,
    /// The version's badges.
    Badges,
    /// The seven facet fingerprints.
    Fingerprints,
    /// The top-level keys that changed from the previous version.
    Changed,
}

/// Which versions a query considers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VersionSelector {
    /// Only the latest non-tombstoned version of each name.
    Latest,
    /// Every non-tombstoned version.
    All,
}

/// One page of results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// Cursor for the next page, or `null` on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
}
