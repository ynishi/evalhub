//! Assembling and serving the contract.
//!
//! `GET /openapi.json` returns the OpenAPI 3.1 document. `aide` collects it
//! from the router: each handler is registered with its request and
//! response types, which derive `schemars::JsonSchema` in
//! `evalhub_schema`, so the schemas in the document are the record schemas.
//! The non-derivable parts (`info`, `servers`, the bearer security scheme,
//! the shared error responses) come from `evalhub_schema::openapi`.
//!
//! `GET /schemas/{name}` serves the committed JSON Schema files
//! (`card`, `eval`, `query`, `error`, and the seven facets) with `$id` set
//! to that URL, so a `$ref` from `openapi.json` resolves to the same bytes
//! a client would fetch directly.
//!
//! The assembled document is snapshot-tested. A handler change that
//! alters it fails the test until the snapshot is updated in the same
//! commit; the `web/` client is regenerated from it in the same pull
//! request.

use aide::openapi::{Info, OpenApi};
use evalhub_schema::openapi as contract;

/// The document before any route is added: `info` only. The router fills
/// in `paths` and `components` when it is finished.
pub fn skeleton() -> OpenApi {
    OpenApi {
        info: Info {
            title: contract::TITLE.to_string(),
            description: Some(contract::DESCRIPTION.to_string()),
            version: contract::API_VERSION.to_string(),
            ..Info::default()
        },
        ..OpenApi::default()
    }
}
