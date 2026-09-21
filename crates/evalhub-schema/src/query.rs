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
