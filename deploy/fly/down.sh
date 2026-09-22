#!/usr/bin/env bash
# Tear the hosted service down: the bucket, the app, the Postgres. Every
# byte of data goes with them; take a backup first (docs/hosting.md
# §Backups) if any of it matters.
#
#   bash deploy/fly/down.sh
#
# The app name comes from fly.toml, as in `up.sh`, so the two agree.
set -euo pipefail
cd "$(dirname "$0")/../.."

app=$(awk -F'"' '/^app = /{print $2; exit}' fly.toml)
[ -n "$app" ] || { echo 'no app = "..." line in fly.toml' >&2; exit 1; }
db_app=${EVALHUB_FLY_DB:-${app}-db}
bucket=${EVALHUB_FLY_BUCKET:-${app}-attachments}

fly auth whoami >/dev/null

# The bucket first, while the app it is attached to still exists. The
# credentials issued for it die with it. `up.sh` may have given it a
# random suffix when the plain name was unavailable.
found=$(fly storage list 2>/dev/null | grep -o -E "\b${bucket}(-[0-9a-f]{4})?\b" | sort -u || true)
if [ -n "$found" ]; then
    for b in $found; do fly storage destroy "$b" --app "$app" --yes; done
else
    echo "bucket $bucket: not found, skipping"
fi

for a in "$app" "$db_app"; do
    if fly apps list --json | grep -q "\"Name\": *\"$a\""; then
        fly apps destroy "$a" --yes
    else
        echo "app $a: not found, skipping"
    fi
done

echo "down: $app, $db_app, $bucket"
