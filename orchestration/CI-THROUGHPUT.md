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
