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

use std::fmt;
use std::str::FromStr;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Failure to produce canonical bytes.
///
/// The only way RFC 8785 serialisation of a `serde_json::Value` fails is
/// through the underlying serialiser (a map with a non-string key cannot
/// occur in a `Value`, so in practice this is unreachable for values parsed
/// from JSON text). It is surfaced rather than unwrapped because the hash of
/// a partially written buffer must never be stored.
#[derive(Debug, thiserror::Error)]
pub enum CanonicalError {
    /// The serialiser refused the value.
    #[error("canonical serialisation failed: {0}")]
    Serialize(#[source] serde_json::Error),
}

/// The sha256 of a record's canonical bytes.
///
/// Thirty-two bytes, shown as sixty-four lower-case hex characters. Used
/// for idempotency and integrity only; never as an address (see the module
/// doc).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// The raw digest, for the `bytea` column.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lower-case hex, sixty-four characters, as carried in API responses.
    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Wrap a digest already computed (for example, read back from the
    /// database). No validation beyond the length the type enforces.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_hex())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.as_hex())
    }
}

/// A hex string that is not a sha256 digest.
#[derive(Debug, thiserror::Error)]
#[error("not a sha256 hex digest: {0}")]
pub struct ParseContentHashError(String);

impl FromStr for ContentHash {
    type Err = ParseContentHashError;

    /// Parses sixty-four hex characters, either case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s).map_err(|e| ParseContentHashError(e.to_string()))?;
        let arr: [u8; 32] = bytes.try_into().map_err(|_| {
            ParseContentHashError(format!("expected 32 bytes, got {}", s.len() / 2))
        })?;
        Ok(Self(arr))
    }
}

/// Serialise `value` as RFC 8785 canonical JSON.
///
/// Deterministic: the same value in any key order yields the same bytes.
/// Cost is one pass over the value plus a sort of each object's keys.
/// Numbers are printed as ECMAScript prints them (`1` not `1.0`); the
/// caller is responsible for having rejected integers outside ±2^53
/// beforehand (`validate`, M2), because past that point the double they
/// become is not the integer the client sent.
///
/// # Errors
///
/// [`CanonicalError::Serialize`] if the serialiser fails; not expected for
/// a `Value` built from JSON text.
pub fn canonicalize(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    serde_json_canonicalizer::to_vec(value).map_err(CanonicalError::Serialize)
}

/// sha256 over canonical bytes. Infallible; O(n) in the input.
pub fn content_hash(canonical: &[u8]) -> ContentHash {
    let digest = Sha256::digest(canonical);
    ContentHash(digest.into())
}

/// [`canonicalize`] then [`content_hash`], returning both, since every
/// caller that wants the hash also stores the bytes.
///
/// # Errors
///
/// Same as [`canonicalize`].
pub fn hash_value(value: &Value) -> Result<(Vec<u8>, ContentHash), CanonicalError> {
    let bytes = canonicalize(value)?;
    let hash = content_hash(&bytes);
    Ok((bytes, hash))
}
