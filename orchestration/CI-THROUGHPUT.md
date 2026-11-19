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
| docs-only (specs/orchestration) | ~2.4 min | **4–6 s per required check** (measured on demo PR #146; PR green in well under a minute) |
| lane push (PR open) | 2 suites per SHA | 1 suite (pull_request only) |
| main push | unchanged | unchanged (full verification by design) |

Classification is **PR-cumulative** (`base...head`): a PR that changes Rust
source anywhere in its diff runs the full suite on every push — conservative
by design. The docs-only fast path therefore applies to PRs whose entire diff
is specs/orchestration/docs (the common spec-lane shape). Validated on a
dedicated docs-only demo PR (#146, draft, base `p5/infra-144`, closed after
measurement): `test` 4 s (shards skipping), `Format and lint` 6 s (fmt/clippy
skipped, migration hygiene still ran), `shell-and-lab-constraints` 4 s
(installs skipped, bash -n + AWS Academy guard still ran); gate log confirms
`rust-changes: false` → "no Rust-source changes in this PR — suite skipped by
design". `main` post-merge numbers to be filled in by the coordinator from
the Actions API.

# CI throughput — wave 5 (issue #149)

## Baseline (measured from the Actions API + job log, 2026-11-19)

Nightly compatibility run #11 (2026-11-18, `main`, warm cache): **1 h 14 m 59 s**
end to end.

| Job | Wall time | Notes |
|---|---:|---|
| `Supabase compatibility` | 74.9 min | 74.35 min inside the single `cargo test --workspace --exclude star-http` step |
| `Coverage (llvm-cov)` | 11.6 min | parallel lane, not the bottleneck |

Root cause: the 41-crate suite ran as **one serial cargo invocation**. Cargo
executes test binaries one at a time, each capped at `--test-threads=2` over the
remote Supabase session pooler. Per-binary wall times parsed from the job log
(timestamps between consecutive `Running` lines):

| Crate | Minutes | Crate | Minutes |
|---|---:|---|---:|
| star-assignments | 8.58 | star-learning-dates | 1.66 |
| star-accounts | 4.84 | star-rubrics | 1.52 |
| star-courses | 4.59 | star-groups | 1.47 |
| star-quizzes | 4.56 | star-permissions | 1.46 |
| star-discussions | 4.48 | star-dashboard | 1.37 |
| star-enrollments | 4.15 | star-lti | 1.27 |
| star-modules | 3.03 | star-outcomes | 1.05 |
| star-notifications | 2.48 | 17 more crates | 0.06–0.68 each |

Summed: ~64.5 min of per-binary wall time across 34 DB-backed crates (the other
7 are ~0-cost smoke suites), plus binary startup/migration overhead and link.

## Changes

1. **Sharded the Supabase suite 4 ways.** Matrix legs balanced by measured
   per-crate wall time (LPT packing): 21.15/21.20/21.17/21.17 min of tests
   per leg. Each leg keeps `--locked --no-fail-fast -- --test-threads=2`;
   `fail-fast: false` so a failing leg does not cancel the nightly signal.
2. **Preflight job** gates the matrix: secret check, the psql connectivity probe
   (apt/`postgresql-client` installed once instead of once per shard), and the
   shard-map coverage guard — `tools/ci/shard_coverage.py` is now parameterized
   (`--workflow`, `--exclude`) and asserts the nightly map covers all 41 crates
   exactly once with `star-http` intentionally outside it.
3. **Fan-in gate** keeps the `Supabase compatibility` job name (and step
   summary) the pre-shard workflow exposed.
4. **Per-shard cache keys** (`shared-key: nightly-shard-N`, `save-if: main`)
   following the wave-3 cache discipline. The first run after merge pays a cold
   compile per leg (~3–5 min); steady state restores each leg's own artifacts.
   Note: `workflow_dispatch` validation runs never save caches, so they always
   measure the cold case.
5. **Grazed-test retry** (`tools/ci/nightly_retry.py`): the 4-leg validation
   runs showed a ~2% per-test graze rate against the pooler ceiling (see
   below). When a leg fails, every failed test's stdout section must look like
   infrastructure noise (EMAXCONNSESSION / database error / connect / pool
   timeout) for a retry to be allowed; those tests alone are then re-run once
   (`--exact <names>`) after a 60 s cool-down. The gate can only cause a
   pointless retry, never mask a failure — whatever it lets through is re-run,
   and a persistent failure fails the retry too. Real failures exit before the
   retry and fail the leg immediately. Shard timeout raised 30 -> 45 min for
   the cold-cache + retry case.

## The binding constraint: the session pooler's 15-client ceiling

The first attempt shipped **8 legs** (7.9–8.6 min of tests each, projected
~12 min wall clock). The validation dispatch on this branch failed all 8 legs
with 505 of 508 test failures reporting:

    (EMAXCONNSESSION) max clients reached in session mode -
    max clients are limited to pool_size: 15

Zero failures were data races — the per-run-prefix isolation held perfectly
under full concurrency; the suite simply exceeded the Supabase session
pooler's hard client ceiling (8 legs x ~2-3 clients at `--test-threads=2`,
one sqlx pool per test with churn between tests).

Consequences for the design:

- **4 legs x ~2-3 clients = 8-12 peak** against the 15-client ceiling — the
  widest safe matrix at the current pooler limit. Even so, transient pool-churn
  spikes graze it: the 4-leg validation runs measured **2-5 failed tests per
  leg (~2%), 100% of them EMAXCONNSESSION** and zero data races. That is what
  the grazed-test retry (change 5) absorbs.
- **Shared-DB contention is real:** running 4 legs concurrently slows the
  aggregate suite ~28% (66.5 -> 84.7 min of summed test-binary wall on the
  identical test set) — the small Supabase compute is the bottleneck, not the
  runners. The shard map is balanced on the contended numbers, so the legs
  stay level in practice (measured 26.2/19.9/19.8/19.0 before this rebalance,
  designed 21.2 +- 0.1 after).
- The ceiling is documented in the workflow header; widening the matrix
  requires raising the Supabase pooler limit (dashboard) or moving
  `SUPABASE_DATABASE_URL` to the transaction pooler (port 6543, much higher
  client ceiling) — the latter needs re-validating `sqlx migrate` advisory
  locks under transaction pooling first.
- If the pooler limit is raised to ~60: an 8-leg map brings the workflow to
  the coverage lane's floor (~11-12 min).

## After (measured)

Validation dispatches on this lane (cold cache — dispatches never save caches;
`save-if: main` means steady-state numbers arrive after the first merge to
main):

| Run | Wall time | Notes |
|---|---:|---|
| 4 legs, pre-retry (v1 map) | 30.5 min | shard 4 hit the 30-min timeout; 20 grazes failed the run |
| 4 legs, retry dead (`bash -e` bug) | 37.7 min | shard 4 37.2 min on the v1 map; grazes un retried |
| 4 legs, rebalanced + retry fixed | _final dispatch_ | 21.2 min of tests per leg by design |

| Workflow | Wall time | Notes |
|---|---:|---|
| Nightly compatibility | _final dispatch number_ | 4 shard legs ∥ coverage (10.5-12.2 min) |
