//! Identifiers the hub assigns.
//!
//! Two kinds, both ULIDs, both minted by the hub on `POST` and never by the
//! client:
//!
//! - `id` — identifies a named record (`{type, ns, name}`) for its lifetime.
//!   Stable across versions, renames (not in v0) and visibility changes.
//! - `version_id` — identifies one immutable version. Relations point at
//!   these; tombstones keep them.
//!
//! ULID rather than UUIDv4 because versions are appended in time order and
//! a time-sortable id makes the `versions` table's natural order match its
//! primary key order. ULID rather than UUIDv7 because ULID's Crockford
//! base32 text form is what appears in URLs and logs, and it is shorter and
//! has no hyphens; in Postgres the value is stored in a `uuid` column (the
//! two are bit-compatible), so the database sees a native 128-bit type.
//!
//! The `(id, seq)` pair is the human-facing address of a version
//! (`{ns}/{name}@3`); `version_id` is the machine-facing one. Both are
//! returned by every write.
