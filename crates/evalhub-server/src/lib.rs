//! evalhub: a hosting service for LLM evaluation results and their materials.
//!
//! This is the top of the workspace and the front page of the design. Read
//! this, then follow the links down.
//!
//! # What evalhub is
//!
//! A hub. Things published to it have a name, `{ns}/{name}`, are public or
//! private, are protected by user tokens and organisation roles, and can be
//! searched, listed and traversed by their relations to each other. There
//! are two kinds of thing:
//!
//! - A **Card** — what was measured, how, and what the score was.
//! - An **Eval** — the material: a set of runs, prompts, tasks or traces.
//!
//! Both are *typed records*: a JSON document that closes over a JSON Schema,
//! plus references to attached files. Under a name, versions accumulate;
//! each version is immutable; a correction is a new version.
//!
//! # What evalhub is not
//!
//! It does not run evaluations. It does not rewrite what it receives. It
//! does not say "verified". It does not rank, average or declare winners.
//! It validates, indexes, and lets a reader line records up and see which
//! axes agree. Leaderboards whose operator runs the aggregation have a
//! record of shutting down (Papers with Code, the Open LLM Leaderboard,
//! HELM); registries whose publishers push have not (crates.io, PyPI,
//! Zenodo, OpenML). evalhub is a registry.
//!
//! # The system in one picture
//!
//! ```text
//!   harness / CLI / web UI            (clients generated from openapi.json)
//!           │  HTTPS, Bearer token
//!           ▼
//!   ┌───────────────────────── evalhub-server ─────────────────────────┐
//!   │ api::records  api::attachments  api::query  api::relations       │
//!   │ api::registry api::export       api::audit  api::auth            │
//!   │ auth (tokens, orgs, visibility)   openapi (aide)   embed (SPA)    │
//!   │ jobs (index build, badge recompute, GC)                          │
//!   └───┬──────────────────┬──────────────────────────────┬────────────┘
//!       │ evalhub-query    │ evalhub-core                 │ evalhub-store
//!       │ parse/typecheck  │ validate/canonical/          │ Postgres (truth)
//!       │ → IR             │ fingerprint/badge            │ S3-compatible (bytes)
//!       └──────────────────┴──────────────────────────────┴────────────
//!                               evalhub-schema (the types; the contract)
//! ```
//!
//! Attachments never pass through the server: clients upload and download
//! by presigned URL, and the hub records only the sha256.
//!
//! # Concepts
//!
//! | Entity         | Key                                  | Meaning                                                                 |
//! | -------------- | ------------------------------------ | ----------------------------------------------------------------------- |
//! | User           | `user_id`                            | login principal; one personal namespace                                 |
//! | Org            | `ns`                                 | members with a role (`read` / `write` / `admin`)                        |
//! | Namespace      | `ns` (slug)                          | a user or an org; owns Cards, Evals and registry entries                |
//! | Token          | `token_id`                           | user-owned; `scope` × `namespaces[]`                                    |
//! | Card / Eval    | `type` + `{ns}/{name}` + `id` (ULID) | a named sequence of versions; setting: `visibility`                     |
//! | Version        | `version_id` (ULID), `(id, seq)`     | immutable record; `content_hash` for idempotency; optional `label`      |
//! | Attachment     | `sha256`                             | an object in storage, referenced from `attachments[]`, deduplicated     |
//! | Relation       | `(from_version_id, type, to, attrs)` | an edge; from `relations[]` on ingest or added via the API              |
//! | Registry entry | `kind/{ns}/{id}@{version}`           | harness / metric / relation_type / ext_schema definition; immutable     |
//!
//! # The contract
//!
//! `/api/v1`, described by OpenAPI 3.1 at `GET /openapi.json`, with the
//! record and query schemas served at `GET /schemas/{name}`. Both are
//! generated from the Rust types in [`evalhub_schema`] — one derive, one
//! description. There is no client crate: each harness generates its client
//! from the document, and the bundled web UI does exactly that, which is
//! the standing proof that the contract is sufficient.
//!
//! # Lifecycle of a record
//!
//! ```text
//! POST /cards/{ns}/{name}
//!   auth: token has `write` on ns                         → 401 / 403
//!   body: JSON                                            → 400 if not JSON
//!   evalhub_core::validate  (all errors collected)        → 422 { errors[] }
//!   attachments all `ready`?                              → 409 attachment_missing
//!   canonical → content_hash; same as latest?             → 200 existing
//!   fingerprints, badges
//!   store: one transaction                                → 201 { id, version_id, seq, label, content_hash, changed[], badges[] }
//! ```
//!
//! Reading is `GET /cards/{ns}/{name}[@{seq}|@{label}]?expand=…`, listing is
//! `GET /cards?ns&search&sort&cursor`, searching is `POST /cards/query`, and
//! the same for `/evals`. Private records are `404` to anyone without
//! access; there is no endpoint that reveals their existence.
//!
//! # Versions, labels, tombstones
//!
//! `seq` is assigned by the hub, gap-free per name. A `label` is a
//! client-chosen slug (never purely numeric) that can be re-pointed. A
//! relation pins `{ns}/{name}@{seq}`. Deletion is a tombstone with a
//! reason; the body goes, the `version_id`, `content_hash` and `changed[]`
//! stay, and edges into the tombstone remain visible as such.
//!
//! # Comparability
//!
//! Seven facets (model, task, harness, generation, trial, grading, env),
//! each fingerprinted. Two Cards pointing at the same Eval version are
//! shown side by side, labelled `same_harness` / `same_model` where the
//! fingerprints agree. That is the extent of the hub's opinion.
//!
//! # Auth and visibility
//!
//! Every write requires a token. A token has a `scope` (`read` / `write` /
//! `admin`) over a list of namespaces. Organisations have members with
//! roles. Visibility is per name. See [`auth`].
//!
//! # Interoperability
//!
//! Converters, not a new standard. `export?format=hf-model-index` produces
//! the YAML a Hugging Face model card embeds; an EEE (Every Eval Ever)
//! import converter is the first inbound path. Converters live on the
//! harness side, not in this workspace: the hub does not know any harness.
//!
//! # Deployment
//!
//! One binary. `evalhub serve --database-url --s3-endpoint --bind`, default
//! bind `127.0.0.1`. The SPA is embedded and served from `/`, same origin
//! as the API, so there is no CORS. Configuration is layered (file, then
//! environment) with provenance, inspectable via `evalhub config show
//! --origin`. The hosted service runs this same binary; there is no
//! proprietary edition and no feature held back from the open source
//! release (AGPL-3.0-only).
//!
//! # Publishing policy
//!
//! **License.** AGPL-3.0-only, contributions under the Developer
//! Certificate of Origin (no CLA; there is no plan to relicense, so there
//! is nothing for a CLA to enable). Apache-2.0 and MIT were rejected
//! because they leave no barrier against a hosted competitor; BSL, SSPL,
//! the Elastic License and FSL were rejected because every source-available
//! relicensing of that kind (HashiCorp, Redis, Elastic, MinIO) was forked
//! and three of the four were reversed or abandoned, and because OSI
//! non-approval narrows where the code can be packaged. AGPL does not stop
//! unmodified hosting, and that is accepted: what the project protects is
//! not the code but the network effect of `{ns}/{name}`.
//!
//! **No open-core boundary.** Organisations, tokens, roles, private
//! namespaces, audit and the registry are all in the open source release.
//! They cannot be split out: visibility and authorisation are part of the
//! record API's meaning (a private record is `404`; a public Card's link to
//! a private Eval shows only a commitment). The hosted service and a
//! self-host run the same binary at the same version with no feature flag
//! between them. What the hosted service charges for is operation and
//! capacity — public is free, private storage is metered — never SSO, audit
//! or RBAC. Portability (`pg_dump` plus bucket sync, `export?format=bundle`)
//! is kept so that leaving the hosted service is always possible.
//!
//! **Interoperability by converter.** The contract is OpenAPI 3.1 plus the
//! served JSON Schema, which satisfies the conditions under which an
//! ecosystem forms around a hub even when the server is closed (open
//! client, standard protocol, open payload) — here the server is open too.
//! Formats are read and written in this order of priority: Hugging Face
//! model-index export (v0), EEE (Every Eval Ever) import (v0), Inspect
//! `.eval`, lm-eval-harness output, Croissant. No new standard is
//! introduced; the facet admission rule in `evalhub_schema::facet` is the
//! same policy applied to the record itself. EEE's two-level split
//! (metadata / instances) matches Card / Eval, which is why it is first.
//! Converters live on the harness side, not in this workspace.
//!
//! **Operating the hosted service** (outside the design): Terms, a Content
//! Policy, DMCA agent registration (renewed every three years) and abuse
//! handling are runbooks under `docs/`. Model providers' terms differ, so
//! the hub makes no blanket judgement about whether a given output may be
//! published; it records `provenance` and `redaction` and makes them
//! visible.
//!
//! # Build order
//!
//! The implementation is cut vertically, not crate by crate: each
//! milestone leaves a binary that does more than the one before, and the
//! integration risks (the OpenAPI generator, the SQL offline metadata, the
//! container fixtures, token extraction) surfaced in the first one rather
//! than after three crates were written in isolation.
//!
//! | Milestone | What works when it is done                                                                  | Status |
//! | --------- | ------------------------------------------------------------------------------------------- | ------ |
//! | M0        | Manifests: the real MSRV, the dsl-kit crates the grammar needs, license allowances           | done   |
//! | M1        | Walking skeleton: record types and served schemas; canonical form and ids; `records` repository with the container fixture and `.sqlx/`; token auth with a CLI bootstrap; `POST` / `GET` of a Card and an Eval, idempotent on `content_hash`, private by default | done |
//! | M2        | Record API complete: validation (`422 errors[]`), fingerprints and badges, attachments and `409 attachment_missing`, versions / labels / settings / tombstones, relations and the comparison view, tokens / orgs, list with signed cursors | done |
//! | M3        | Query DSL and registry: grammar, type check, IR → SQL, `POST …/query`; `core/` seed, `ext_schemas` index job, badge recompute; `hf-model-index` export | done |
//! | M4        | Web UI: SvelteKit SPA generated from `openapi.json`, embedded in the binary; and the `bundle` export, whose shape is still open | next |
//!
//! Within a milestone the order follows the ingest path: schema types,
//! then the pure rules in `evalhub_core`, then the store, then the
//! handlers. The schema crate's fixtures are the contract with harness
//! authors; `evalhub_core` and `evalhub_query` close without a database.
//!
//! # Open questions
//!
//! - Hosted pricing: the billing unit and the size of the free tier.
//! - The wording of the Terms and Content Policy.
//! - Where the EEE converter lives: a separate repository, or `tools/` here.
//! - What `export?format=bundle` is. Tarring a record's attachments would
//!   stream them through the hub, which the presigned-URL design exists to
//!   avoid; a manifest of presigned URLs, or a bundle pre-built in object
//!   storage, would not. The endpoint answers `501` until that is settled.
//!
//! # Modules
//!
//! - [`config`] — layered configuration and the `config show` command.
//! - [`auth`] — tokens, organisations, visibility, the UI cookie.
//! - [`api`] — the handlers, one module per resource group.
//! - [`openapi`] — assembling and serving `openapi.json` and `/schemas/*`.
//! - [`embed`] — the SPA.
//! - [`jobs`] — background work: index builds, badge recompute, GC.
//! - [`error`] — the `ApiError` type and its HTTP mapping.
//! - [`state`] — the shared `AppState`.

pub mod api;
pub mod auth;
pub mod config;
pub mod embed;
pub mod error;
pub mod jobs;
pub mod openapi;
pub mod state;
