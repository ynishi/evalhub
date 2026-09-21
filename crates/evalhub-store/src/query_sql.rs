//! Compilation of `evalhub_query::ir::Query` to SQL.
//!
//! The IR arrives with every path already resolved to a `Column` kind, so
//! this module is a renderer, not a planner:
//!
//! | `Column`              | Renders to                                                    |
//! | --------------------- | ------------------------------------------------------------- |
//! | `Generated(name)`     | `v.<name>`                                                    |
//! | `Fingerprint(facet)`  | `EXISTS (SELECT 1 FROM fingerprints f WHERE f.version_id = v.version_id AND f.facet = $ AND f.fingerprint = $)` |
//! | `Ext { path, Number }`| `evalhub_num(v.body #> $)` — identical text to the index expression |
//! | `Ext { path, Str }`   | `evalhub_str(v.body #> $)`                                    |
//! | `Body(json_path)` eq  | `v.body @> $` (jsonb containment, served by the GIN index)   |
//! | `Body(json_path)` exists | `v.body @? $` (jsonpath existence)                         |
//! | `Any { results, … }`  | `EXISTS (SELECT 1 FROM results r WHERE r.version_id = v.version_id AND …)` |
//! | `Any { relations, … }`| likewise against `relations`                                   |
//!
//! Queries are built with `sqlx::QueryBuilder`; every literal is a bind
//! parameter, never interpolated. Visibility is applied unconditionally:
//! `AND (r.visibility = 'public' OR r.ns = ANY($caller_namespaces))`.
//! `version = latest` adds `AND v.seq = (SELECT max(seq) FROM versions
//! WHERE record_id = v.record_id AND tombstoned_at IS NULL)`; `all` does
//! not.
//!
//! # Sorting and keyset pagination
//!
//! Sort keys are rendered from the same `Column` table, with
//! `results[{metric}].value` becoming a lateral join on `results`. The
//! cursor is a tuple `(sort keys…, version_id)` and the page predicate is
//! the row-value comparison `(k1, k2, version_id) > ($, $, $)` (or `<` for
//! descending), which Postgres serves from the sort index. `LIMIT` is
//! `limit + 1` so the presence of a next page is known without a count.
//!
//! # Tests
//!
//! Snapshot tests render each IR fixture to SQL and assert the text;
//! container tests run the same SQL against a seeded database and assert
//! the rows, including that `EXPLAIN` shows the expression index in use.
