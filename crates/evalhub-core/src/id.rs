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

use std::fmt;
use std::str::FromStr;

use ulid::Ulid;
use uuid::Uuid;

/// Failure to parse an identifier from its Crockford base32 text form.
#[derive(Debug, thiserror::Error)]
#[error("not a ULID: {0}")]
pub struct ParseIdError(#[source] ulid::DecodeError);

macro_rules! ulid_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Ulid);

        impl $name {
            /// Mint a fresh identifier from the current time and 80 random
            /// bits. Two calls in the same millisecond are distinct but not
            /// ordered relative to each other; across milliseconds the later
            /// one sorts higher.
            pub fn new() -> Self {
                Self(Ulid::generate())
            }

            /// The value as Postgres stores it: the same 128 bits as a `uuid`.
            pub fn as_uuid(&self) -> Uuid {
                Uuid::from(self.0)
            }

            /// Rebuild from a `uuid` column. Every 128-bit value is a valid
            /// ULID, so this cannot fail.
            pub fn from_uuid(uuid: Uuid) -> Self {
                Self(Ulid::from(uuid))
            }

            /// Milliseconds since the Unix epoch encoded in the identifier.
            pub fn timestamp_ms(&self) -> u64 {
                self.0.timestamp_ms()
            }

            /// Build from explicit parts. For tests and fixtures; production
            /// code mints with [`Self::new`].
            pub fn from_parts(timestamp_ms: u64, random: u128) -> Self {
                Self(Ulid::from_parts(timestamp_ms, random))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            /// Twenty-six characters of Crockford base32, as in URLs and logs.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut buf = [0u8; ulid::ULID_LEN];
                f.write_str(self.0.array_to_str(&mut buf))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;

            /// Parses the base32 form; case-insensitive as ULID specifies.
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ulid::from_string(s).map(Self).map_err(ParseIdError)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Uuid {
                id.as_uuid()
            }
        }

        impl From<Uuid> for $name {
            fn from(uuid: Uuid) -> Self {
                Self::from_uuid(uuid)
            }
        }
    };
}

ulid_newtype! {
    /// Identifies a named record (`{type, ns, name}`) for its lifetime.
    RecordId
}

ulid_newtype! {
    /// Identifies one immutable version of a record. Relations point at
    /// these; tombstones keep them.
    VersionId
}
