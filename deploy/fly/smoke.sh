#!/usr/bin/env bash
# Publish a real Eval (a header and its runs) and a Card that judges those
# runs, with attachments, to the hosted service and read them back through
# the API and the UI: the acceptance test of a deployment, run from outside.
#
#   bash deploy/fly/user.sh alice              # once; writes the token file
#   bash deploy/fly/smoke.sh
#
# The token is read from EVALHUB_TOKEN, else from ~/.config/evalhub/token
# (or EVALHUB_TOKEN_FILE), else prompted for without echo when stdin is a
# terminal; it reaches curl through a header file, never a command line.
# Bodies are the schema crate's fixtures (eval-run-set.json, run.json,
# card-run-results.json) with the attachment digests replaced by the bytes
# this script uploads and the Card's references pointed at this Eval.
# Needs: curl, jq, sha256sum.
set -euo pipefail
cd "$(dirname "$0")/../.."

app=$(awk -F'"' '/^app = /{print $2; exit}' fly.toml)
[ -n "$app" ] || { echo 'no app = "..." line in fly.toml' >&2; exit 1; }
# The service URL: the Fly hostname, or EVALHUB_URL for a custom domain.
base=${EVALHUB_URL:-https://$app.fly.dev}
ns=${EVALHUB_NS:-alice}
api="$base/api/v1"
eval_name=single2-k4
card_name=qwen3-6-32b-on-single2-k4

for tool in curl jq sha256sum; do command -v "$tool" >/dev/null || { echo "need $tool" >&2; exit 1; }; done

token_file=${EVALHUB_TOKEN_FILE:-$HOME/.config/evalhub/token}
if [ -z "${EVALHUB_TOKEN:-}" ] && [ -r "$token_file" ]; then
    EVALHUB_TOKEN=$(tr -d '[:space:]' < "$token_file")
fi
if [ -z "${EVALHUB_TOKEN:-}" ]; then
    if [ -t 0 ]; then
        read -rsp "evalhub token for $ns: " EVALHUB_TOKEN; echo
    else
        echo "no token: put it in $token_file (mode 600) or set EVALHUB_TOKEN; stdin is not a terminal so it cannot be prompted for" >&2
        exit 1
    fi
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
printf 'Authorization: Bearer %s\n' "$EVALHUB_TOKEN" > "$work/auth.h"
unset EVALHUB_TOKEN

fails=0
check() {  # check <label> <command...>; the command's own output is dropped
    local label=$1; shift
    if "$@" >/dev/null 2>&1; then printf '  ok    %s\n' "$label"; else printf '  FAIL  %s\n' "$label"; fails=$((fails+1)); fi
}
# req <method> <path> [json-body] → status on stdout, body in $work/body
req() {
    local m=$1 p=$2 d=${3:-}
    if [ -n "$d" ]; then
        curl -s -o "$work/body" -w '%{http_code}' -X "$m" -H @"$work/auth.h" -H 'content-type: application/json' --data "$d" "$api$p"
    else
        curl -s -o "$work/body" -w '%{http_code}' -X "$m" -H @"$work/auth.h" "$api$p"
    fi
}
# upload <file> → announce, PUT to the presigned URL, complete; prints sha
upload() {
    local f=$1 sha size st url
    sha=$(sha256sum "$f" | cut -d' ' -f1); size=$(stat -c %s "$f")
    st=$(req POST /attachments "{\"sha256\":\"$sha\",\"size\":$size,\"media_type\":\"application/x-ndjson\"}")
    case "$st" in
        201) url=$(jq -r .upload_url "$work/body")
             curl -s -f -o /dev/null -X PUT --data-binary @"$f" "$url" || { echo "PUT to presigned URL failed for $f" >&2; return 1; }
             st=$(req POST "/attachments/$sha/complete")
             [ "$st" = 200 ] || { echo "complete: $st $(cat "$work/body")" >&2; return 1; } ;;
        200) ;;  # already there from an earlier run
        *)   echo "announce: $st $(cat "$work/body")" >&2; return 1 ;;
    esac
    echo "$sha"
}

echo "== whoami"
st=$(req GET /whoami); echo "  $st $(cat "$work/body")"
check "whoami is 200" [ "$st" = 200 ]
check "token has namespace $ns" jq -e --arg ns "$ns" '.namespaces | index($ns) != null' "$work/body"

echo "== attachments"
printf '{"id":"r1-1","prompt":"2+2","response":"4"}\n{"id":"r1-2","prompt":"3+3","response":"6"}\n' > "$work/calls.jsonl"
printf -- '--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n' > "$work/diff.patch"
printf '{"id":"s1","input":"2+2","output":"4","score":1}\n' > "$work/samples.jsonl"
calls_sha=$(upload "$work/calls.jsonl");     echo "  calls.jsonl   $calls_sha"
diff_sha=$(upload "$work/diff.patch");       echo "  diff.patch    $diff_sha"
samples_sha=$(upload "$work/samples.jsonl"); echo "  samples.jsonl $samples_sha"

echo "== eval $ns/$eval_name (header)"
# The 2.0 header carries no runs; they are written below, one PUT each.
jq '.attachments = [] | .relations = []' crates/evalhub-schema/fixtures/eval-run-set.json > "$work/eval.json"
st=$(req POST "/evals/$ns/$eval_name" "$(cat "$work/eval.json")"); echo "  $st $(head -c 300 "$work/body")"
check "eval header POST is 201 or 200" bash -c "[ '$st' = 201 ] || [ '$st' = 200 ]"
eval_seq=$(jq -r '.seq // empty' "$work/body")
# The Card below cites this seq; without one it would cite a wrong version.
[ -n "$eval_seq" ] || { echo "smoke: no seq from the Eval POST" >&2; exit 1; }

echo "== runs of $ns/$eval_name"
# r1 is fixtures/run.json with its attachments' digests replaced by the
# uploaded bytes; r2 is the same run failed, so the Card below can name
# both runs its attrs.runs lists.
jq --arg s1 "$calls_sha" --argjson n1 "$(stat -c %s "$work/calls.jsonl")" \
   --arg s2 "$diff_sha"  --argjson n2 "$(stat -c %s "$work/diff.patch")" '
   .attachments = [
     {path:"calls/r1.jsonl", sha256:$s1, size:$n1, media_type:"application/x-ndjson"},
     {path:"artifacts/r1/diff.patch", sha256:$s2, size:$n2, media_type:"text/x-diff"}]' \
   crates/evalhub-schema/fixtures/run.json > "$work/r1.json"
jq '.run_id = "r2" | .status = "error" | .error = {kind:"timeout", message:"no answer in 600 s"}
   | del(.calls, .artifacts, .attachments) | .metrics = {"core/duration_ms": 600000.0}' \
   crates/evalhub-schema/fixtures/run.json > "$work/r2.json"
for run in r1 r2; do
    st=$(req PUT "/evals/$ns/$eval_name/runs/$run" "$(cat "$work/$run.json")"); echo "  $run $st $(head -c 300 "$work/body")"
    check "run $run PUT is 201 or 200" bash -c "[ '$st' = 201 ] || [ '$st' = 200 ]"
done
st=$(req GET "/evals/$ns/$eval_name/runs"); echo "  runs GET $st $(head -c 300 "$work/body")"
check "runs GET is 200" [ "$st" = 200 ]
check "runs GET lists r1 as ok" jq -e '[.items[] | select(.run_id == "r1" and .status == "ok")] | length == 1' "$work/body"
check "runs GET lists r2 as error timeout" jq -e '[.items[] | select(.run_id == "r2" and .status == "error" and .error_kind == "timeout")] | length == 1' "$work/body"
st=$(req GET "/evals/$ns/$eval_name"); check "eval GET counts the runs" bash -c "[ '$st' = 200 ] && jq -e '.runs.count >= 2' '$work/body' >/dev/null"

echo "== card $ns/$card_name"
jq --arg s "$samples_sha" --argjson n "$(stat -c %s "$work/samples.jsonl")" \
   --arg to "$ns/$eval_name@$eval_seq" --arg eval "$ns/$eval_name" '
   .attachments = [{path:"samples.jsonl", sha256:$s, size:$n, media_type:"application/x-ndjson"}]
   | .relations[0].to = $to
   | .run_results |= map(.eval = $eval)' crates/evalhub-schema/fixtures/card-run-results.json > "$work/card.json"
st=$(req POST "/cards/$ns/$card_name" "$(cat "$work/card.json")"); echo "  $st $(head -c 300 "$work/body")"
check "card POST is 201 or 200" bash -c "[ '$st' = 201 ] || [ '$st' = 200 ]"

echo "== runs of $ns/$eval_name with the judgements of $ns/$card_name"
st=$(req GET "/evals/$ns/$eval_name/runs?cards=$ns/$card_name"); echo "  $st $(head -c 300 "$work/body")"
check "runs GET with cards= is 200" [ "$st" = 200 ]
check "the card used both runs" jq -e --arg c "$ns/$card_name" '.cards[0].card == $c and .cards[0].runs_used == 2' "$work/body"
check "r1 is judged pass" jq -e --arg c "$ns/$card_name" '.items[] | select(.run_id == "r1") | .cards[$c].results | any(.metric == "core/pass" and .label == "pass")' "$work/body"
check "r2 is judged fail" jq -e --arg c "$ns/$card_name" '.items[] | select(.run_id == "r2") | .cards[$c].results | any(.metric == "core/pass" and .label == "fail")' "$work/body"

echo "== read back"
st=$(req GET "/cards/$ns/$card_name?expand=fingerprints,badges,changed"); echo "  card GET $st"
check "card GET is 200" [ "$st" = 200 ]
check "card has fingerprints" jq -e '.fingerprints != null' "$work/body"
st=$(req GET "/evals/$ns/$eval_name/versions"); check "eval versions is 200" [ "$st" = 200 ]
st=$(req GET "/evals/$ns/$eval_name/cards"); echo "  comparison $st $(head -c 200 "$work/body")"
check "comparison view is 200" [ "$st" = 200 ]
st=$(req GET "/cards/$ns/$card_name/relations?direction=out"); check "relations walk is 200" [ "$st" = 200 ]
loc=$(curl -s -o /dev/null -w '%{redirect_url}' -H @"$work/auth.h" "$api/attachments/$samples_sha")
check "attachment GET redirects to a presigned URL" [ -n "$loc" ]
if [ -n "$loc" ]; then
    check "presigned download returns the bytes" bash -c "curl -s -f '$loc' | cmp -s - '$work/samples.jsonl'"
fi
st=$(req GET "/cards?ns=$ns&limit=10"); check "card list is 200 and names the card" bash -c "[ '$st' = 200 ] && jq -e --arg n '$card_name' '[.items[]?.name] | index(\$n) != null' '$work/body' >/dev/null"
st=$(req GET "/cards/$ns/$card_name/export?format=hf-model-index"); echo "  export $st"; check "hf-model-index export is 200" [ "$st" = 200 ]

echo "== make public, then read without a token"
st=$(req PATCH "/evals/$ns/$eval_name/settings" '{"visibility":"public"}'); check "eval public" [ "$st" = 200 ]
st=$(req PATCH "/cards/$ns/$card_name/settings" '{"visibility":"public"}'); check "card public" [ "$st" = 200 ]
anon=$(curl -s -o /dev/null -w '%{http_code}' "$api/cards/$ns/$card_name"); check "anonymous card GET is 200" [ "$anon" = 200 ]
anon=$(curl -s -o /dev/null -w '%{http_code}' "$api/evals/$ns/$eval_name/runs?cards=$ns/$card_name"); check "anonymous runs GET with cards= is 200" [ "$anon" = 200 ]
anon=$(curl -s -o /dev/null -w '%{redirect_url}' "$api/attachments/$samples_sha"); check "anonymous attachment GET redirects" [ -n "$anon" ]
ui=$(curl -s -o /dev/null -w '%{http_code}' "$base/cards/$ns/$card_name"); check "UI deep link is 200" [ "$ui" = 200 ]
ui=$(curl -s -o /dev/null -w '%{http_code}' "$base/evals/$ns/$eval_name"); check "UI Eval deep link is 200" [ "$ui" = 200 ]

echo
if [ "$fails" -eq 0 ]; then
    echo "all checks passed: $base/cards/$ns/$card_name"
else
    echo "$fails check(s) failed" >&2; exit 1
fi
