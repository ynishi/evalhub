//! Named records and their versions.
//!
//! # Create or append
//!
//! `POST /{cards|evals}/{ns}/{name}` always means "append a version". If
//! the name does not exist it is created in the same transaction (the
//! caller must hold `write` on `ns`). The sequence is:
//!
//! ```text
//! 1. lock the record row (or insert it)                    SELECT ... FOR UPDATE
//! 2. read the latest non-tombstoned version's content_hash
//! 3. equal to the new hash?  → return it, 200, done        (idempotent)
//! 4. seq = max(seq) + 1
//! 5. changed[] = top-level keys whose value differs from the previous version
//! 6. insert versions, fingerprints, results, relations, attachment_refs, audit
//! 7. commit → 201 { id, version_id, seq, label, content_hash, changed[], badges[] }
//! ```
//!
//! The row lock in step 1 is what makes `seq` gap-free under concurrent
//! posts to the same name. Different names do not contend.
//!
//! # Addressing
//!
//! A version is addressed as `@{seq}` or `@{label}`. `seq` is the hub's;
//! `label` is a client-chosen slug, unique within the name, re-pointable
//! with `PATCH .../label`, and **never purely numeric** so it cannot be
//! mistaken for a `seq`. No address is derived from `content_hash`.
//!
//! # `changed[]`
//!
//! Each version records which top-level keys differ from the version
//! before it (`results`, `counts`, `model`, …). It costs one comparison at
//! write time and lets the version list show at a glance that "version 4
//! changed the results" without diffing bodies. It is kept on tombstones.
//!
//! # Visibility
//!
//! `records.visibility` is `private` or `public`, set per name with
//! `PATCH .../settings`. Every read path in this module takes the caller's
//! namespaces and filters on them, so a private record is indistinguishable
//! from a nonexistent one (`404`) to anyone without access. There is no
//! query that returns a private record's existence.
