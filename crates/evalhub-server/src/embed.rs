//! The embedded web UI.
//!
//! `web/build/` — the SvelteKit static build — is compiled into the binary
//! with `rust-embed` and served under `/`. Any path that is not
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
//! # Building it, or not
//!
//! `#[allow_missing = true]` means a checkout without `web/build`
//! compiles: `cargo build` is not held hostage to `pnpm`. The asset set
//! is then empty and the fallback serves the placeholder page, which
//! names the contract and says how to build the UI. `cd web && pnpm
//! install && pnpm build` fills `web/build`, and the next `cargo build`
//! picks it up.
//!
//! In a debug build without the `debug-embed` feature `rust-embed` reads
//! the files from disk at request time, so a `pnpm build` shows up
//! without recompiling; a release build bakes them in. That also means a
//! debug binary moved away from its source tree finds nothing and falls
//! back to the placeholder.
//!
//! # Caching
//!
//! SvelteKit emits hashed filenames under `_app/immutable/`, so those are
//! served `public, max-age=31536000, immutable`: the name changes when
//! the bytes do. Everything else, `index.html` above all, is `no-cache` —
//! the browser may keep it but must revalidate, or a deploy would leave
//! clients on an `index.html` pointing at assets that no longer exist.
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

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::Embed;

use evalhub_schema::openapi::API_PREFIX;

/// The built SPA, or nothing when `web/build` was absent at compile time.
///
/// The folder is relative, which `rust-embed` resolves against this
/// crate's `Cargo.toml`. It must stay relative: `$CARGO_MANIFEST_DIR`
/// is only expanded with the `interpolate-folder-path` feature, and
/// without it the literal `$…` becomes part of a path that cannot
/// exist — which `allow_missing` then turns into an empty asset set
/// and a binary that silently serves the placeholder. The test below
/// is the guard against that returning.
#[derive(Embed)]
#[folder = "../../web/build"]
#[allow_missing = true]
struct WebAssets;

/// Entry document of the SPA.
const INDEX: &str = "index.html";

/// Prefix SvelteKit gives the files whose names carry a content hash.
const IMMUTABLE_PREFIX: &str = "_app/immutable/";

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>evalhub</title>
<style>body{font:16px/1.5 system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem;color:#222}code{background:#f3f3f3;padding:.1em .3em}</style>
</head>
<body>
<h1>evalhub</h1>
<p>A hosting service for named, versioned LLM evaluation results and their materials.</p>
<p>The web UI is not built into this binary. The API contract is at <a href="/openapi.json"><code>/openapi.json</code></a>; health is at <a href="/api/v1/healthz"><code>/api/v1/healthz</code></a>.</p>
<p>To build the UI: <code>cd web &amp;&amp; pnpm install &amp;&amp; pnpm build</code>, then rebuild the server.</p>
</body>
</html>
"#;

/// Whether any UI was compiled in.
fn ui_is_built() -> bool {
    WebAssets::get(INDEX).is_some()
}

/// Serve one embedded file, with the cache policy its name earns.
fn asset(path: &str) -> Option<Response> {
    let file = WebAssets::get(path)?;
    let mime = file.metadata.mimetype();
    let cache = if path.starts_with(IMMUTABLE_PREFIX) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let headers = [
        (
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        ),
        (header::CACHE_CONTROL, HeaderValue::from_static(cache)),
    ];
    Some((headers, file.data.into_owned()).into_response())
}

/// Router fallback: the SPA for browser paths, a plain `404` for anything
/// under the API prefix.
///
/// A path that names an embedded file gets that file. Anything else that
/// a browser could be asking for gets `index.html`, so the SPA's own
/// router decides what it means — that is what makes `/cards/alice/x` a
/// page rather than a `404`. With no UI built in, the placeholder stands
/// in for `index.html`.
pub async fn fallback(uri: Uri) -> Response {
    if uri.path().starts_with(API_PREFIX) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = uri.path().trim_start_matches('/');
    if !path.is_empty()
        && let Some(response) = asset(path)
    {
        return response;
    }
    if ui_is_built()
        && let Some(index) = asset(INDEX)
    {
        return index;
    }
    Html(PLACEHOLDER).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immutable_assets_are_cached_forever_and_the_rest_revalidate() {
        // The rule is about the path, so it holds whether or not a UI was
        // built into this particular binary.
        assert!(IMMUTABLE_PREFIX.ends_with('/'));
        assert!(!INDEX.starts_with(IMMUTABLE_PREFIX));
    }

    /// A built UI must actually reach the binary.
    ///
    /// `allow_missing` exists so a checkout without `web/build` still
    /// compiles, and it will just as happily swallow a folder path that
    /// is wrong. So: if the directory is there on disk, the embed has to
    /// see it. In CI, where the UI is not built, this asserts nothing and
    /// passes.
    #[test]
    fn a_built_ui_is_embedded() {
        let built = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/build")
            .join(INDEX);
        if built.is_file() {
            assert!(
                ui_is_built(),
                "{} exists but the binary cannot see it: check the #[folder] path",
                built.display()
            );
        }
    }

    #[test]
    fn placeholder_names_the_contract_and_the_build_command() {
        assert!(PLACEHOLDER.contains("/openapi.json"));
        assert!(PLACEHOLDER.contains("pnpm build"));
    }
}
