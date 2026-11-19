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

1. **Sharded the Supabase suite 4 ways.** Matrix legs balanced by the measured
   per-crate times above (LPT packing): 16.07/16.09/16.32/16.07 min of tests
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
  widest safe matrix at the current pooler limit.
- The ceiling is documented in the workflow header; widening the matrix
  requires raising the Supabase pooler limit (dashboard) or moving
  `SUPABASE_DATABASE_URL` to the transaction pooler (port 6543, much higher
  client ceiling) — the latter needs re-validating `sqlx migrate` advisory
  locks under transaction pooling first.
- If the pooler limit is raised to ~60: an 8-leg map (7.9–8.6 min/leg) brings
  the workflow to ~12 min, at which point the coverage lane (11.6 min) becomes
  the floor.

## After (measured)

Workflow-dispatch run on this lane (4 legs, cold cache on the first attempt;
`save-if: main` means steady-state numbers arrive after the first merge to
main):

| Workflow | Wall time | Notes |
|---|---:|---|
| Nightly compatibility | _recorded from the dispatch run before merge_ | 4 shard legs ∥ coverage (11.6 min) |
