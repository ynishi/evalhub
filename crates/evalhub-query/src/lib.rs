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
//! The filter AST is a `dsl-kit` enum, and the conformance check, the typed
//! builder and the published JSON Schema of the `where` value all come from
//! it. The operator vocabulary in particular is declared once, as
//! [`grammar::ScalarOp`]: adding an operator is a variant plus its row in
//! the type table, and no parser arm changes.
//!
//! What *is* hand-written is the lowering from the wire shape
//! (`{"and": [...]}`) into the tagged shape `dsl-kit`'s JSON front end reads
//! (`{"type": "And", …}`), which names the five node shapes. See the
//! [`grammar`] module doc for why that is a shape concern rather than a
//! vocabulary one.
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
//! The type check also knows which paths are indexed: the sixteen facet
//! keys the migration materialises as generated columns, the fingerprints,
//! and the `results` / `relations` / `attachments` side tables, plus `ext`
//! paths whose `ext_schema` is `applied`. Every other path — an
//! unregistered `ext` key, and equally a facet key with no column of its
//! own — accepts `eq` and `exists` only, which is what the GIN index over
//! the JSON body answers; any other operator is `422 not_indexed` with a
//! hint naming the registry. This keeps a query from silently becoming a
//! sequential scan.
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
//! # The second target: the run projection
//!
//! `GET /evals/{ns}/{name}/runs` reads one row per run of one Eval, joined
//! with the `run_results` of the Cards the request names (`cards=`). It
//! speaks the same grammar — `where`, `sort`, `limit`, `cursor` — against
//! a different path table, and compiles to a different IR:
//!
//! ```text
//! JSON ──▶ grammar::parse                          (unchanged)
//!      ──▶ typecheck::check  against PathTable::for_runs(cards)
//!      ──▶ ir::RunQuery      { cards, filter, sort, limit, cursor: RunCursor }
//! ```
//!
//! [`PathTable::for_runs`] is built by hand, not from a record schema: the
//! run's columns (`run_id`, `status`, `error.kind`, `started_at`,
//! `ended_at`), the keys of its six facets (the Eval header's key set,
//! `model.id` and so on), `fingerprint.{facet}`, `metrics[{ns}/{name}]`,
//! `results[{card}][{metric}].value` and `.label` for the named Cards, and
//! `ext.*` through the same [`PathTable::with_ext`]. Parameters are
//! bracketed, as in `results[core/pass_rate].value`, because a slug may
//! contain `.` and `/`; the dotted forms are refused with a hint. A Card
//! the request did not name is `unknown_path`, and so is a malformed metric
//! id; a metric the registry does not know is accepted, because a run's
//! metrics are.
//!
//! ```json
//! {
//!   "where": {"and": [
//!     {"path": "status", "op": "eq", "value": "ok"},
//!     {"path": "metrics[core/tokens_out]", "op": "lt", "value": 4000},
//!     {"path": "results[alice/judge][core/pass].value", "op": "gte", "value": 1}
//!   ]},
//!   "sort": [{"path": "metrics[core/duration_ms]", "dir": "asc"}]
//! }
//! ```
//!
//! Every path of the projection sorts and ranges as its type allows
//! (a boolean facet key does not range), facet keys included, except an
//! unregistered `ext` key, which answers `eq` and `exists` only because
//! nothing declares its type. The projection is scoped to one Eval and
//! cannot grow into a scan of the hub; the `not_indexed` rule above is
//! about record queries.
//! Paging is keyset on `(sort key, run_id)` with [`ir::RunCursor`], which
//! the server signs exactly as it signs [`ir::Cursor`]. Which Eval, whether
//! archived and deleted runs are included (`include=archived,deleted`) and
//! which Cards a caller may name are the server's decisions, not the
//! DSL's. The [`typecheck`] module doc has the full table.
//!
//! # Modules
//!
//! - [`grammar`] — the `dsl-kit` definition and the parser it yields.
//! - [`typecheck`] — the path tables (schema-derived for records, built by
//!   hand for runs) and the type rules.
//! - [`ir`] — the trees handed to the store, [`Query`] and [`RunQuery`].

pub mod grammar;
pub mod ir;
pub mod typecheck;

pub use grammar::{Filter, MatchTerm, ScalarOp, parse, request_schema};
pub use ir::{CardRef, Query, RunCursor, RunQuery};
pub use typecheck::{ExtSchema, Indexed, PathInfo, PathTable, check, compile, compile_runs};
