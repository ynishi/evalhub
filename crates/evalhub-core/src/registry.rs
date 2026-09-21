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
//! table. The store seeds the `core/` rows from the constants here, so
//! there is one definition of the vocabulary.
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

use evalhub_schema::RecordKind;

/// The version every `core/` entry is published under.
pub const CORE_VERSION: &str = "1";

/// A metric definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricDef {
    /// Registry id, `core/{name}`.
    pub id: &'static str,
    /// Whether a smaller value is the better one (sort direction).
    pub lower_is_better: bool,
    /// One line for a reader.
    pub description: &'static str,
}

/// The metrics that ship with the hub.
pub const CORE_METRICS: &[MetricDef] = &[
    MetricDef {
        id: "core/pass_rate",
        lower_is_better: false,
        description: "Fraction of items graded as passing.",
    },
    MetricDef {
        id: "core/accuracy",
        lower_is_better: false,
        description: "Fraction of items answered correctly.",
    },
    MetricDef {
        id: "core/f1",
        lower_is_better: false,
        description: "Harmonic mean of precision and recall.",
    },
    MetricDef {
        id: "core/exact_match",
        lower_is_better: false,
        description: "Fraction of items whose output matched the reference exactly.",
    },
    MetricDef {
        id: "core/pass_at_k",
        lower_is_better: false,
        description: "Probability that at least one of k samples passes.",
    },
    MetricDef {
        id: "core/mean_score",
        lower_is_better: false,
        description: "Mean of a per-item score on the producer's scale.",
    },
    MetricDef {
        id: "core/latency_ms",
        lower_is_better: true,
        description: "Wall-clock time per item, in milliseconds.",
    },
    MetricDef {
        id: "core/cost_usd",
        lower_is_better: true,
        description: "Cost per item, in US dollars.",
    },
];

/// A relation type definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelationTypeDef {
    /// Registry id, `core/{name}`.
    pub id: &'static str,
    /// Kind of the record the edge starts from.
    pub from: RecordKind,
    /// Kind of the record the edge points at.
    pub to: RecordKind,
    /// Name of the edge read backwards.
    pub inverse: &'static str,
}

/// The relation types that ship with the hub.
pub const CORE_RELATION_TYPES: &[RelationTypeDef] = &[
    RelationTypeDef {
        id: "core/uses_eval",
        from: RecordKind::Card,
        to: RecordKind::Eval,
        inverse: "used_by",
    },
    RelationTypeDef {
        id: "core/retry_of",
        from: RecordKind::Card,
        to: RecordKind::Card,
        inverse: "retried_by",
    },
    RelationTypeDef {
        id: "core/rerun_of",
        from: RecordKind::Card,
        to: RecordKind::Card,
        inverse: "rerun_by",
    },
    RelationTypeDef {
        id: "core/judged_by",
        from: RecordKind::Card,
        to: RecordKind::Card,
        inverse: "judges",
    },
    RelationTypeDef {
        id: "core/baseline_of",
        from: RecordKind::Card,
        to: RecordKind::Card,
        inverse: "compared_to",
    },
    RelationTypeDef {
        id: "core/derived_from",
        from: RecordKind::Eval,
        to: RecordKind::Eval,
        inverse: "derives",
    },
    RelationTypeDef {
        id: "core/subset_of",
        from: RecordKind::Eval,
        to: RecordKind::Eval,
        inverse: "superset_of",
    },
];

/// Look a `core/` metric up by id.
pub fn core_metric(id: &str) -> Option<&'static MetricDef> {
    CORE_METRICS.iter().find(|m| m.id == id)
}

/// Look a `core/` relation type up by id.
pub fn core_relation_type(id: &str) -> Option<&'static RelationTypeDef> {
    CORE_RELATION_TYPES.iter().find(|r| r.id == id)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::validate::is_valid_id;

    #[test]
    fn ids_are_well_formed_and_in_core() {
        for m in CORE_METRICS {
            assert!(is_valid_id(m.id), "{}", m.id);
            assert!(m.id.starts_with("core/"), "{}", m.id);
        }
        for r in CORE_RELATION_TYPES {
            assert!(is_valid_id(r.id), "{}", r.id);
            assert!(r.id.starts_with("core/"), "{}", r.id);
            assert!(!r.inverse.is_empty());
        }
        assert_eq!(CORE_METRICS.len(), 8);
        assert_eq!(CORE_RELATION_TYPES.len(), 7);
    }

    #[test]
    fn lookups() {
        assert!(core_metric("core/latency_ms").is_some_and(|m| m.lower_is_better));
        assert!(core_metric("core/pass_rate").is_some_and(|m| !m.lower_is_better));
        assert!(core_metric("alice/x").is_none());
        let r = core_relation_type("core/uses_eval").unwrap();
        assert_eq!(
            (r.from, r.to, r.inverse),
            (RecordKind::Card, RecordKind::Eval, "used_by")
        );
        assert!(core_relation_type("core/nope").is_none());
    }
}
