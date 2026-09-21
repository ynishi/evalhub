//! `POST /{cards|evals}/query`.
//!
//! Parse and type-check with `evalhub_query` (errors → `422`), attach the
//! caller's namespaces for the visibility predicate, decode and verify the
//! cursor, compile and run with `evalhub_store::query_sql`, sign the next
//! cursor. The handler holds the current `PathTable`, swapped when an
//! `ext_schema` becomes `applied`.
