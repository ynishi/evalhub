//! Per-facet fingerprints: the hub's only notion of "measured under the same
//! conditions".
//!
//! ```text
//! fingerprint[facet] = sha256( canonical( facet - ext - {keys with x-fingerprint: false} ) )
//! ```
//!
//! Seven of them, one per facet (model, task, harness, generation, trial,
//! grading, env), computed on ingest and stored in the `fingerprints` table.
//! An Eval has six: it carries no `grading` facet.
//!
//! A run of an Eval has the same six, computed by [`for_run`] from the
//! run's own facets after the header's defaults were copied onto it
//! ([`crate::run::materialise`]). Conditions are per run, so "measured
//! under the same conditions" is decided on run fingerprints; the header's
//! fingerprints are the fingerprints of its defaults. Records with equal
//! fingerprints on a facet agree on every core key of that facet. That is
//! all a fingerprint claims. The hub never combines them into a single
//! "same evaluation" verdict; the reader chooses which axes must match for
//! the comparison they are making, and the query language lets them filter
//! on `fingerprint.{facet}` directly.
//!
//! # What is excluded, and where that is written
//!
//! Two things are removed from a facet before hashing:
//!
//! 1. The facet's `ext` map. Extensions are producer-private by default
//!    and must not make two otherwise-identical records look different.
//! 2. Any key whose schema carries `x-fingerprint: false`. These are
//!    recorded because they are useful to see (`model.context_window`,
//!    `env.os`, `env.hardware`) but do not change what was measured.
//!
//! The exclusion list is read from the JSON Schema in `evalhub_schema` at
//! start-up: each facet's object schema under `$defs` is scanned for
//! properties annotated `"x-fingerprint": false`. There is no
//! hand-maintained list in this crate; if one existed it would drift from
//! the schema, and a drift here changes every fingerprint in the index.
//! Only top-level facet keys are excluded; nested objects are hashed as
//! they are, because that is the only place the schema annotates today.
//!
//! # Absent versus null
//!
//! Both are preserved. A facet with `"seed": null` (recorded, unknown) and
//! one with no `seed` key (not recorded) fingerprint differently. This is
//! intentional: a producer that states "I do not know the seed" is making a
//! different claim from one that did not think to record it.
//!
//! A facet that is absent from the record altogether, or is `null`, is
//! fingerprinted as the empty object `{}`, so every record of a kind has a
//! fingerprint on every facet of that kind and "no model facet" compares
//! equal to "no model facet".
//!
//! # Opting an extension in
//!
//! A namespace that registers an `ext_schema` with `fingerprint: true`
//! asks for its extension keys to participate. From the moment the registry
//! entry is `applied`, those keys are typed, indexed, and folded into the
//! facet's fingerprint for new versions. Existing versions are not
//! re-fingerprinted (a fingerprint is a fact about ingest time), which is
//! why the `fingerprints` table records the registry version it was computed
//! under. That opt-in is not implemented yet; today `ext` is always
//! excluded.
//!
//! # Tests
//!
//! A fixture pairs the complete Card example with its seven expected
//! fingerprints. Property tests assert that changing any excluded key
//! leaves the fingerprint unchanged and changing any core key changes it.

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::sync::OnceLock;

use serde_json::Value;
use sha2::{Digest, Sha256};

use evalhub_schema::RecordKind;

use crate::canonical::{CanonicalError, canonicalize};

/// One of the seven comparability axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Facet {
    /// The model that was evaluated.
    Model,
    /// The task it was evaluated on.
    Task,
    /// The harness that ran it.
    Harness,
    /// Generation settings.
    Generation,
    /// Trial protocol.
    Trial,
    /// How outputs were graded (Cards only).
    Grading,
    /// The execution environment.
    Env,
}

impl Facet {
    /// All seven, in the order they are stored and reported.
    pub const ALL: [Facet; 7] = [
        Facet::Model,
        Facet::Task,
        Facet::Harness,
        Facet::Generation,
        Facet::Trial,
        Facet::Grading,
        Facet::Env,
    ];

    /// The facets a record of `kind` carries. An Eval has no `grading`.
    pub fn for_kind(kind: RecordKind) -> &'static [Facet] {
        const EVAL: [Facet; 6] = [
            Facet::Model,
            Facet::Task,
            Facet::Harness,
            Facet::Generation,
            Facet::Trial,
            Facet::Env,
        ];
        match kind {
            RecordKind::Card => &Facet::ALL,
            RecordKind::Eval => &EVAL,
        }
    }

    /// The key of this facet in the record, and its name in the
    /// `fingerprints` table.
    pub const fn as_str(self) -> &'static str {
        match self {
            Facet::Model => "model",
            Facet::Task => "task",
            Facet::Harness => "harness",
            Facet::Generation => "generation",
            Facet::Trial => "trial",
            Facet::Grading => "grading",
            Facet::Env => "env",
        }
    }

    /// The `$defs` entry that describes this facet in the generated schema.
    const fn def_name(self) -> &'static str {
        match self {
            Facet::Model => "Model",
            Facet::Task => "Task",
            Facet::Harness => "Harness",
            Facet::Generation => "Generation",
            Facet::Trial => "Trial",
            Facet::Grading => "Grading",
            Facet::Env => "Env",
        }
    }
}

/// The text form was not a facet name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a facet: {0:?}")]
pub struct ParseFacetError(String);

impl FromStr for Facet {
    type Err = ParseFacetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Facet::ALL
            .into_iter()
            .find(|f| f.as_str() == s)
            .ok_or_else(|| ParseFacetError(s.to_string()))
    }
}

impl std::fmt::Display for Facet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The fingerprints of one record, one per facet of its kind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fingerprints(BTreeMap<Facet, [u8; 32]>);

impl Fingerprints {
    /// The fingerprint of `facet`, if the record's kind has that facet.
    pub fn get(&self, facet: Facet) -> Option<&[u8; 32]> {
        self.0.get(&facet)
    }

    /// Every fingerprint, in [`Facet::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (Facet, &[u8; 32])> {
        self.0.iter().map(|(f, h)| (*f, h))
    }

    /// The same, hex-encoded and keyed by facet name; the form the API
    /// returns under `expand=fingerprints`.
    pub fn to_hex_map(&self) -> BTreeMap<&'static str, String> {
        self.0
            .iter()
            .map(|(f, h)| (f.as_str(), hex::encode(h)))
            .collect()
    }

    /// Number of facets fingerprinted.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no facet was fingerprinted.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Keys excluded from each facet, read from the generated schema.
fn exclusions() -> &'static BTreeMap<Facet, HashSet<String>> {
    static TABLE: OnceLock<BTreeMap<Facet, HashSet<String>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let schema = RecordKind::Card.schema().to_value();
        let mut table = BTreeMap::new();
        for facet in Facet::ALL {
            table.insert(facet, excluded_keys(&schema, facet));
        }
        table
    })
}

/// Property names of `$defs/<Facet>` whose schema carries
/// `"x-fingerprint": false`. `ext` is always excluded and is added here so
/// the hashing loop has one list.
fn excluded_keys(schema: &Value, facet: Facet) -> HashSet<String> {
    let mut keys: HashSet<String> = HashSet::from(["ext".to_string()]);
    let props = schema
        .pointer(&format!("/$defs/{}/properties", facet.def_name()))
        .and_then(Value::as_object);
    if let Some(props) = props {
        for (name, prop) in props {
            if prop.get("x-fingerprint") == Some(&Value::Bool(false)) {
                keys.insert(name.clone());
            }
        }
    }
    keys
}

/// Whether the generated schema has the shape the exclusion scan expects:
/// a `$defs/<Facet>/properties` object for every facet. A unit test calls
/// this so a schema restructuring fails loudly instead of silently
/// excluding nothing.
pub fn schema_shape_is_understood() -> bool {
    let schema = RecordKind::Card.schema().to_value();
    Facet::ALL.into_iter().all(|f| {
        schema
            .pointer(&format!("/$defs/{}/properties", f.def_name()))
            .is_some_and(Value::is_object)
    })
}

/// Compute the fingerprints of `value`, a record of `kind`.
///
/// Absent or `null` facets hash as `{}`. Fails only if canonicalisation
/// fails, which it cannot for a value that came out of a JSON parser.
pub fn fingerprints(kind: RecordKind, value: &Value) -> Result<Fingerprints, CanonicalError> {
    let table = exclusions();
    let mut out = BTreeMap::new();
    for &facet in Facet::for_kind(kind) {
        let mut object = match value.get(facet.as_str()) {
            Some(Value::Object(m)) => m.clone(),
            _ => serde_json::Map::new(),
        };
        if let Some(excluded) = table.get(&facet) {
            object.retain(|k, _| !excluded.contains(k));
        }
        let bytes = canonicalize(&Value::Object(object))?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        out.insert(facet, digest);
    }
    Ok(Fingerprints(out))
}

/// The fingerprints of one run of an Eval: the same per-facet rule as
/// [`fingerprints`], over the run's own six facets (an Eval's facets; a run
/// has no `grading`).
///
/// Pass the run as stored, after [`crate::run::materialise`], so that a
/// facet the run inherited from the header is fingerprinted as the run's
/// own. A facet still absent after materialisation (neither the run nor the
/// header had it) hashes as `{}`, as on a record. Fails only as
/// [`fingerprints`] does.
pub fn for_run(run: &Value) -> Result<Fingerprints, CanonicalError> {
    fingerprints(RecordKind::Eval, run)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn schema_shape() {
        assert!(schema_shape_is_understood());
    }

    #[test]
    fn exclusion_table_matches_the_schema_annotations() {
        let t = exclusions();
        let names = |f: Facet| {
            let mut v: Vec<&str> = t[&f].iter().map(String::as_str).collect();
            v.sort_unstable();
            v
        };
        assert_eq!(names(Facet::Model), ["context_window", "ext"]);
        assert_eq!(names(Facet::Env), ["ext", "hardware", "os"]);
        for f in [
            Facet::Task,
            Facet::Harness,
            Facet::Generation,
            Facet::Trial,
            Facet::Grading,
        ] {
            assert_eq!(names(f), ["ext"], "{f}");
        }
    }

    #[test]
    fn facet_names_round_trip() {
        for f in Facet::ALL {
            assert_eq!(f.as_str().parse::<Facet>().unwrap(), f);
        }
        assert!("nope".parse::<Facet>().is_err());
        assert_eq!(Facet::for_kind(RecordKind::Eval).len(), 6);
        assert!(!Facet::for_kind(RecordKind::Eval).contains(&Facet::Grading));
    }
}
