//! The `core/` registry: the vocabulary that ships with the hub.
//!
//! The registry is the hub's namespace-scoped, immutable catalogue of
//! definitions: `kind/{ns}/{id}@{version}` for four kinds —
//!
//! | Kind             | Defines                                                          | Used by                              |
//! | ---------------- | ---------------------------------------------------------------- | ------------------------------------ |
//! | `metrics`        | `{ id, lower_is_better, description }`                           | `metric_registered` badge, sort direction |
//! | `harnesses`      | `{ name, version, homepage }`                                    | `harness_registered` badge           |
//! | `relation_types` | `{ from, to, inverse }`                                          | relation validation and traversal    |
//! | `ext_schemas`    | `{ ns, schema, fingerprint: bool }`                              | typed `ext` indexing, fingerprint opt-in |
//!
//! Entries under `core/` are defined in this module and are read-only over
//! the API. Everything else is written by namespaces with `write` scope via
//! `PUT /registry/{kind}/{ns}/{id}@{version}` and stored in the `registry`
//! table.
//!
//! # `core/` metrics (v0)
//!
//! `pass_rate`, `accuracy`, `f1`, `exact_match`, `pass_at_k`, `mean_score`,
//! `latency_ms` (lower is better), `cost_usd` (lower is better). This is a
//! starting vocabulary, not a taxonomy; producers register their own.
//!
//! # `core/` relation types
//!
//! | Type           | From → To    | Inverse       |
//! | -------------- | ------------ | ------------- |
//! | `uses_eval`    | Card → Eval  | `used_by`     |
//! | `retry_of`     | Card → Card  | `retried_by`  |
//! | `rerun_of`     | Card → Card  | `rerun_by`    |
//! | `judged_by`    | Card → Card  | `judges`      |
//! | `baseline_of`  | Card → Card  | `compared_to` |
//! | `derived_from` | Eval → Eval  | `derives`     |
//! | `subset_of`    | Eval → Eval  | `superset_of` |
//!
//! # Immutability
//!
//! A registry entry is never edited. A change is a new `@{version}`. This
//! matters for `ext_schemas` in particular: the fingerprints and index
//! entries computed under one version of an extension schema stay valid,
//! and the `fingerprints` table records which registry version they were
//! computed under.
//!
//! # `ext_schemas` and the `applying` state
//!
//! Registering an `ext_schema` returns `202`: the entry is created in state
//! `applying` and the store starts building the typed index for the new
//! paths (see `evalhub_store::index`). While applying, those paths behave as
//! unregistered (`eq` / `exists` only). When the index is valid the entry
//! flips to `applied`. This module defines the entry shape and the state;
//! the store owns the transition.
