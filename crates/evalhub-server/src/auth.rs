//! Tokens, organisations, visibility, and the UI session.
//!
//! # Tokens
//!
//! A token is a 32-byte random secret, shown to the user once at issue,
//! presented as `Authorization: Bearer <secret>`. The hub stores only its
//! sha256 (`tokens.token_hash`) and compares in constant time. A token that
//! random needs no slow hash; a password would, but there are no passwords
//! in the API. Each token has a `scope` and a list of namespaces:
//!
//! | Scope   | Allows                                                                 |
//! | ------- | ---------------------------------------------------------------------- |
//! | `read`  | read private records in the token's namespaces                         |
//! | `write` | + create names, append versions, add relations, change settings, delete, registry `PUT` |
//! | `admin` | + manage org members, issue tokens for the org, read audit             |
//!
//! `GET, POST, DELETE /tokens` manage a user's own tokens. Every write
//! endpoint requires a token; reads of public records require none.
//!
//! # Organisations
//!
//! An org is a namespace with members. `GET, POST, DELETE
//! /orgs/{org}/members { user, role }`, `admin` only. A user's token acts
//! with the lesser of the token's scope and the user's role in the org.
//!
//! # Visibility
//!
//! Per name, `private` (default) or `public`, changed with
//! `PATCH .../settings`. Private records do not appear in lists, searches
//! or relation traversals, and `GET` on them is `404`, to anyone without a
//! token covering the namespace. A public Card pointing at a private Eval
//! exposes the Eval's `version_id` and `content_hash` only.
//!
//! # The UI session
//!
//! The SPA does not hold a bearer token in JavaScript. After login the
//! server issues a UI-scoped token and sets it in a private (encrypted,
//! signed) cookie via `axum-extra`; the cookie is `HttpOnly`, `Secure`,
//! `SameSite=Strict`. The API accepts either the header or the cookie, and
//! treats them identically after extraction.
//!
//! # Cursors
//!
//! Pagination cursors are keyset tuples, serialised, HMAC-signed with
//! `auth.cursor_key` and base64url-encoded. A tampered cursor is `400`.
//! Signing makes the cursor opaque without needing server-side state.
