//! Validation: everything the hub checks before it stores a record.
//!
//! Validation runs in two passes and collects every failure from both into
//! one list, so a producer sees all problems in one round trip.
//!
//! # Pass 1: structural
//!
//! The record is checked against the JSON Schema for its declared `schema`
//! (`evalhub.card/1.0` or `evalhub.eval/1.0`) using `jsonschema` with draft
//! 2020-12. The core is closed, so an unknown key anywhere outside `ext` is
//! a `schema` error. In the same pass every number is inspected: an integer
//! outside ±2^53 is `number_too_large`.
//!
//! The schema is the generated one from `evalhub_schema` (the same
//! generator output that is committed under `schemas/`), built once per
//! process. The validator is built in offline mode: every `$ref` in the
//! schema is local (`#/$defs/…`), and a reference that would need fetching
//! is a build error, not a fetch.
//!
//! # Pass 2: semantic
//!
//! Rules the schema language cannot express:
//!
//! | Rule                                                                                   | Code                       |
//! | -------------------------------------------------------------------------------------- | -------------------------- |
//! | `results` non-empty ⇒ `counts` present                                                 | `counts_missing`           |
//! | `counts.attempted >= completed + failed + skipped + errored`                           | `counts_inconsistent`      |
//! | every `results[].samples_ref`, `runs[].calls`, `runs[].artifacts[]` is an `attachments[].path` | `attachment_ref_unknown` |
//! | `attachments[].path` unique, relative, contains no `..`                                | `attachment_path_invalid`  |
//! | `results[].metric` matches `{ns}/{name}`                                               | `metric_id_invalid`        |
//! | `relations[].type` matches `{ns}/{name}`                                               | `schema`                   |
//! | `relations[].to` is `{ns}/{name}@{seq}`, `external:…`, or `hf:…`                       | `schema`                   |
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
//! - A `harness` or `metric` the registry does not know. Accepted; the
//!   `harness_registered` / `metric_registered` badges are withheld. The hub
//!   does not gate on vocabulary.
//! - An `attachments[].sha256` that has not been uploaded. This is not a
//!   validation failure but a state conflict, `409 attachment_missing`,
//!   raised by the store, because only the store knows.
//!
//! # Output
//!
//! `Vec<Error { path, code, hint }>`, empty on success. `path` is a JSON
//! pointer into the submitted record (`/results/0/metric`; the root is
//! `""`). `hint` is prose and not part of the contract.

use std::collections::HashSet;
use std::sync::OnceLock;

use jsonschema::{Draft, Validator};
use serde_json::Value;

use evalhub_schema::RecordKind;
use evalhub_schema::error::{ErrorCode, ErrorEntry};

/// Largest integer a JSON number may carry without losing precision as a
/// double: 2^53.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_992.0;

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

/// Run both passes and return every failure, sorted by `(path, code)`.
pub fn validate(kind: RecordKind, value: &Value) -> Vec<ErrorEntry> {
    let mut errors = Vec::new();
    structural(kind, value, &mut errors);
    numbers("", value, &mut errors);
    semantic(kind, value, &mut errors);
    // `ErrorCode` is a plain enum; its declaration order is the tie-break.
    errors.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| (a.code as u8).cmp(&(b.code as u8)))
    });
    errors
}

fn validator(kind: RecordKind) -> &'static Validator {
    static CARD: OnceLock<Validator> = OnceLock::new();
    static EVAL: OnceLock<Validator> = OnceLock::new();
    let cell = match kind {
        RecordKind::Card => &CARD,
        RecordKind::Eval => &EVAL,
    };
    cell.get_or_init(|| {
        let schema = kind.schema().to_value();
        // The generated schema only references `#/$defs/*`; `offline()`
        // turns any other reference into a build failure here, which the
        // unit tests exercise, rather than a fetch in production.
        #[allow(clippy::expect_used)]
        jsonschema::options()
            .with_draft(Draft::Draft202012)
            .offline()
            .build(&schema)
            .expect("the generated record schema compiles")
    })
}

fn structural(kind: RecordKind, value: &Value, out: &mut Vec<ErrorEntry>) {
    for e in validator(kind).iter_errors(value) {
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

fn semantic(kind: RecordKind, value: &Value, out: &mut Vec<ErrorEntry>) {
    let Some(obj) = value.as_object() else {
        return;
    };
    let push = |out: &mut Vec<ErrorEntry>, path: String, code: ErrorCode, hint: String| {
        out.push(ErrorEntry {
            path,
            code,
            hint: Some(hint),
        })
    };

    // Attachment paths: unique, relative, no `..`.
    let mut paths: HashSet<&str> = HashSet::new();
    if let Some(attachments) = obj.get("attachments").and_then(Value::as_array) {
        for (i, a) in attachments.iter().enumerate() {
            let Some(p) = a.get("path").and_then(Value::as_str) else {
                continue;
            };
            let pointer = format!("/attachments/{i}/path");
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
                    pointer,
                    ErrorCode::AttachmentPathInvalid,
                    format!("attachment path {p:?} is {why}"),
                );
            }
        }
    }
    let ref_unknown = |out: &mut Vec<ErrorEntry>, pointer: String, p: &str| {
        if !paths.contains(p) {
            push(
                out,
                pointer,
                ErrorCode::AttachmentRefUnknown,
                format!("{p:?} names no attachments[].path"),
            );
        }
    };

    if kind == RecordKind::Card {
        let results = obj.get("results").and_then(Value::as_array);
        if let Some(results) = results {
            for (i, r) in results.iter().enumerate() {
                if let Some(m) = r.get("metric").and_then(Value::as_str)
                    && !is_valid_id(m)
                {
                    push(
                        out,
                        format!("/results/{i}/metric"),
                        ErrorCode::MetricIdInvalid,
                        format!("{m:?} is not of the form {{ns}}/{{name}}"),
                    );
                }
                if let Some(s) = r.get("samples_ref").and_then(Value::as_str) {
                    ref_unknown(out, format!("/results/{i}/samples_ref"), s);
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
    }

    if kind == RecordKind::Eval
        && let Some(runs) = obj.get("runs").and_then(Value::as_array)
    {
        for (i, run) in runs.iter().enumerate() {
            if let Some(c) = run.get("calls").and_then(Value::as_str) {
                ref_unknown(out, format!("/runs/{i}/calls"), c);
            }
            if let Some(artifacts) = run.get("artifacts").and_then(Value::as_array) {
                for (j, a) in artifacts.iter().enumerate() {
                    if let Some(a) = a.as_str() {
                        ref_unknown(out, format!("/runs/{i}/artifacts/{j}"), a);
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
    fn validators_build_offline() {
        let _ = validator(RecordKind::Card);
        let _ = validator(RecordKind::Eval);
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
