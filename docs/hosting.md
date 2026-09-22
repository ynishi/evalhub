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

Once, with `flyctl` logged in. The app name is the one in `fly.toml`; if
`evalhub` is taken on Fly, change it there and in every `--app` below.

```bash
# 1. The app, without deploying yet.
fly apps create evalhub

# 2. Postgres. A single unmanaged node is the cheapest way to find out
#    whether the service is worth running (a few dollars a month); Fly
#    does not support it, and Managed Postgres (`fly mpg create`, from
#    $38/month) is the upgrade path when the answer is yes.
fly postgres create --name evalhub-db --region nrt \
    --initial-cluster-size 1 --vm-size shared-cpu-1x --volume-size 1
# Creates a database and a user on that cluster and sets the URL as a
# secret on the app, under the name the hub reads.
fly postgres attach evalhub-db --app evalhub --variable-name EVALHUB_DATABASE__URL

# 3. Attachments. `fly storage create` prints the bucket's credentials and
#    sets them on the app under AWS_* names, which the hub does not read;
#    copy the printed values into the hub's own keys.
fly storage create --app evalhub --name evalhub-attachments
fly secrets set --app evalhub \
    EVALHUB_S3__ENDPOINT=https://fly.storage.tigris.dev \
    EVALHUB_S3__BUCKET=evalhub-attachments \
    EVALHUB_S3__ACCESS_KEY=<printed access key> \
    EVALHUB_S3__SECRET_KEY=<printed secret key>

# 4. Auth keys. Each is any string, hashed before use. Unset, each is
#    generated per process: every session would end at a restart and two
#    Machines would not agree with each other.
fly secrets set --app evalhub \
    EVALHUB_AUTH__COOKIE_KEY="$(openssl rand -base64 48)" \
    EVALHUB_AUTH__CURSOR_KEY="$(openssl rand -base64 48)"

# 5. Build, migrate (the release command), serve.
fly deploy
```

`s3.public_endpoint` is left unset, so presigned URLs are minted for the
same Tigris endpoint the hub talks to; browsers can reach it. `s3.region`
is `auto` in `fly.toml`, which is what Tigris expects.

## First user

There is no signup endpoint. The operator creates users and the first
token of each:

```bash
fly ssh console --app evalhub -C "evalhub user create alice --scope admin"
```

The token is printed once. Everything after that (organisations, more
tokens, visibility) is done through the API or the UI with that token.

## Every later release

```bash
fly deploy
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
