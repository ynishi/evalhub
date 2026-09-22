#!/usr/bin/env bash
# Stand the hosted service up on Fly.io: the app, a Postgres, a Tigris
# bucket, the secrets, the first deploy. docs/hosting.md §Standing it up
# says what each step is for; this file is the steps.
#
#   fly auth login                 # once, interactive
#   bash deploy/fly/up.sh
#
# Nothing secret reaches the terminal. Three flyctl commands print
# credentials by design (`postgres create` the superuser password,
# `postgres attach` the connection URL, `storage create` the bucket
# keys); each is captured, the values are handed to `fly secrets import`
# on stdin, and only redacted lines are shown. Do not add `set -x`.
#
# Re-runnable: an app, cluster or bucket that exists is reused. The
# bucket's keys are only readable on the run that creates it, so a rerun
# after a failed first run must either destroy the bucket
# (`deploy/fly/down.sh`) or set EVALHUB_S3__ACCESS_KEY and
# EVALHUB_S3__SECRET_KEY by hand.
set -euo pipefail
cd "$(dirname "$0")/../.."

# The app name is `app` in fly.toml, the one place it is set; the
# Postgres and the bucket are named after it. `evalhub` is a global Fly
# name, so a second deployment of this repo changes it there.
app=$(awk -F'"' '/^app = /{print $2; exit}' fly.toml)
[ -n "$app" ] || { echo 'no app = "..." line in fly.toml' >&2; exit 1; }
db_app=${EVALHUB_FLY_DB:-${app}-db}
bucket=${EVALHUB_FLY_BUCKET:-${app}-attachments}
region=${EVALHUB_FLY_REGION:-nrt}

fly auth whoami >/dev/null

# Print flyctl output with anything credential-shaped blanked. Used on
# every captured command so a failure is still diagnosable.
redact() {
    sed -E \
        -e 's#(postgres://[^:/]+:)[^@]+@#\1<redacted>@#g' \
        -e 's#^([[:space:]]*(Password|AWS_SECRET_ACCESS_KEY|AWS_ACCESS_KEY_ID)[[:space:]]*:).*#\1 <redacted>#' \
        -e 's#(tsec_|tid_)[A-Za-z0-9_+/=-]+#\1<redacted>#g'
}
has_app() { fly apps list --json | grep -q "\"Name\": *\"$1\""; }

# 1. The app, without deploying.
if has_app "$app"; then
    echo "app $app exists, reusing"
else
    fly apps create "$app"
fi

# 2. Postgres: one unmanaged node, the cheapest way to find out whether
#    the service is worth running. Managed Postgres is the upgrade path.
#    The superuser password it prints is not shown; admin access is
#    `fly postgres connect --app $db_app`, which needs none.
if has_app "$db_app"; then
    echo "postgres $db_app exists, reusing"
else
    out=$(fly postgres create --name "$db_app" --region "$region" \
        --initial-cluster-size 1 --vm-size shared-cpu-1x --volume-size 1 2>&1) \
        || { printf '%s\n' "$out" | redact >&2; exit 1; }
    printf '%s\n' "$out" | redact
fi
# Creates a user and a database on the cluster and sets the URL on the
# app under the name the hub reads. Fails when already attached; fine.
if out=$(fly postgres attach "$db_app" --app "$app" \
        --variable-name EVALHUB_DATABASE__URL --yes 2>&1); then
    printf '%s\n' "$out" | redact
else
    printf '%s\n' "$out" | redact
    echo "attach: not repeated (already attached?)"
fi

# 3. Attachments: a Tigris bucket. `fly storage create` prints the keys
#    once and sets them under AWS_* names the hub does not read; they are
#    copied into EVALHUB_S3__* without touching the terminal.
#    Bucket names are global to Tigris and a destroyed name stays
#    unavailable for a while, so when the plain name is refused the
#    bucket gets a random suffix; `down.sh` destroys either form.
existing=$(fly storage list 2>/dev/null | grep -o -E "\b${bucket}(-[0-9a-f]{4})?\b" | head -1 || true)
if [ -n "$existing" ]; then
    echo "bucket $existing exists, reusing (S3 keys must already be set as secrets)"
else
    if ! out=$(fly storage create --app "$app" --name "$bucket" --yes 2>&1); then
        if printf '%s\n' "$out" | grep -q "recently deleted"; then
            bucket="$bucket-$(openssl rand -hex 2)"
            echo "the plain bucket name was recently destroyed; creating $bucket instead"
            out=$(fly storage create --app "$app" --name "$bucket" --yes 2>&1) \
                || { printf '%s\n' "$out" | redact >&2; exit 1; }
        else
            printf '%s\n' "$out" | redact >&2; exit 1
        fi
    fi
    printf '%s\n' "$out" | redact
    # Split on the first ": " only: the endpoint value itself contains ":".
    field() { printf '%s\n' "$out" | awk -v k="$1" '$1 == k ":" { sub(/^[^:]*:[ \t]*/, ""); print; exit }'; }
    access=$(field AWS_ACCESS_KEY_ID)
    secret=$(field AWS_SECRET_ACCESS_KEY)
    endpoint=$(field AWS_ENDPOINT_URL_S3)
    [ -n "$access" ] && [ -n "$secret" ] \
        || { echo "could not read the bucket keys from 'fly storage create' output" >&2; exit 1; }
    printf 'EVALHUB_S3__ENDPOINT=%s\nEVALHUB_S3__BUCKET=%s\nEVALHUB_S3__ACCESS_KEY=%s\nEVALHUB_S3__SECRET_KEY=%s\n' \
        "${endpoint:-https://fly.storage.tigris.dev}" "$bucket" "$access" "$secret" \
        | fly secrets import --app "$app" --stage
    unset access secret
    echo "bucket $bucket: keys staged as EVALHUB_S3__*"
fi

# 4. Auth keys, generated here and never printed. Each is any string,
#    hashed before use; unset, each would be regenerated per process.
printf 'EVALHUB_AUTH__COOKIE_KEY=%s\nEVALHUB_AUTH__CURSOR_KEY=%s\n' \
    "$(openssl rand -base64 48)" "$(openssl rand -base64 48)" \
    | fly secrets import --app "$app" --stage
echo "auth keys: staged"

# 5. Build, migrate (the release command), serve. One Machine: Fly adds a
#    second for availability by default, which the trial does not need.
fly deploy --app "$app" --ha=false

# 6. Is it up?
url="https://$app.fly.dev"
code=$(curl -s -o /dev/null -w '%{http_code}' "$url/api/v1/healthz")
echo "healthz: $code at $url/api/v1/healthz"
[ "$code" = 200 ] || { echo "not healthy; see: fly logs --app $app" >&2; exit 1; }

echo
echo "up: $url"
echo "first user (token to ~/.config/evalhub/token, not shown):"
echo "  bash deploy/fly/user.sh alice"
