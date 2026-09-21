//! Token, namespace and organisation handlers. Semantics in
//! [`crate::auth`]. Token secrets are returned exactly once, in the `POST
//! /tokens` response; every later read shows the prefix and the hash only.
