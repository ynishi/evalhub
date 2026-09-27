#!/usr/bin/env bash
# List every stored `ext` key that is not `{ns}/{name}` on the hosted
# database, before a release that refuses such keys is deployed.
#
#   bash deploy/fly/ext-keys-check.sh
#
# Read-only: runs deploy/fly/ext-keys.sql through `fly postgres connect`,
# which authenticates over `fly ssh` and prints no credential. An empty
# result (only the header and `(0 rows)`) means every stored body already
# satisfies the rule and the release changes nothing for existing data.
# Record the outcome on the issue that introduced the rule.
set -euo pipefail
cd "$(dirname "$0")/../.."

app=$(awk -F'"' '/^app = /{print $2; exit}' fly.toml)
[ -n "$app" ] || { echo 'no app = "..." line in fly.toml' >&2; exit 1; }
db_app=${EVALHUB_FLY_DB:-${app}-db}
# `fly postgres attach` names the database after the app, `-` as `_`.
database=${EVALHUB_FLY_DATABASE:-${app//-/_}}

fly auth whoami >/dev/null

echo "ext keys outside {ns}/{name} on $db_app/$database:"
fly postgres connect --app "$db_app" --database "$database" < deploy/fly/ext-keys.sql
