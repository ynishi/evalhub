# evalhub

A hosting service for LLM evaluation results. A **Card** says what was
measured, how, and what the score was. An **Eval** is the material the Card was
measured from: a set of runs, prompts, tasks or traces. Both are published under
a name, `{ns}/{name}`, as an immutable sequence of versions, and can be public
or private, searched, and linked to each other.

The hub validates, indexes and lets you compare. It does not run evaluations,
does not rewrite what it receives, and never calls anything "verified".

## Status

Pre-alpha. The API is complete: records with validation, fingerprints and
badges, an Eval's runs and their projection with Cards' judgements,
attachments, relations, the query language, the registry and the
audit log. The web UI is compiled into the binary; a debug build without it
serves the API and a placeholder page. Read the design in the crate docs:

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

`compose.yml` starts the same thing with Docker for local development
(Postgres, MinIO and the hub, nothing tuned or secured). The hosted service
is deployed from `Dockerfile` and `fly.toml`; `docs/hosting.md` is its
runbook. The application is distributed as that image, not through
crates.io: the published crates (`evalhub-schema`, `evalhub-core`,
`evalhub-query`) are the SDK a client or a converter links against.

Configuration is layered: defaults, then a TOML file (`--config` or
`EVALHUB_CONFIG`, or `evalhub.toml` in the working directory), then
`EVALHUB_*` environment variables (nested keys joined with `__`, e.g.
`EVALHUB_DATABASE__URL`), then flags. `config show --origin` prints every
key with the layer that set it; secrets are redacted.

What one request may carry is configured in the `[limits]` table of the
file (or `EVALHUB_LIMITS__BODY_BYTES` and so on); a request over a limit
is refused, never truncated:

| Key                  | Default             | Over it                          |
| -------------------- | ------------------- | -------------------------------- |
| `limits.body_bytes`  | `16777216` (16 MiB) | `413 body_too_large`, any endpoint |
| `limits.batch_runs`  | `1000`              | `413 batch_too_large` on `runs:batch` |
| `limits.run_results` | `100000`            | `422 too_many_run_results` on a Card |

```toml
[limits]
body_bytes = 16777216
batch_runs = 1000
run_results = 100000
```

Read pages are not configurable: `limit` on the record, run and audit
listings is 1–200 (default 50), clamped rather than refused.

`serve` refuses to start without a database, and with one it refuses to
start until `evalhub migrate` has brought the schema current.

## API

Everything is under `/api/v1`; the contract is `GET /openapi.json` (OpenAPI
3.1) and the record schemas are `GET /schemas/{card|eval-2|run|eval|error|query}`.
`eval-2` is the Eval header (`evalhub.eval/2.0`) and `run` one run;
`card` covers `evalhub.card/1.1` and the `evalhub.card/1.0` it still
accepts; `eval` stays the 0.1.x document (`evalhub.eval/1.0`, with
`runs[]`) that 0.2.0 still accepts, until 0.3.0 removes it.
Writes need `Authorization: Bearer <token>` with `write` on the namespace;
reads of public records need nothing, and private records are `404` to
anyone the token does not cover. A token acts only in the namespaces it
names, so a personal token does not reach an organisation: ask for one that
covers it.

### Records

`{cards|evals}` below is one or the other; the two behave identically.

| Method   | Path                                            | Does                                                                                     |
| -------- | ----------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `POST`   | `/{cards\|evals}/{ns}/{name}?label=`            | Validate, canonicalise and append a version. `201` with `{ id, version_id, seq, label, content_hash, created_at, changed[], badges[] }`; `200` and the existing version when the canonical body equals the latest; `422 { errors[] }` with every violation; `409 attachment_missing` or `label_in_use`. An Eval header carrying `runs` is `422 runs_moved` (runs are written below); a Card with more `run_results` than `limits.run_results` is `422 too_many_run_results`. |
| `GET`    | `/{cards\|evals}/{ns}/{name}[@{seq}\|@{label}]` | The latest live version, or one addressed by sequence number or label, with the canonical `record`. `?expand=fingerprints,badges,changed` adds the hub's derived facts. A tombstoned version comes back with `tombstone` and no `record`. An Eval also carries `runs: { count, by_status, archived, deleted, runs_hash }` (the same at every `@seq`; `archived` / `deleted` for members of the namespace only). Card judgements of an Eval the reader may not see are removed and counted in `withheld.run_results`. |
| `GET`    | `/{cards\|evals}/{ns}/{name}/versions`          | Every version, oldest first, tombstones included, without bodies.                          |
| `PATCH`  | `/{cards\|evals}/{ns}/{name}@{seq}/label`       | Point a label at that version. Labels are unique within the name and never purely numeric. |
| `PATCH`  | `/{cards\|evals}/{ns}/{name}/settings`          | `{ "visibility": "public" \| "private" }`.                                                 |
| `DELETE` | `/{cards\|evals}/{ns}/{name}@{seq}`             | Tombstone with `{ "reason": "withdrawn\|duplicate\|takedown\|other", "note": … }`. The body goes; the version id, the content hash and `changed[]` stay. |
| `GET`    | `/{cards\|evals}?ns&search&sort&cursor&limit`   | Page over names with a live version. `sort` is `created_desc` (default), `created_asc` or `name_asc`; paging is by opaque cursor. |

**The `evalhub.eval/1.0` body is deprecated.** 0.2.0 still accepts a
0.1.x Eval body (header and `runs[]` in one) on `POST /evals/{ns}/{name}`:
it is stored as a 2.0 header plus run rows, and the response carries a
`Deprecation` header, `converted_from: "evalhub.eval/1.0"` and
`converted_runs`. **0.3.0 removes this**; post the header, then the runs
(see Upgrading from 0.1.x, below).

### Runs

An Eval's runs are rows of the Eval, not versions: written by `run_id`,
overwritten in place, archived and deleted without appending a header
version. They follow the Eval's visibility; every route below is `404`
for an Eval the caller may not see. Writes need `write` on the namespace.

| Method   | Path                                           | Does                                                                                     |
| -------- | ---------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `PUT`    | `/evals/{ns}/{name}/runs/{run_id}`             | Write one run (`GET /schemas/run`; its `run_id` must equal the path's). `201` created, `200` with `result: updated` or `unchanged`; body `{ run_id, content_hash, status, result, runs_hash }`. `409 run_deleted` for a deleted id, `409 attachment_missing`, `422` otherwise. |
| `POST`   | `/evals/{ns}/{name}/runs:batch`                | `{ runs: [Run, …] }`, all or nothing: on failure nothing is written and every failing element is listed (`/runs/{index}/…`). `200 { runs: [{ run_id, content_hash, result }], runs_hash }`. More than `limits.batch_runs` is `413 batch_too_large`. |
| `GET`    | `/evals/{ns}/{name}/runs?cards&where&sort&limit&cursor&include` | Runs × metrics × each named Card's judgements. `cards={ns}/{name}` repeats; `where` is the query grammar as JSON text over the run paths below; `sort={path}[:asc\|:desc]` repeats, `run_id` is always the last key; `limit` 1–200; `include=archived,deleted` for members. Per Card: `runs_used`, `used_set_hash`, `changed_since_card`. A Card that is unknown, private to the caller or not using the Eval is `404`. |
| `GET`    | `/evals/{ns}/{name}/runs/{run_id}`             | One run. Archived runs are `404` to non-members; a deleted run is `{ run_id, content_hash, tombstone }`. |
| `PATCH`  | `/evals/{ns}/{name}/runs/{run_id}`             | `{ "archived": true \| false }`. Hides a run from non-members and the counts; no hash changes. |
| `DELETE` | `/evals/{ns}/{name}/runs/{run_id}`             | Tombstone with `{ "reason", "note" }`. The body and attachment references go; the id, content hash and metrics stay, and the id is never written again. |

The paths `where` and `sort` accept on `GET …/runs` are the run's, not a
record's. Parameters are in brackets, because a metric id or a Card
name contains `/` and may contain `.`; the dotted forms
(`metrics.core/tokens_out`) are `422 unknown_path` with a hint.

| Path                                   | Type              | Is                                                     |
| -------------------------------------- | ----------------- | ------------------------------------------------------ |
| `run_id`, `status`, `error.kind`       | string            | the run's id, `ok` / `error` / `skipped`, the error slug |
| `started_at`, `ended_at`               | string (RFC 3339) | compared as timestamps; `prefix` / `contains` refused  |
| `{facet}.{key…}` (`model.id`, `generation.temperature`, `env.git.commit`, …) | as in the schema | the run's own facets (six, no `grading`), header defaults copied in |
| `fingerprint.{facet}`                  | string            | the run's fingerprint of that facet                    |
| `metrics[{ns}/{name}]`                 | number            | a metric the run measured; registered or not           |
| `results[{card}][{metric}].value`      | number            | that Card's judgement of the run; `{card}` must be in `cards=` |
| `results[{card}][{metric}].label`      | string            | the same, as a label                                   |
| `ext.*`                                | as for records    | the run's own `ext`; an unregistered key answers `eq` / `exists` only |

Every typed path takes every operator its type allows, sorting
included: the projection is scoped to one Eval, so facet keys range and
sort here although they do not in a record query. The one exception is
an unregistered `ext` key, which is `422 not_indexed` for anything but
`eq` / `exists`, sorting included. There are no array paths, so no
`any`.

```bash
curl -G https://hub.example/api/v1/evals/alice/loop/runs \
  --data-urlencode 'cards=alice/judge' \
  --data-urlencode 'where={"and":[{"path":"status","op":"eq","value":"ok"},{"path":"results[alice/judge][core/pass].value","op":"gte","value":1}]}' \
  --data-urlencode 'sort=metrics[core/tokens_out]:desc' \
  --data-urlencode 'limit=100'
```

A page is `{ cards, items, next_cursor }`; pass `next_cursor` back as
`cursor`. A cursor is bound to the Eval and the `sort` it was issued for
and is `400` anywhere else.

### Identity

| Method   | Path                              | Does                                                                     |
| -------- | --------------------------------- | ------------------------------------------------------------------------ |
| `POST`   | `/session`                        | Exchange a token for a session cookie: `{ token }`. The hub has no passwords, so a token is the credential. |
| `DELETE` | `/session`                        | End the session, revoking the token its cookie carries.                  |
| `GET`    | `/whoami`                         | The token's user, scope and namespaces.                                  |
| `GET`    | `/tokens`                         | The caller's tokens, by prefix. Secrets are shown once, at issue.        |
| `POST`   | `/tokens`                         | Issue one: `{ scope, namespaces[] }`, capped by the presenting token's scope and by the caller's own login and admin organisations. |
| `DELETE` | `/tokens/{token_id}`              | Revoke one of the caller's tokens, effective at once.                    |
| `GET`    | `/namespaces/{ns}`                | Kind, creation time, and counts of what the caller may see.              |
| `POST`   | `/orgs`                           | Create an organisation; the caller becomes its first `admin`.            |
| `GET`    | `/orgs/{org}/members`             | The roster. Any role, with a token that names the organisation.          |
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
| `GET`    | `/evals/{ns}/{name}[@…]/cards`                    | The comparison view: Cards measured on this Eval, with their fingerprints, `same_harness` / `same_model` (against every run each Card used), `runs_used`, `used_set_hash` and `changed_since_card`. `group_by=fingerprint.{facet}` groups them. |

The hub never carries attachment bytes: uploads and downloads are
presigned URLs straight to the object store, and the hub records only the
sha256. A deployment without `s3.endpoint` and its credentials answers
`503` on these four routes and works normally otherwise.

A node the caller may not see appears as `{ private: true, version_id,
content_hash }`: enough to know the edge is pinned to an exact version,
not enough to learn what it is. That is what lets a public Card cite a
private Eval. The comparison view lines Cards up and labels which axes
agree; it does not rank them.

### Search, registry and export

| Method | Path                                    | Does                                                                          |
| ------ | --------------------------------------- | ------------------------------------------------------------------------------ |
| `POST` | `/{cards\|evals}/query`                 | A typed filter over the record schema. Body is `{ where, sort, limit, cursor, expand, version }`; the grammar is in `GET /schemas/query`. |
| `GET`  | `/registry/{kind}?ns&limit&offset`      | The vocabulary: `metrics`, `harnesses`, `relation_types`, `ext_schemas`.       |
| `GET`  | `/registry/{kind}/{ns}/{id}@{version}`  | One entry.                                                                     |
| `PUT`  | `/registry/{kind}/{ns}/{id}@{version}`  | Register one; `write` on `ns`, `core/` is read-only, entries are immutable. An `ext_schemas` entry answers `202` while its indexes build. |
| `GET`  | `/{cards\|evals}/{ns}/{name}[@…]/export?format=` | `hf-model-index` projects a Card onto the YAML a Hugging Face model card embeds. |

A query is checked before it runs. Every path is looked up in the record
schema, so a typo is `422 unknown_path` rather than an empty page — an
empty page for a typo looks like data. Every operator is checked against
what an index can serve, so a range over a key with no index of its own
is `422 not_indexed` rather than a sequential scan that works until the
table grows. What the check accepts is:

| Operator                 | On                            |
| ------------------------ | ----------------------------- |
| `eq`, `ne`, `in`         | any scalar path               |
| `gt`, `gte`, `lt`, `lte` | numbers and strings, indexed  |
| `prefix`, `contains`     | strings, indexed              |
| `exists`                 | any path                      |
| `any` with `match`       | `results`, `relations`, `attachments` |

The registry is what turns a guess into a declaration. An extension key
under `ext.{ns}/{name}` answers equality from the index over the whole
record; registering an `ext_schemas` entry for it builds a typed index and
makes it rangeable and sortable. A metric can be sorted on only once it is
registered, because `lower_is_better` is what says which way is up.
Registering a metric or a harness also earns the records that cite it
their `metric_registered` / `harness_registered` badge, on the next sweep.

`export?format=bundle` is `501`: tarring a record's attachments would
stream them through the hub, which the presigned-URL design exists to
avoid, and the alternative has not been chosen yet.

## Upgrading from 0.1.x

0.2.0 moves an Eval's runs out of its body. The versioned body is now a
header; the runs are rows of the Eval, written by `run_id`, and the
verdict on a run is the Card's. What a 0.1.x client changes:

| 0.1.x                                                   | 0.2.0                                                                                   |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| `POST /evals/{ns}/{name}` with `runs[]` in the body     | `POST` the header without `runs`, then `PUT …/runs/{run_id}` per run or `POST …/runs:batch` |
| `runs[].outcome` (`pass` / `fail` / `error` / `skipped`) | the run's `status` (`ok` / `error` with `error.kind` / `skipped` with `skip_reason`), and the pass / fail on the Card, in `run_results[]` |
| `"schema": "evalhub.eval/1.0"`                          | `"schema": "evalhub.eval/2.0"` (`GET /schemas/eval-2`)                                   |
| `"schema": "evalhub.card/1.0"`                          | `"schema": "evalhub.card/1.1"`, which adds `run_results`; `evalhub.card/1.0` is still accepted |

A 1.0 Eval body with `runs[]` is accepted by 0.2.0 only: it is converted
into a 2.0 header and run rows, and the response carries a `Deprecation`
header. 0.3.0 refuses it with `422 runs_moved`, as 0.2.0 already refuses a
2.0 header that carries `runs`. Two things a 1.0 client will notice in
0.2.0 already:

- the `content_hash` it gets back is the converted 2.0 header's, not the
  hash of the body it posted;
- re-posting a body that leaves a run out no longer drops that run. Runs
  are rows now; archive (`PATCH …/runs/{run_id}`) or delete it.

On a hosted database, `evalhub migrate` for 0.2.0 does the same
conversion to every stored Eval once (`docs/hosting.md`, "Upgrading to
0.2.0"). Header `content_hash` values change, with the old and the new
hash of every version in the audit log (`migration.runs_split`).
Versions that differed only in `runs` become identical header versions.
A Card's relations keep resolving, because they point at version ids,
not hashes, and each Card's used runs are rebuilt from the Eval version
it points at.

## Web UI

The single-page application in `web/` is one more client of `/api/v1`: it
has no route the API does not have, which is what keeps the contract honest
for everyone else. It is compiled into the binary, so a self-host is still
one file and there is no second origin to configure.

```bash
just web-build                          # pnpm, into crates/evalhub-server/web-dist
cargo build --release -p evalhub-server
```

A debug build of a checkout without `web-dist/` still builds and runs: the
asset set is empty and any browser path gets a placeholder that names
`/openapi.json`. A debug build reads the files from disk at request time, so
`just web-build` shows up on reload. A release build bakes them in, and
refuses to build without them unless `EVALHUB_ALLOW_MISSING_UI=1` says the
omission is deliberate. `just package` builds the UI, packages every crate
and checks that the server's `.crate` carries it.

The UI does not hold a token in JavaScript. `POST /api/v1/session` takes one
and returns a cookie — private, `HttpOnly`, `SameSite=Strict`, and `Secure`
unless the server binds a loopback address — carrying a second token minted
for the same user with the same scope and namespaces. `DELETE
/api/v1/session` revokes that one and clears the cookie, leaving the token
that opened the session alone. Set `auth.cookie_key` in production: without
it the key is generated per process and every session ends at a restart.

## License

AGPL-3.0-only. See `LICENSE` and `CONTRIBUTING.md`. The hosted service and the
self-hosted binary are the same code under the same license; there is no
separate proprietary edition.
