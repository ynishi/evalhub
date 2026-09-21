//! Attachment lifecycle: presign, confirm, reference count, garbage collect.
//!
//! ```text
//!  client                         hub                              object store
//!    │  POST /attachments {sha256,size,media_type}                     │
//!    │────────────────────────────▶│  exists & ready? → 200, done       │
//!    │                             │  else insert (pending), presign PUT │
//!    │◀──── { upload_url } ────────│                                    │
//!    │  PUT bytes ─────────────────┼───────────────────────────────────▶│
//!    │  POST /attachments/{sha256}/complete                            │
//!    │────────────────────────────▶│  HEAD object; size matches? ──────▶│
//!    │                             │  (optionally stream & hash)        │
//!    │◀──── 200 (state = ready) ───│                                    │
//! ```
//!
//! Objects are keyed by sha256, so the same file attached to two records is
//! stored once, and a record can only be posted against objects that are
//! `ready` (`409 attachment_missing` otherwise). The hub never receives the
//! bytes on the request path: upload and download are presigned URLs
//! (`GET /attachments/{sha256}` answers `302`), which is what lets the server
//! stay stateless and lets storage with free egress do the serving.
//!
//! # Verification on `complete`
//!
//! `complete` checks the object exists and its size matches the declared
//! size. Hashing the object to confirm the sha256 requires streaming it
//! through the hub once; v0 does this for objects below a configured size
//! and trusts the client's hash above it, recording which. The hub does not
//! look inside the file in either case — no row counts, no JSONL schema.
//!
//! # Reference counting and GC
//!
//! `attachment_refs (version_id, sha256, path)` is written in the record's
//! transaction. A tombstone deletes the version's refs; the GC job (see
//! `evalhub_server::jobs`) deletes objects with zero refs older than a
//! grace period, so an upload that is `ready` but not yet referenced is not
//! collected under the client. `pending` objects older than the grace
//! period are also collected.
//!
//! # Private records
//!
//! A presigned GET for an attachment of a private record is issued only to
//! a caller with access to that record. The URL itself is time-limited;
//! the hub does not try to revoke it.
//!
//! # Backend
//!
//! `object_store` with the `aws` feature, configured with a custom endpoint
//! and path-style addressing so MinIO, R2 and Tigris all work; presigning
//! is `object_store::signer::Signer`. The full AWS SDK was rejected as
//! dependency weight for a service that only ever signs URLs.
