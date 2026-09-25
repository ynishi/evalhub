//! Pieces shared by Card, Eval and Run: producer, attachments, relations, `ext`.
//!
//! # Producer
//!
//! `producer { name, version }` identifies the software that wrote the
//! record. It is informational and is not a facet: two harnesses can produce
//! comparable records, and one harness can produce incomparable ones.
//!
//! # Attachments
//!
//! ```text
//! attachments[] { path, sha256, size, media_type }
//! ```
//!
//! An attachment is a file in object storage, addressed by its sha256. The
//! record refers to it by `path`, a relative, `..`-free, unique-within-record
//! name that other fields (a Card's `results[].samples_ref`, a run's
//! `calls`, `artifacts[]` and `error.log`) point at. A run has its own
//! `attachments[]` and its pointers name paths there, not in the Eval
//! header's (see `crate::run`). The hub checks that referenced paths
//! exist in `attachments[]` and that every `sha256` has been uploaded and
//! confirmed (see `evalhub_store::objects`); it does not open the file.
//! `media_type` is a hint for display and download, nothing more.
//!
//! Content addressing means two records attaching the same file share one
//! object, and a record cannot be published against an object that does not
//! exist. It also means the record commits to the exact bytes: a different
//! file is a different sha256 and therefore a different record.
//!
//! # Relations
//!
//! ```text
//! relations[] { type, to, attrs }
//! ```
//!
//! `type` is a registry id (`core/uses_eval`, `core/retry_of`, …). `to` is a
//! pinned reference, `{ns}/{name}@{seq}`, which the hub resolves to a
//! `version_id` on ingest; an unresolvable reference is accepted and simply
//! does not earn the `refs_resolved` badge. Targets outside the hub are
//! written `external:https://…` or `hf:org/repo@sha` and stored with no
//! `version_id`. `attrs` is free-form per relation type.
//!
//! For `core/uses_eval`, `attrs.runs` is a list of `run_id`s: the runs of
//! the cited Eval this Card used. It feeds the Card's *used set*: per Eval
//! record, the union of `attrs.runs` over the Card's `core/uses_eval`
//! relations resolved to that record, or, when none of them carries
//! `attrs.runs`, every run of that record that is neither archived nor
//! deleted when the Card is posted. The used set bounds the Card's
//! `run_results` and is defined in full in `crate::card` ("The used set").
//! `attrs.runs` names runs of the *record*, not of the pinned version:
//! runs are not versioned with the Eval header.
//!
//! Relations are edges between *versions*. Publishing a new version of a
//! Card does not move its edges; a reader who wants "the latest Card that
//! used this Eval" asks the API with `follow_latest=true`.
//!
//! # `ext`
//!
//! The one open door. `ext` is a map from namespace (`{ns}/{name}`) to an
//! arbitrary JSON object. Namespacing is mandatory so that two producers'
//! extensions cannot collide, and so that a namespace can later register an
//! `ext_schema` to make its keys typed and indexable. Until it does, `ext`
//! values can be searched with `eq` and `exists` only.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Producer-private extensions, keyed by namespace (`{ns}/{name}`). Each value
/// is an arbitrary JSON object owned by that namespace.
pub type Ext = BTreeMap<String, serde_json::Value>;

/// Free-form attributes carried by a relation, keyed by attribute name.
pub type Attrs = serde_json::Map<String, serde_json::Value>;

/// True when an extension map is empty; used to omit it on output.
pub(crate) fn ext_is_empty(ext: &Ext) -> bool {
    ext.is_empty()
}

/// The software that wrote the record. Informational; not a facet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Producer {
    /// Name of the producing software (a harness, a converter, a script).
    pub name: String,
    /// Version of the producing software.
    pub version: String,
}

/// A file in object storage that the record refers to by `path`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    /// Relative, `..`-free name, unique within the record; other fields point at it.
    pub path: String,
    /// Lower-case hex sha256 of the file's bytes; the object's address in storage.
    pub sha256: String,
    /// Size of the file in bytes.
    pub size: u64,
    /// Media type of the file, for display and download only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

/// An edge from this version to another record version or an external target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    /// Relation type as a registry id, `{ns}/{name}` (for example `core/uses_eval`).
    #[serde(rename = "type")]
    pub relation_type: String,
    /// Target: `{ns}/{name}@{seq}` on the hub, or `external:<url>` / `hf:<org>/<repo>@<sha>` outside it.
    pub to: String,
    /// Attributes specific to the relation type (for `uses_eval`, which runs were used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Attrs>,
}

/// The producer's statement about what it removed before publishing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Redaction {
    /// Whether any redaction was applied.
    pub applied: bool,
    /// How it was applied (for example `regex+llm`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Which fields were redacted, as `attachment path:field` or a record path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
}
