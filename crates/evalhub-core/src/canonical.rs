//! RFC 8785 canonical JSON and the content hash.
//!
//! The hub needs one answer to "are these two records the same?" that does
//! not depend on how the client serialised them. RFC 8785 (JSON
//! Canonicalization Scheme) gives it: object keys sorted by UTF-16 code
//! units, no insignificant whitespace, numbers printed the way ECMAScript
//! prints them (`1` not `1.0`, `1e21` not `1000000000000000000000`), strings
//! escaped minimally. Two records that differ only in key order or
//! formatting produce identical bytes.
//!
//! `content_hash = sha256(canonical_bytes)`, hex-encoded. It is stored on
//! every version and used for two things only:
//!
//! - **Idempotency.** A `POST` whose hash equals the latest version's hash
//!   returns that version with `200` instead of creating a duplicate.
//! - **Integrity.** `GET` responses carry the hash so a client can verify
//!   the body it received is the body that was stored.
//!
//! It is deliberately *not* an address. Versions are addressed by `@{seq}`
//! or `@{label}`; a hash-addressed API would let two names share a version
//! and make tombstoning ambiguous.
//!
//! # Numbers
//!
//! Canonicalisation requires parsing every number as an IEEE 754 double.
//! An integer outside ±2^53 would lose precision silently, so validation
//! rejects it first (`number_too_large`) and the producer sends it as a
//! string. Non-finite values cannot appear in JSON and are not a case.
//!
//! # Why `serde_json_canonicalizer`
//!
//! It implements RFC 8785 including the ECMAScript number formatting (via
//! `ryu-js`), which the simpler "sort keys and strip whitespace" crates do
//! not. Canonical form is a contract with every client that computes the
//! hash locally, so the number formatting has to be exactly the RFC's.
//!
//! # Tests
//!
//! Fixtures in `fixtures/canonical/` pair an input JSON with its expected
//! canonical bytes and hash, including the RFC's own test vectors. A
//! property test asserts that canonicalising a value, re-parsing it, and
//! canonicalising again is a fixed point.
