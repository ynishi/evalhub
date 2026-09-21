//! Badges: facts the hub can state about a version without judging it.
//!
//! A badge is a boolean derived from the record and the hub's own state,
//! computed on ingest and stored with the version. Badges are how the hub
//! says "this record is anchored" or "this harness is known" without ever
//! saying "this result is correct". They are inputs to the reader's
//! judgement, never a verdict.
//!
//! | Badge                | Condition                                                                           | Source of truth   |
//! | -------------------- | ----------------------------------------------------------------------------------- | ----------------- |
//! | `refs_resolved`      | every `relations[].to` resolved to a `version_id`                                   | store, at ingest  |
//! | `harness_registered` | `harness.name@version` is a registry entry                                           | registry          |
//! | `metric_registered`  | every `results[].metric` is a registry entry                                        | registry          |
//! | `env_pinned`         | `env.git.dirty == false && env.git.commit && trial.sandbox.digest` all present      | record            |
//! | `redacted`           | `redaction.applied == true`                                                         | record            |
//!
//! # Recomputation
//!
//! Badges derived from the record alone (`env_pinned`, `redacted`) never
//! change, because the record never changes. Badges derived from the
//! registry can: when a harness or metric is registered later, a job
//! recomputes `harness_registered` / `metric_registered` for the versions
//! that cite it. `refs_resolved` is not recomputed — a Card that cited an
//! Eval before it existed keeps its unresolved status as a fact about
//! publication order; the relation itself resolves lazily in the graph API.
//!
//! # Why there is no `verified` badge
//!
//! Because the hub did not run anything. Every badge names a fact the hub
//! actually checked; "verified" would name a judgement it is not in a
//! position to make.
//!
//! This module takes the record plus a small [`BadgeInput`] the server
//! assembles (which references resolved, which registry entries exist) and
//! returns the badge set. It does not query anything.

/// What the caller learned from the store, needed to decide the badges that
/// depend on hub state rather than on the record alone.
#[derive(Debug, Default, Clone)]
pub struct BadgeInput {
    /// `true` when every `relations[].to` in the record resolved to a stored version.
    pub all_refs_resolved: bool,
    /// `true` when `harness.name@version` exists in the registry.
    pub harness_registered: bool,
    /// `true` when every `results[].metric` exists in the registry.
    pub all_metrics_registered: bool,
}
