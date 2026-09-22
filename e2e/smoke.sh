#!/usr/bin/env bash
# End-to-end smoke of the real `evalhub` binary.
#
# What the integration tests under crates/*/tests cannot show: that the
# binary a user runs boots against a database, and that the web UI is inside
# it. So this starts a throwaway Postgres, runs `evalhub migrate`, starts
# `evalhub serve` from a directory that is *not* the source tree (a debug
# build reads the UI from disk; a release build must not need to), and asks
# it for the things a browser would ask for.
#
# Usage: e2e/smoke.sh [path-to-evalhub-binary]
#   default binary: target/release/evalhub (see `just e2e`, which builds it)
# Needs: docker, curl, python3.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
bin=${1:-$repo/target/release/evalhub}
# The script changes directory before running it, so the path must be absolute.
bin=$(readlink -f "$bin")
[ -x "$bin" ] || { echo "e2e: $bin is not an executable; run \`just e2e\`" >&2; exit 2; }
for tool in docker curl python3; do
    command -v "$tool" >/dev/null || { echo "e2e: $tool is required" >&2; exit 2; }
done

free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }

name=evalhub-e2e-$$
pg_port=$(free_port)
http_port=$(free_port)
work=$(mktemp -d)
server_pid=""
cleanup() {
    [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
    docker stop "$name" >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT

pass=0
fail=0
ok()   { pass=$((pass + 1)); echo "ok   $1"; }
bad()  { fail=$((fail + 1)); echo "FAIL $1" >&2; }
check() { # check <description> <command...>: the command's exit code is the verdict
    local desc=$1; shift
    if "$@"; then ok "$desc"; else bad "$desc"; fi
}

echo "e2e: binary $bin"
echo "e2e: postgres on :$pg_port, evalhub on :$http_port, cwd $work"

docker run -d --rm --name "$name" \
    -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=evalhub \
    -p "127.0.0.1:$pg_port:5432" postgres:16-alpine >/dev/null
# The official image starts postgres twice during init; wait for the second
# "ready" or the first connection is reset.
for _ in $(seq 1 60); do
    [ "$(docker logs "$name" 2>&1 | grep -c 'ready to accept connections')" -ge 2 ] && break
    sleep 1
done

export EVALHUB_DATABASE__URL="postgres://postgres:pw@127.0.0.1:$pg_port/evalhub"
cd "$work"

check "evalhub migrate applies the schema" "$bin" migrate

# Create alice (admin) and bob for the browser suite.  The token is captured
# into a variable and handed to Playwright on its command line only (an exported
# EVALHUB_* variable would reach the server, whose config rejects unknown keys); it is never
# printed to the terminal or a log.
created=$("$bin" user create alice --scope admin)
e2e_token=$(printf '%s\n' "$created" | awk '$1 == "token:" { print $2 }')
check "evalhub user create alice (admin) prints a token" [ -n "$e2e_token" ]
check "evalhub user create bob" bash -c '"$0" user create bob >/dev/null' "$bin"

"$bin" serve --bind "127.0.0.1:$http_port" >server.log 2>&1 &
server_pid=$!
base="http://127.0.0.1:$http_port"
for _ in $(seq 1 60); do
    curl -sf "$base/api/v1/healthz" >/dev/null 2>&1 && break
    kill -0 "$server_pid" 2>/dev/null || { echo "e2e: server exited early:" >&2; cat server.log >&2; exit 1; }
    sleep 0.5
done

status() { curl -s -o /dev/null -w '%{http_code}' "$base$1"; }

check "GET /api/v1/healthz is 200" [ "$(status /api/v1/healthz)" = 200 ]
check "GET /openapi.json is 200"   [ "$(status /openapi.json)" = 200 ]
check "GET /api/v1/nope is 404, not the SPA" [ "$(status /api/v1/nope)" = 404 ]

curl -s -D root.h -o root.html "$base/"
check "GET / is 200"                         grep -q '^HTTP/1.1 200' root.h
check "GET / is text/html"                   grep -qi '^content-type: text/html' root.h
check "GET / is no-cache"                    grep -qi '^cache-control: no-cache' root.h
check "GET / is the built UI, not the placeholder" bash -c '! grep -q "not built into this binary" root.html'
check "GET / loads a script bundle"          grep -q '<script' root.html

asset=$(grep -o '/_app/immutable/[^"]*\.js' root.html | head -1 || true)
if [ -n "$asset" ]; then
    curl -s -D asset.h -o /dev/null "$base$asset"
    check "immutable asset $asset is 200"         grep -q '^HTTP/1.1 200' asset.h
    check "immutable asset is text/javascript"    grep -qi '^content-type: text/javascript' asset.h
    check "immutable asset is cached for a year"  grep -qi '^cache-control: public, max-age=31536000, immutable' asset.h
else
    bad "index.html names an /_app/immutable/ script"
fi

curl -s -o deep.html "$base/cards/alice/x"
check "deep link /cards/alice/x is 200"      [ "$(status /cards/alice/x)" = 200 ]
check "deep link serves index.html"          cmp -s root.html deep.html

check "server log shows it listening"        grep -q 'evalhub listening' server.log

# The browser suite (web/tests/browser) runs against the same server. It
# needs the Chromium headless shell that `just e2e-install` fetches.
echo "e2e: browser tests against $base"
if (cd "$repo/web" && EVALHUB_E2E_BASE_URL="$base" EVALHUB_E2E_TOKEN="$e2e_token" EVALHUB_E2E_MEMBER=bob corepack pnpm exec playwright test); then
    ok "browser suite (web/tests/browser)"
else
    bad "browser suite (web/tests/browser); if the browser is missing, run \`just e2e-install\`"
fi

echo "e2e: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
