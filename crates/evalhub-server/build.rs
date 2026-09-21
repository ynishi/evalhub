//! Refuses a release build that would embed no web UI.
//!
//! `embed.rs` declares the SPA with `#[allow_missing = true]` so that a
//! debug build on a checkout without `web-dist/` still compiles and serves
//! a placeholder. That leniency is right for development and wrong for
//! anything that ships: a release binary with an empty asset set is a
//! silent failure, and it is exactly how a UI-less crate would get
//! published. So the rule is the one Tauri applies to `frontendDist`:
//! in a release build the directory must exist, and its absence is an
//! error that names the path and the command that fills it.
//!
//! Two exceptions, both explicit:
//!
//! - `DOCS_RS` is set: docs.rs builds without network and without node,
//!   and only needs the crate to compile.
//! - `EVALHUB_ALLOW_MISSING_UI` is set: a release build that deliberately
//!   has no UI (an API-only CI job, say). This is the equivalent of
//!   Tauri's dev URL, an opt-in that has to be spelled out.
//!
//! `cargo package` verifies with the dev profile, so this check does not
//! run there; the packaging gate in the `justfile` is what inspects the
//! `.crate` for `web-dist/index.html`.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=web-dist");
    println!("cargo:rerun-if-env-changed=DOCS_RS");
    println!("cargo:rerun-if-env-changed=EVALHUB_ALLOW_MISSING_UI");

    let release = env::var("PROFILE").as_deref() == Ok("release");
    if !release
        || env::var_os("DOCS_RS").is_some()
        || env::var_os("EVALHUB_ALLOW_MISSING_UI").is_some()
    {
        return;
    }

    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);
    let Some(manifest_dir) = manifest_dir else {
        // Cargo always sets it for build scripts; if it is missing, this is
        // not a cargo build and there is nothing sensible to check.
        return;
    };
    let index = manifest_dir.join("web-dist").join("index.html");
    if !index.is_file() {
        panic!(
            "release build of evalhub-server without a web UI: {} does not exist.\n\
             Run `just web-build` (or `cd web && corepack pnpm install && corepack pnpm build`) \
             first, or set EVALHUB_ALLOW_MISSING_UI=1 to build without one on purpose.",
            index.display()
        );
    }
}
