//! Record handlers: create/append, read, list, versions, label, settings,
//! tombstone.
//!
//! Response envelope for a version (what the hub adds around the client's
//! record):
//!
//! ```text
//! {
//!   "id", "version_id", "seq", "label", "content_hash", "created_at",
//!   "changed": [...], "badges": [...],
//!   "record": { ...the canonical body... },
//!   "fingerprints": { "model": "…", ... },      // expand=fingerprints
//!   "relations": [...],                          // expand=relations
//!   "tombstone": { "at", "reason", "note" }      // when tombstoned; record is absent
//! }
//! ```
//!
//! `POST` semantics — idempotent on `content_hash`, new `seq` otherwise —
//! are in `evalhub_store::records`. `?label=` on `POST` labels the new
//! version in the same transaction (`409 label_in_use` if taken).
