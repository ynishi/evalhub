//! Per-facet fingerprints: the hub's only notion of "measured under the same
//! conditions".
//!
//! ```text
//! fingerprint[facet] = sha256( canonical( facet - ext - {keys with x-fingerprint: false} ) )
//! ```
//!
//! Seven of them, one per facet (model, task, harness, generation, trial,
//! grading, env), computed on ingest and stored in the `fingerprints` table.
//! Records with equal fingerprints on a facet agree on every core key of
//! that facet. That is all a fingerprint claims. The hub never combines them
//! into a single "same evaluation" verdict; the reader chooses which axes
//! must match for the comparison they are making, and the query language
//! lets them filter on `fingerprint.{facet}` directly.
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
//! start-up. There is no hand-maintained list in this crate; if one existed
//! it would drift from the schema, and a drift here changes every
//! fingerprint in the index.
//!
//! # Absent versus null
//!
//! Both are preserved. A facet with `"seed": null` (recorded, unknown) and
//! one with no `seed` key (not recorded) fingerprint differently. This is
//! intentional: a producer that states "I do not know the seed" is making a
//! different claim from one that did not think to record it.
//!
//! # Opting an extension in
//!
//! A namespace that registers an `ext_schema` with `fingerprint: true`
//! asks for its extension keys to participate. From the moment the registry
//! entry is `applied`, those keys are typed, indexed, and folded into the
//! facet's fingerprint for new versions. Existing versions are not
//! re-fingerprinted (a fingerprint is a fact about ingest time), which is
//! why the `fingerprints` table records the registry version it was computed
//! under.
//!
//! # Tests
//!
//! Fixtures pair a record with its seven expected fingerprints. A property
//! test asserts that changing any excluded key leaves the fingerprint
//! unchanged and changing any core key changes it.
