//! Attachment handlers: presigned upload, completion, presigned download.
//!
//! The two-step upload and the `ready` gate are described in
//! `evalhub_store::objects`. `GET /attachments/{sha256}` answers `302` to a
//! presigned URL and is authorised against the records that reference the
//! object: if any referencing record is public, anyone may download; else
//! the caller needs access to at least one of them.
