# evalhub

A hosting service for LLM evaluation results. A **Card** says what was
measured, how, and what the score was. An **Eval** is the material the Card was
measured from: a set of runs, prompts, tasks or traces. Both are published under
a name, `{ns}/{name}`, as an immutable sequence of versions, and can be public
or private, searched, and linked to each other.

The hub validates, indexes and lets you compare. It does not run evaluations,
does not rewrite what it receives, and never calls anything "verified".

## Status

Pre-alpha. The record API accepts and returns Cards and Evals (create,
append, read); validation beyond the record's shape, attachments, relations,
query and the web UI are not there yet. Read the design in the crate docs:

```bash
cargo doc --no-deps --open
```

Start with `evalhub_server`, which describes the whole system, then follow the
links down to `evalhub_core` and `evalhub_store`.

## Layout

| Crate            | Role                                                                      |
| ---------------- | ------------------------------------------------------------------------- |
| `evalhub-schema` | The record types. Source of the JSON Schema and OpenAPI contract.         |
| `evalhub-core`   | Canonical form, fingerprints, validation, badges. No I/O.                 |
| `evalhub-query`  | The query DSL: grammar, type check, backend-independent IR.               |
| `evalhub-store`  | Postgres and S3-compatible object storage. IR to SQL lives here.          |
| `evalhub-server` | The HTTP API, auth, OpenAPI, embedded web UI, and the `evalhub` binary.   |
| `web/`           | The SPA. A client of `/api/v1` and nothing else.                          |

## Running

One binary and a Postgres. Bring the schema current, create the first user
(there is no signup endpoint; the token is printed once), then serve:

```bash
export EVALHUB_DATABASE__URL=postgres://user:pass@localhost/evalhub
cargo run -p evalhub-server -- migrate
cargo run -p evalhub-server -- user create alice        # prints alice's token
cargo run -p evalhub-server -- serve --bind 127.0.0.1:8080
cargo run -p evalhub-server -- config show --origin
```

Configuration is layered: defaults, then a TOML file (`--config` or
`EVALHUB_CONFIG`, or `evalhub.toml` in the working directory), then
`EVALHUB_*` environment variables (nested keys joined with `__`, e.g.
`EVALHUB_DATABASE__URL`), then flags. `config show --origin` prints every
key with the layer that set it; secrets are redacted.

`serve` refuses to start without a database, and with one it refuses to
start until `evalhub migrate` has brought the schema current.

## API

Everything is under `/api/v1`; the contract is `GET /openapi.json` (OpenAPI
3.1) and the record schemas are `GET /schemas/{card|eval|error|query}`.
Writes need `Authorization: Bearer <token>` with `write` on the namespace;
reads of public records need nothing, and private records are `404` to
anyone the token does not cover.

| Method | Path                                  | Does                                                                                   |
| ------ | ------------------------------------- | -------------------------------------------------------------------------------------- |
| `POST` | `/cards/{ns}/{name}?label=`           | Append a Card version. `201` with `{ id, version_id, seq, label, content_hash, changed[], badges[] }`; `200` and the existing version when the canonical body equals the latest; `422 { errors[] }` when the shape is wrong; `409 label_in_use`. |
| `GET`  | `/cards/{ns}/{name}[@{seq}]`          | The latest live version, or one by sequence number, with the canonical `record`.       |
| `POST` | `/evals/{ns}/{name}?label=`           | Same for an Eval.                                                                      |
| `GET`  | `/evals/{ns}/{name}[@{seq}]`          | Same for an Eval.                                                                      |
| `GET`  | `/whoami`                             | The token's user, scope and namespaces, or all empty.                                  |
| `GET`  | `/healthz`                            | Liveness.                                                                              |

A record is the client's claim, kept verbatim in canonical form (RFC 8785);
the hub adds the identifiers, the sequence number and the `content_hash`.
Attachments, relations, labels as addresses, tombstones, listing and the
query language are the next milestones (see the `evalhub_server` crate doc,
"Build order").

## License

AGPL-3.0-only. See `LICENSE` and `CONTRIBUTING.md`. The hosted service and the
self-hosted binary are the same code under the same license; there is no
separate proprietary edition.
