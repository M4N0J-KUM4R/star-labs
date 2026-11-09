# star-labs → star-lms Migration Task Board

Master checklist for the migration effort: bring the 28-crate Rust workspace
(Canvas LMS parity re-implementation, axum 0.8 + sqlx + PostgreSQL) to full
integration-test coverage, CI-gated quality, and a production build pipeline.

Repo: <https://github.com/M4N0J-KUM4R/star-labs>
Last updated: 2026-11-09

## 1. Ground rules (established during this migration)

- One feature branch per crate / topic. `main` only receives merges.
- Every DB-backed test skips cleanly when `DATABASE_URL` is unset (prints
  `SKIP: DATABASE_URL not set`), so `cargo test` works anywhere.
- Every DB-backed test uses a unique-per-run prefix (pid + millis) and full
  teardown, making it safe against shared/dev databases and repeat runs.
- Tests document the ACTUAL handler behavior (including quirks and deviations
  from Canvas), not the behavior we wish existed.
- Quality gates, enforced by CI on every PR touching `migration/**`:
  `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, workspace tests against a PostgreSQL 16 service, and a
  production `cargo build --release --locked` artifact.
- Local DB for development: PostgreSQL 17 on `127.0.0.1:5433`,
  `DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test`.

## 2. Infrastructure workstreams

| # | Workstream | Branch | Status |
|---|------------|--------|--------|
| 1 | Production deploy: hardened release-build systemd service, `lab-deploy.sh` release profile | `feature/production-deploy` | ✅ pushed |
| 2 | CI: three-stage production pipeline (`quality` → `test` → `release-build`, `.github/workflows/production-build.yml`) | `feature/ci-production-build` | ✅ pushed |
| 3 | Workspace lint cleanup: `cargo fmt` everywhere, clippy `-D warnings` to zero, star-accounts BFS traversal bugfix, un-break star-grading-standards tests (missing reqwest `json` feature) | `chore/lint-cleanup` | ✅ committed, verified (101 passed / 0 failed) |
| 4 | Tests for the first 5 previously-untested domain crates | `feature/tests-star-{auth,accounts,courses,users,assignments}` | ✅ committed (150 tests) |
| 5 | Tests for the remaining 7 infrastructure/aggregator crates | `feature/tests-star-discussions`, `feature/tests-star-dashboard`, `feature/tests-star-api-common`, `feature/tests-star-infra` | 🔄 4 parallel agents in flight |
| 6 | Push all branches, define merge order (this file) | `docs/migration-tasks` | ✅ this branch |

## 3. Per-crate test coverage matrix (28 crates)

Crates marked **main** already had committed tests before this migration;
crates marked **branch** get their suite on the named feature branch.

| Crate | Tests | Where | Status |
|-------|-------|-------|--------|
| star-enrollments | 23 | main | ✅ |
| star-groups | 10 | main (rewritten on lint branch: always-true asserts → source-pinned parity checks) | ✅ |
| star-notifications | 6 | main | ✅ |
| star-grading-standards | 5 | main (un-broken on lint branch) | ✅ |
| star-pages | 4 | main | ✅ |
| star-outcomes | 4 | main | ✅ |
| star-calendar | 4 | main | ✅ |
| star-rubrics | 3 | main | ✅ |
| star-conversations | 3 | main | ✅ |
| star-assignment-groups | 3 | main | ✅ |
| star-submission-comments | 2 | main | ✅ |
| star-quizzes | 2 | main | ✅ |
| star-modules | 2 | main | ✅ |
| star-files | 2 | main | ✅ |
| star-announcements | 2 | main | ✅ |
| star-courses | 48 | `feature/tests-star-courses` | ✅ committed |
| star-assignments | 44 | `feature/tests-star-assignments` | ✅ committed |
| star-accounts | 25 | `feature/tests-star-accounts` | ✅ committed |
| star-users | 21 | `feature/tests-star-users` | ✅ committed |
| star-auth | 12 | `feature/tests-star-auth` | ✅ committed |
| star-discussions | — | `feature/tests-star-discussions` | 🔄 agent |
| star-dashboard | — | `feature/tests-star-dashboard` | 🔄 agent |
| star-api + star-common | — | `feature/tests-star-api-common` | 🔄 agent |
| star-db + star-content + star-http | — | `feature/tests-star-infra` | 🔄 agent |

Totals: **111 tests already on main + 150 new on feature branches = 261**,
before the 4 in-flight suites land.

## 4. What the new suites cover

- **star-auth (12)** — register/login/me/logout over the real router, JWT
  verify/token helpers, AuthUser extractor resolving roles from DB.
- **star-accounts (25)** — account tree, sub-accounts, per-account
  courses/users listings, admin grants, authz matrix.
- **star-courses (48)** — CRUD + workflow states, enrollment CRUD, pagination,
  full lifecycle, 500-regression guard for enrollment creation.
- **star-users (21)** — list/self/get/update/delete, pagination clamping,
  password rehash, token invalidation after delete, role enforcement (DB role,
  never the token claim).
- **star-assignments (44)** — CRUD, submissions incl. resubmit upsert,
  grading, gradebook, cascades, full authz matrix.

## 5. How to run everything locally

```bash
export DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
cd migration
cargo fmt --all -- --check                       # gate 1
cargo clippy --workspace --all-targets -- -D warnings   # gate 2
cargo test --workspace                           # gate 3 (DB-backed tests skip without DATABASE_URL)
cargo build --release --locked                   # production build (what CI + lab-deploy.sh ship)
```

## 6. Recommended merge order

1. `chore/lint-cleanup` — must land first: it fixes tests that do not compile
   on `main` (star-grading-standards) and zeroes the clippy gate the CI
   enforces. Verified: fmt clean, clippy `-D warnings` clean, 101 passed /
   0 failed.
2. `feature/ci-production-build` — already on the remote; PRs run green once
   (1) merges.
3. The five test branches — any order. `star-courses`, `star-users` and
   `star-assignments` carry an identical `Cargo.lock`, so they merge without
   conflicts; `star-auth` and `star-accounts` only add files.
4. The four in-flight infra/discussion/dashboard branches — land after their
   agents report green runs.

## 7. Open items after this wave

- [ ] Land + verify the 4 in-flight agent branches (discussions, dashboard,
      api+common, db+content+http).
- [ ] Post-merge: rebase remaining branches on updated `main` so PRs merge
      conflict-free.
- [ ] Optional hardening: coverage report (llvm-cov) wired into CI, Dependabot,
      CODEOWNERS, release tagging of the `--locked` binary.
- [ ] Deep end-to-end pass over star-http (currently smoke-level: /health,
      fail-fast on bad config, 404 shape).
