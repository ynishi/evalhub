//! Record types for evalhub, and the source of its public contract.
//!
//! evalhub is a hosting service for LLM evaluation results. This crate holds
//! the shape of what the hub accepts and returns: the two record kinds, the
//! facets they share, the query language's request and response, and the
//! error envelope. Everything else in the workspace is built on these types.
//!
//! # What lives here and what does not
//!
//! This crate is **types only**. It has no I/O, no validation beyond what
//! `serde` enforces structurally, no knowledge of Postgres or HTTP. The
//! semantic rules (counts must add up, attachment references must resolve,
//! integers must fit in 2^53) live in `evalhub_core`; the transport lives in
//! `evalhub_server`. Keeping the types free of behaviour is what lets them be
//! the contract: a client that deserialises `schemas/card.json` sees exactly
//! what the server sees.
//!
//! # The contract is generated from these types, not the other way round
//!
//! The Rust types are the source. Each public record type derives
//! [`schemars::JsonSchema`], and the JSON Schema (draft 2020-12) is generated
//! from it and committed under `schemas/`. A snapshot test asserts that the
//! generated schema equals the committed file, so a change to a type is not
//! done until the schema in the same commit has changed with it. The OpenAPI
//! 3.1 document served by `evalhub_server` embeds the same schemas, produced by
//! the same derive, so there is one description of a Card and it is this
//! crate.
//!
//! The alternative — hand-written JSON Schema with Rust types generated from
//! it — was rejected: generated Rust cannot carry the `serde` attributes and
//! doc comments the server needs, and two hand-maintained documents (record
//! schema and OpenAPI) drift.
//!
//! Doc comments on these types are public. They become the `description` in
//! the served schema and in `openapi.json`, so they are written for the person
//! writing a client, not for the person reading this crate.
//!
//! # The two record kinds
//!
//! A `card::Card` says *what was measured, how, and what the score was*. An
//! `eval::Eval` is *the material*: a set of runs, prompts, tasks or traces
//! that a Card was measured from. Both are published under a name,
//! `{ns}/{name}`, as an append-only sequence of immutable versions. The
//! relation between them (`core/uses_eval`) is what makes comparison possible:
//! two Cards that point at the same Eval version measured the same thing.
//!
//! A Card has no `kind` field. Whether a score came from a judge, an
//! aggregation, or was transcribed from a paper is not a kind of Card; those
//! are properties of how it was graded (`facet::Grading`), how it was
//! aggregated (`results[].aggregation`), and where it came from
//! (`card::Provenance`). Each already has a home.
//!
//! # Facets
//!
//! The parameters that decide whether two records are comparable are grouped
//! into seven facets — model, task, harness, generation, trial, grading, env —
//! and each facet gets a fingerprint (see `evalhub_core::fingerprint`). The
//! rule for what goes into a facet's core keys, as opposed to `ext`, is in
//! [`facet`]. Keys that are recorded but must not affect the fingerprint are
//! marked in the schema with `x-fingerprint: false`; the code has no separate
//! exclusion list.
//!
//! # Closed core, open `ext`
//!
//! Every object in the core is closed (`additionalProperties: false`, via
//! `#[serde(deny_unknown_fields)]`). The only open places are the top-level
//! `ext` and each facet's `ext`, which are maps keyed by namespace
//! (`{ns}/{name}`) so that two producers' extensions cannot collide. A hub
//! that accepted unknown keys silently would let the contract erode one
//! producer at a time.
//!
//! # What the client does not send
//!
//! A record carries no `id` and no `created_at`. The record is the client's
//! claim; the identifier and the time are the hub's facts, assigned on
//! `POST` and returned in the response. The `content_hash` (sha256 of the
//! canonical record) is likewise computed by the hub and is used for
//! idempotency, never as an address.
//!
//! # Numbers
//!
//! Numeric values are JSON numbers, read as IEEE 754 doubles. An integer
//! outside ±2^53 is rejected (`number_too_large`); a producer with such a
//! value sends it as a string. `null` means *recorded but unknown*; an absent
//! key means *not recorded*. The two are different and both are preserved.
//!
//! # Modules
//!
//! - [`card`] — the Card record: results, counts, provenance, redaction.
//! - [`eval`] — the Eval record: runs, origin, source kind.
//! - [`facet`] — the seven facets and their core keys.
//! - [`common`] — pieces shared by both records: producer, attachments,
//!   relations, `ext`.
//! - [`query`] — the query request and response envelope.
//! - [`error`] — the `422 { errors: [...] }` envelope and its code enum.
//! - [`openapi`] — the parts of the OpenAPI document that are not derived
//!   from a handler (info, servers, shared error responses).
//!
//! # Versioning of the schema itself
//!
//! Each record names its schema (`"schema": "evalhub.card/1.0"`). A change
//! that adds an optional key is a minor bump; a change that removes a key,
//! changes a type, or tightens a constraint is a major bump and a new
//! `schemas/card-2.json` alongside the old one. The hub keeps accepting the
//! previous major for at least one release.

pub mod card;
pub mod common;
pub mod error;
pub mod eval;
pub mod facet;
pub mod openapi;
pub mod query;

/// Schema identifier carried by every Card record.
pub const CARD_SCHEMA: &str = "evalhub.card/1.0";

/// Schema identifier carried by every Eval record.
pub const EVAL_SCHEMA: &str = "evalhub.eval/1.0";

/// Names under which the server serves the generated schemas
/// (`GET /schemas/{name}`), in the order [`all_schemas`] returns them.
pub const SCHEMA_NAMES: [&str; 4] = ["card", "eval", "error", "query"];

/// Generator settings shared by every schema: JSON Schema draft 2020-12, the
/// dialect OpenAPI 3.1 uses, so the served files and `openapi.json` agree.
fn generator() -> schemars::SchemaGenerator {
    schemars::generate::SchemaSettings::draft2020_12().into_generator()
}

/// The JSON Schema of a [`card::Card`]. `$id` is not set; the server sets it
/// to the URL it serves the schema from.
pub fn schema_for_card() -> schemars::Schema {
    generator().into_root_schema_for::<card::Card>()
}

/// The JSON Schema of an [`eval::Eval`].
pub fn schema_for_eval() -> schemars::Schema {
    generator().into_root_schema_for::<eval::Eval>()
}

/// The JSON Schema of the [`error::ErrorEnvelope`] returned with `422` / `409`.
pub fn schema_for_error() -> schemars::Schema {
    generator().into_root_schema_for::<error::ErrorEnvelope>()
}

/// The JSON Schema of the [`query::QueryRequest`] envelope. The filter inside
/// `where` is left open here; the query language defines it.
pub fn schema_for_query() -> schemars::Schema {
    generator().into_root_schema_for::<query::QueryRequest>()
}

/// Every served schema with its name, in [`SCHEMA_NAMES`] order. The
/// committed files under `schemas/` are generated from this list, and a test
/// asserts they are current.
pub fn all_schemas() -> Vec<(&'static str, schemars::Schema)> {
    vec![
        ("card", schema_for_card()),
        ("eval", schema_for_eval()),
        ("error", schema_for_error()),
        ("query", schema_for_query()),
    ]
}
