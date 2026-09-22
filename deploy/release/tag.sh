#!/usr/bin/env bash
# Tag the release that is on main and push the tag. The push is the one
# thing a release needs from a person; the `release` workflow does the
# rest (crates.io, the image, the GitHub Release) and this script follows
# it to the end.
#
#   bash deploy/release/tag.sh            # version from Cargo.toml
#
# Stops, without tagging, unless: on main, clean, HEAD is origin/main,
# and the ci workflow for HEAD has completed with success (waits for it,
# up to 20 minutes, since it is usually still running right after a
# merge). Re-runnable: an existing tag that points at HEAD is reused.
set -euo pipefail
cd "$(dirname "$0")/../.."
repo=ynishi/evalhub

[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || { echo "not on main" >&2; exit 1; }
[ -z "$(git status --porcelain)" ] || { echo "working tree not clean" >&2; git status --short >&2; exit 1; }
git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || { echo "HEAD differs from origin/main; pull first" >&2; exit 1; }

version=$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)
[ -n "$version" ] || { echo "no version in Cargo.toml" >&2; exit 1; }
tag="v$version"
head=$(git rev-parse HEAD)

# CI for this exact commit.
for _ in $(seq 1 80); do
    read -r status conclusion < <(gh run list --repo "$repo" --commit "$head" --workflow ci \
        --json status,conclusion --jq '.[0] | "\(.status // "none") \(.conclusion // "")"')
    [ "$status" = completed ] && break
    echo "ci for $head: $status; waiting"
    sleep 15
done
[ "${conclusion:-}" = success ] || { echo "ci for HEAD is '${conclusion:-$status}', not success" >&2; exit 1; }

# The tag, once.
if git rev-parse -q --verify "refs/tags/$tag^{commit}" >/dev/null; then
    [ "$(git rev-parse "$tag^{commit}")" = "$head" ] \
        || { echo "tag $tag exists but points elsewhere; a release is not re-tagged" >&2; exit 1; }
else
    git tag -a "$tag" -m "evalhub $version"
fi
git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null || git push origin "$tag"
echo "tag $tag at $head"

# Follow the release workflow to its end.
run=""
for _ in $(seq 1 20); do
    run=$(gh run list --repo "$repo" --workflow release --branch "$tag" --json databaseId --jq '.[0].databaseId // empty')
    [ -n "$run" ] && break
    sleep 5
done
[ -n "$run" ] || { echo "no release run for $tag after 100s; see gh run list --workflow release" >&2; exit 1; }
gh run watch "$run" --repo "$repo" --exit-status
echo
echo "released evalhub $version:"
echo "  https://github.com/$repo/releases/tag/$tag"
for c in evalhub-schema evalhub-core evalhub-query; do echo "  https://crates.io/crates/$c/$version"; done
echo "  ghcr.io/$repo:$version"
