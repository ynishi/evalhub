//! The HTTP handlers, one module per resource group, all under `/api/v1`.
//!
//! | Group                    | Endpoints                                                                                                   |
//! | ------------------------ | ----------------------------------------------------------------------------------------------------------- |
//! | [`meta`]                 | `GET /openapi.json`, `GET /schemas/{name}`, `GET /whoami`, `GET /healthz`                                   |
//! | [`auth`]                 | `GET,POST,DELETE /tokens`, `GET /namespaces/{ns}`, `GET,POST,DELETE /orgs/{org}/members`                    |
//! | [`records`]              | `POST /{cards\|evals}/{ns}/{name}?label=`, `GET …/{name}[@{seq}\|@{label}]?expand=`, `GET …/versions`, `DELETE …@{…}`, `PATCH …@{…}/label`, `PATCH …/settings`, `GET /{cards\|evals}?ns&search&sort&cursor` |
//! | [`attachments`]          | `POST /attachments`, `POST /attachments/{sha256}/complete`, `HEAD,GET /attachments/{sha256}` (GET → 302)    |
//! | [`query`]                | `POST /{cards\|evals}/query`                                                                                |
//! | [`relations`]            | `GET …/{name}/relations?direction&types&depth&follow_latest&version`, `POST …@{…}/relations`, `GET /evals/{ns}/{name}/cards?version&group_by` |
//! | [`registry`]             | `GET,PUT /registry/{harnesses\|metrics\|relation_types\|ext_schemas}/{ns}/{id}@{version}` (`core/` read-only) |
//! | [`export`]               | `GET …/{name}/export?format=hf-model-index\|bundle&version=`                                                |
//! | [`audit`]                | `GET /audit?ns&cursor`                                                                                      |
//!
//! Statuses: `201` created, `200` idempotent hit or read, `409`
//! (`attachment_missing`, `label_in_use`), `422` (`errors[]`), `404`
//! (including private), `403` (scope), `401` (token), `400` (not JSON, bad
//! cursor). See [`crate::error`].
//!
//! Handlers are thin: extract, authorise, call the store or core, map the
//! result. Rules live in the library crates so that they are the same for
//! every caller and testable without HTTP.
//!
//! [`router`] builds the whole tree. Routes registered through `aide`'s
//! `ApiRouter` appear in `openapi.json`; the document itself and the SPA
//! fallback are plain axum routes and do not.

use std::sync::Arc;

use aide::axum::ApiRouter;
use aide::axum::routing::get_with;
use axum::Router;
use axum::routing::get;
use tower_http::trace::TraceLayer;

use evalhub_schema::openapi::API_PREFIX;
use evalhub_store::PgPool;

use crate::config::Config;
use crate::state::AppState;

pub mod attachments;
pub mod audit;
pub mod auth;
pub mod export;
pub mod meta;
pub mod query;
pub mod records;
pub mod registry;
pub mod relations;

/// Build the application router and the OpenAPI document it describes.
///
/// `db` is `None` when no database is configured; endpoints that need one
/// are not mounted in that case (there are none yet).
pub fn router(config: Arc<Config>, db: Option<PgPool>) -> Router {
    let mut openapi = crate::openapi::skeleton();

    let api = ApiRouter::new()
        .api_route(
            "/healthz",
            get_with(meta::healthz, |op| {
                op.id("healthz")
                    .summary("Liveness")
                    .description("Answers as long as the process is up.")
            }),
        )
        .api_route(
            "/whoami",
            get_with(meta::whoami, |op| {
                op.id("whoami")
                    .summary("Who the hub thinks the caller is")
                    .description("Identity and scope of the presented token, or anonymous.")
            }),
        );

    let app = ApiRouter::new()
        .nest(API_PREFIX, api)
        .finish_api(&mut openapi);

    let state = AppState {
        config,
        db,
        openapi: Arc::new(openapi),
    };

    app.route("/openapi.json", get(meta::openapi))
        .fallback(crate::embed::fallback)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
