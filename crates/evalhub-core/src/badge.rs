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
//! "Every" is read the usual way: a record with no relations has every
//! relation resolved, and one with no results has every metric registered.
//! The caller passes those facts in [`BadgeInput`]; the store computes them
//! at ingest and is expected to report `true` for the vacuous cases.
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

use std::str::FromStr;

use serde_json::Value;

/// What the caller learned from the store, needed to decide the badges that
/// depend on hub state rather than on the record alone.
#[derive(Debug, Default, Clone)]
pub struct BadgeInput {
    /// `true` when every `relations[].to` in the record resolved to a stored
    /// version (vacuously `true` when there are none).
    pub all_refs_resolved: bool,
    /// `true` when `harness.name@version` exists in the registry.
    pub harness_registered: bool,
    /// `true` when every `results[].metric` exists in the registry
    /// (vacuously `true` when there are none).
    pub all_metrics_registered: bool,
}

/// The badges the hub can award.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Badge {
    /// Every relation target resolved to a stored version at ingest.
    RefsResolved,
    /// The harness is a registry entry.
    HarnessRegistered,
    /// Every metric is a registry entry.
    MetricRegistered,
    /// The run is reproducible from the record: clean git tree with a
    /// commit, and a sandbox image pinned by digest.
    EnvPinned,
    /// The producer states it redacted something before publishing.
    Redacted,
}

impl Badge {
    /// All badges, in the order [`compute`] reports them.
    pub const ALL: [Badge; 5] = [
        Badge::RefsResolved,
        Badge::HarnessRegistered,
        Badge::MetricRegistered,
        Badge::EnvPinned,
        Badge::Redacted,
    ];

    /// The badge's name as stored and served.
    pub const fn as_str(self) -> &'static str {
        match self {
            Badge::RefsResolved => "refs_resolved",
            Badge::HarnessRegistered => "harness_registered",
            Badge::MetricRegistered => "metric_registered",
            Badge::EnvPinned => "env_pinned",
            Badge::Redacted => "redacted",
        }
    }
}

/// The text form was not a badge name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a badge: {0:?}")]
pub struct ParseBadgeError(String);

impl FromStr for Badge {
    type Err = ParseBadgeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Badge::ALL
            .into_iter()
            .find(|b| b.as_str() == s)
            .ok_or_else(|| ParseBadgeError(s.to_string()))
    }
}

impl std::fmt::Display for Badge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Decide the badges of `value` given what the store reported in `input`.
/// Returned in [`Badge::ALL`] order. Pure; never fails.
pub fn compute(value: &Value, input: &BadgeInput) -> Vec<Badge> {
    let mut out = Vec::new();
    if input.all_refs_resolved {
        out.push(Badge::RefsResolved);
    }
    if input.harness_registered {
        out.push(Badge::HarnessRegistered);
    }
    if input.all_metrics_registered {
        out.push(Badge::MetricRegistered);
    }
    if env_pinned(value) {
        out.push(Badge::EnvPinned);
    }
    if value.pointer("/redaction/applied") == Some(&Value::Bool(true)) {
        out.push(Badge::Redacted);
    }
    out
}

fn env_pinned(value: &Value) -> bool {
    let non_empty = |p: &str| {
        value
            .pointer(p)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    value.pointer("/env/git/dirty") == Some(&Value::Bool(false))
        && non_empty("/env/git/commit")
        && non_empty("/trial/sandbox/digest")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    fn pinned() -> Value {
        json!({
            "env": {"git": {"commit": "abc", "dirty": false}},
            "trial": {"sandbox": {"image": "x", "digest": "sha256:1"}},
        })
    }

    #[test]
    fn record_only_badges() {
        let none = BadgeInput::default();
        assert_eq!(compute(&pinned(), &none), vec![Badge::EnvPinned]);

        let mut v = pinned();
        v["env"]["git"]["dirty"] = json!(true);
        assert_eq!(compute(&v, &none), vec![]);

        let mut v = pinned();
        v["env"]["git"]["commit"] = json!("");
        assert_eq!(compute(&v, &none), vec![]);

        let mut v = pinned();
        v["trial"]["sandbox"]["digest"] = json!(null);
        assert_eq!(compute(&v, &none), vec![]);

        let v = json!({"redaction": {"applied": true}});
        assert_eq!(compute(&v, &none), vec![Badge::Redacted]);
        let v = json!({"redaction": {"applied": false}});
        assert_eq!(compute(&v, &none), vec![]);
    }

    #[test]
    fn input_badges_and_order() {
        let input = BadgeInput {
            all_refs_resolved: true,
            harness_registered: true,
            all_metrics_registered: true,
        };
        let mut v = pinned();
        v["redaction"] = json!({"applied": true});
        assert_eq!(compute(&v, &input), Badge::ALL.to_vec());
        assert_eq!(
            compute(&json!({}), &input),
            vec![
                Badge::RefsResolved,
                Badge::HarnessRegistered,
                Badge::MetricRegistered
            ]
        );
    }

    #[test]
    fn names_round_trip() {
        for b in Badge::ALL {
            assert_eq!(b.as_str().parse::<Badge>().unwrap(), b);
        }
        assert!("verified".parse::<Badge>().is_err());
    }
}
