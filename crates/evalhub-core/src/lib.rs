//! The domain rules of evalhub: what a record or a run must satisfy, what
//! the hub derives from it, and nothing that touches a database or a
//! socket.
//!
//! This crate is pure. Every function takes a value and returns a value. It
//! knows the record types from [`evalhub_schema`] and it knows the rules in
//! the design; it does not know that Postgres exists. That boundary is what
//! makes the rules testable from fixtures alone (steps 1–3 of the build plan
//! run with no database) and what keeps the store from growing opinions
//! about records.
//!
//! # The model: records, and the runs of an Eval
//!
//! There are two record kinds, a Card and an Eval, each an append-only
//! sequence of immutable versions under a name. An Eval's versioned body is
//! a *header*; its runs are rows of the Eval record, outside every version.
//! Writing a run does not append a header version, and a header version
//! does not rewrite a run.
//!
//! ```text
//! Eval record {ns}/{name}
//! ├── header versions @1, @2, …   validate::validate, content_hash, fingerprints
//! └── runs, keyed by run_id        validate::run, run::materialise,
//!                                  run::run_content_hash, fingerprint::for_run
//!     runs_hash = run::runs_hash over every run of the record
//! ```
//!
//! A run is not a record kind: it has no name, no versions and no
//! visibility of its own, and it is addressed under its Eval. So it has its
//! own functions here rather than a third [`evalhub_schema::RecordKind`].
//! Its conditions are its own: the header's facets are defaults, copied
//! onto a run that omits them when it is written ([`run::materialise`]),
//! and a run is fingerprinted from its own facets ([`fingerprint::for_run`]).
//! A run records what happened, never whether it passed; the verdict is a
//! Card's `run_results`, whose form this crate checks and whose references
//! to stored runs the store checks.
//!
//! # What the hub derives from a record
//!
//! On `POST`, the server calls into this crate in this order:
//!
//! ```text
//! bytes ──▶ validate::structural   JSON Schema of the declared identifier, 2^53 check
//!       ──▶ validate::semantic     counts, attachment paths, metric id form, run_results form
//!       ──▶ canonical::canonicalize   RFC 8785 bytes  ──▶ content_hash (sha256)
//!       ──▶ fingerprint::of_record    seven sha256s, one per facet (six for an Eval)
//!       ──▶ badge::compute            refs_resolved? harness_registered? env_pinned? redacted?
//!       ──▶ (store) one transaction
//! ```
//!
//! All validation errors are collected, not short-circuited, and returned as
//! one `422 { errors[] }` (see `evalhub_schema::error`). The record that
//! reaches the store is the client's bytes, canonicalised; the hub adds
//! facts around it and never rewrites it.
//!
//! One exception to "all errors": an `evalhub.eval/2.0` header that carries
//! `runs` gets exactly one error, `runs_moved`, naming the run endpoints
//! (`PUT /evals/{ns}/{name}/runs/{run_id}`,
//! `POST /evals/{ns}/{name}/runs:batch`).
//!
//! # What the hub derives from a run
//!
//! On `PUT /evals/{ns}/{name}/runs/{run_id}` (and per element of a batch):
//!
//! ```text
//! body ──▶ validate::run            run.json, 2^53, run_id, status ⇔ detail,
//!                                   pointers into the run's own attachments, metrics keys
//!      ──▶ run::materialise         copy the facets the run omits from the latest header
//!      ──▶ run::run_content_hash    sha256(JCS({ …body, "run_id": … }))
//!      ──▶ fingerprint::for_run     six sha256s
//!      ──▶ (store) the row, then run::runs_hash over the record's runs
//! ```
//!
//! # The 1.0 arm
//!
//! Release 0.2.0 still accepts an `evalhub.eval/1.0` body, with `runs[]` in
//! it, for one release. [`eval::declared_schema`] tells the caller which
//! arm a body declares; [`validate()`] checks a 1.0 body as 0.1.x did,
//! including that its runs' `calls` / `artifacts[]` name the body's
//! attachments; and [`eval::split_v1`] converts it into the 2.0 header and
//! runs a 2.0 producer would have written. The ingest of a 1.0 body and the
//! data migration of stored 1.0 versions both use that one function.
//! Release 0.3.0 removes the arm.
//!
//! # Hashes
//!
//! Every hash is SHA-256 over RFC 8785 (JCS) canonical JSON, hex-encoded in
//! lower case. They are formulas a client can recompute, not opaque ids:
//!
//! | Hash                  | Formula                                                                   |
//! | --------------------- | ------------------------------------------------------------------------- |
//! | record `content_hash` | `sha256(JCS(body))` ([`canonical::hash_value`])                            |
//! | run `content_hash`    | `sha256(JCS({ …body, "run_id": run_id }))`, body after materialisation ([`run::run_content_hash`]) |
//! | `runs_hash`           | `sha256(JCS([[run_id, hex(content_hash)], …]))`, sorted by `run_id` ascending, every run of the record including archived and deleted ones ([`run::runs_hash`]) |
//! | used-set hash         | the same function as `runs_hash`, over the runs of one Eval a Card used    |
//!
//! Archiving or deleting a run changes none of them, as tombstoning a
//! version does not change its `content_hash`: the material is the same,
//! only how it is shown and kept has changed. A header's hash changes when
//! a header version is appended; a run's when its content is overwritten;
//! `runs_hash` when a run is added or overwritten; a used-set hash when a
//! run in the used set is overwritten.
//!
//! # Invariants this crate is responsible for
//!
//! - **Canonical form is total and deterministic.** The same JSON value, in
//!   any key order and any whitespace, canonicalises to the same bytes, and
//!   therefore the same `content_hash`. This is what makes `POST` idempotent:
//!   the store compares the new hash with the latest version's and returns
//!   `200` with the existing version if they match. The same holds for a
//!   run's hash and an identical `PUT`.
//! - **A fingerprint depends only on the facet's core keys.** `ext` and keys
//!   marked `x-fingerprint: false` in the schema are removed before hashing.
//!   The exclusion is read from the schema, so the schema is the single
//!   source for what "the same conditions" means.
//! - **Validation is the same everywhere.** The server, a future CLI
//!   `evalhub check`, and a harness embedding this crate all reject the same
//!   records and runs for the same codes.
//! - **One conversion from 1.0.** A 1.0 run converted at ingest and one
//!   converted by the migration are the same bytes and the same hash,
//!   because both go through [`eval::split_v1`].
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
//! - It does not know which runs exist. Whether a `run_results[].run_id`
//!   names a stored run, and whether it is in the Card's used set, is the
//!   store's check (`run_unknown`, `run_not_in_used_set`); how many
//!   `run_results` a Card may carry is the server's (`too_many_run_results`).
//!
//! # Modules
//!
//! - [`canonical`] — RFC 8785 serialisation and the content hash.
//! - [`fingerprint`] — per-facet hashing with schema-driven exclusion, for
//!   records and runs.
//! - [`mod@validate`] — structural and semantic checks, error collection,
//!   for records and runs.
//! - [`eval`] — the two Eval schema arms and the 1.0 → 2.0 conversion.
//! - [`run`] — facet materialisation, the run content hash and `runs_hash`.
//! - [`badge`] — the badge rules and their inputs.
//! - [`registry`] — the `core/` registry entries (metrics, relation types)
//!   that ship with the hub and are read-only.
//! - [`id`] — ULID generation and the version identifier.

pub mod badge;
pub mod canonical;
pub mod eval;
pub mod fingerprint;
pub mod id;
pub mod registry;
pub mod run;
pub mod validate;

pub use badge::{Badge, BadgeInput, compute as compute_badges};
pub use canonical::{ContentHash, canonicalize, content_hash};
pub use eval::{EvalSchema, SplitV1, declared_schema, split_v1};
pub use fingerprint::{Facet, Fingerprints, fingerprints};
pub use id::{RecordId, VersionId};
pub use run::{materialise, run_content_hash, runs_hash};
pub use validate::validate;
