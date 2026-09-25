# Hosting evalhub

The runbook for the hosted service: one Fly.io app built from `Dockerfile`
and configured by `fly.toml`, a Postgres, and a Tigris bucket. The design is
in the crate docs (`cargo doc --no-deps --open`, start at `evalhub_server`);
this page is the operational side only.

This is not a self-hosting guide. The application is distributed as the
container image, and the same image runs anywhere that can give it a
Postgres and an S3-compatible store; but the file that is kept deployable
is `fly.toml`, and `compose.yml` is for local development only.

## What a deployment is

One binary, `evalhub`, with the web UI inside it, plus:

- **Postgres** (16 or later). Records, versions, tokens, the registry, the
  query index. The schema is applied by `evalhub migrate`; `serve` refuses
  to start while migrations are pending.
- **An S3-compatible object store** for attachments. Optional: without
  `s3.endpoint` and credentials the attachment endpoints answer `503` and
  everything else works. The bucket must exist; the hub does not create it.

Configuration is layered (defaults, then a TOML file, then `EVALHUB_*`
variables with `__` joining nested keys, then flags) and `evalhub config
show --origin` prints every effective value with the layer that set it.
Secrets are redacted there but their origin is shown. That command is the
first thing to run when a deployment misbehaves:

```bash
fly ssh console --app evalhub -C "evalhub config show --origin"
```

## The image

`Dockerfile` builds the image in three stages: the SvelteKit UI, the
release binary (which refuses to build without the UI), and a slim Debian
with CA certificates. It binds `0.0.0.0:8080`, runs as an unprivileged
user, and its default command is `serve`. Fly builds it from the checkout
on every `fly deploy`; the `release` workflow also pushes it to
`ghcr.io/ynishi/evalhub` on every `v*` tag as `:<version>` and `:latest`,
alongside the SDK crates and the GitHub Release (CONTRIBUTING
§Releases), which is the artifact for anyone running it elsewhere.

```bash
docker build -t evalhub .
docker run --rm evalhub --version
```

## Standing it up

Once, with `flyctl` logged in:

```bash
fly auth login
bash deploy/fly/up.sh
```

The script is the runbook; it does, in order:

1. `fly apps create` for the app named in `fly.toml`. That line is the
   one place the name is set: the scripts read it, the Postgres and the
   bucket are named after it, and the URL is `https://<app>.fly.dev`
   (or `EVALHUB_URL` once a custom domain is in front). `evalhub` is a
   global Fly name; a second deployment of this repository changes it.
2. **Postgres.** `fly postgres create`: a single unmanaged node, the
   cheapest way to find out whether the service is worth running (a few
   dollars a month). Fly does not support it; Managed Postgres (`fly mpg
   create`, from $38/month) is the upgrade path when the answer is yes.
   Then `fly postgres attach`, which creates a database and a user on
   the cluster and sets the URL on the app as `EVALHUB_DATABASE__URL`.
3. **Attachments.** `fly storage create` for a Tigris bucket. Fly sets
   its keys on the app under `AWS_*` names the hub does not read; the
   script copies them into `EVALHUB_S3__*`.
4. **Auth keys.** `auth.cookie_key` and `auth.cursor_key`, generated
   with `openssl rand`. Each is any string, hashed before use; unset,
   each is generated per process, every session ends at a restart, and
   two Machines do not agree with each other.
5. `fly deploy --ha=false`: build, `migrate` as the release command,
   one Machine (Fly adds a second for availability by default, which the
   trial does not need).
6. `GET /api/v1/healthz` must answer `200`.

Three of those flyctl commands print credentials to the terminal by
design (the Postgres superuser password, the attached connection URL,
the bucket's keys). The script captures each, passes the values to `fly
secrets import` on stdin, and shows only redacted lines, so nothing
secret is on screen or in a terminal log. Run the commands by hand and
that protection is gone; the Postgres superuser password in particular
is shown once and never again, and the script deliberately does not keep
it (admin access is `fly postgres connect --app evalhub-db`, which needs
none).

`s3.public_endpoint` is left unset, so presigned URLs are minted for the
same Tigris endpoint the hub talks to; browsers can reach it. `s3.region`
is `auto` in `fly.toml`, which is what Tigris expects.

`bash deploy/fly/down.sh` destroys all three (bucket, app, Postgres) and
every byte in them.

## First user

There is no signup endpoint. The operator creates users and the first
token of each:

```bash
bash deploy/fly/user.sh alice          # scope admin; `user.sh alice write` for less
```

That runs `evalhub user create` on the Machine, which prints the token
once and never again, and writes it to `~/.config/evalhub/token` (mode
600) without showing it. Run the command by hand over `fly ssh console`
and the token is on your screen and in any terminal log instead. A login
cannot be created twice; a lost token means a new login, or `POST
/tokens` with one that still works. Everything after the first token
(organisations, more tokens, visibility) is done through the API or the
UI.

## An organisation

A namespace with members; the one the operator publishes imported
results under is one of them. It is made in the UI, in two steps,
because a token's namespaces are fixed when it is issued:

1. Settings → Organisations creates it; the caller becomes its first
   `admin`.
2. Settings → Issue a token naming it. The token in use was issued
   before the organisation existed, so it does not name it, and until a
   token does, even the roster answers `403`.

A `write` token publishes into the organisation and reads its roster;
adding and removing members needs an `admin` token naming it. The
secret is shown once, in the browser; put the one a converter publishes
with in a file of mode 600, as `user.sh` does for the first token.

`bash deploy/fly/smoke.sh` is the acceptance test of a deployment: with
the token in that file it uploads three attachments, publishes an Eval
and a Card built from the schema crate's fixtures, reads them back with
fingerprints, the comparison view, the relation graph and the export,
makes them public, and fetches them and the UI deep link without a
token.

## Every later release

```bash
fly deploy --ha=false
```

`fly deploy` builds the image from the checkout, runs `evalhub migrate` in
a throwaway Machine with the new image, and only then rolls the serving
Machine. Migrations are embedded in the binary and forward-only. With more
than one Machine, the old binary keeps running against the new schema
during the roll, which is fine for changes that add; read the release
notes before a major version.

`fly logs --app evalhub` follows the JSON log; `fly status --app evalhub`
shows the Machines and the last release.

`evalhub migrate` applies the SQL migrations and then any one-shot data
migration the release carries, and prints one line per data migration it
applied (`applied data migration <name>: …`), or `no data migration
pending`. `serve` counts a data migration that has not run as pending,
exactly like a SQL one.

## Upgrading to 0.2.0

0.2.0 is not deployed like every later release. Its `migrate` carries a
data migration, `0003_runs_split`, that rewrites stored data: every Eval
version body loses its `runs[]` (they become run rows), its `schema`
becomes `evalhub.eval/2.0` and its `content_hash` changes (the audit log
keeps each old and new hash, action `migration.runs_split`), and each
Card's used runs are rebuilt from the Eval version it points at.

The reason for the different procedure is the window a plain `fly deploy`
leaves open. The release command runs `migrate` first, and only then is
the serving Machine rolled; until it is, the *old* binary is still
serving against the migrated database. An Eval posted to it in that
window is stored as a 1.0 body with `runs[]` inside and no run rows,
which is exactly what the migration has just removed, and nothing would
ever split it (the migration runs once; the new binary converts a 1.0
body only when it is posted). So writes stop before the migration and
resume only with the new binary.

With `flyctl` logged in, from the checkout of the 0.2.0 tag:

1. **Snapshot the production Postgres.** The data step is one
   transaction and rolls back whole if it fails, but it rewrites data
   that exists nowhere else, so take a copy first:

   ```bash
   fly volumes list --app evalhub-db                 # the cluster's volume id
   fly volumes snapshots create <volume-id> --app evalhub-db
   fly volumes snapshots list <volume-id> --app evalhub-db
   ```

   A `pg_dump` in hand as well (Backups and leaving, below) costs a few
   minutes and does not depend on Fly to restore.

2. **Stop the serving Machine, so writes stop.** `fly.toml` sets
   `auto_start_machines = true`, which makes the Fly proxy start a
   stopped Machine on the next request; turn that off for the Machine
   before stopping it, or a request arriving in between brings the old
   binary back by itself:

   ```bash
   fly machine list --app evalhub                    # the serving Machine's id
   fly machine update <machine-id> --autostart=false --skip-start --app evalhub
   fly machine stop <machine-id> --app evalhub
   curl -fsS https://evalhub.fly.dev/api/v1/healthz  # must fail now
   ```

3. **Deploy.**

   ```bash
   fly deploy --ha=false
   ```

   The release command's log (in the deploy output, or `fly logs`) shows
   `applied data migration 0003_runs_split: <n> Eval(s), …, <k> with
   unmatched run ids`. The release command has a default timeout of 5
   minutes (`fly deploy --release-command-timeout`); the data step is one
   transaction, so a timeout rolls it back and leaves it pending, and the
   fix is to deploy again with a longer timeout. The data step is part of the release command, so
   if it fails the release fails closed: the deploy stops, the Machine
   is not updated, and the database has no run rows and no rewritten
   bodies (the `0003` DDL, which only adds tables and a column, stays
   applied; the old binary does not read them). A run id the migration
   refuses is named in the error, with its Eval and version. Fix the
   data or ask, and deploy again; to serve with the old binary in the
   meantime, `fly machine update <machine-id> --autostart=true` and
   `fly machine start <machine-id>`.

   The deploy rewrites the Machine's configuration from `fly.toml`,
   which turns autostart back on. If `fly status --app evalhub` still
   shows the Machine stopped afterwards, `fly machine start <machine-id>
   --app evalhub`.

4. **Check it.**

   ```bash
   fly releases --app evalhub                        # the new release, complete
   fly ssh console --app evalhub -C "evalhub --version"   # evalhub 0.2.0
   curl -fsS https://evalhub.fly.dev/api/v1/healthz
   curl -fsS https://evalhub.fly.dev/openapi.json | jq -r .info.version   # 1.0.0
   bash deploy/fly/smoke.sh
   ```

   The running release is what `fly releases` and the binary's
   `--version` say. `/api/v1/healthz` answers only `{status, database}`,
   and `info.version` in `/openapi.json` is the API contract's version
   (`1.0.0`), which 0.2.0 does not change; both confirm the server is up
   and serving the contract, not which release it is.

   `migration.card_runs_unmatched` audit rows are expected: 0.1.x never
   checked a Card's `attrs.runs`, and the ids that named no run are
   skipped and recorded there, not errors.

Releases after 0.2.0 go back to the plain `fly deploy --ha=false` above,
unless their release notes say otherwise.

## A custom domain

```bash
fly certs add hub.example.com --app evalhub
```

Then the DNS records `fly certs show` asks for. Nothing in the hub changes:
it never sees the hostname, and the cookie is already `Secure`.

## Backups and leaving

The data is the database plus the bucket, and both are in standard forms.
`fly postgres create --enable-backups` turns on WAL backups to Tigris for
the unmanaged cluster; Managed Postgres backs up on its own. For a copy
in hand:

```bash
fly proxy 15432:5432 --app evalhub-db &          # local port to the cluster
pg_dump postgres://evalhub:<password>@localhost:15432/evalhub > evalhub.sql
# any S3 client, with the bucket's credentials:
aws --endpoint-url https://fly.storage.tigris.dev s3 sync s3://evalhub-attachments ./attachments
```

Restore is `psql < evalhub.sql` and a sync back. A record's own portable
form is `GET …/export?format=…`. Anyone running the image elsewhere runs
the same binary at the same version, so a `pg_dump` from the hosted
service is a valid starting point for them.

## Health

`GET /api/v1/healthz` answers `200` once the server is up; it does not
touch the database, and it is what `fly.toml` polls. `GET /openapi.json`
is the contract the UI and every client are generated from; its
`info.version` is the API contract's version, which moves only when the
contract does, not the release. Which release is running is `fly releases
--app evalhub`, or `fly ssh console --app evalhub -C "evalhub --version"`.

## Not in this file

Terms, a content policy, DMCA agent registration and abuse handling are
runbooks of their own once the service has users. The crate docs say what
the hub records (`provenance`, `redaction`) so that those runbooks have
something to act on.
