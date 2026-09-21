//! Pieces shared by Card and Eval: producer, attachments, relations, `ext`.
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
//! name that other fields (`results[].samples_ref`, `runs[].calls`,
//! `runs[].artifacts[]`) point at. The hub checks that referenced paths
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
//! `version_id`. `attrs` is free-form per relation type (for `uses_eval`,
//! which runs of the Eval this Card used).
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
