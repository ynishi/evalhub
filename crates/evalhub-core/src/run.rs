//! What the hub derives from a run: its materialised conditions and its
//! content hash, and the hash over a set of runs.
//!
//! A run is a row of an Eval record, not a version of it (see
//! `evalhub_schema::run`). Writing one goes through this crate in this
//! order:
//!
//! ```text
//! run body ──▶ validate::run          run.json, 2^53, run rules
//!          ──▶ run::materialise       facets the run omits, from the latest header
//!          ──▶ run::run_content_hash  sha256(JCS({ …body, "run_id": … }))
//!          ──▶ fingerprint::for_run   six sha256s, one per facet
//!          ──▶ (store) row, then runs_hash of the record
//! ```
//!
//! # Materialisation
//!
//! The header's facets are defaults for its runs. [`materialise`] copies
//! each facet the run omits from the header onto the run, and the copy is
//! what is stored and hashed. A later change to the header's defaults
//! therefore changes no run already written: the run holds its own copy,
//! and materialising it again against the new header finds every facet it
//! had already present. A facet the run carries itself, `null` included,
//! is the run's claim and wins.
//!
//! Materialisation is a pure function here, rather than a step inside the
//! store, so that the run write path and the 1.0 → 2.0 conversion
//! ([`crate::eval::split_v1`]) share one implementation.
//!
//! # Hashes
//!
//! | Hash               | Over                                   | Formula                                                          |
//! | ------------------ | -------------------------------------- | ---------------------------------------------------------------- |
//! | run `content_hash` | one run, after materialisation         | `sha256(JCS({ …body, "run_id": run_id }))`                       |
//! | `runs_hash`        | every run of an Eval record            | `sha256(JCS([[run_id, hex(content_hash)], …]))`, by `run_id` asc |
//! | used-set hash      | the runs of one Eval a Card used       | the same function as `runs_hash`, over the used set only         |
//!
//! `JCS` is RFC 8785 ([`crate::canonical::canonicalize`]); `sha256` is the
//! digest of those bytes; `hex` is lower-case, sixty-four characters. The
//! formulas are public so that a client can recompute every hash the hub
//! returns from the bodies it holds.
//!
//! The run hash covers the run *as stored*: the facets copied from the
//! header are in it, and `run_id` is set to the id the run is written
//! under, whether or not the body carries it (validation has already
//! required the two to agree). `runs_hash` and the used-set hash sort by
//! `run_id` in ascending byte order of its UTF-8 (Rust's `str` order); the
//! list is sorted before canonicalisation because JCS sorts object keys but
//! keeps array order.
//!
//! `runs_hash` covers every run of the record, archived and deleted ones
//! included. Archiving or deleting a run changes none of these hashes: a
//! deleted run keeps its row and its content hash, as a tombstoned version
//! keeps its `content_hash`, because the material did not change, only how
//! it is shown and kept. A hash changes when a run is added or its content
//! is overwritten, and only then.

use serde_json::{Map, Value};

use evalhub_schema::RecordKind;

use crate::canonical::{CanonicalError, ContentHash, canonicalize, content_hash, hash_value};
use crate::fingerprint::Facet;

/// Copy onto `run` each facet (`model`, `task`, `harness`, `generation`,
/// `trial`, `env`) it does not have, from `header`.
///
/// A facet the run has, even as `null`, is kept. A facet the header lacks
/// or has as `null` is not copied. Idempotent: a second call with the same
/// or any other header copies only facets still absent. Does nothing when
/// `run` is not an object. Cost: six lookups and at most six clones.
pub fn materialise(run: &mut Value, header: &Value) {
    let Some(run) = run.as_object_mut() else {
        return;
    };
    for facet in Facet::for_kind(RecordKind::Eval) {
        let key = facet.as_str();
        if run.contains_key(key) {
            continue;
        }
        if let Some(default) = header.get(key).filter(|v| !v.is_null()) {
            run.insert(key.to_string(), default.clone());
        }
    }
}

/// The content hash of one run: `sha256(JCS({ …run, "run_id": run_id }))`.
///
/// `run` is the body as it will be stored, that is after [`materialise`].
/// `run_id` is written into the hashed object whether or not `run` carries
/// it, and replaces a different value if it does. A `run` that is not an
/// object contributes no keys. Cost: one clone and one canonicalisation of
/// the body.
///
/// # Errors
///
/// As [`crate::canonical::canonicalize`]; not expected for a value parsed
/// from JSON text.
pub fn run_content_hash(run_id: &str, run: &Value) -> Result<ContentHash, CanonicalError> {
    let mut object = match run {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    object.insert("run_id".to_string(), Value::String(run_id.to_string()));
    Ok(hash_value(&Value::Object(object))?.1)
}

/// The hash over a set of runs:
/// `sha256(JCS([[run_id, hex(content_hash)], …]))`, the pairs sorted by
/// `run_id` ascending (byte order of the UTF-8), ties by hash.
///
/// For an Eval record this is `runs_hash`, over every run including the
/// archived and deleted ones; over the runs a Card used it is the used-set
/// hash. The input order does not matter. The empty set hashes `[]`.
/// `run_id`s are unique within a record, so ties do not arise from the
/// store; the tie-break only makes the function total.
///
/// # Errors
///
/// As [`crate::canonical::canonicalize`]; not expected, since the input is
/// strings only.
pub fn runs_hash<S: AsRef<str>>(runs: &[(S, ContentHash)]) -> Result<ContentHash, CanonicalError> {
    let mut pairs: Vec<(&str, String)> = runs
        .iter()
        .map(|(id, hash)| (id.as_ref(), hash.as_hex()))
        .collect();
    pairs.sort();
    let list = Value::Array(
        pairs
            .into_iter()
            .map(|(id, hex)| Value::Array(vec![Value::String(id.to_string()), Value::String(hex)]))
            .collect(),
    );
    Ok(content_hash(&canonicalize(&list)?))
}
