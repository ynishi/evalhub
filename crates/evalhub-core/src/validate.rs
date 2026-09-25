//! Validation: everything the hub checks before it stores a record or a
//! run.
//!
//! Validation runs in two passes and collects every failure from both into
//! one list, so a producer sees all problems in one round trip. There are
//! two entry points:
//!
//! - [`validate`] for a record (a Card, or an Eval header), keyed by
//!   [`RecordKind`];
//! - [`run()`] for one run of an Eval, given the `run_id` it is written
//!   under.
//!
//! A run is not a [`RecordKind`]. `RecordKind` is the two kinds that appear
//! in URLs and in the store's `type` column, and every match on it is
//! exhaustive; a run is a row of an Eval record, addressed under it, so it
//! has its own entry point rather than a third variant that every one of
//! those matches would have to reject.
//!
//! # Pass 1: structural
//!
//! The value is checked against a JSON Schema using `jsonschema` with
//! draft 2020-12. The core is closed, so an unknown key anywhere outside
//! `ext` is a `schema` error. In the same pass every number is inspected:
//! an integer outside ±2^53 is `number_too_large`.
//!
//! Which schema: a record is checked against the document for the
//! identifier its own `schema` key declares, through
//! [`RecordKind::schema_for`]:
//!
//! | Declared `schema`                      | Document               |
//! | -------------------------------------- | ---------------------- |
//! | `evalhub.card/1.1`, `evalhub.card/1.0` | `schemas/card.json`    |
//! | `evalhub.eval/2.0`                     | `schemas/eval-2.json`  |
//! | `evalhub.eval/1.0`                     | `schemas/eval.json`    |
//! | anything else, or no `schema` key      | the kind's current one |
//!
//! The last row is what makes an unknown identifier an error: the current
//! document's `schema` key is a constant, so the value fails it at
//! `/schema` (`schema`), exactly as it did before a kind had two
//! identifiers. A run is checked against `schemas/run.json`.
//!
//! The schemas are the generated ones from `evalhub_schema` (the same
//! generator output that is committed under `schemas/`). Each is compiled
//! once per process, one validator per accepted identifier plus one for
//! the run, on first use. The validators are built in offline mode: every
//! `$ref` in the schema is local (`#/$defs/…`), and a reference that would
//! need fetching is a build error, not a fetch.
//!
//! # The two Eval arms
//!
//! Release 0.2.0 accepts two majors of the Eval schema
//! ([`crate::eval::EvalSchema`]):
//!
//! - **2.0** (`evalhub.eval/2.0`) is a header. Runs are not part of it; they
//!   are written one by one (`PUT /evals/{ns}/{name}/runs/{run_id}`) or in
//!   a batch (`POST /evals/{ns}/{name}/runs:batch`). A 2.0 body that has a
//!   `runs` key at all, whatever its value, is answered with one error,
//!   `runs_moved` at `/runs`, *before* the structural pass, and nothing
//!   else: every other error the body might have would be noise next to
//!   "this is the wrong endpoint for your runs", and the producer that
//!   sends `runs` is typically a 1.0 producer that bumped the identifier.
//! - **1.0** (`evalhub.eval/1.0`) is the old shape with `runs[]` in the
//!   body, validated as 0.1.x validated it, including the check that each
//!   `runs[].calls` and `runs[].artifacts[]` names an `attachments[].path`
//!   of the body. That check exists for the 1.0 arm only. A 1.0 body that
//!   passes is converted into a 2.0 header and its runs by
//!   [`crate::eval::split_v1`]; release 0.3.0 removes the arm.
//!
//! # Pass 2: semantic
//!
//! Rules the schema language cannot express. On a record:
//!
//! | Rule                                                                                   | Code                       |
//! | -------------------------------------------------------------------------------------- | -------------------------- |
//! | `results` non-empty ⇒ `counts` present                                                 | `counts_missing`           |
//! | `counts.attempted >= completed + failed + skipped + errored`                           | `counts_inconsistent`      |
//! | every `results[].samples_ref` is an `attachments[].path`                               | `attachment_ref_unknown`   |
//! | Eval 1.0 only: every `runs[].calls`, `runs[].artifacts[]` is an `attachments[].path`   | `attachment_ref_unknown`   |
//! | `attachments[].path` unique, relative, contains no `..`                                | `attachment_path_invalid`  |
//! | `results[].metric` matches `{ns}/{name}`                                               | `metric_id_invalid`        |
//! | `run_results[].metric` matches `{ns}/{name}`                                           | `metric_id_invalid`        |
//! | `run_results[]` element has `value` or `label` (or both)                               | `run_result_value_or_label`|
//! | `run_results[].eval` is the `{ns}/{name}` of a `core/uses_eval` relation's target      | `run_results_eval_unknown` |
//! | `relations[].type` matches `{ns}/{name}`                                               | `schema`                   |
//! | `relations[].to` is `{ns}/{name}@{seq}`, `external:…`, or `hf:…`                       | `schema`                   |
//!
//! `run_results[].eval` is compared with the target of each of the Card's
//! `core/uses_eval` relations *without* its `@{seq}`: runs belong to the
//! Eval record, not to a version, so a verdict names the record. Only a
//! hub target (`{ns}/{name}@{seq}`) can match; an `external:` or `hf:`
//! target has no runs on this hub. Whether that target resolves, and
//! whether the writer may see it, is not this crate's question.
//!
//! Three more rules about `run_results` need data this crate does not
//! have, and are checked elsewhere:
//!
//! | Rule                                                                                   | Code                       | Checked by |
//! | -------------------------------------------------------------------------------------- | -------------------------- | ---------- |
//! | every `run_id` of a used set has a row in its Eval (archived and deleted rows count)   | `run_unknown`              | the store  |
//! | every `run_results[].run_id` is in the used set of its `eval`                          | `run_not_in_used_set`      | the store  |
//! | `run_results[]` is no longer than the configured limit                                 | `too_many_run_results`     | the server |
//!
//! On a run ([`run()`]), everything is checked against the run itself; a
//! run never points into the header:
//!
//! | Rule                                                                                   | Code                       |
//! | -------------------------------------------------------------------------------------- | -------------------------- |
//! | the given `run_id` is non-empty, has no `/`, is at most 200 bytes                      | `run_id_invalid`           |
//! | the body's `run_id` equals the given one                                               | `run_id_mismatch`          |
//! | `error` present iff `status` is `error`; `skip_reason` iff `status` is `skipped`       | `run_status_detail`        |
//! | `calls`, `artifacts[]`, `error.log` each name an `attachments[].path` of the run       | `attachment_ref_unknown`   |
//! | the run's `attachments[].path` unique, relative, contains no `..`                      | `attachment_path_invalid`  |
//! | every key of `metrics` matches `{ns}/{name}`                                           | `metric_id_invalid`        |
//!
//! `ext` is treated on a run as on a record: an object whose keys are
//! meant to be namespaces, accepted as the JSON Schema describes it, with
//! no semantic rule of its own in this crate today. `meta` is never looked
//! into.
//!
//! The semantic pass runs on whatever is present even when the structural
//! pass failed, so a producer sees both kinds of problem at once. Errors
//! are sorted by `(path, code)`; the order is deterministic.
//!
//! # What is deliberately not an error
//!
//! - A `relations[].to` that does not resolve. The record is accepted; the
//!   `refs_resolved` badge is withheld. A Card may legitimately be published
//!   before the Eval it cites.
//! - A `harness` or `metric` the registry does not know, including a run's
//!   `metrics` key. Accepted; the `harness_registered` /
//!   `metric_registered` badges are withheld. The hub does not gate on
//!   vocabulary.
//! - An `attachments[].sha256` that has not been uploaded. This is not a
//!   validation failure but a state conflict, `409 attachment_missing`,
//!   raised by the store, because only the store knows.
//! - An Eval header with zero runs, of any `eval_kind`. A header is
//!   complete on its own; its runs are written afterwards.
//!
//! # Output
//!
//! `Vec<ErrorEntry { path, code, hint }>`, empty on success. `path` is a
//! JSON pointer into the submitted value (`/results/0/metric`; the root is
//! `""`). `hint` is prose and not part of the contract.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use jsonschema::{Draft, Validator};
use serde_json::{Map, Value};

use evalhub_schema::RecordKind;
use evalhub_schema::error::{ErrorCode, ErrorEntry};

use crate::eval::{EvalSchema, declared_schema};

/// Largest integer a JSON number may carry without losing precision as a
/// double: 2^53.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_992.0;

/// Longest `run_id` accepted, in bytes of UTF-8.
pub const MAX_RUN_ID_BYTES: usize = 200;

/// Where a relation points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationTarget {
    /// A version on this hub, `{ns}/{name}@{seq}`.
    Version {
        /// Namespace of the target record.
        ns: String,
        /// Name of the target record.
        name: String,
        /// Sequence number of the target version, from 1.
        seq: u32,
    },
    /// Something outside the hub, written `external:<anything>`.
    External(String),
    /// A Hugging Face reference, written `hf:<org>/<repo>@<sha>`.
    Hf(String),
}

/// Whether `s` has the form `{ns}/{name}`: exactly one `/`, both parts
/// non-empty, each starting with `[a-z0-9]` and continuing with
/// `[a-z0-9._-]`.
pub fn is_valid_id(s: &str) -> bool {
    let Some((ns, name)) = s.split_once('/') else {
        return false;
    };
    is_slug(ns) && is_slug(name)
}

/// Whether `s` is a valid `run_id`: non-empty, no `/`, at most
/// [`MAX_RUN_ID_BYTES`] bytes of UTF-8. Nothing else is imposed (see
/// `evalhub_schema::run`); every `run_id` a 0.1.x producer wrote passes.
pub fn is_valid_run_id(s: &str) -> bool {
    !s.is_empty() && !s.contains('/') && s.len() <= MAX_RUN_ID_BYTES
}

fn is_slug(part: &str) -> bool {
    let mut chars = part.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// Parse a `relations[].to` value. `None` when it has none of the accepted
/// forms.
pub fn parse_relation_target(s: &str) -> Option<RelationTarget> {
    if let Some(rest) = s.strip_prefix("external:") {
        return (!rest.is_empty()).then(|| RelationTarget::External(rest.to_string()));
    }
    if let Some(rest) = s.strip_prefix("hf:") {
        return (!rest.is_empty()).then(|| RelationTarget::Hf(rest.to_string()));
    }
    let (id, seq) = s.rsplit_once('@')?;
    let (ns, name) = id.split_once('/')?;
    if !is_slug(ns) || !is_slug(name) {
        return None;
    }
    // `parse::<u32>` accepts a leading `+`; the address form does not.
    if seq.is_empty() || !seq.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seq: u32 = seq.parse().ok()?;
    if seq == 0 {
        return None;
    }
    Some(RelationTarget::Version {
        ns: ns.to_string(),
        name: name.to_string(),
        seq,
    })
}

/// Validate a record of `kind` and return every failure, sorted by
/// `(path, code)`; empty when the record is accepted.
///
/// The JSON Schema is the one for the identifier the body declares in
/// `schema` (see the module doc). An `evalhub.eval/2.0` body with a `runs`
/// key returns exactly one error, `runs_moved` at `/runs`, and nothing
/// else. Cost: one pass of the compiled validator plus one walk of the
/// value; the validator is compiled on the first call for its identifier.
pub fn validate(kind: RecordKind, value: &Value) -> Vec<ErrorEntry> {
    let eval_schema = match kind {
        RecordKind::Eval => declared_schema(value),
        RecordKind::Card => None,
    };
    if eval_schema == Some(EvalSchema::V2) && value.get("runs").is_some() {
        return vec![runs_moved()];
    }
    let declared = value.get("schema").and_then(Value::as_str);
    let mut errors = Vec::new();
    structural(record_validator(kind, declared), value, &mut errors);
    numbers("", value, &mut errors);
    semantic(kind, eval_schema, value, &mut errors);
    sort(&mut errors);
    errors
}

/// Validate one run of an Eval, written under `run_id` (the path segment
/// of `PUT /evals/{ns}/{name}/runs/{run_id}`, or the element's own
/// `run_id` in a batch). Returns every failure, sorted by `(path, code)`;
/// empty when the run is accepted.
///
/// Both passes run: `schemas/run.json`, the ±2^53 check, then the run
/// rules in the module doc. `run_id_invalid` and `run_id_mismatch` are
/// reported at `/run_id`. The run's pointers are checked against its own
/// `attachments[]` only. Cost: as for [`validate`].
pub fn run(run_id: &str, value: &Value) -> Vec<ErrorEntry> {
    let mut errors = Vec::new();
    structural(run_validator(), value, &mut errors);
    numbers("", value, &mut errors);
    run_semantic(run_id, value, &mut errors);
    sort(&mut errors);
    errors
}

/// The one error a 2.0 header carrying `runs` gets.
fn runs_moved() -> ErrorEntry {
    ErrorEntry {
        path: "/runs".to_string(),
        code: ErrorCode::RunsMoved,
        hint: Some(
            "an evalhub.eval/2.0 header does not carry runs; write each run with \
             PUT /evals/{ns}/{name}/runs/{run_id}, or several with \
             POST /evals/{ns}/{name}/runs:batch"
                .to_string(),
        ),
    }
}

fn sort(errors: &mut [ErrorEntry]) {
    // `ErrorCode` is a plain enum; its declaration order is the tie-break.
    errors.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| (a.code as u8).cmp(&(b.code as u8)))
    });
}

/// Compile a generated schema. The generated schemas only reference
/// `#/$defs/*`; `offline()` turns any other reference into a build failure
/// here, which the unit tests exercise, rather than a fetch in production.
fn compile(schema: &Value) -> Validator {
    #[allow(clippy::expect_used)]
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .offline()
        .build(schema)
        .expect("the generated schema compiles")
}

/// One compiled validator per accepted identifier of every kind, keyed by
/// the identifier. Two Card identifiers share a document and are compiled
/// twice; that is two compilations per process, and it keeps the cache a
/// plain map from what the body says to what checks it.
fn record_validators() -> &'static HashMap<&'static str, Validator> {
    static CACHE: OnceLock<HashMap<&'static str, Validator>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut map = HashMap::new();
        for kind in [RecordKind::Card, RecordKind::Eval] {
            for &id in kind.schema_ids() {
                if let Some(schema) = kind.schema_for(id) {
                    map.insert(id, compile(schema.as_value()));
                }
            }
        }
        map
    })
}

/// The validator for a record of `kind` declaring `declared`: the one for
/// that identifier when `kind` accepts it, otherwise the kind's current one.
fn record_validator(kind: RecordKind, declared: Option<&str>) -> &'static Validator {
    let cache = record_validators();
    let accepted = declared.filter(|id| kind.schema_ids().contains(id));
    let id = accepted.unwrap_or(kind.schema_id());
    // Every accepted identifier has a document (a unit test asserts it);
    // a miss here is a bug in `evalhub_schema`, not a property of the input.
    #[allow(clippy::expect_used)]
    cache
        .get(id)
        .expect("every accepted schema identifier has a document")
}

fn run_validator() -> &'static Validator {
    static RUN: OnceLock<Validator> = OnceLock::new();
    RUN.get_or_init(|| compile(evalhub_schema::schema_for_run().as_value()))
}

fn structural(validator: &Validator, value: &Value, out: &mut Vec<ErrorEntry>) {
    for e in validator.iter_errors(value) {
        out.push(ErrorEntry {
            path: e.instance_path().as_str().to_string(),
            code: ErrorCode::Schema,
            hint: Some(e.to_string()),
        });
    }
}

fn numbers(path: &str, value: &Value, out: &mut Vec<ErrorEntry>) {
    match value {
        Value::Number(n) => {
            let too_large = if let Some(i) = n.as_i64() {
                i.unsigned_abs() > MAX_SAFE_INTEGER as u64
            } else if let Some(u) = n.as_u64() {
                u > MAX_SAFE_INTEGER as u64
            } else if let Some(f) = n.as_f64() {
                f.is_finite() && f.fract() == 0.0 && f.abs() > MAX_SAFE_INTEGER
            } else {
                false
            };
            if too_large {
                out.push(ErrorEntry {
                    path: path.to_string(),
                    code: ErrorCode::NumberTooLarge,
                    hint: Some(format!(
                        "{n} is an integer outside ±2^53; send it as a string"
                    )),
                });
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                numbers(&format!("{path}/{i}"), item, out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                numbers(&format!("{path}/{}", escape_pointer(k)), v, out);
            }
        }
        _ => {}
    }
}

/// RFC 6901 escaping for one reference token.
fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn push(out: &mut Vec<ErrorEntry>, path: String, code: ErrorCode, hint: String) {
    out.push(ErrorEntry {
        path,
        code,
        hint: Some(hint),
    });
}

/// The `attachments[].path` set of one object (a record or a run), after
/// reporting every path that is empty, absolute, contains `..`, or repeats
/// an earlier one. The set holds the first occurrence of each valid path.
fn attachment_paths<'a>(
    obj: &'a Map<String, Value>,
    out: &mut Vec<ErrorEntry>,
) -> HashSet<&'a str> {
    let mut paths: HashSet<&str> = HashSet::new();
    let Some(attachments) = obj.get("attachments").and_then(Value::as_array) else {
        return paths;
    };
    for (i, a) in attachments.iter().enumerate() {
        let Some(p) = a.get("path").and_then(Value::as_str) else {
            continue;
        };
        let invalid = if p.is_empty() {
            Some("empty")
        } else if p.starts_with('/') {
            Some("absolute")
        } else if p.split('/').any(|seg| seg == "..") {
            Some("contains `..`")
        } else if !paths.insert(p) {
            Some("duplicate")
        } else {
            None
        };
        if let Some(why) = invalid {
            push(
                out,
                format!("/attachments/{i}/path"),
                ErrorCode::AttachmentPathInvalid,
                format!("attachment path {p:?} is {why}"),
            );
        }
    }
    paths
}

/// `attachment_ref_unknown` at `pointer` unless `p` is in `paths`.
fn ref_known(paths: &HashSet<&str>, out: &mut Vec<ErrorEntry>, pointer: String, p: &str) {
    if !paths.contains(p) {
        push(
            out,
            pointer,
            ErrorCode::AttachmentRefUnknown,
            format!("{p:?} names no attachments[].path"),
        );
    }
}

/// `metric_id_invalid` at `pointer` unless `m` is `{ns}/{name}`.
fn metric_id(out: &mut Vec<ErrorEntry>, pointer: String, m: &str) {
    if !is_valid_id(m) {
        push(
            out,
            pointer,
            ErrorCode::MetricIdInvalid,
            format!("{m:?} is not of the form {{ns}}/{{name}}"),
        );
    }
}

/// Whether a key is absent or `null`: both mean "not given" for the
/// pairings checked here.
fn given(obj: &Map<String, Value>, key: &str) -> bool {
    obj.get(key).is_some_and(|v| !v.is_null())
}

fn semantic(
    kind: RecordKind,
    eval_schema: Option<EvalSchema>,
    value: &Value,
    out: &mut Vec<ErrorEntry>,
) {
    let Some(obj) = value.as_object() else {
        return;
    };

    let paths = attachment_paths(obj, out);

    if kind == RecordKind::Card {
        let results = obj.get("results").and_then(Value::as_array);
        if let Some(results) = results {
            for (i, r) in results.iter().enumerate() {
                if let Some(m) = r.get("metric").and_then(Value::as_str) {
                    metric_id(out, format!("/results/{i}/metric"), m);
                }
                if let Some(s) = r.get("samples_ref").and_then(Value::as_str) {
                    ref_known(&paths, out, format!("/results/{i}/samples_ref"), s);
                }
            }
            let counts = obj.get("counts").filter(|c| !c.is_null());
            match counts {
                None if !results.is_empty() => push(
                    out,
                    "/counts".to_string(),
                    ErrorCode::CountsMissing,
                    "results are present, so counts is required".to_string(),
                ),
                Some(c) => {
                    let n = |k: &str| c.get(k).and_then(Value::as_u64).unwrap_or(0);
                    let sum = n("completed") + n("failed") + n("skipped") + n("errored");
                    if let Some(attempted) = c.get("attempted").and_then(Value::as_u64)
                        && attempted < sum
                    {
                        push(
                            out,
                            "/counts".to_string(),
                            ErrorCode::CountsInconsistent,
                            format!(
                                "attempted ({attempted}) < completed + failed + skipped + errored ({sum})"
                            ),
                        );
                    }
                }
                None => {}
            }
        }
        run_results(obj, out);
    }

    // The 1.0 arm only: runs in the body point into the body's attachments.
    if eval_schema == Some(EvalSchema::V1)
        && let Some(runs) = obj.get("runs").and_then(Value::as_array)
    {
        for (i, run) in runs.iter().enumerate() {
            if let Some(c) = run.get("calls").and_then(Value::as_str) {
                ref_known(&paths, out, format!("/runs/{i}/calls"), c);
            }
            if let Some(artifacts) = run.get("artifacts").and_then(Value::as_array) {
                for (j, a) in artifacts.iter().enumerate() {
                    if let Some(a) = a.as_str() {
                        ref_known(&paths, out, format!("/runs/{i}/artifacts/{j}"), a);
                    }
                }
            }
        }
    }

    if let Some(relations) = obj.get("relations").and_then(Value::as_array) {
        for (i, rel) in relations.iter().enumerate() {
            if let Some(t) = rel.get("type").and_then(Value::as_str)
                && !is_valid_id(t)
            {
                push(
                    out,
                    format!("/relations/{i}/type"),
                    ErrorCode::Schema,
                    format!("{t:?} is not a registry id of the form {{ns}}/{{name}}"),
                );
            }
            if let Some(to) = rel.get("to").and_then(Value::as_str)
                && parse_relation_target(to).is_none()
            {
                push(
                    out,
                    format!("/relations/{i}/to"),
                    ErrorCode::Schema,
                    format!(
                        "{to:?} is not `{{ns}}/{{name}}@{{seq}}`, `external:<ref>` or `hf:<ref>`"
                    ),
                );
            }
        }
    }
}

/// The `{ns}/{name}` of every hub target of the Card's `core/uses_eval`
/// relations, without the `@{seq}`.
fn used_eval_records(obj: &Map<String, Value>) -> HashSet<String> {
    let Some(relations) = obj.get("relations").and_then(Value::as_array) else {
        return HashSet::new();
    };
    relations
        .iter()
        .filter(|rel| rel.get("type").and_then(Value::as_str) == Some("core/uses_eval"))
        .filter_map(|rel| rel.get("to").and_then(Value::as_str))
        .filter_map(|to| match parse_relation_target(to)? {
            RelationTarget::Version { ns, name, .. } => Some(format!("{ns}/{name}")),
            RelationTarget::External(_) | RelationTarget::Hf(_) => None,
        })
        .collect()
}

/// The Card's `run_results[]` rules that need nothing but the Card.
fn run_results(obj: &Map<String, Value>, out: &mut Vec<ErrorEntry>) {
    let Some(items) = obj.get("run_results").and_then(Value::as_array) else {
        return;
    };
    let evals = used_eval_records(obj);
    for (i, item) in items.iter().enumerate() {
        let Some(item) = item.as_object() else {
            continue;
        };
        if let Some(m) = item.get("metric").and_then(Value::as_str) {
            metric_id(out, format!("/run_results/{i}/metric"), m);
        }
        if !given(item, "value") && !given(item, "label") {
            push(
                out,
                format!("/run_results/{i}"),
                ErrorCode::RunResultValueOrLabel,
                "a run result needs a value, a label, or both".to_string(),
            );
        }
        if let Some(e) = item.get("eval").and_then(Value::as_str)
            && !evals.contains(e)
        {
            push(
                out,
                format!("/run_results/{i}/eval"),
                ErrorCode::RunResultsEvalUnknown,
                format!(
                    "{e:?} is not the {{ns}}/{{name}} (without @seq) of any core/uses_eval target of this Card"
                ),
            );
        }
    }
}

fn run_semantic(run_id: &str, value: &Value, out: &mut Vec<ErrorEntry>) {
    if !is_valid_run_id(run_id) {
        push(
            out,
            "/run_id".to_string(),
            ErrorCode::RunIdInvalid,
            format!(
                "run_id {run_id:?} must be non-empty, contain no `/`, and be at most {MAX_RUN_ID_BYTES} bytes"
            ),
        );
    }
    let Some(obj) = value.as_object() else {
        return;
    };
    if let Some(body_id) = obj.get("run_id").and_then(Value::as_str)
        && body_id != run_id
    {
        push(
            out,
            "/run_id".to_string(),
            ErrorCode::RunIdMismatch,
            format!("the body says run_id {body_id:?}, but it is written as {run_id:?}"),
        );
    }

    // status ⇔ detail. Only a recognised status is paired; an unknown one
    // is already a `schema` error.
    let status = obj.get("status").and_then(Value::as_str);
    let expects = match status {
        Some("ok") => Some((false, false)),
        Some("error") => Some((true, false)),
        Some("skipped") => Some((false, true)),
        _ => None,
    };
    if let (Some(status), Some((wants_error, wants_skip))) = (status, expects) {
        for (key, wanted) in [("error", wants_error), ("skip_reason", wants_skip)] {
            let has = given(obj, key);
            if has != wanted {
                let hint = if wanted {
                    format!("status {status:?} requires {key}")
                } else {
                    format!(
                        "{key} is only allowed when status is {:?}",
                        if key == "error" { "error" } else { "skipped" }
                    )
                };
                push(out, format!("/{key}"), ErrorCode::RunStatusDetail, hint);
            }
        }
    }

    let paths = attachment_paths(obj, out);
    if let Some(c) = obj.get("calls").and_then(Value::as_str) {
        ref_known(&paths, out, "/calls".to_string(), c);
    }
    if let Some(artifacts) = obj.get("artifacts").and_then(Value::as_array) {
        for (j, a) in artifacts.iter().enumerate() {
            if let Some(a) = a.as_str() {
                ref_known(&paths, out, format!("/artifacts/{j}"), a);
            }
        }
    }
    if let Some(log) = obj
        .get("error")
        .and_then(|e| e.get("log"))
        .and_then(Value::as_str)
    {
        ref_known(&paths, out, "/error/log".to_string(), log);
    }

    if let Some(metrics) = obj.get("metrics").and_then(Value::as_object) {
        for key in metrics.keys() {
            metric_id(out, format!("/metrics/{}", escape_pointer(key)), key);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn ids() {
        assert!(is_valid_id("core/pass_rate"));
        assert!(is_valid_id("alice/qwen-loop.v2"));
        assert!(!is_valid_id("pass_rate"));
        assert!(!is_valid_id("Core/pass_rate"));
        assert!(!is_valid_id("core/"));
        assert!(!is_valid_id("/x"));
        assert!(!is_valid_id("a/b/c"));
        assert!(!is_valid_id("-a/b"));
    }

    #[test]
    fn run_ids() {
        assert!(is_valid_run_id("r1"));
        assert!(is_valid_run_id("task 7, trial 2"));
        assert!(is_valid_run_id(&"x".repeat(MAX_RUN_ID_BYTES)));
        assert!(!is_valid_run_id(""));
        assert!(!is_valid_run_id("a/b"));
        assert!(!is_valid_run_id(&"x".repeat(MAX_RUN_ID_BYTES + 1)));
        // Bytes, not characters: 67 three-byte characters are 201 bytes.
        assert!(!is_valid_run_id(&"あ".repeat(67)));
        assert!(is_valid_run_id(&"あ".repeat(66)));
    }

    #[test]
    fn relation_targets() {
        assert_eq!(
            parse_relation_target("alice/single2-k4@3"),
            Some(RelationTarget::Version {
                ns: "alice".into(),
                name: "single2-k4".into(),
                seq: 3
            })
        );
        assert_eq!(
            parse_relation_target("external:https://x/y"),
            Some(RelationTarget::External("https://x/y".into()))
        );
        assert_eq!(
            parse_relation_target("hf:org/repo@sha"),
            Some(RelationTarget::Hf("org/repo@sha".into()))
        );
        for bad in [
            "alice/x",
            "alice/x@0",
            "alice/x@+1",
            "alice/x@label",
            "alice@3",
            "external:",
            "hf:",
            "",
        ] {
            assert_eq!(parse_relation_target(bad), None, "{bad}");
        }
    }

    #[test]
    fn validators_build_offline_one_per_accepted_identifier() {
        let cache = record_validators();
        for kind in [RecordKind::Card, RecordKind::Eval] {
            for id in kind.schema_ids() {
                assert!(cache.contains_key(id), "{id}");
            }
        }
        assert_eq!(
            cache.len(),
            RecordKind::Card.schema_ids().len() + RecordKind::Eval.schema_ids().len()
        );
        let _ = run_validator();
    }

    #[test]
    fn pointer_escaping() {
        let v = serde_json::json!({"ext": {"a/b": {"n": 9007199254740993u64}}});
        let mut out = Vec::new();
        numbers("", &v, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "/ext/a~1b/n");
        assert_eq!(out[0].code, ErrorCode::NumberTooLarge);
    }
}
