# Agents

Pointers for coding agents working in this repository. Nothing here is a second
copy of anything; follow the links.

- **Conventions** (issues, branches, verification, commits, DCO): `CONTRIBUTING.md`.
- **Design**: the crate docs. `cargo doc --no-deps --open`, start at
  `evalhub_server`. The design is not in `docs/`; `docs/` holds runbooks only.
- **User-facing reference**: `README.md`.
- **Verification**: per crate, `cargo test -p <crate>`. Never `cargo test
  --workspace` as the routine check. `evalhub-store` and `evalhub-server` tests
  need Docker.
- **Contracts that are generated and committed**: `crates/evalhub-schema/schemas/`,
  `.sqlx/`, the OpenAPI snapshot, the `web/` client. Regenerate in the same
  commit as the change that invalidated them.
- **Do not**: add a design document under `docs/`; add a dependency without
  checking `cargo deny check`; commit anything `.gitignore` excludes.
