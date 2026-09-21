//! The schema-derived path table and the type check.
//!
//! [`PathTable`] is built once per process from the committed JSON Schema in
//! `evalhub_schema` plus the currently `applied` `ext_schemas` from the
//! registry. It maps each legal query path to `(type, indexed, kind)`:
//!
//! | Path form                          | Type                 | Indexed | Source                    |
//! | ---------------------------------- | -------------------- | ------- | ------------------------- |
//! | `{facet}.{key}` (…nested)          | from schema leaf     | yes     | generated column          |
//! | `fingerprint.{facet}`              | string               | yes     | `fingerprints` table      |
//! | `results`, `relations`, `attachments`, `runs` | array of object | yes | own tables / GIN     |
//! | `results[{metric}].value` (sort)   | number               | yes     | `results` table           |
//! | `ext.{ns}.{key}` registered        | from ext_schema      | yes     | expression index          |
//! | `ext.{ns}.{key}` unregistered      | unknown              | eq/exists only | GIN on `body`      |
//! | `title`, `producer.name`, …        | string               | yes     | generated column          |
//!
//! The check walks the parsed tree and, for each leaf, looks the path up,
//! confirms the operator is legal for the type (table in the crate doc),
//! confirms the literal is of that type, and confirms the operator is
//! served by the path's index. Failures are collected into
//! `422 { errors[] }` with codes `unknown_path`, `type_mismatch`,
//! `not_indexed`.
//!
//! The table is rebuilt when an `ext_schema` transitions to `applied`; the
//! server holds it behind an `ArcSwap`-style handle so a query in flight
//! sees a consistent table.

/// The set of legal query paths with their types and index status,
/// projected from the record schema and the applied `ext_schemas`.
#[derive(Debug, Default, Clone)]
pub struct PathTable {}
