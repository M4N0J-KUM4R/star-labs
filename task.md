# star-labs → star-lms Migration Task Board

Master checklist for the migration effort: bring the 28-crate Rust workspace
(Canvas LMS parity re-implementation, axum 0.8 + sqlx + PostgreSQL) to full
integration-test coverage, CI-gated quality, and a production build pipeline.

Repo: <https://github.com/M4N0J-KUM4R/star-labs>
Last updated: 2026-11-09 — **all branches merged to `main`; workspace green
(357 passed / 0 failed / 51 ignored by design).**

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

**Workspace totals: 357 passed / 0 failed / 51 ignored (canvas_parity
documentation suites, ignored by design).**

## 4. How to run everything locally

```bash
export DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
cd migration
cargo fmt --all -- --check                       # gate 1
cargo clippy --workspace --all-targets -- -D warnings   # gate 2
cargo test --workspace                           # gate 3: 357 passed / 0 failed
cargo build --release --locked                   # what CI + lab-deploy.sh ship
```

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

- [ ] Fix the four pinned bugs above (small, well-localized fixes; tests
      already document the intended behavior).
- [ ] Verify the first GitHub Actions run of `production-build.yml` on the
      pushed `main`.
- [ ] UI parity (**0/232** feature dirs, 0/5459 JS/TSX files) and analytics
      (**0** endpoints) remain the largest gaps; measured REST parity is
      **47/1202** (3.9%) and GraphQL **0/390** — see
      [migration/parity/report.md](migration/parity/report.md).
- [x] Phase 1: parity scoreboard tool and CI baseline
      ([README](migration/tools/parity/README.md)).
- [ ] Optional: llvm-cov coverage report in CI, Dependabot, release tagging.
