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
cargo sqlx prepare --check --workspace
cargo deny check
```

Do not run `cargo test --workspace` as the routine check: linking every test
binary in parallel is what exhausts memory on a shared machine. Run the crates
you touched, then the ones that depend on them.

Three things are contracts and have a dedicated check:

- **JSON Schema**: `crates/evalhub-schema/schemas/*.json` is generated from the
  Rust types and committed. A change to a record type is not done until
  `cargo insta test -p evalhub-schema` is green and the updated snapshot is in
  the same commit.
- **SQL**: `.sqlx/` holds the offline query data. A change to a query is not
  done until `cargo sqlx prepare --workspace` has been rerun and the result is
  in the same commit.
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
  keys, the API in one table, self-hosting). A new flag, endpoint or config key
  is not done until it is in there.
- `docs/`: guides and runbooks for things that are done by hand (releasing,
  migrating a database, standing up MinIO for local development). Not design,
  not architecture, not decision records.

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
- Generated files that are contracts (`schemas/*.json`, `.sqlx/`, the OpenAPI
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

## Working with coding agents

What this repository tells an agent is `AGENTS.md` at the root. It is a page of
pointers into this file, the README and the crate docs, not a second copy of
any of them.
