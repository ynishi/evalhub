#!/usr/bin/env bash
# Create a user on the hosted service and keep its first token in a file,
# never on the terminal.
#
#   bash deploy/fly/user.sh <login> [scope]        # scope: admin (default) | write | read
#
# `evalhub user create` prints the token once, by design; run by hand
# over `fly ssh console` that puts it on screen and in any terminal log.
# This captures the output, writes the token to ~/.config/evalhub/token
# (EVALHUB_TOKEN_FILE), mode 600, where `smoke.sh` and any client read
# it, and shows only the login and scope.
set -euo pipefail
cd "$(dirname "$0")/../.."

login=${1:?usage: user.sh <login> [scope]}
scope=${2:-admin}
app=$(awk -F'"' '/^app = /{print $2; exit}' fly.toml)
[ -n "$app" ] || { echo 'no app = "..." line in fly.toml' >&2; exit 1; }
token_file=${EVALHUB_TOKEN_FILE:-$HOME/.config/evalhub/token}

fly auth whoami >/dev/null

out=$(fly ssh console --app "$app" -C "evalhub user create $login --scope $scope" 2>&1) || {
    printf '%s\n' "$out" | grep -v -E '^token:|"token"' >&2
    exit 1
}
token=$(printf '%s\n' "$out" | awk '$1 == "token:" {print $2; exit}')
[ -n "$token" ] || { echo "no token line in the hub's output:" >&2; printf '%s\n' "$out" | grep -v -E '"token' >&2; exit 1; }

umask 077
mkdir -p "$(dirname "$token_file")"
printf '%s' "$token" > "$token_file"
unset token

printf '%s\n' "$out" | grep -E '^(login|scope):'
echo "token:    written to $token_file (mode 600), not shown"
