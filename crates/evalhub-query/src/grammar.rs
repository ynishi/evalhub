//! The `dsl-kit` grammar of a query and the parser derived from it.
//!
//! ```text
//! query   := { where?: filter, sort?: [sort], limit?: int, cursor?: string,
//!              expand?: [expand], version?: "latest" | "all" }
//! filter  := { and: [filter+] } | { or: [filter+] } | { not: filter } | leaf
//! leaf    := { path, op: scalar_op, value }
//!          | { path, op: "exists" }
//!          | { path, op: "any", match: { key: (value | { op, value }) ... } }
//! scalar_op := eq | ne | gt | gte | lt | lte | in | prefix | contains
//! sort    := { path, dir: "asc" | "desc" }
//! expand  := relations | badges | fingerprints | changed
//! ```
//!
//! The grammar is declared once, as a `dsl-kit` schema. From it come the
//! parser (JSON → [`crate::ir`] pre-typed tree), the JSON Schema of the
//! request body that `evalhub_schema::query` exposes in OpenAPI, and the
//! structural error messages. `path` is a string at this stage; its
//! meaning is assigned in [`crate::typecheck`].
//!
//! `any` with `match` is the one non-trivial construct. It applies to array
//! paths (`results`, `relations`, `attachments`, `runs`) and matches if any
//! element satisfies every key in `match`; a key's value is either a literal
//! (implicit `eq`) or `{ op, value }`. This is how `results` is filtered by
//! metric and value at once without exposing array indices.
