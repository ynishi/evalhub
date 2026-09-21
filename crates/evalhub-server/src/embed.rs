//! The embedded web UI.
//!
//! `web/build/` — the SvelteKit static build — will be compiled into the
//! binary with `rust-embed` and served under `/`. Any path that is not
//! `/api/v1`, `/openapi.json`, `/schemas/*` or an existing static file
//! falls through to `index.html`, which is what a single-page application
//! needs for client-side routing.
//!
//! Same origin as the API means no CORS configuration, no separate deploy,
//! and a self-host that is still one binary. The `#[folder]` path is
//! relative to this crate's `Cargo.toml`
//! (`$CARGO_MANIFEST_DIR/../../web/build`), not to the working directory of
//! the build.
//!
//! # Why an embedded SPA
//!
//! The UI is written as one client of `/api/v1`, with no route the API does
//! not have. That the UI works from `GET /openapi.json` alone is the proof
//! that the contract is complete for other clients. Alternatives rejected:
//! server-side rendering (htmx and the like), because a UI that can call
//! internal functions directly stops being a test of the API; a separately
//! deployed SPA, because self-hosting would become two deployments with
//! CORS and cookie domains to manage; no UI, because a hub with nothing to
//! look at has no face.
//!
//! Stack: SvelteKit with `adapter-static` in SPA mode, TypeScript, a client
//! generated from `/openapi.json` with `openapi-typescript` and called
//! through `openapi-fetch`. Source lives in `web/`; it is not a Rust crate.
//!
//! # Screens (v0)
//!
//! | Screen         | Contents                                                                                         | API                                                        |
//! | -------------- | ------------------------------------------------------------------------------------------------ | ---------------------------------------------------------- |
//! | List / search  | Cards and Evals; filter by ns / search / sort; a query builder that completes paths from the schema and narrows operators by type; results as a table | `GET /cards`, `POST /cards/query`, `GET /schemas/*` |
//! | Card page      | title, the seven facets, results, counts, badges, attachments (download links), relations, version history with `changed[]` | `GET /cards/{ns}/{name}@…?expand=*`, `/versions`, `/relations` |
//! | Eval page      | runs, attachments, the comparison view: Cards that use this Eval grouped by model / harness with `same_harness` / `same_model` | `GET /evals/{ns}/{name}/cards?group_by=` |
//! | Namespace page | a user's or org's Cards and Evals; members (admin only)                                          | `GET /namespaces/{ns}`, `/orgs/{org}/members`              |
//! | Settings       | issue / revoke tokens, change visibility, move labels                                            | `/tokens`, `PATCH …/settings`, `PATCH …/label`             |
//! | Registry       | browse metrics / harnesses / ext_schemas, show `applying`                                        | `GET /registry/*`                                          |
//!
//! Not in v0: creating records or uploading attachments from the UI (a
//! harness does that through the API), charts (the comparison view is a
//! table), and relation-graph visualisation.
//!
//! Until the UI exists, the fallback serves a one-page placeholder so that
//! a browser pointed at the server sees the service name and a link to the
//! contract rather than a 404.

use axum::http::{StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};

use evalhub_schema::openapi::API_PREFIX;

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>evalhub</title>
<style>body{font:16px/1.5 system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem;color:#222}code{background:#f3f3f3;padding:.1em .3em}</style>
</head>
<body>
<h1>evalhub</h1>
<p>A hosting service for named, versioned LLM evaluation results and their materials.</p>
<p>The web UI is not built into this binary yet. The API contract is at <a href="/openapi.json"><code>/openapi.json</code></a>; health is at <a href="/api/v1/healthz"><code>/api/v1/healthz</code></a>.</p>
</body>
</html>
"#;

/// Router fallback: placeholder page for browser paths, plain 404 for
/// anything under the API prefix.
pub async fn fallback(uri: Uri) -> Response {
    if uri.path().starts_with(API_PREFIX) {
        StatusCode::NOT_FOUND.into_response()
    } else {
        Html(PLACEHOLDER).into_response()
    }
}
