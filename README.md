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
anyone the token does not cover. A token acts only in the namespaces it
names, so a personal token does not reach an organisation: ask for one that
covers it.

### Records

`{cards|evals}` below is one or the other; the two behave identically.

| Method   | Path                                            | Does                                                                                     |
| -------- | ----------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `POST`   | `/{cards\|evals}/{ns}/{name}?label=`            | Validate, canonicalise and append a version. `201` with `{ id, version_id, seq, label, content_hash, created_at, changed[], badges[] }`; `200` and the existing version when the canonical body equals the latest; `422 { errors[] }` with every violation; `409 attachment_missing` or `label_in_use`. |
| `GET`    | `/{cards\|evals}/{ns}/{name}[@{seq}\|@{label}]` | The latest live version, or one addressed by sequence number or label, with the canonical `record`. `?expand=fingerprints,badges,changed` adds the hub's derived facts. A tombstoned version comes back with `tombstone` and no `record`. |
| `GET`    | `/{cards\|evals}/{ns}/{name}/versions`          | Every version, oldest first, tombstones included, without bodies.                          |
| `PATCH`  | `/{cards\|evals}/{ns}/{name}@{seq}/label`       | Point a label at that version. Labels are unique within the name and never purely numeric. |
| `PATCH`  | `/{cards\|evals}/{ns}/{name}/settings`          | `{ "visibility": "public" \| "private" }`.                                                 |
| `DELETE` | `/{cards\|evals}/{ns}/{name}@{seq}`             | Tombstone with `{ "reason": "withdrawn\|duplicate\|takedown\|other", "note": … }`. The body goes; the version id, the content hash and `changed[]` stay. |
| `GET`    | `/{cards\|evals}?ns&search&sort&cursor&limit`   | Page over names with a live version. `sort` is `created_desc` (default), `created_asc` or `name_asc`; paging is by opaque cursor. |

### Identity

| Method   | Path                              | Does                                                                     |
| -------- | --------------------------------- | ------------------------------------------------------------------------ |
| `GET`    | `/whoami`                         | The token's user, scope, namespaces and organisation roles.              |
| `GET`    | `/tokens`                         | The caller's tokens, by prefix. Secrets are shown once, at issue.        |
| `POST`   | `/tokens`                         | Issue one: `{ scope, namespaces[] }`, capped by the presenting token's scope and by the caller's own login and admin organisations. |
| `DELETE` | `/tokens/{token_id}`              | Revoke one of the caller's tokens, effective at once.                    |
| `GET`    | `/namespaces/{ns}`                | Kind, creation time, and counts of what the caller may see.              |
| `POST`   | `/orgs`                           | Create an organisation; the caller becomes its first `admin`.            |
| `GET`    | `/orgs/{org}/members`             | The roster. Any member may read it.                                      |
| `POST`   | `/orgs/{org}/members`             | `{ user, role }`; `admin` on the organisation.                           |
| `DELETE` | `/orgs/{org}/members/{user}`      | Remove a member; `admin` on the organisation.                            |
| `GET`    | `/audit?ns&cursor&limit`          | The namespace's append-only log, newest first; `admin` on the namespace. |
| `GET`    | `/healthz`                        | Liveness.                                                                |

A record is the client's claim, kept verbatim in canonical form (RFC 8785);
the hub adds the identifiers, the sequence number, the `content_hash`, the
seven per-facet fingerprints and the badges. Badges name facts the hub
checked (`refs_resolved`, `env_pinned`, `redacted`, and, once the registry
exists, `harness_registered` and `metric_registered`); there is no
`verified` badge, because the hub did not run anything.

### Attachments and relations

| Method   | Path                                              | Does                                                                        |
| -------- | ------------------------------------------------- | --------------------------------------------------------------------------- |
| `POST`   | `/attachments`                                    | Announce `{ sha256, size, media_type }`. `201` with a presigned `PUT` URL, or `200 { state: "ready" }` when those bytes are already stored. Any valid token. |
| `POST`   | `/attachments/{sha256}/complete`                  | Confirm the upload. Checks the size and, below `attachments.hash_verify_max_bytes`, re-hashes the bytes; a mismatch is `422`. |
| `GET`    | `/attachments/{sha256}`                           | `302` to a presigned, time-limited download. Open to everyone once a public record references the object. |
| `HEAD`   | `/attachments/{sha256}`                           | Size and media type, same authorisation.                                    |
| `POST`   | `/{cards\|evals}/{ns}/{name}@{seq}/relations`     | Add an edge: `{ type, to, attrs }`, where `to` is `{ns}/{name}@{seq}`, `external:<url>` or `hf:<repo>@<sha>`. |
| `GET`    | `/{cards\|evals}/{ns}/{name}[@…]/relations`       | Walk the graph: `direction=out\|in\|both`, `depth=1..5`, `types=`, `follow_latest=`. Returns `{ nodes, edges }`. |
| `GET`    | `/evals/{ns}/{name}[@…]/cards`                    | The comparison view: Cards measured on this Eval, with their fingerprints and `same_harness` / `same_model`. `group_by=fingerprint.{facet}` groups them. |

The hub never carries attachment bytes: uploads and downloads are
presigned URLs straight to the object store, and the hub records only the
sha256. A deployment without `s3.endpoint` and its credentials answers
`503` on these four routes and works normally otherwise.

A node the caller may not see appears as `{ private: true, version_id,
content_hash }`: enough to know the edge is pinned to an exact version,
not enough to learn what it is. That is what lets a public Card cite a
private Eval. The comparison view lines Cards up and labels which axes
agree; it does not rank them.

The query language, the registry and the web UI are the remaining
milestones (see the `evalhub_server` crate doc, "Build order").

## License

AGPL-3.0-only. See `LICENSE` and `CONTRIBUTING.md`. The hosted service and the
self-hosted binary are the same code under the same license; there is no
separate proprietary edition.
