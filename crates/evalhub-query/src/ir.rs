//! The backend-independent query tree.
//!
//! [`Query`] is what the type check produces and what
//! `evalhub_store::query_sql` consumes. Every path in it has been resolved
//! to a `Column` that names *where* the value lives (a generated column, a
//! fingerprint row, a side table, a JSON expression) so the SQL compiler
//! needs no knowledge of the schema — it only needs to know how to render
//! each `Column` kind. That split keeps the SQL compiler small and keeps
//! this crate free of SQL.
//!
//! ```text
//! Query { filter: Filter, sort: Vec<Sort>, limit, cursor: Option<Cursor>, expand, version }
//! Filter := And(Vec) | Or(Vec) | Not(Box) | Cmp { column, op, value }
//!         | Exists { column } | Any { table, conditions: Vec<Cmp> }
//! Column := Generated(name) | Fingerprint(facet) | Ext { path, ty } | Body(json_path)
//! Cursor := { keys: Vec<Value>, version_id }   // decoded; encoding is the server's
//! ```
//!
//! The IR is `serde`-serialisable so a query can be snapshot-tested end to
//! end: JSON in, IR out, without a database.

/// A checked, resolved query ready for a backend to compile.
#[derive(Debug, Default, Clone)]
pub struct Query {}
