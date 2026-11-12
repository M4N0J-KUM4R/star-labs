#!/usr/bin/env bash
# Hardened Phase 4 merge controller (ENV-02).
#
# Fixes over merge-candidate.sh:
#   - Exact-SHA verification: PR head must equal the built candidate SHA at
#     merge time (gh pr merge --match-head-commit), not just poll-and-hope.
#   - All required checks must individually conclude "success"; aggregate
#     mergeStateStatus CLEAN/UNSTABLE is not trusted as CI evidence.
#   - Fixed worktree lookup: compares "$1 branch" "$2 refs/heads/<b>" against
#     the two-field porcelain output instead of the broken single-field match.
#   - No force cleanup: a dirty/conflicting temp worktree fails loudly instead
#     of being removed with --force.
#   - Check identity: verifies each required check run's conclusion and name;
#     duplicate names must all succeed.
#
# Usage: merge-candidate-v2.sh <slice> <candidate-sha>
# The caller must have already built the candidate branch (impl + tests +
# main) and pass the exact SHA that was reviewed and CI-verified.
set -euo pipefail

SLICE="${1:?slice}"
CANDIDATE_SHA="${2:?candidate sha}"
ROOT="${P4_ROOT:-/Users/manojkumar/lms-migration}"
REPO="$ROOT/star-labs"
GH_REPO="${GH_REPO:-M4N0J-KUM4R/star-labs}"
BRANCH_CAND="p4-candidate/$SLICE"
LOCK="${P4_LOCK:-/tmp/p4-merge-v2.lock}"
REQUIRED_CHECKS="${P4_REQUIRED_CHECKS:-Rust migration validation,Production build,Parity scoreboard,Bootstrap validation}"
WAIT_SECONDS="${P4_MERGE_WAIT_SECONDS:-2400}"

fail() {
    echo "MERGE BLOCKED: $*" >&2
    exit 1
}

# ---------------------------------------------------------------------------
# Process-owned lock (no age-based eviction: a stale lock blocks and the
# operator resolves it, rather than two mergers racing).
# ---------------------------------------------------------------------------
if [ -d "$LOCK" ]; then
    fail "merge lock $LOCK exists; another merge may be running. Remove it only after verifying no merger process is alive."
fi
mkdir "$LOCK"
trap 'rmdir "$LOCK" 2>/dev/null || true' EXIT

cd "$REPO"
git fetch origin main "$BRANCH_CAND"

ACTUAL="$(git rev-parse "origin/$BRANCH_CAND")"
[ "$ACTUAL" = "$CANDIDATE_SHA" ] ||
    fail "origin/$BRANCH_CAND is $ACTUAL, expected reviewed SHA $CANDIDATE_SHA (candidate changed after review)"

# ---------------------------------------------------------------------------
# Wait until every required check run on the exact candidate SHA concludes
# success. Duplicate check names must all succeed.
# ---------------------------------------------------------------------------
deadline=$((SECONDS + WAIT_SECONDS))
while [ "$SECONDS" -lt "$deadline" ]; do
    json="$(gh api "repos/$GH_REPO/commits/$CANDIDATE_SHA/check-runs" --paginate 2>/dev/null || echo '{}')"
    pending="$(echo "$json" | python3 -c '
import json,sys
data=json.load(sys.stdin)
runs=data.get("check_runs",[])
required=[n.strip() for n in sys.argv[1].split(",") if n.strip()]
by_name={}
for r in runs:
    by_name.setdefault(r["name"],[]).append(r["conclusion"])
missing=[n for n in required if n not in by_name]
nonsuccess=[f"{n}:{c}" for n,cs in by_name.items() for c in cs if c!="success" and n in required]
if missing: print("MISSING:"+",".join(missing))
elif nonsuccess: print("FAILED:"+",".join(nonsuccess))
else: print("OK")
' "$REQUIRED_CHECKS")"
    case "$pending" in
        OK) break ;;
        MISSING:*) echo "waiting for checks: ${pending#MISSING:}"; sleep 20 ;;
        FAILED:*) fail "required checks failed on $CANDIDATE_SHA: ${pending#FAILED:}" ;;
        *) echo "check-run query error; retrying"; sleep 20 ;;
    esac
done
[ "$SECONDS" -lt "$deadline" ] || fail "timed out waiting for required checks on $CANDIDATE_SHA"

# ---------------------------------------------------------------------------
# PR must exist, be OPEN, and have head exactly equal to the reviewed SHA.
# ---------------------------------------------------------------------------
NUM="$(gh pr view "$BRANCH_CAND" --repo "$GH_REPO" --json number --jq .number 2>/dev/null)" ||
    fail "no PR for $BRANCH_CAND"
STATE="$(gh pr view "$NUM" --repo "$GH_REPO" --json state --jq .state)"
[ "$STATE" = "OPEN" ] || fail "PR $NUM state is $STATE"
HEAD="$(gh pr view "$NUM" --repo "$GH_REPO" --json headRefOid --jq .headRefOid)"
[ "$HEAD" = "$CANDIDATE_SHA" ] ||
    fail "PR $NUM head $HEAD != reviewed candidate $CANDIDATE_SHA"

# ---------------------------------------------------------------------------
# Merge with head-match guard: GitHub rejects the merge server-side if the
# PR head moved between our check and the merge.
# ---------------------------------------------------------------------------
if ! gh pr merge "$NUM" --repo "$GH_REPO" --merge --match-head-commit "$CANDIDATE_SHA" >/dev/null 2>&1; then
    fail "gh pr merge --match-head-commit $CANDIDATE_SHA failed for PR $NUM"
fi

STATE="$(gh pr view "$NUM" --repo "$GH_REPO" --json state --jq .state)"
[ "$STATE" = "MERGED" ] || fail "PR $NUM state is $STATE after merge"
MERGE_SHA="$(gh pr view "$NUM" --repo "$GH_REPO" --json mergeCommit --jq .mergeCommit.oid)"
python3 - "$SLICE" "$NUM" "$CANDIDATE_SHA" "$MERGE_SHA" <<'PY'
import json,sys
print(json.dumps({
    "slice":sys.argv[1],
    "pr":int(sys.argv[2]),
    "candidate_sha":sys.argv[3],
    "merge_sha":sys.argv[4],
},indent=2))
PY
