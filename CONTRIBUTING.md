# Contributing

Conventions for changes to this repository, for people and coding agents alike.
evalhub is one workspace of five crates (`evalhub-schema` / `-core` / `-query` /
`-store` / `-server`) and a `web/` SPA; the design is in the crate docs (see
Documentation below). The rules here are few, and each one exists because
skipping it has cost something at least once.

## Issues

Work starts from a GitHub issue. Open one for anything that is more than a typo:
a bug report, a feature, a refactor, a doc gap. An issue records the problem and
the evidence; the pull request records what was done about it.

### Labels

Assign at least one when you open an issue. The branch prefix follows the label.

| Label           | The change                                     | Branch      |
| --------------- | ---------------------------------------------- | ----------- |
| `bug`           | behaviour contradicts what it promises         | `fix/`      |
| `enhancement`   | behaviour that does not exist yet              | `feat/`     |
| `refactor`      | behaviour unchanged                            | `refactor/` |
| `chore`         | production code untouched — CI, tests, tooling | `chore/`    |
| `documentation` | prose only (README, docs/, doc comments)       | `docs/`     |

### What an issue says

- **Problem**: what happens, on what input. For API reports: the request
  (method, path, body with secrets removed), the response status and body,
  verbatim.
- **Evidence**: measured, not assumed. A timing says which build (debug or
  release) and how many records were in the database. A "validation error"
  quotes the `errors[]` entry.
- **Proposal**: optional. The 4-axis habit from the design discussions applies —
  a patch, an architecture change, a narrower requirement, or "not now" are all
  legitimate answers, and the issue should say which one it is asking for.
- **Acceptance**: what the API returns, what `cargo test -p <crate>` prints, or
  what the UI shows when it is done.

Keep internal paths and identifiers out of public issues: name the crate and
the file, not the machine it lives on. Never paste a token, a database URL or
an object storage credential, even an expired one.

## Branches

Never work on `main`. One branch per issue, named `<type>/<slug>` with the
prefix from the label table above.

## Verification

Green is defined per crate, because `evalhub-store` and `evalhub-server` need a
Postgres and an S3-compatible store (started by `testcontainers`, so Docker must
be running) while the other three run in seconds with no I/O:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p evalhub-schema     # includes the schema snapshot check
cargo test -p evalhub-core
cargo test -p evalhub-query
cargo test -p evalhub-store      # needs Docker
cargo test -p evalhub-server     # needs Docker
(cd crates/evalhub-store && cargo sqlx prepare --check)
cargo deny check
```

`just check` runs that list, and CI (`.github/workflows/ci.yml`) runs it,
`just e2e` and `just package` on every push and pull request. Do not run `cargo test --workspace` as the
routine check: linking every test binary in parallel is what exhausts memory
on a shared machine. Run the crates you touched, then the ones that depend on
them.

`just e2e` is the one check that runs the binary a user would: it builds the
UI and a release `evalhub`, starts a throwaway Postgres, migrates, serves
from outside the source tree, and fetches the embedded UI, an immutable
asset, a deep link and the API (`e2e/smoke.sh`), then drives the same server
with Playwright (`web/tests/browser`): the bundle boots, routes on the
client, and paints a list the API filled. Run it after a change to
`embed.rs`, `build.rs`, `main.rs` or the web build; it needs Docker. The
browser is fetched by `just e2e-install` into `~/.cache/ms-playwright`
without root; the recipe checks the host has the shared libraries the
headless shell links against and says what to do if not.

Packaging has its own gate, and it covers the SDK only. Three crates are
published to crates.io: `evalhub-schema`, `evalhub-core` and
`evalhub-query`, the ones a client or a converter links against.
`evalhub-store` and `evalhub-server` are the application; they are
`publish = false` and ship as the container image (`Dockerfile`), the way a
web application is not uploaded to npm. `just package` packages the three,
then inspects each `.crate`: LICENSE and README present, none over the
crates.io size limit. Publishing is not typed by anyone: the `release`
workflow does it from the tag (see Releases below).

The UI reaches the binary through `build.rs`, which refuses a release build
without `crates/evalhub-server/web-dist`; the image build and `just e2e`
are what exercise that, so a server with no UI cannot be built for
release, let alone shipped.

Three things are contracts and have a dedicated check:

- **JSON Schema**: `crates/evalhub-schema/schemas/*.json` is generated from the
  Rust types and committed. A change to a record type is not done until
  `cargo insta test -p evalhub-schema` is green and the updated snapshot is in
  the same commit.
- **SQL**: `crates/evalhub-store/.sqlx/` holds the offline query data. It
  lives in the crate, not at the workspace root, so that `cargo package`
  ships it and the crate builds without a database. A change to a query is
  not done until `cargo sqlx prepare` has been rerun from
  `crates/evalhub-store` and the result is in the same commit.
- **OpenAPI**: `GET /openapi.json` is what clients generate from. A change to a
  handler signature is checked by the server's OpenAPI snapshot test, and the
  `web/` client is regenerated in the same pull request.

Report what was run and on what; "store tests not run, no Docker here" is a
usable report, a green claim resting on a run nobody made is not.

## Documentation

The design lives in the code, rustdoc style, and it is written thick rather
than thin: the design is in there, not only the signatures. There is no design
document beside the code; `docs/` holds guides and nothing else.

Where each level of the design goes:

- **Crate level** (`//!` in `lib.rs`): what the crate is for, what it does not
  do, how its modules fit together, and why it is built that way. This is where
  the decisions go — the invariants (a version is write-once, the record is the
  client's claim and the id is the hub's fact), the rejected alternatives and
  the reason, the data flow through the crate. A reader who opens only this
  page should be able to say what the crate would refuse to do.
- **Module level** (`//!` in each module): the same for the module — the shape
  it owns (the fingerprint rule, the cursor encoding, the attachment state
  machine), the tables or endpoints it touches, and the ordering constraints
  between its functions. Diagrams are ASCII, in the doc comment.
- **Item level** (`///`): what the type or function promises, its error cases,
  and what it costs. How it does it today is a comment in the body, where it
  changes with the code.
- Doc comments on `JsonSchema` types are public: they become the `description`
  in the served schema and in `openapi.json`. Write them for the client author.

`#![warn(missing_docs)]` is on in every library crate. `cargo doc --no-deps
--open` is where a reader is sent, and a design question that `cargo doc` does
not answer is a doc bug: fix the comment, not a wiki. When a comment and an
issue or a chat disagree, the code wins, then the comment.

Beyond doc comments there are two places, and nothing else:

- README.md: the user-facing reference (running the server, configuration
  keys, the API in one table, where the image and the crates come from). A
  new flag, endpoint or config key is not done until it is in there.
- `docs/`: guides and runbooks for things that are done by hand (releasing,
  migrating a database, operating the hosted service). Not design, not
  architecture, not decision records.

Write a rule once, where the thing it constrains is defined, and link to it
from anywhere else; a second copy is the one nobody updates.

## Commits

```text
<subject: what changed, one line>

<prose: the problem, why this fix and not the alternative, what it cost>

Verified: <what was run, and the outcome>

Refs #<issue>
Signed-off-by: Your Name <you@example.com>
```

- Every commit carries a `Signed-off-by` line (`git commit -s` adds it). It is
  your statement that you have the right to submit the change under the
  project's license, as set out in the [Developer Certificate of
  Origin](https://developercertificate.org/). Use your real name; a pull
  request with an unsigned commit is not merged.
- Formatting and clippy fixes go in their own commits.
- Generated files that are contracts (`schemas/*.json`,
  `crates/evalhub-store/.sqlx/`, the OpenAPI
  snapshot, the `web/` client) ride in the same commit as the change that
  invalidated them.
- Never commit anything `.gitignore` excludes: local working areas, agent
  state, data. If a commit needs `git add -f`, stop: something is filed wrong.
- The version bump (`[workspace.package] version` and the internal dependency
  versions in `Cargo.toml`) rides with the last change of a release, not in a
  commit of its own; that commit's subject ends with the version, `(0.1.N)`,
  which is how the history reads as a release log.

## License and contributions

evalhub is licensed under the GNU Affero General Public License, version 3
only (`LICENSE`). By contributing you agree that your contribution is licensed
under the same terms. There is no contributor license agreement to sign; the
project uses the Developer Certificate of Origin instead, asserted per commit
with the `Signed-off-by` line described above.

The hosted service and the self-hosted binary are the same code under the same
license. There is no separate proprietary edition, and features are not held
back from the open source release.

## Pull requests

One issue per pull request, against `main`. Before opening it, run the
verification list above on the final tree, including the Docker-backed crates
when the change touches them.

The body records what changed, what was verified (the commands and their
outcome), and what it deliberately does not cover, and ends with
`Refs #<issue>`. Longer bodies are easier to write as a file and pass with
`--body-file`; keep that file somewhere `.gitignore` excludes.

## Releases

A release is a pull request whose last commit carries the version bump
(above), merged, then tagged. The tag push is the only release action a
person takes; `.github/workflows/release.yml` does the rest from it:

1. `check`: the tag names the version in `Cargo.toml` and the tagged
   commit is on `main`. Otherwise nothing below runs.
2. `crates`: `just package`, then `cargo semver-checks` against the
   versions on crates.io, then `cargo publish` for whichever of
   `evalhub-schema`, `evalhub-core` and `evalhub-query` is not yet on
   crates.io at that version. Authentication is crates.io Trusted
   Publishing (OIDC from this repository and this workflow file); no
   token is stored. A rerun publishes nothing twice.
3. `image`: `ghcr.io/ynishi/evalhub:<version>` and `:latest`.
4. `release`: a GitHub Release for the tag, its notes taken from the
   merged pull request's body, so write that body as the release notes.

`deploy/release/land.sh <pr> <issue>` merges the pull request (a merge
commit, never a squash: the commits carry the `Signed-off-by`), closes the
issue, fast-forwards `main`, waits for its CI, tags, pushes the tag and
follows the workflow to the end. `deploy/release/tag.sh` is the second
half on its own. Both refuse to tag a commit whose CI is not green or
whose version does not match.

Merges are merge commits only; squash and rebase merges rewrite the
commits and drop the sign-off. A break in an SDK crate's public API needs
a higher version than a patch (0.1.x → 0.2.0 while on 0.x); `just
semver-check` says so before the workflow does.

The hosted service is not deployed by the workflow; that is `fly deploy`,
by hand (`docs/hosting.md`).

## Working with coding agents

What this repository tells an agent is `AGENTS.md` at the root. It is a page of
pointers into this file, the README and the crate docs, not a second copy of
any of them.
