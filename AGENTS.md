# Agents

Pointers for coding agents working in this repository. Nothing here is a second
copy of anything; follow the links.

- **Conventions** (issues, branches, verification, commits, DCO): `CONTRIBUTING.md`.
- **Design**: the crate docs. `cargo doc --no-deps --open`, start at
  `evalhub_server`. The design is not in `docs/`; `docs/` holds runbooks only.
- **User-facing reference**: `README.md`. Local development with Docker:
  `compose.yml`. The hosted service: `Dockerfile`, `fly.toml`,
  `docs/hosting.md`, and the scripts in `deploy/fly/` (`up.sh` /
  `down.sh` / `smoke.sh`), which never print a credential; keep it that
  way when editing them. Only the three SDK crates (`evalhub-schema`,
  `-core`, `-query`) are published to crates.io; the store and the server
  are `publish = false` and ship as the image.
- **Verification**: per crate, `cargo test -p <crate>` (`just check` runs
  the full list). Never `cargo test --workspace` as the routine check.
  `evalhub-store` and `evalhub-server` tests need Docker.
- **E2E**: `just e2e` boots the release binary against Docker Postgres,
  fetches the embedded UI and the API (`e2e/smoke.sh`), then runs the
  Playwright suite in `web/tests/browser` against it.
- **Web UI**: `just web-build` writes `crates/evalhub-server/web-dist`
  (gitignored, shipped via the crate's `include`). A release build of the
  server fails without it; `just package` checks the `.crate` carries it.
- **Contracts that are generated and committed**: `crates/evalhub-schema/schemas/`,
  `crates/evalhub-store/.sqlx/`, the OpenAPI snapshot, the `web/` client. Regenerate in the same
  commit as the change that invalidated them.
- **Do not**: add a design document under `docs/`; add a dependency without
  checking `cargo deny check`; commit anything `.gitignore` excludes.
