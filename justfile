# evalhub task runner. `just` lists the recipes; CONTRIBUTING.md is the
# reference for what green means.
#
# The web UI is built by pnpm into `crates/evalhub-server/web-dist`, which
# is gitignored and shipped to crates.io through the server crate's
# `include` list. `cargo build` alone never builds it: like `tauri build`,
# the recipes here are the entry point that puts the UI in place before
# cargo compiles or packages.

set shell := ["bash", "-euo", "pipefail", "-c"]

web_dir := "web"
web_dist := "crates/evalhub-server/web-dist"

# crates.io refuses a `.crate` above this many bytes.
crate_size_limit := "10000000"

default:
    @just --list

# ---------------------------------------------------------------- web UI

# Install the UI's node dependencies from the committed lockfile.
web-install:
    cd {{ web_dir }} && corepack pnpm install --frozen-lockfile

# Build the UI into the server crate (`web-dist/`). Prerequisite of a
# release build and of `package`.
web-build: web-install
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{ web_dir }}
    corepack pnpm build
    cd ..
    test -f {{ web_dist }}/index.html || {
        echo "web-build: {{ web_dist }}/index.html was not produced" >&2
        exit 1
    }
    echo "web-build: $(find {{ web_dist }} -type f | wc -l) files in {{ web_dist }}"

# Type-check and lint the UI.
web-check:
    cd {{ web_dir }} && corepack pnpm check && corepack pnpm lint

# Remove the built UI. A debug server then serves the placeholder.
web-clean:
    rm -rf {{ web_dist }}

# ------------------------------------------------------------- verification

# The per-crate checks from CONTRIBUTING.md. Store and server need Docker.
check:
    cargo fmt --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test -p evalhub-schema
    cargo test -p evalhub-core
    cargo test -p evalhub-query
    cargo test -p evalhub-store
    cargo test -p evalhub-server
    just sqlx-check
    cargo deny check

# Confirm the committed sqlx offline data matches the queries. Needs
# DATABASE_URL pointing at a migrated database.
sqlx-check:
    cd crates/evalhub-store && cargo sqlx prepare --check

# Regenerate the sqlx offline data after a query change. Needs
# DATABASE_URL pointing at a migrated database; commit the result.
sqlx-prepare:
    cd crates/evalhub-store && cargo sqlx prepare

# ---------------------------------------------------------------------- e2e

# Boot the real release binary against a throwaway Postgres and ask it for
# what a browser would: the embedded UI, an immutable asset, a deep link,
# the API. Needs Docker. See e2e/smoke.sh.
e2e: web-build e2e-install
    cargo build --release -p evalhub-server
    e2e/smoke.sh target/release/evalhub

# Fetch the Chromium headless shell Playwright drives. No root: it lands in
# ~/.cache/ms-playwright (or $PLAYWRIGHT_BROWSERS_PATH). Then confirm the
# host has the shared libraries it links against, because a missing one
# fails later with a bare "error while loading shared libraries".
e2e-install: web-install
    #!/usr/bin/env bash
    set -euo pipefail
    (cd {{ web_dir }} && corepack pnpm exec playwright install --only-shell chromium)
    root=${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}
    shell=$(find "$root" -type f -name chrome-headless-shell | head -1)
    [ -n "$shell" ] || { echo "e2e-install: no chrome-headless-shell under $root" >&2; exit 1; }
    missing=$(ldd "$shell" | grep 'not found' || true)
    if [ -n "$missing" ]; then
        echo "e2e-install: the headless shell needs libraries this host lacks:" >&2
        echo "$missing" >&2
        echo "e2e-install: install them (\`corepack pnpm exec playwright install-deps chromium\`, needs sudo) or run the suite in the Playwright Docker image" >&2
        exit 1
    fi
    echo "e2e-install: $shell, all shared libraries present"

# ---------------------------------------------------------------- packaging

# Build the UI, package every crate, and gate the result. This is the
# step before `cargo publish --workspace`; publishing itself is a manual
# command, on purpose.
#
# `--allow-dirty` is required: cargo's own dirty check counts the
# gitignored `web-dist/` files it is about to package as uncommitted
# changes. The clean-tree check is therefore done here, on tracked files
# only, before cargo runs, so a real uncommitted edit still stops the
# packaging.
package: web-build
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "$(git status --porcelain --untracked-files=normal)" ]; then
        echo "package: the working tree has uncommitted or untracked changes; commit or stash them first" >&2
        git status --short >&2
        exit 1
    fi
    cargo package --workspace --allow-dirty
    just package-gate

# Inspect the `.crate` files in target/package: each must carry LICENSE and
# README.md, the server must carry the UI, and none may exceed the
# crates.io size limit. Fails loudly, because `allow_missing` in embed.rs
# would otherwise let a UI-less server ship without a word.
package-gate:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    crates=(target/package/*.crate)
    if [ ${#crates[@]} -eq 0 ]; then
        echo "package-gate: no .crate under target/package; run \`just package\`" >&2
        exit 1
    fi
    fail=0
    for crate in "${crates[@]}"; do
        name=$(basename "$crate" .crate)
        list=$(tar tzf "$crate")
        bad=0
        for want in LICENSE README.md; do
            if ! grep -qx "$name/$want" <<<"$list"; then
                echo "package-gate: $name lacks $want" >&2
                bad=1
            fi
        done
        case "$name" in
            evalhub-server-*)
                if ! grep -qx "$name/web-dist/index.html" <<<"$list"; then
                    echo "package-gate: $name lacks web-dist/index.html (run \`just web-build\` before packaging)" >&2
                    bad=1
                fi
                ;;
            evalhub-store-*)
                if ! grep -q "^$name/\.sqlx/query-" <<<"$list"; then
                    echo "package-gate: $name lacks .sqlx offline data" >&2
                    bad=1
                fi
                ;;
        esac
        size=$(stat -c %s "$crate")
        if [ "$size" -gt {{ crate_size_limit }} ]; then
            echo "package-gate: $name is $size bytes, over the crates.io limit of {{ crate_size_limit }}" >&2
            bad=1
        fi
        if [ "$bad" -eq 0 ]; then
            printf "package-gate: %-28s %8d bytes  ok\n" "$name" "$size"
        else
            fail=1
        fi
    done
    exit $fail
