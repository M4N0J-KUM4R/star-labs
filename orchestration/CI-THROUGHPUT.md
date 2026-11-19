# CI throughput — wave 3, lane A01 (issue #105)

Measured before/after for the fan-out redesign. All numbers are GitHub Actions
wall-clock timings; nothing is built or run on an operator workstation.

## Baseline (measured on `main` @ `f6d6fc75`, 2026-11-18)

| Workflow | Job | Wall time |
|---|---|---:|
| Rust migration validation | `test` (whole workspace, 1 runner) | 10.2 min |
| Production build | `Format and lint` | 0.6 min |
| Production build | `Dependency audit` | 0.1 min |
| Production build | `Test (debug, PostgreSQL)` — **duplicate** of the above | 11.1 min |
| Production build | `Release build (glibc + musl)` — sequential in one job | 6.6 min |
| Bootstrap validation | `shell-and-lab-constraints` | 0.3 min |
| Parity scoreboard | `parity` | 0.2 min |

- **Critical path per push: 18.5 min** (`quality` → `test` → `release-build`, strictly serial).
- **Redundant work: 21.3 min of testing per push** — the full workspace suite ran
  twice, once in each workflow.
- Predecessor run `e6bab4cd` was worse: 20.3 min critical path.

## Changes

1. **Removed the serial chain.** `quality`, `security`, `test-shard*` and
   `release-build*` all start at once. `needs:` is used only for fan-in gates,
   never to serialize independent work.
2. **Sharded the test suite 4 ways.** 35 crates partitioned by measured test
   count into 4 balanced shards (8 / 7 / 9 / 11 crates), each on its own runner
   with its own PostgreSQL 16 service. Shards compile only their own crates plus
   shared deps, so compile work is split too.
3. **De-duplicated testing.** `production-build.yml` no longer runs the workspace
   suite; `rust-migration.yml` owns it. Saves ~11 min of runner time per push.
4. **Split the release build into two parallel jobs** (`glibc`, `musl`) instead of
   one job doing both back-to-back; each boots its binary and smoke-tests
   `/health`. Release builds run on `main` (and on demand), not on every PR push —
   PRs already compile every target via `clippy --all-targets` and the shards.
5. **Cache discipline.** `Swatinem/rust-cache` with per-role `shared-key`
   (`quality`, `shard-N`, `release-<target>`) and `save-if: main`, so PR jobs
   restore the main cache without fighting each other to overwrite it.
   `CARGO_INCREMENTAL=0` cuts CI compile time and cache size.
6. **`merge_group` trigger** on both workflows so the exact-SHA gate keeps working
   if a merge queue is enabled later.
7. **Shard-coverage guard** (`tools/ci/shard_coverage.py`): a new workspace crate
   that nobody added to a shard fails CI instead of silently going untested.
8. **CI assist** (`.github/workflows/ci-assist.yml`): dispatchable `fmt`, `lock`,
   `fmt+lock`, `clippy-fix` repairs that commit back to a lane branch. Agents have
   no local toolchain, so formatting and `Cargo.lock` refreshes happen in Actions.

## Required checks are unchanged

The protocol in issue #20 names three required checks. They still exist with the
same names:

| Check | Provider |
|---|---|
| `test` | `rust-migration.yml` fan-in gate over the 4 shards + shard coverage |
| `Format and lint` | `production-build.yml` `quality` |
| `shell-and-lab-constraints` | `bootstrap.yml` (untouched) |

The exact-SHA merge gate (`--match-head-commit`) is unaffected: the coordinator
still merges serially, only after all three are green on the head SHA.

## After (measured on this branch and on `main` post-merge)

Filled in by the coordinator from the Actions API once the runs complete; see
`orchestration/receipts/wave3-ci-timings.json`.

| Workflow | Critical path | Notes |
|---|---:|---|
| Rust migration validation | _recorded_ | 4 shards in parallel + fan-in gate |
| Production build (PR) | _recorded_ | lint ∥ audit, no test duplication |
| Production build (main) | _recorded_ | + glibc ∥ musl release builds |

## Merge queue

Repo-level merge queue is **not** enabled by this lane: it requires a ruleset
change and would replace the coordinator's serialized `--match-head-commit`
merges. Both workflows already accept `merge_group`, so enabling it later is a
settings-only change. What this lane delivers is the measurable cycle-time
reduction plus the concurrency/caching work from the issue scope.

# CI throughput — wave 4 (issue #144)

## Baseline (measured from the Actions API, 2026-11-19)

Warm-cache runs; `p5/infra-135 @ 69e6f515` is the representative lane PR
(test workflow runs 402/403, production build 209, bootstrap 232, parity 357).

| Workflow | Job | Wall time |
|---|---|---:|
| Rust migration validation (pull_request) | `Test shard 1` (slowest) | 2.1–2.3 min |
| Rust migration validation (pull_request) | other shards | 1.6–2.2 min |
| Rust migration validation (**push**, same SHA) | all four shards | 2.4 min |
| Production build (PR) | `Format and lint` (clippy) | 0.7 min |
| Production build (PR) | `Dependency audit` | 0.2 min |
| Bootstrap validation | `shell-and-lab-constraints` | 0.3–0.4 min |
| Parity scoreboard (PR **and** push, same SHA) | `parity` | 0.1 + 0.1 min |

Step-level breakdown inside a shard job (run 403): service-container init
18–26 s, checkout 1 s, toolchain 8–10 s, cache restore 5–7 s, the
`Migrate schema` pre-step 3–13 s, actual shard tests 60–85 s.

Waste identified:

1. **Docs-only PRs paid the full Rust suite.** `specs/**` and
   `orchestration/**` PRs (a large share of lanes) ran the 4-shard matrix and
   clippy although no Rust source changed: ~2.4 min critical path for nothing.
2. **Every lane push ran the suite twice.** With an open PR, the same SHA was
   tested by the `push` event and again by `pull_request` (rust-migration
   2.4 + 2.3 min, parity 0.1 + 0.1 min of runner time per push).
3. **`star-db` suite ran 5× per push.** Each shard ran
   `cargo test -p star-db` as a pre-step, and shard 1 also lists `-p star-db`.
   Redundant: all 88 DB-backed test files apply `star_db::MIGRATOR` themselves
   (checksummed, idempotent) and star-http migrates at startup.
4. **`bootstrap.yml` installed shellcheck + cfn-lint on every PR**, even
   README-only ones (apt/pip time and a classic flake source).

## Changes

1. **Change-class fast path.** A `changes` job in `rust-migration.yml`,
   `production-build.yml`, and `bootstrap.yml` classifies the PR diff with
   `tools/ci/change_class.py` (single source of truth for the rules; reads the
   compare API, degrades to a full run on truncation/hiccups — never to a
   wrong skip). Docs-only PRs skip the shard matrix, fmt/clippy, and the
   shellcheck/cfn-lint installs; the required checks (`test`,
   `Format and lint`, `shell-and-lab-constraints`) still run on every PR and
   report green with an explicit skip reason. `merge_group`, `push` to main,
   and dispatch always run everything — the exact-SHA safety net is intact.
2. **Push runs restricted to `main`** for `rust-migration.yml` and `parity.yml`
   (production-build and bootstrap were already main-only). Lane pushes are
   validated through their pull_request runs; no more double testing per SHA.
3. **Dropped the per-shard `Migrate schema` pre-step** (see waste #3).
4. **Postgres health polling 10 s → 5 s** (retries 5 → 10) in the shard and
   release jobs: the service-init wait was 18–26 s of a ~2 min job.
5. **`tools/ci/**` added to `production-build.yml` PR paths** so
   `Format and lint` also reports on CI-tooling-only PRs (gap: it did not
   before, which would have left the required check missing on such PRs).

## After (measured)

Rust-classified round — this branch @ `f49f905a` (test run 404, production
build 210, bootstrap 233), i.e. the full suite including the new `changes`
jobs:

| Workflow | Critical path | Notes |
|---|---:|---|
| Rust migration validation | 2.5 min | changes 6 s → 4 shards (1.6–2.3 min) → gate 4 s |
| Production build (PR) | 0.9 min | fmt+clippy 0.8 min ∥ audit |
| Bootstrap validation | 0.7 min | lab fast path: shellcheck/cfn-lint installs skipped |

The Rust-changing PR path is therefore neutral-to-slightly-better (the removed
pre-step and faster DB polling roughly pay for the added `changes` hop); the
wins are the docs-only path below and the halved runner usage per lane push.

| PR type | Before | After |
|---|---:|---:|
| Rust-changing | ~2.4 min | ~2.5 min (measured; neutral within noise) |
| docs-only (specs/orchestration) | ~2.4 min | measured by the docs-only validation round on this branch — see PR #145 |
| lane push (PR open) | 2 suites per SHA | 1 suite (pull_request only) |
| main push | unchanged | unchanged (full verification by design) |

Classification is **PR-cumulative** (`base...head`): a PR that changes Rust
source anywhere in its diff runs the full suite on every push — conservative
by design. The docs-only fast path therefore applies to PRs whose entire diff
is specs/orchestration/docs (the common spec-lane shape), and was validated on
a dedicated docs-only demo PR (draft, closed after measurement); its measured
numbers are recorded in PR #145. `main` post-merge numbers to be filled in by
the coordinator from the Actions API.
