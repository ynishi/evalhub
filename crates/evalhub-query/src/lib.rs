//! The evalhub query language: a small, typed filter DSL whose vocabulary is
//! projected from the record schema.
//!
//! `POST /cards/query` and `POST /evals/query` accept a JSON query
//! (envelope in `evalhub_schema::query`). This crate turns that JSON into
//! something a backend can execute, in three stages, none of which knows
//! SQL:
//!
//! ```text
//! JSON ──▶ grammar::parse     structure: and / or / not / leaf { path, op, value | match }
//!      ──▶ typecheck::check   every path exists, has a type, and the op fits it
//!      ──▶ ir::Query          backend-independent tree the store compiles to SQL
//! ```
//!
//! A query that exercises every construct:
//!
//! ```json
//! {
//!   "where": {"and": [
//!     {"path": "model.id", "op": "eq", "value": "qwen3.6-32b"},
//!     {"path": "generation.temperature", "op": "lte", "value": 0.2},
//!     {"path": "results", "op": "any", "match": {"metric": "core/pass_rate", "value": {"op": "gte", "value": 0.5}}},
//!     {"path": "relations", "op": "any", "match": {"type": "core/uses_eval", "to": "alice/single2-k4"}},
//!     {"path": "fingerprint.generation", "op": "eq", "value": "…"},
//!     {"path": "ext.alice/qwen-loop.rung", "op": "eq", "value": 4}
//!   ]},
//!   "sort": [{"path": "results[core/pass_rate].value", "dir": "desc"}],
//!   "limit": 50, "cursor": null,
//!   "expand": ["relations", "badges", "fingerprints", "changed"],
//!   "version": "latest"
//! }
//! ```
//!
//! # The grammar is the specification
//!
//! The query language is defined as a `dsl-kit` grammar. There is no
//! hand-written `match` over operator names anywhere in the workspace; the
//! grammar definition is the source, and the parser, the JSON Schema for the
//! request body, and the error messages are all derived from it. When an
//! operator is added it is added once.
//!
//! # The path table is projected from the schema
//!
//! Which paths are legal (`model.id`, `generation.temperature`,
//! `results[core/pass_rate].value`, `fingerprint.model`,
//! `ext.alice/qwen-loop.rung`) and what type each has is not written by
//! hand either. At start-up [`typecheck`] walks the committed JSON Schema
//! from `evalhub_schema` and builds the table: every leaf of a facet becomes
//! a path with its JSON type, `fingerprint.{facet}` is added for each facet,
//! `results` / `relations` / `attachments` become array paths that accept
//! `any`, and registered `ext_schemas` contribute typed `ext.` paths. A key
//! added to the schema is queryable in the same commit.
//!
//! # Type rules
//!
//! | Operator                            | Accepts                          |
//! | ----------------------------------- | -------------------------------- |
//! | `eq`, `ne`, `in`                    | any scalar                       |
//! | `gt`, `gte`, `lt`, `lte`            | number, string (lexical)         |
//! | `prefix`, `contains`                | string                           |
//! | `exists`                            | any path                         |
//! | `any` with `match`                  | array of objects                 |
//!
//! A mismatch is `422 type_mismatch`. A path not in the table is
//! `422 unknown_path` — never an empty result, because an empty result for
//! a typo looks like data.
//!
//! # Index awareness
//!
//! The type check also knows which paths are indexed: core facet keys,
//! fingerprints, `results`, `relations`, and `ext` paths whose `ext_schema`
//! is `applied`. An unregistered `ext` path accepts `eq` and `exists` only
//! (the store serves those from a GIN index over the JSON body); any other
//! operator is `422 not_indexed` with a hint naming the registry. This
//! keeps a query from silently becoming a sequential scan.
//!
//! # Sorting and cursors
//!
//! `sort` accepts the same paths. `results[{metric}].value` sorts in the
//! direction the metric's registry entry declares (`lower_is_better`); an
//! unregistered metric cannot be sorted on, because the hub does not know
//! which way is up. Pagination is keyset (`sort key + version_id`), and the
//! IR carries the decoded cursor so the store can emit the `WHERE (k, id) >
//! (?, ?)` clause; the cursor's opaque, signed encoding is the server's
//! business.
//!
//! # Modules
//!
//! - [`grammar`] — the `dsl-kit` definition and the parser it yields.
//! - [`typecheck`] — the schema-derived path table and the type rules.
//! - [`ir`] — the tree handed to the store.

pub mod grammar;
pub mod ir;
pub mod typecheck;
