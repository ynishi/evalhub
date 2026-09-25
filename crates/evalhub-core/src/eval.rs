//! The two Eval schema arms, and the one 1.0 → 2.0 conversion.
//!
//! Release 0.2.0 accepts two majors of the Eval record schema (see
//! "Versioning of the schema itself" in `evalhub_schema`):
//!
//! ```text
//! evalhub.eval/1.0   header + runs[] in one body          (accepted by 0.2.0 only)
//! evalhub.eval/2.0   header only; runs are rows of the record, written apart
//! ```
//!
//! [`declared_schema`] says which arm a body declares, so that the
//! validator, the store and the server agree on it without each parsing
//! `schema` for themselves. [`split_v1`] turns a 1.0 body into what a 2.0
//! producer would have sent: a header and one run per `runs[]` element. It
//! is the only implementation of that conversion; the ingest of a 1.0 body
//! and the one-shot data migration of stored 1.0 versions both call it, so
//! a run converted at ingest and a run converted by the migration are the
//! same run, byte for byte, and have the same content hash.
//!
//! # The conversion
//!
//! ```text
//! 1.0 body ──▶ header: body − runs, schema := "evalhub.eval/2.0"
//!          └─▶ runs[i] ──▶ run: run_id, status (+ error / skip_reason),
//!                               started_at, ended_at, calls, artifacts   (verbatim)
//!                               attachments   the header entries calls/artifacts name
//!                               meta          { "v0.2.0_migration": { "outcome": … } }
//!                               facets        materialised from the header
//! ```
//!
//! Nothing else in the header changes: its `attachments[]` keep the
//! entries the runs point at (they are copied, not moved), so the header
//! is exactly the 1.0 body minus `runs` under the 2.0 identifier, and its
//! content hash is the hash of that header posted as 2.0.
//!
//! `outcome` mixed the execution state with a verdict; 2.0 keeps only the
//! state:
//!
//! | 1.0 `outcome` | 2.0 `status` | detail                                  |
//! | ------------- | ------------ | --------------------------------------- |
//! | `pass`        | `ok`         |                                         |
//! | `fail`        | `ok`         |                                         |
//! | `error`       | `error`      | `error: { "kind": "unrecorded" }`       |
//! | `skipped`     | `skipped`    | `skip_reason: "unrecorded"`             |
//!
//! A 1.0 run says that it errored or was skipped but never why, so the
//! conversion writes `unrecorded` rather than a guess. The dropped
//! `outcome` is kept verbatim in the run's `meta`, under
//! [`V1_MIGRATION_META_KEY`] (`v0.2.0_migration`), so the verdict a 0.1.x
//! producer wrote is not lost; it is simply no longer the hub's to
//! interpret. A later release that has to drop a meaning again keeps it the
//! same way, under its own `v{version}_migration` key.
//!
//! The run's facets are the header's, copied onto the run
//! ([`crate::run::materialise`]); `split_v1` owns materialisation on this
//! path, so the run write path's own `materialise`, which is idempotent,
//! finds nothing left to copy.
//!
//! A 1.0 run's `calls` and `artifacts[]` named paths in the *body's*
//! `attachments[]`. A 2.0 run points only into its own, so each header
//! entry a run names is copied into that run's `attachments[]`, in the
//! header's order, once.

use serde_json::{Map, Value};

use evalhub_schema::EVAL_SCHEMA;

use crate::run::materialise;

/// The `meta` key under which the 1.0 → 2.0 conversion keeps what it
/// dropped: `{ "v0.2.0_migration": { "outcome": … } }`.
pub const V1_MIGRATION_META_KEY: &str = "v0.2.0_migration";

/// The value the conversion writes where 1.0 recorded no reason: the
/// `error.kind` of an `error` run, the `skip_reason` of a `skipped` one.
pub const UNRECORDED: &str = "unrecorded";

/// The `evalhub.eval/1.0` identifier.
const EVAL_SCHEMA_1_0: &str = "evalhub.eval/1.0";

/// Which major of the Eval schema a body declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvalSchema {
    /// `evalhub.eval/1.0`: `runs[]` in the body. Accepted by release 0.2.0
    /// only; converted by [`split_v1`].
    V1,
    /// `evalhub.eval/2.0`: the header alone.
    V2,
}

impl EvalSchema {
    /// The identifier a body declares for this arm.
    pub const fn id(self) -> &'static str {
        match self {
            EvalSchema::V1 => EVAL_SCHEMA_1_0,
            EvalSchema::V2 => EVAL_SCHEMA,
        }
    }
}

/// The Eval arm `body` declares in its `schema` key, or `None` when the key
/// is absent, not a string, or not an accepted Eval identifier.
///
/// This reads one key and checks nothing else; a body that declares an arm
/// can still fail that arm's validation. `None` means the body is not an
/// Eval the hub accepts, and [`crate::validate::validate`] reports why.
pub fn declared_schema(body: &Value) -> Option<EvalSchema> {
    match body.get("schema").and_then(Value::as_str)? {
        EVAL_SCHEMA => Some(EvalSchema::V2),
        EVAL_SCHEMA_1_0 => Some(EvalSchema::V1),
        _ => None,
    }
}

/// A 1.0 body split into its 2.0 parts. See [`split_v1`].
#[derive(Debug, Clone, PartialEq)]
pub struct SplitV1 {
    /// The 2.0 header: the body without `runs`, `schema` set to
    /// `evalhub.eval/2.0`.
    pub header: Value,
    /// One `(run_id, run)` per distinct `run_id`, in the order each id first
    /// appears in `runs[]`; for a repeated id, the run is the *last*
    /// element carrying it. Each run is a 2.0 run body, materialised.
    pub runs: Vec<(String, Value)>,
    /// Every `run_id` that appeared more than once in `runs[]`, once each,
    /// in the order of first appearance. Empty for a body without
    /// duplicates. The conversion does not fail on them (0.1.x accepted
    /// such bodies, and the migration must convert what is stored); the
    /// caller decides whether to report, log or audit them.
    pub duplicate_run_ids: Vec<String>,
}

/// Split an `evalhub.eval/1.0` body into a 2.0 header and its runs.
///
/// Pure: no I/O, the input is not modified, and the same body always gives
/// the same result. The mapping is in the module doc. Duplicate `run_id`s
/// do not fail the call: the last element with an id wins, and the ids are
/// listed in [`SplitV1::duplicate_run_ids`].
///
/// Meant for a body [`crate::validate::validate`] accepted as 1.0. On
/// anything else it still returns, without guessing more than it must: a
/// body that is not an object is returned as the header with no runs; a
/// `runs[]` element without a string `run_id` is dropped; an `outcome`
/// other than the four 1.0 values is treated as `error` (`unrecorded`),
/// and kept verbatim in `meta` like any other.
///
/// Cost: one clone of the body plus one pass over `runs[]` and the header's
/// `attachments[]` per run.
pub fn split_v1(body: &Value) -> SplitV1 {
    let Some(obj) = body.as_object() else {
        return SplitV1 {
            header: body.clone(),
            runs: Vec::new(),
            duplicate_run_ids: Vec::new(),
        };
    };

    let mut header_obj = obj.clone();
    let runs_v1 = header_obj.remove("runs");
    header_obj.insert("schema".to_string(), Value::String(EVAL_SCHEMA.to_string()));
    let header = Value::Object(header_obj);

    let mut runs: Vec<(String, Value)> = Vec::new();
    let mut duplicate_run_ids: Vec<String> = Vec::new();
    let elements = runs_v1
        .as_ref()
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for element in elements {
        let Some(element) = element.as_object() else {
            continue;
        };
        let Some(run_id) = element.get("run_id").and_then(Value::as_str) else {
            continue;
        };
        let run = convert_run(run_id, element, &header);
        match runs.iter_mut().find(|(id, _)| id == run_id) {
            Some(slot) => {
                slot.1 = run;
                if !duplicate_run_ids.iter().any(|d| d == run_id) {
                    duplicate_run_ids.push(run_id.to_string());
                }
            }
            None => runs.push((run_id.to_string(), run)),
        }
    }

    SplitV1 {
        header,
        runs,
        duplicate_run_ids,
    }
}

/// One 1.0 `runs[]` element as a 2.0 run body.
fn convert_run(run_id: &str, v1: &Map<String, Value>, header: &Value) -> Value {
    let mut run = Map::new();
    run.insert("run_id".to_string(), Value::String(run_id.to_string()));

    let outcome = v1.get("outcome");
    match outcome.and_then(Value::as_str) {
        Some("pass" | "fail") => {
            run.insert("status".to_string(), "ok".into());
        }
        Some("skipped") => {
            run.insert("status".to_string(), "skipped".into());
            run.insert("skip_reason".to_string(), UNRECORDED.into());
        }
        // `error`, and anything a validated body cannot carry.
        _ => {
            run.insert("status".to_string(), "error".into());
            let mut error = Map::new();
            error.insert("kind".to_string(), UNRECORDED.into());
            run.insert("error".to_string(), Value::Object(error));
        }
    }

    for key in ["started_at", "ended_at", "calls", "artifacts"] {
        if let Some(v) = v1.get(key) {
            run.insert(key.to_string(), v.clone());
        }
    }

    // The header entries this run points at, in header order, once each.
    let mut named: Vec<&str> = Vec::new();
    if let Some(c) = v1.get("calls").and_then(Value::as_str) {
        named.push(c);
    }
    if let Some(artifacts) = v1.get("artifacts").and_then(Value::as_array) {
        named.extend(artifacts.iter().filter_map(Value::as_str));
    }
    let mut copied: Vec<&str> = Vec::new();
    let mut attachments: Vec<Value> = Vec::new();
    if let Some(entries) = header.get("attachments").and_then(Value::as_array) {
        for entry in entries {
            let Some(path) = entry.get("path").and_then(Value::as_str) else {
                continue;
            };
            if named.contains(&path) && !copied.contains(&path) {
                copied.push(path);
                attachments.push(entry.clone());
            }
        }
    }
    if !attachments.is_empty() {
        run.insert("attachments".to_string(), Value::Array(attachments));
    }

    if let Some(outcome) = outcome {
        let mut dropped = Map::new();
        dropped.insert("outcome".to_string(), outcome.clone());
        let mut meta = Map::new();
        meta.insert(V1_MIGRATION_META_KEY.to_string(), Value::Object(dropped));
        run.insert("meta".to_string(), Value::Object(meta));
    }

    let mut run = Value::Object(run);
    materialise(&mut run, header);
    run
}
