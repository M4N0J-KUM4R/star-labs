#!/usr/bin/env bash
# Merge a protected PR only when GitHub says it is ready.
# Usage: merge-ready-pr.sh <pr-number>
set -euo pipefail

NUM="${1:?pull request number}"
GH_REPO=M4N0J-KUM4R/star-labs
LOCK=/tmp/p4-merge.lock

while ! mkdir "$LOCK" 2>/dev/null; do
    echo "waiting for another merge to finish"
    sleep 5
done
trap 'rmdir "$LOCK" 2>/dev/null || true' EXIT

STATE="$(gh pr view "$NUM" --repo "$GH_REPO" --json state --jq .state)"
[ "$STATE" = "OPEN" ] || { echo "PR $NUM is $STATE, not OPEN"; exit 2; }
MERGEABLE="$(gh pr view "$NUM" --repo "$GH_REPO" --json mergeable --jq .mergeable)"
[ "$MERGEABLE" = "MERGEABLE" ] || { echo "PR $NUM is $MERGEABLE"; exit 2; }
MERGE_STATE="$(gh pr view "$NUM" --repo "$GH_REPO" --json mergeStateStatus --jq .mergeStateStatus)"
HEAD="$(gh pr view "$NUM" --repo "$GH_REPO" --json headRefOid --jq .headRefOid)"
case "$MERGE_STATE" in
    CLEAN|UNSTABLE) ;;
    *)
        echo "PR $NUM is not ready: mergeStateStatus=$MERGE_STATE"
        exit 2
        ;;
esac

if ! gh pr merge "$NUM" --repo "$GH_REPO" --merge --match-head-commit "$HEAD" --delete-branch >/dev/null; then
    echo "gh pr merge failed for PR $NUM (head may have moved)"
    exit 1
fi

STATE="$(gh pr view "$NUM" --repo "$GH_REPO" --json state --jq .state)"
[ "$STATE" = "MERGED" ] || { echo "PR $NUM state is $STATE after merge"; exit 1; }
CAND_SHA="$(gh pr view "$NUM" --repo "$GH_REPO" --json headRefOid --jq .headRefOid)"
MERGE_SHA="$(gh pr view "$NUM" --repo "$GH_REPO" --json mergeCommit --jq .mergeCommit.oid)"
URL="$(gh pr view "$NUM" --repo "$GH_REPO" --json url --jq .url)"
python3 - "$NUM" "$URL" "$CAND_SHA" "$MERGE_SHA" <<'PY'
import json,sys
print(json.dumps({
    "pr": int(sys.argv[1]),
    "url": sys.argv[2],
    "candidate_sha": sys.argv[3],
    "merge_sha": sys.argv[4],
}, indent=2))
PY
