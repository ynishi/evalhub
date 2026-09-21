//! Relations: edges between versions, their resolution, traversal, and the
//! comparison view.
//!
//! # Storage
//!
//! `relations (from_version_id, type, to_version_id, to_external, attrs)`.
//! Edges are written from `relations[]` on ingest and can be added later
//! with `POST .../{name}@{seq}/relations`. `to_version_id` is the resolved
//! target, or `NULL` with `to_external` set for targets outside the hub.
//! `type` is a registry id; the inverse name is looked up from the registry
//! at read time, not stored.
//!
//! # Resolution
//!
//! `{ns}/{name}@{seq}` resolves to a `version_id` at ingest. A reference to
//! a name that does not exist yet, or a version that does not exist yet, is
//! stored unresolved (`to_version_id NULL`, `to_external` holding the
//! textual reference). The traversal API re-attempts resolution lazily on
//! read, so a Card published before its Eval links up once the Eval exists,
//! while the `refs_resolved` badge stays as recorded (a fact about ingest).
//!
//! # Traversal
//!
//! `GET .../{name}/relations?direction=out|in|both&types=&depth=&follow_latest=&version=`
//! walks the graph from a version. Edges attach to versions, so by default
//! a new version of a Card has no edges; `follow_latest=true` reports
//! edges by *name*, mapping each endpoint to the latest non-tombstoned
//! version of its record. Private endpoints the caller cannot see are
//! reported as `{ private: true, version_id, content_hash }` — a
//! commitment, not a leak.
//!
//! # The comparison view
//!
//! `GET /evals/{ns}/{name}/cards?version=&group_by=fingerprint.{facet}`
//! lists every Card whose `uses_eval` edge points at the given Eval
//! version, grouped by a facet fingerprint. Each Card is annotated with
//! `same_harness` / `same_model` when its harness / model fingerprint equals
//! the Eval's. That is the whole of the hub's comparison logic: it lines
//! records up and labels agreement on axes; it does not rank, average, or
//! declare a winner.
