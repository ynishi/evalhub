# web

The evalhub web UI: a SvelteKit application built with `adapter-static` in SPA
mode, compiled to `crates/evalhub-server/web-dist/`, which `evalhub-server`
embeds with `rust-embed` and serves under `/`.

The UI is one client of `/api/v1`. It has no route the API does not have, and
it holds no privileged path into the hub; that it works from
`GET /openapi.json` alone is the standing proof that the contract is complete
for other clients too.

## Running it

`pnpm` is not installed globally on the development machine, and does not need
to be: Node 22 ships `corepack`, which resolves pnpm into a user cache with no
`sudo` and no global install. Every command below is therefore
`corepack pnpm …`.

```bash
corepack pnpm install
corepack pnpm dev        # http://localhost:5173, API proxied to :8080
corepack pnpm build      # writes ../crates/evalhub-server/web-dist
corepack pnpm check      # svelte-check: types and Svelte diagnostics
corepack pnpm lint       # prettier --check
corepack pnpm format     # prettier --write
```

`pnpm dev` proxies `/api`, `/schemas` and `/openapi.json` to
`http://127.0.0.1:8080`, so a hub started with
`cargo run -p evalhub-server -- serve` answers the UI and the session cookie
stays same-origin in development, exactly as it is in production.

## The generated client

```bash
corepack pnpm run generate-client                      # a running hub
corepack pnpm run generate-client ../path/openapi.json # a file
```

`openapi-typescript` turns the hub's OpenAPI document into
`src/lib/api/schema.d.ts`, and `openapi-fetch` calls it. That file is
**committed**: it is a contract artifact like `.sqlx/` and
`crates/evalhub-schema/schemas/*.json`. Regenerate it in the same pull request
as any handler change that moves the contract — the same rule `CONTRIBUTING.md`
states for the other three.

The file argument exists because a contributor without a database still needs
to regenerate; the server crate's snapshot test writes the same document to
`crates/evalhub-server/tests/snapshots/boot__openapi.snap`.

Everything that talks to the hub goes through `src/lib/api/client.ts`. No
component calls `fetch`: the session cookie needs `credentials: 'include'` on
every request, and a `401` anywhere has to return the reader to the login
screen, which is one middleware there rather than a habit in thirty places.

### The filter type

The served document embeds the query grammar's own JSON Schema at
`QueryRequest.where`. The hub hoists that schema's definitions into
`components/schemas` as it splices them, so the client gets a real
`QueryFilter` type to name rather than a pointer into a `$defs` section the
OpenAPI document does not have. `scripts/generate-client.mjs` refuses to
generate if a `#/$defs/…` reference ever reappears, because a client built
from such a document loses the filter type quietly.

## Auth

The SPA never holds a token in JavaScript. The login screen takes one,
`POST /api/v1/session` exchanges it for a private cookie the script cannot
read, and every later request relies on that cookie.
`DELETE /api/v1/session` revokes it. `GET /api/v1/whoami` decides what the
masthead shows and which write controls are worth rendering — the hub checks
permissions again on every request, so a hidden control is a courtesy, never
the enforcement.

## How the build is embedded

`crates/evalhub-server/src/embed.rs` reads `web-dist/` in that crate at
compile time, relative to its `Cargo.toml`. The output lands inside the server
crate rather than here because `cargo package` only ships files under a
crate's root; that is what lets the crate on crates.io carry the UI.

`web-dist/` is **not** committed — `crates/evalhub-server/.gitignore`
excludes it, and the crate's `include` list in `Cargo.toml` is what puts it in
the `.crate` anyway. `node_modules` and `.svelte-kit` are excluded here.
`pnpm-lock.yaml` **is** committed.

A debug build of a checkout without a built UI still compiles and runs: the
server serves a placeholder page naming the contract. A release build refuses
to compile without `web-dist/` (see `crates/evalhub-server/build.rs`), so a
binary that ships always has the UI in it.

To see the UI inside the binary:

```bash
just web-build
cargo run -p evalhub-server -- serve --bind 127.0.0.1:8080
```

## Screens (v0)

| Screen        | Route                | What it is for                                                                                                   |
| ------------- | -------------------- | ---------------------------------------------------------------------------------------------------------------- |
| Home          | `/`                  | What the hub is, and what it refuses to do.                                                                      |
| List / search | `/cards`, `/evals`   | Filter by namespace, search, sort, page by cursor; and a query builder over the typed filter language.           |
| Card page     | `/cards/{ns}/{name}` | Title, the seven facets with fingerprints, results, counts, badges, attachments, relations, version history.     |
| Eval page     | `/evals/{ns}/{name}` | Runs, attachments, and the comparison view: the Cards measured on this Eval, groupable by any facet fingerprint. |
| Namespace     | `/ns/{ns}`           | What a namespace holds, and the roster when it is an organisation the caller may see.                            |
| Settings      | `/settings`          | Issue and revoke tokens; change a record's visibility; move a label.                                             |
| Registry      | `/registry`          | Browse metrics, harnesses, relation types and ext schemas, with the `applying` state visible.                    |
| Login         | `/login`             | Exchange a token for a session.                                                                                  |

The query builder walks its path vocabulary out of `GET /schemas/card` and
`GET /schemas/eval` rather than a hand-written list, so a key added to the
record schema is offered in the same release. What the browser cannot know is
which paths are _indexed_ — that follows from the migration, not the schema —
so the builder offers every operator the type allows and shows the hub's
`not_indexed` answer against the row that caused it.

Not in v0, deliberately: creating records or uploading attachments from the UI
(a harness does that through the API), charts, and relation-graph drawing.

## Design

Plain, legible, fast: a table-first information tool. System fonts, a light and
a dark palette driven by `prefers-color-scheme`, no UI framework. Real `<table>`
markup, labelled controls, visible focus.

One rule runs through every screen: a number in this hub is a producer's claim,
not a finding. The UI shows results as reported, shows badges as the facts the
hub checked, and shows `same_model` / `same_harness` as agreement on an axis.
There is no ranking and no winner column, because the hub does not have that
opinion to give.

## Dependencies

| Package                               | Why                                                  |
| ------------------------------------- | ---------------------------------------------------- |
| `@sveltejs/kit`, `svelte`, `vite`     | The framework and the build.                         |
| `@sveltejs/adapter-static`            | SPA output: one `index.html` plus hashed assets.     |
| `openapi-typescript`, `openapi-fetch` | The generated client and the typed calls through it. |
| `svelte-check`, `typescript`          | The type check that `pnpm check` runs.               |
| `prettier`, `prettier-plugin-svelte`  | Formatting, checked by `pnpm lint`.                  |

Nothing else. There is no component library, no state-management package and no
icon set: the screens are tables and forms, and the styling is one stylesheet.
