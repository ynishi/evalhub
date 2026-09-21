//! The domain rules of evalhub: what a record must satisfy, what the hub
//! derives from it, and nothing that touches a database or a socket.
//!
//! This crate is pure. Every function takes a value and returns a value. It
//! knows the record types from [`evalhub_schema`] and it knows the rules in
//! the design; it does not know that Postgres exists. That boundary is what
//! makes the rules testable from fixtures alone (steps 1–3 of the build plan
//! run with no database) and what keeps the store from growing opinions
//! about records.
//!
//! # What the hub derives from a record
//!
//! On `POST`, the server calls into this crate in this order:
//!
//! ```text
//! bytes ──▶ validate::structural   JSON Schema (closed core), 2^53 check
//!       ──▶ validate::semantic     counts, attachment paths, metric id form
//!       ──▶ canonical::canonicalize   RFC 8785 bytes  ──▶ content_hash (sha256)
//!       ──▶ fingerprint::of_record    seven sha256s, one per facet
//!       ──▶ badge::compute            refs_resolved? harness_registered? env_pinned? redacted?
//!       ──▶ (store) one transaction
//! ```
//!
//! All validation errors are collected, not short-circuited, and returned as
//! one `422 { errors[] }` (see `evalhub_schema::error`). The record that
//! reaches the store is the client's bytes, canonicalised; the hub adds
//! facts around it and never rewrites it.
//!
//! # Invariants this crate is responsible for
//!
//! - **Canonical form is total and deterministic.** The same JSON value, in
//!   any key order and any whitespace, canonicalises to the same bytes, and
//!   therefore the same `content_hash`. This is what makes `POST` idempotent:
//!   the store compares the new hash with the latest version's and returns
//!   `200` with the existing version if they match.
//! - **A fingerprint depends only on the facet's core keys.** `ext` and keys
//!   marked `x-fingerprint: false` in the schema are removed before hashing.
//!   The exclusion is read from the schema, so the schema is the single
//!   source for what "the same conditions" means.
//! - **Validation is the same everywhere.** The server, a future CLI
//!   `evalhub check`, and a harness embedding this crate all reject the same
//!   records for the same codes.
//!
//! # What this crate refuses to do
//!
//! - It does not open attachments. `samples_ref` must name an entry in
//!   `attachments[]`; whether the file has the rows it claims is not checked
//!   (design decision: content validation is out of scope for v0 and, when
//!   it comes, it is a separate job, not part of ingest).
//! - It does not decide whether two records are "the same evaluation". It
//!   produces per-facet fingerprints; the reader compares them.
//! - It does not resolve relations. Resolution needs the store; this crate
//!   only checks the *form* of a reference and reports, via the badge input,
//!   whether the store said it resolved.
//!
//! # Modules
//!
//! - [`canonical`] — RFC 8785 serialisation and the content hash.
//! - [`fingerprint`] — per-facet hashing with schema-driven exclusion.
//! - [`validate`] — structural and semantic checks, error collection.
//! - [`badge`] — the badge rules and their inputs.
//! - [`registry`] — the `core/` registry entries (metrics, relation types)
//!   that ship with the hub and are read-only.
//! - [`id`] — ULID generation and the version identifier.

pub mod badge;
pub mod canonical;
pub mod fingerprint;
pub mod id;
pub mod registry;
pub mod validate;

pub use canonical::{ContentHash, canonicalize, content_hash};
pub use id::{RecordId, VersionId};
