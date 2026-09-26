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
//! `{ns}/{name}`, as an append-only sequence of immutable versions (the one
//! exception, the 0.2.0 data migration that rewrote stored
//! `evalhub.eval/1.0` bodies into 2.0 headers, is `evalhub_store`'s). The
//! relation between them (`core/uses_eval`) is what makes comparison possible:
//! two Cards that point at the same Eval version measured the same thing.
//!
//! An Eval's versioned body is a *header*. Its runs (`run::Run`) are rows
//! of the Eval record, outside every version: writing a run does not append
//! a header version. A run records what happened and what was measured
//! directly; the verdict on it is the Card's (`card::Card::run_results`).
//!
//! The runs a Card judged are its *used set*, per Eval it uses: the
//! `attrs.runs` of its `core/uses_eval` relations, or, without them, every
//! run neither archived nor deleted when the Card is posted ([`common`],
//! [`card`]). Four hashes tie the pieces together, all computed by the
//! hub and none sent by the client: the header's `content_hash`, each
//! run's `content_hash`, the record's `runs_hash` over every run, and a
//! used-set hash over the runs a Card used; the formulas are
//! `evalhub_core`'s. Runs have no visibility of their own and follow their
//! Eval; a Card's `run_results` follow the Card, except that a reader who
//! may not see an Eval is never shown the entries that name it. How many
//! runs a batch and how many `run_results` a Card may carry are the
//! server's limits, not constraints of these types.
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
//! - [`eval`] — the Eval header: origin, source kind, default conditions;
//!   and [`eval::v1`], the 1.0 shape with `runs[]` in the body.
//! - [`run`] — a run of an Eval: status, per-run facets, metrics, its own
//!   attachments.
//! - [`facet`] — the seven facets and their core keys.
//! - [`common`] — pieces shared by Card, Eval and Run: producer, attachments,
//!   relations, `ext`.
//! - [`query`] — the query request and response envelope.
//! - [`error`] — the `422 { errors: [...] }` envelope and its code enum.
//! - [`openapi`] — the parts of the OpenAPI document that are not derived
//!   from a handler (info, servers, shared error responses).
//!
//! # Versioning of the schema itself
//!
//! Each record names its schema (`"schema": "evalhub.card/1.1"`). A change
//! that adds an optional key is a minor bump; a change that removes a key,
//! changes a type, or tightens a constraint is a major bump and a new
//! `schemas/card-2.json` alongside the old one. The hub keeps accepting the
//! previous major for at least one release.
//!
//! A kind can therefore have more than one accepted identifier.
//! [`RecordKind::schema_id`] and [`RecordKind::schema`] name the current
//! one, which is what a producer should write; [`RecordKind::schema_ids`]
//! lists every identifier the hub accepts, and [`RecordKind::schema_for`]
//! returns the document for one of them. A minor bump keeps one document
//! whose `schema` key accepts every minor of the major; a major bump adds a
//! document.
//!
//! The current state: `evalhub.card/1.1` added the optional `run_results`,
//! and `evalhub.card/1.0` remains accepted under the same document
//! (`schemas/card.json`). `evalhub.eval/2.0` removed `runs` from the Eval
//! body, a major bump, so it has its own document (`schemas/eval-2.json`)
//! beside the 1.0 one (`schemas/eval.json`, from [`eval::v1`]). Release
//! 0.2.0 is the one release that still accepts `evalhub.eval/1.0` bodies,
//! converting them at ingest into a 2.0 header and its runs; release 0.3.0
//! removes `evalhub.eval/1.0`, [`eval::v1`] and `schemas/eval.json`.

pub mod card;
pub mod common;
pub mod error;
pub mod eval;
pub mod facet;
pub mod openapi;
pub mod query;
pub mod run;

/// Schema identifier a Card record should declare: the current one.
/// `evalhub.card/1.0` is still accepted (see [`RecordKind::schema_ids`]).
pub const CARD_SCHEMA: &str = "evalhub.card/1.1";

/// Schema identifier an Eval header should declare: the current one.
/// `evalhub.eval/1.0` is accepted by release 0.2.0 only (see
/// [`RecordKind::schema_ids`] and [`eval::v1`]).
pub const EVAL_SCHEMA: &str = "evalhub.eval/2.0";

/// The previous minor of the Card schema, still accepted, described by the
/// same document as [`CARD_SCHEMA`].
const CARD_SCHEMA_1_0: &str = "evalhub.card/1.0";

/// The previous major of the Eval schema, accepted by release 0.2.0 only.
const EVAL_SCHEMA_1_0: &str = "evalhub.eval/1.0";

/// Names under which the server serves the generated schemas
/// (`GET /schemas/{name}`), in the order [`all_schemas`] returns them.
/// `eval` is the 1.0 document (with `runs[]`) until release 0.3.0 removes
/// it; `eval-2` is the current Eval header; `run` is one run.
pub const SCHEMA_NAMES: [&str; 6] = ["card", "eval", "eval-2", "run", "error", "query"];

/// The two record kinds, as they appear in URLs (`/cards`, `/evals`) and in
/// the `type` column of the store. Everything that differs between a Card
/// and an Eval (its schema, its facets, its semantic rules) is keyed on this.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    /// A Card: what was measured, how, and what the score was.
    Card,
    /// An Eval: the material a Card was measured from.
    Eval,
}

impl RecordKind {
    /// The current `schema` identifier for this kind: the one a producer
    /// should declare. Other identifiers may still be accepted; see
    /// [`RecordKind::schema_ids`].
    pub const fn schema_id(self) -> &'static str {
        match self {
            RecordKind::Card => CARD_SCHEMA,
            RecordKind::Eval => EVAL_SCHEMA,
        }
    }

    /// Every `schema` identifier the hub accepts for this kind, the current
    /// one ([`RecordKind::schema_id`]) first.
    pub const fn schema_ids(self) -> &'static [&'static str] {
        match self {
            RecordKind::Card => &[CARD_SCHEMA, CARD_SCHEMA_1_0],
            RecordKind::Eval => &[EVAL_SCHEMA, EVAL_SCHEMA_1_0],
        }
    }

    /// The generated JSON Schema for the current identifier of this kind.
    pub fn schema(self) -> schemars::Schema {
        match self {
            RecordKind::Card => schema_for_card(),
            RecordKind::Eval => schema_for_eval(),
        }
    }

    /// The generated JSON Schema for one accepted identifier of this kind,
    /// or `None` when `id` is not one of [`RecordKind::schema_ids`]. Both
    /// Card identifiers share one document; each Eval major has its own.
    pub fn schema_for(self, id: &str) -> Option<schemars::Schema> {
        match (self, id) {
            (RecordKind::Card, CARD_SCHEMA | CARD_SCHEMA_1_0) => Some(schema_for_card()),
            (RecordKind::Eval, EVAL_SCHEMA) => Some(schema_for_eval()),
            (RecordKind::Eval, EVAL_SCHEMA_1_0) => Some(schema_for_eval_v1()),
            _ => None,
        }
    }

    /// The short name: `card` or `eval`.
    pub const fn as_str(self) -> &'static str {
        match self {
            RecordKind::Card => "card",
            RecordKind::Eval => "eval",
        }
    }
}

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

/// The JSON Schema of an [`eval::Eval`] header (`evalhub.eval/2.0`), served
/// as `eval-2`.
pub fn schema_for_eval() -> schemars::Schema {
    generator().into_root_schema_for::<eval::Eval>()
}

/// The JSON Schema of an [`eval::v1::Eval`] (`evalhub.eval/1.0`, with
/// `runs[]`), served as `eval` until release 0.3.0 removes it.
pub fn schema_for_eval_v1() -> schemars::Schema {
    generator().into_root_schema_for::<eval::v1::Eval>()
}

/// The JSON Schema of a [`run::Run`].
pub fn schema_for_run() -> schemars::Schema {
    generator().into_root_schema_for::<run::Run>()
}

/// The JSON Schema of the [`error::ErrorEnvelope`] returned with `422` / `409`
/// (and `413` for the size codes).
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
        ("eval", schema_for_eval_v1()),
        ("eval-2", schema_for_eval()),
        ("run", schema_for_run()),
        ("error", schema_for_error()),
        ("query", schema_for_query()),
    ]
}
