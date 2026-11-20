# Browser smoke tests

Playwright end-to-end smoke tests for the server-rendered `star-ui` shell,
defined by `specs/p5-ui-shell.md` §4 (journeys) and §6 (CI plan). The suite
runs in GitHub Actions (`.github/workflows/browser-smoke.yml`) against a real
`star-http` debug binary and a real PostgreSQL 16 service container — no
mocks, no route interception.

## Coverage

| Journey | Spec file | What it proves |
| --- | --- | --- |
| Login → dashboard | `tests/login-dashboard.spec.ts` | Logged-out `/` and `/dashboard` redirect to `/login?next=…`; the seeded teacher logs in through the real CSRF-protected form; the dashboard renders at least one `.ic-DashboardCard` with `data-course-id` linking `/courses/{id}` via `a.ic-DashboardCard__link`; `window.ENV.current_user_id` is set. |
| Dashboard → course home | `tests/dashboard-course-home.spec.ts` | Clicking a card lands on `/courses/{id}`; `ul#section-tabs` renders the 13 tabs in the fixed `DEFAULT_COURSE_TABS` order (Settings last for a teacher); the front-page body renders in `#wiki_page_show .user_content`; `ENV.context_asset_string === "course_{id}"` and `ENV.active_context_tab === "home"`; a course with no front page renders `#context_modules .context_module`. |
| Access control + logout | `tests/access-control-logout.spec.ts` | Logged-out `/dashboard` redirects to login; an unenrolled non-admin gets the error page (403/404) on `/courses/{id}` with no course-name leak; teacher logout via `form[action="/logout"]` (hidden `_csrf`, double-submit) redirects to `/login` and the stale session no longer authorizes `/dashboard`. |

## Local run

```bash
# 1. PostgreSQL with an empty database (migrations run at server boot).
createdb star_lms_smoke

# 2. Build + start the real binary (STAR_PORT default 8080; 18089 matches CI).
cd migration
DATABASE_URL=postgres://localhost/star_lms_smoke \
STAR_JWT_SECRET=dev-secret STAR_PORT=18089 \
  ./target/debug/star-http        # or: cargo run --locked -p star-http

# 3. Seed the fixture (idempotent — safe to re-run).
psql "postgres://localhost/star_lms_smoke" \
  -v ON_ERROR_STOP=1 -f browser-smoke/fixtures/seed.sql

# 4. Install + run the suite (config defaults to http://localhost:18089).
cd ../browser-smoke
npm ci
npx playwright install --with-deps chromium
npx playwright test                       # or: STAR_BASE_URL=http://localhost:18089
```

## Seeding strategy

- **Why SQL and not the API:** `POST /api/v1/auth/register` only allows
  `role=student`, and the journeys need a teacher with enrollments. The
  fixture therefore inserts users, courses, enrollments, a front page and a
  module directly, with the exact column lists used by
  `migration/crates/star-ui/tests/ui_integration.rs`.
- **Password hash:** an argon2id PHC string for `SmokeTest-123!` with the
  Rust `argon2` crate defaults (`v=19, m=19456, t=2, p=1`) — identical to
  what `star-auth::hash_password` produces, so it verifies through the
  normal login path. No plaintext password is ever stored.
- **Sentinel ids + `setval`:** fixture rows use explicit `9900xx` ids and
  every touched sequence (`users_id_seq`, `courses_id_seq`,
  `enrollments_id_seq`, `pages_id_seq`, `modules_id_seq`) is re-based with
  `setval(..., GREATEST(MAX(id), sentinel))`, so later inserts never
  collide. The fixture deletes its sentinel rows first, making it
  idempotent for local re-runs.
- **Ordering:** the seed runs *after* the server boots — `star-http` runs
  `star_db::MIGRATOR` at startup (see `migration/crates/star-http/src/main.rs`),
  so the schema exists by the time `psql -f seed.sql` executes.

## Maintenance notes

- **Tab order** in `tests/dashboard-course-home.spec.ts` mirrors
  `DEFAULT_COURSE_TABS` in `migration/crates/star-courses/src/lib.rs`
  (plus the teacher-only Settings tab). If the tabs API changes, update
  `EXPECTED_TAB_LABELS` to match.
- **Sentinel ids** live in two places: `fixtures/seed.sql` and
  `tests/helpers.ts`. Keep them in sync.
- **Port:** the workflow uses `STAR_PORT=18089`; `playwright.config.ts`
  defaults to `http://localhost:18089` and can be overridden with
  `STAR_BASE_URL`.
- **Selectors** follow the Canvas-style contracts rendered by the askama
  templates (`#login_form`, `.ic-DashboardCard`, `#section-tabs`,
  `#wiki_page_show`, `#context_modules`, `form[action="/logout"]`); prefer
  `getByRole`/`getByLabel` where the template gives accessible names.
- Failures upload `migration/server.log`, `test-results/` and
  `playwright-report/` as CI artifacts (`browser-smoke-failure`).
