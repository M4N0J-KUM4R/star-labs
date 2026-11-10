# star-labs → star-lms Migration Task Board

Master checklist for the migration effort: bring the 30-crate Rust workspace
(Canvas LMS parity re-implementation, axum 0.8 + sqlx + PostgreSQL) to full
integration-test coverage, CI-gated quality, and a production build pipeline.

Repo: <https://github.com/M4N0J-KUM4R/star-labs>
Last updated: 2026-11-10 — **workspace green (599 passed / 0 failed / 50
ignored by design, CI on PostgreSQL 16); REST parity 244/1403; all four CI
workflows green on `f55b479`.**

## Current status — 2026-11-10

**Completed**
- Phase 0: CI green; four pinned product bugs fixed; Supabase dev database;
  INT4/(i64,) decode family fixed; schema-portability migration 0030.
- Phase 1: parity scoreboard + CI baseline
  ([report](migration/parity/report.md)).
- Nested-route compatibility: 30 Canvas-compatible aliases across 8 crates.
- Phase 2 wave: permissions engine, developer keys/OAuth2, jobs API, and 21
  Canvas domain slices (terms/sections, files/folders, quizzes/questions,
  overrides, completions, revisions, read states, categories, outcomes,
  channels, enrollments, rubrics, grading periods, profiles, content exports);
  schema 68 tables; REST 167/1403.
- Phase 3 wave (24 branches, migrations 0055–0078): course/account/user
  settings, jobs v2, outcomes/files/content/calendar/group/module/
  quiz/discussion/enrollment/rubric/grading-scheme/dashboard/notification/
  page/conversation/submission-comment extras, role overrides, auth tokens,
  and LTI registrations. New `star-lti` crate; schema 93 tables; tests
  599/0/50; REST 244/1403 (17.4%), UI-referenced 149/503 (29.6%).
- Teradata trial: user-space Rust 1.99.1 dev environment; backend runs against
  Supabase. CI is the authoritative gate; Teradata is interactive-dev only.

**Pending**
- REST: 244/1403 routes (17.4%); UI-referenced 149/503 (29.6%).
- GraphQL: 0/390 files (0/170 types, 0/110 mutations).
- UI: 0/232 feature dirs, 0/5,459 JS/TSX files, 0/124 js_env files.
- Data migration, analytics, SIS/uploads/email/live events, cutover.
- Persistent deployment (the current Teradata exposure is a temporary tunnel).

## 1. Ground rules (established during this migration)

- One feature branch per crate / topic; `main` only receives `--no-ff` merges.
- Every DB-backed test skips cleanly when `DATABASE_URL` is unset (prints
  `SKIP: DATABASE_URL not set`), so `cargo test` works anywhere.
- Every DB-backed test uses a unique-per-run prefix (pid + millis) and full
  teardown, making it safe against shared/dev databases and repeat runs.
- Tests document the ACTUAL handler behavior (including quirks and deviations
  from Canvas), not the behavior we wish existed.
- Quality gates, enforced by CI on every push/PR touching `migration/**`:
  `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, workspace tests against a PostgreSQL 16 service, and a
  production `cargo build --release --locked` artifact.
- Local DB for development: PostgreSQL 17 on `127.0.0.1:5433`,
  `DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test`.

## 2. Infrastructure workstreams — all merged to `main`

| # | Workstream | Branch (merged) | Status |
|---|------------|--------|--------|
| 1 | Production deploy: hardened release-build systemd service, `lab-deploy.sh` | `feature/production-deploy` | ✅ merged |
| 2 | CI: three-stage production pipeline (`quality` → `test` → `release-build`) | `feature/ci-production-build` | ✅ merged |
| 3 | Workspace lint cleanup: fmt everywhere, clippy `-D warnings` to zero, star-accounts BFS traversal bugfix, un-break star-grading-standards tests | `chore/lint-cleanup` | ✅ merged |
| 4 | Test suites for 5 domain crates (auth, accounts, courses, users, assignments) | `feature/tests-star-*` ×5 | ✅ merged |
| 5 | Test suites for dashboard / api+common / db+content+http | `feature/tests-star-{dashboard,api-common,infra}` | ✅ merged |
| 6 | Deep suites for discussions / quizzes / modules (advances those phases) | `feature/tests-star-{discussions,quizzes,modules}` | ✅ merged |
| 7 | Post-merge lint fixes + pinning account-courses listing to the fixed BFS behavior | direct commit `39e9752` | ✅ on main |

## 3. Per-crate test coverage matrix (28 crates) — all covered

| Crate | Tests | Origin |
|-------|-------|--------|
| star-courses | 48 | new branch suite |
| star-assignments | 44 | new branch suite |
| star-api | 34 (15 unit + 19 integration) | new branch suite |
| star-accounts | 25 | new branch suite (BFS listing re-pinned post-fix) |
| star-quizzes | 20 + 2 parity | new branch suite |
| star-modules | 20 + 2 parity | new branch suite |
| star-discussions | 20 | new branch suite |
| star-users | 21 | new branch suite |
| star-auth | 12 | new branch suite |
| star-common | 10 | new branch suite |
| star-dashboard | 9 | new branch suite |
| star-db | 6 | new branch suite |
| star-grading-standards | 5 | main (un-broken: reqwest `json` feature) |
| star-content | 4 + 1 unit | new branch suite (merged-router) |
| star-pages / outcomes / calendar | 4 each | main |
| star-http | 3 (real-binary e2e) | new branch suite |
| star-groups | 10 | main (rewritten to source-pinned parity checks) |
| star-notifications | 6 | main |
| star-enrollments | 23 | main |
| star-rubrics / conversations / assignment-groups | 3 each | main |
| star-submission-comments / quizzes-parity / modules-parity / files / announcements | 2 each | main |

**Workspace totals: 599 passed / 0 failed / 50 ignored (canvas_parity
documentation suites, ignored by design), as reported by the CI validation
run on `f55b479`.** The matrix above is the Phase-1-era baseline; Phase 2/3
added suites to the same crates plus `star-permissions`, `star-jobs`, and
`star-lti`.

## 4. How to run everything

CI is the authoritative gate (`.github/workflows/`). On every push/PR touching
`migration/**`: `quality` (fmt, clippy `-D warnings`, migration-file checks),
a RustSec dependency audit, the full workspace test suite against PostgreSQL 16,
a production release build for both glibc and static musl (each booted and
smoke-tested on `/health`), bootstrap validation, and the parity scoreboard.
A nightly workflow runs the suite against Supabase (secret-gated) and publishes
llvm-cov coverage (baseline: 72.4% lines / 70.3% regions / 72.9% functions).
All five workflows are green on `221b748`.

```bash
cd migration
cargo fmt --all -- --check                       # gate 1
cargo clippy --workspace --all-targets -- -D warnings   # gate 2
cargo test --workspace                           # gate 3: 599 passed / 0 failed
cargo build --release --locked                   # what CI + lab-deploy.sh ship
```

The Teradata trial box (`~/JupyterLabRoot/star-labs-dev`, `./dev.sh`) is kept
only for interactive debugging: 2 vCPUs means a cold rebuild + full suite
takes ~30 min, so day-to-day validation runs in CI.

## 5. Merge log (all done)

1. `chore/lint-cleanup` — un-breaks grading-standards tests, zeroes clippy gate.
2. `feature/ci-production-build`, `feature/production-deploy`.
3. Eleven test branches (`--no-ff`, zero conflicts).
4. `docs/migration-tasks` (this file) — last.

## 6. Product bugs found by the new suites (pinned by tests, NOT yet fixed)

1. **star-quizzes authz leak (highest priority):** `GET
   /courses/{courseId}/quizzes/{id}/submissions` authorizes on the path
   course only — a teacher of course B can read another course's quiz
   submissions; missing quiz id returns `200 []` instead of 404.
2. **star-modules `POST /modules/{id}/items` always 500s:** the upsert uses
   `ON CONFLICT (module_id, item_type, item_id)` but migration 0002 defines
   no such unique constraint (SQLSTATE 42P10).
3. **star-modules `GET /courses/{courseId}/modules/{moduleId}/items` always
   500s** for existing modules: `modules.id` (BIGINT) is decoded into `i32`.
4. Minor: blank quiz title accepted on update (create validates it);
   discussions soft-deleted topic still returned by GET; teachers can start
   quizzes they are enrolled in; modules nested item routes take no auth.

## 7. Open items

- [x] Fix the four pinned bugs above (done in Phase 0; tests now assert the
      intended behavior).
- [x] Verify the first GitHub Actions run of `production-build.yml` on the
      pushed `main` (green).
- [x] Nested-route compatibility pass: 30 Canvas-compatible aliases added
      across eight crates (REST 167/1403, 11.9%).
- [x] Phase 1: parity scoreboard tool and CI baseline
      ([README](migration/tools/parity/README.md)).
- [x] Phase 2 and Phase 3 waves merged: 48 branches total, migrations up to
      0078, schema 93 tables, 599 tests, all CI workflows green on `f55b479`.
- [ ] Next parity wave: continue REST (244/1403 → target ~100% of
      UI-referenced 503), then GraphQL, then Canvas UI and analytics.
- [ ] UI parity (**0/232** feature dirs, 0/5459 JS/TSX files) and analytics
      (**0** endpoints) remain the largest gaps — see
      [migration/parity/report.md](migration/parity/report.md).
- [x] Optional: llvm-cov coverage report in CI (nightly, 72.4% lines),
      Dependabot-style supply-chain protection via `cargo audit` in CI.
- [ ] Add `SUPABASE_DATABASE_URL` repository secret to enable the nightly
      Supabase compatibility job.
