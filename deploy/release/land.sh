#!/usr/bin/env bash
# Land a merge-ready pull request and, if it ends a release, release it:
# merge (a merge commit, never squash: the branch commits carry the
# Signed-off-by), close the issue, fast-forward main, drop the local
# branch, then `tag.sh` (waits for CI, tags, pushes, follows the release
# workflow).
#
#   bash deploy/release/land.sh <pr> <issue>          # merge, close, tag
#   bash deploy/release/land.sh <pr> <issue> --no-tag # merge and close only
#   bash deploy/release/land.sh <pr> - ...            # keep the issue open
#
# The merge and the issue close are the remote writes here; the tag push
# is tag.sh's. `set -e`: a step that fails stops the ones after it.
set -euo pipefail
cd "$(dirname "$0")/../.."
repo=ynishi/evalhub
pr=${1:?usage: land.sh <pr> <issue> [--no-tag]}
issue=${2:?usage: land.sh <pr> <issue> [--no-tag]}
tag=1; [ "${3:-}" = --no-tag ] && tag=0

[ -z "$(git status --porcelain)" ] || { echo "working tree not clean" >&2; git status --short >&2; exit 1; }

read -r state branch < <(gh pr view "$pr" -R "$repo" --json state,headRefName --jq '"\(.state) \(.headRefName)"')
case "$state" in
    OPEN)
        gh pr merge "$pr" -R "$repo" --merge
        [ "$issue" = - ] || gh issue close "$issue" -R "$repo" --comment "Shipped in #${pr}."
        ;;
    MERGED) echo "PR #$pr already merged" ;;
    *) echo "PR #$pr is $state" >&2; exit 1 ;;
esac

git switch -q main
git pull -q --ff-only origin main
if git show-ref -q --verify "refs/heads/$branch"; then
    if git worktree list --porcelain | grep -q "^branch refs/heads/$branch$"; then
        echo "branch $branch is checked out in a worktree; remove it with workspace/after-merge.sh"
    else
        git branch -D "$branch" >/dev/null && echo "deleted local branch $branch"
    fi
fi
git log --oneline -1

[ "$tag" = 1 ] && exec bash deploy/release/tag.sh
echo "merged; not tagged (--no-tag)"
