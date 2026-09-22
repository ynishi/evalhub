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
which is the artifact for anyone running it elsewhere.

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
is the contract the UI and every client are generated from, and a quick
way to confirm which version is running.

## Not in this file

Terms, a content policy, DMCA agent registration and abuse handling are
runbooks of their own once the service has users. The crate docs say what
the hub records (`provenance`, `redaction`) so that those runbooks have
something to act on.
