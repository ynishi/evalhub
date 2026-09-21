//! The hand-written skeleton of the OpenAPI 3.1 document.
//!
//! Almost all of `openapi.json` is derived: the record and query schemas come
//! from the `JsonSchema` derives in this crate, and the paths come from the
//! handlers in `evalhub_server`, described through `aide`. What is not
//! derivable — `info`, `servers`, the security scheme, the shared `422` /
//! `409` / `404` responses that reference [`crate::error`] — is assembled
//! here so that the server only has to merge it.
//!
//! OpenAPI 3.1 was chosen over 3.0 because 3.1's schema dialect *is* JSON
//! Schema 2020-12. The schema a client fetches from `GET /schemas/card` and
//! the one embedded in `openapi.json` are byte-identical; with 3.0 they
//! could not be.
//!
//! The document is served at `GET /openapi.json` and snapshot-tested in the
//! server crate. A change to a handler that changes the document is not done
//! until the snapshot has been updated in the same commit and the `web/`
//! client regenerated in the same pull request.
//!
//! This crate does not depend on an OpenAPI library; it exposes the
//! constants and the server builds the `Info` object from them.

/// `info.title` of the served document.
pub const TITLE: &str = "evalhub";

/// `info.description` of the served document.
pub const DESCRIPTION: &str = "A hosting service for named, versioned LLM evaluation results (Cards) \
and their materials (Evals). The hub validates, indexes and compares; it does not run evaluations.";

/// The API's version, as carried in `info.version`. Independent of the crate
/// version: it changes when the contract changes.
pub const API_VERSION: &str = "1.0.0";

/// Path prefix of every API route.
pub const API_PREFIX: &str = "/api/v1";
