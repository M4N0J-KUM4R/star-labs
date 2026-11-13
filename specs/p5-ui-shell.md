# Spec: UI shell + first vertical journey (MIG-06, issue #22)

> Phase-5 spec for the `specs` epic. Defines the server-rendered UI shell for
> star-lms and the first end-to-end vertical journey (login → dashboard →
> course home). Documentation only: no Rust code, no migrations, no cargo
> changes ship with this file.

## 0. Scope provenance

`full-migration-plan.md` was **not found anywhere under `/home/z/my-project`**
(verified with a recursive `find`; the Canvas reference checkout and all
worktrees were searched). No agent has it, and it is not in git history of
this branch. The scope of this spec is therefore derived from:

1. `task.md` (repo root) — the master task board: 30-crate axum 0.8 + sqlx +
   PostgreSQL workspace, CI-gated quality (fmt / clippy `-D warnings` /
   workspace tests on PostgreSQL 16 / release build), REST 244/1403, UI
   parity 0/232 feature dirs.
2. The Canvas reference checkout at `/home/z/my-project/repos/canvas-lms` —
   specifically `app/views/layouts/application.html.erb`,
   `app/views/layouts/_head.html.erb`, `js_env` defaults in
   `app/controllers/application_controller.rb:239`, `users#user_dashboard`
   and `users#dashboard_cards` (`app/controllers/users_controller.rb`),
   `courses#show` (`app/controllers/courses_controller.rb`), and
   `config/routes.rb`.
   Note: the issue research originally pointed at
   `app/helpers/js_env_helper.rb`; that file does not exist in the current
   Canvas tree — `js_env` lives in `application_controller.rb:239`, which is
   what this spec is derived from.
3. The committed parity scoreboard (`migration/parity/report.md`,
   `migration/parity/coverage.json`, refreshed scoreboard numbers below) and
   the current star-lms code (`migration/crates/star-http`,
   `star-auth`, `star-dashboard`, `star-pages`, `star-announcements`).

Baseline parity numbers this spec plans against (committed scoreboard,
`migration/parity/report.md`): **REST 244/1403 (17.4%)**, **UI-referenced
REST 149/503 (29.6%)**, **0/232 UI feature dirs**, **0/124 js_env files**,
0/5459 JS/TSX files, 0/475 ERB views, GraphQL 0/390 files. The UI side of
the migration is 0% in every measured dimension; this spec is the entry
ramp for the first non-zero slice.

## 1. Overview & goals

**Goal.** Stand up a server-rendered HTML shell in Rust — global navigation,
layout hierarchy, static assets, session auth, and a `window.ENV` contract —
and ship one complete vertical journey through it (login → dashboard →
course home). This mirrors how Canvas itself is built: Rails ERB layouts
(`application.html.erb` + `_head.html.erb`) with progressive-enhancement
JavaScript reading `window.ENV`, not an SPA.

**Server-rendered shell, askama recommended.** The shell is rendered by the
existing `star-http` binary using **askama** templates. Justification:

- Compile-time templates: a broken template is a compile error, caught by the
  existing `cargo clippy --workspace --all-targets -D warnings` / build CI
  gate instead of a runtime 500. This matches the repo's "fail in CI, not in
  production" posture (`task.md` ground rules).
- Zero runtime template dependency, zero node/webpack build — consistent with
  the no-SPA constraint and the existing release pipeline (`cargo build
  --release --locked` + musl static artifact boot smoke test).
- Typed context structs force the js_env contract (section 3) to be explicit
  Rust structs rather than stringly-typed hashes.
- Alternatives rejected: minijinja (runtime errors surface in production;
  no typed contexts), maud (HTML-in-Rust macros are hostile to designers
  editing layout markup), a JS SPA (explicitly out of scope — no rewrite).

**Progressive enhancement, no SPA rewrite.** Phase 1 pages must be fully
usable with JavaScript disabled: course cards, section tabs, the front page,
and module lists render server-side. A small vanilla-JS layer may later read
`window.ENV` to enhance (as Canvas's bundles do), but nothing in the journey
may *require* JS. No React, no Canvas `ui/` backport, no GraphQL dependency
in phase 1.

**Non-goals (phase 1).** Brandable/themed CSS, i18n/RTL, the full 232 Canvas
feature dirs, Canvas's webpack bundle system, K-5/educator dashboards,
masquerade/student-view chrome, and any GraphQL-backed UI.

## 2. Shell architecture

### 2.1 Layout hierarchy

Three askama layout templates, mirroring Canvas's ERB structure:

| Template | Canvas analogue | Contents |
|---|---|---|
| `layouts/base.html` | `layouts/_head.html.erb` + outer shell of `application.html.erb` | `<!DOCTYPE html>`, `<head>` (charset, viewport, `<title>`, hashed CSS `<link>`, theme-color), the `ENV`/`INST` `<script>` block, flash-notice region, `aria-live` alert region, hashed JS includes at end of body |
| `layouts/authenticated.html` (extends base) | `application.html.erb` with `shared/new_nav_header` | global navigation (left rail: Dashboard, Courses, Calendar, Inbox, Help; user menu), breadcrumb bar, `#wrapper` main column, right-side `<aside>` slot, footer; body classes `with-left-side`, `primary-nav-expanded`, etc. |
| `layouts/course.html` (extends authenticated) | `application.html.erb` course context branch | course context: left-side section tabs (from `GET /api/v1/courses/:id/tabs`, same order the API returns), `context_asset_string`, `active_context_tab`, course-menu collapse toggle |

Page templates render into the `content` block of the innermost layout:
`login.html` (base only), `dashboard.html` (authenticated), and
`course_home.html` / per-tab pages (course). Canvas parity details carried
over: body class `with-left-side` when tabs exist, `no-headers` for the login
page, and `window.CANVAS_COOKIE_CONSENT_STATE` omitted entirely (no consent
tooling in star-lms).

### 2.2 Routing map

New crate `star-ui` provides a `Router<AppState>` merged in
`star-http/src/main.rs::build_app_router`-level assembly (mounted **at the
top level, not nested under `/api/v1`**), matching the existing pattern
where `/health` and `/api/graphql` already sit outside the REST nest:

| Path | Purpose | Served by | Auth |
|---|---|---|---|
| `/` | redirect → `/dashboard` (Canvas `root => users#user_dashboard`) | star-ui | session cookie |
| `/login` | login form (Canvas `login_controller`) | star-ui | public |
| `/dashboard` | dashboard page (Canvas `users#user_dashboard` HTML) | star-ui | session cookie |
| `/courses/:id` | course home (Canvas `courses#show` HTML branch) | star-ui | session cookie |
| `/courses/:id/:tab` | course tab landing pages | star-ui | session cookie |
| `/static/{hash}/{file}` | hashed static assets | star-ui (tower-http `ServeDir`) | public, immutable |
| `/health` | health probe (existing) | star-http | public |
| `/api/v1/*` | REST parity — **unchanged** | 28 existing crate routers | `Authorization: Bearer` JWT |
| `/api/graphql` | GraphQL gateway (existing mount) | star-graphql | Bearer |

Shell handlers are *thin*: they authorize, call the same underlying query
functions the REST handlers use (or issue internal HTTP calls to
`/api/v1/...`), and render. No business logic is duplicated into templates.

### 2.3 Asset pipeline

- **No node build for the shell.** Hand-authored CSS (one file per layout
  scope: `app.css`, `course.css`) and at most one small vanilla JS file.
- Files are served with content-hashed names, `/static/{sha256-prefix}/{file}`,
  generated by an xtask/build script that verifies the hash matches at
  startup. Response headers: `Cache-Control: public, max-age=31536000,
  immutable`.
- `ENV.ASSET_HOST` defaults to `null` (same-origin) — Canvas uses it for CDN
  offload; star-lms keeps the key in the contract so the shape is stable.
- Fonts: system font stack in phase 1 (Canvas's Lato preloads are a branding
  concern, deferred).

## 3. js_env contract

Canvas renders `window.ENV` in `layouts/_head.html.erb` from the `@js_env`
hash initialized in `application_controller.rb#js_env` (line 239). Canvas has
124 js_env files / 490 call sites; star-lms has 0. The shell re-introduces a
**single, typed ENV struct** rendered once in `base.html`. Phase-1 keys are
required by the shell and the first journey; phase-2 keys are frozen in
shape now and returned as null/false constants until their subsystems exist.

### Phase 1 — required for the journey (18 keys)

| Canvas key | star-lms mapping | Phase | Serialization shape |
|---|---|---|---|
| `current_user_id` | `GET /api/v1/auth/me` (star-auth) / `users.id` | 1 | integer |
| `current_user_roles` | `users.role` via `/auth/me` | 1 | array of strings (single role today) |
| `current_user_is_student` | derived: `role == "student"` | 1 | boolean |
| `DOMAIN_ROOT_ACCOUNT_ID` | root account id (accounts table) | 1 | integer |
| `RAILS_ENVIRONMENT` | new `STAR_ENV` env var (default `"development"`) | 1 | string |
| `ASSET_HOST` | static mount config | 1 | string or null |
| `DEEP_LINKING_POST_MESSAGE_ORIGIN` | request base URL | 1 | string |
| `active_context_tab` | shell route handler arg (e.g. `"home"`) | 1 | string or null |
| `context_asset_string` | `"{context}_{id}"` (e.g. `course_42`) | 1 | string |
| `PREFERENCES.dashboard_view` | user preference (needs column, see §7) | 1 | `"cards"` or `"activity"` |
| `PREFERENCES.custom_colors` | course_id → hex color | 1 | object<string, string> |
| `PREFERENCES.hide_dashcard_color_overlays` | user preference | 1 | boolean |
| `SETTINGS.collapse_global_nav` | user preference | 1 | boolean |
| `use_high_contrast` | user preference | 1 | boolean |
| `help_link_name` | constant `"Help"` | 1 | string |
| `k12` | account setting | 1 | boolean (false) |
| `files_domain` | config | 1 | string |
| `notices` | flash messages on redirect | 1 | array of strings |

### Phase 2 — frozen shape, constant/null until subsystems land (~17 keys)

| Canvas key | star-lms mapping | Phase | Serialization shape |
|---|---|---|---|
| `current_user_global_id` | global ids (no shard concept) | 2 | integer (equals `current_user_id`) |
| `current_user_uuid` | `users.uuid` (column needed) | 2 | string |
| `current_user_usage_metrics_id` | no metrics collection | 2 | null |
| `current_user_types` | account-users role names | 2 | array of strings |
| `current_user_disabled_inbox` | user preference | 2 | boolean |
| `current_user_visited_tabs` | user preference | 2 | array of strings |
| `discussions_reporting` | feature flag | 2 | boolean (false) |
| `group_information` | groups API | 2 | object or null |
| `DOMAIN_ROOT_ACCOUNT_UUID` | accounts uuid | 2 | string |
| `captcha_site_key` | no captcha provider | 2 | null |
| `confetti_branding_enabled` | site-admin flag | 2 | boolean (false) |
| `active_brand_config` | branding subsystem (none) | 2 | null |
| `active_brand_config_json_url` | branding subsystem (none) | 2 | null |
| `url_to_what_gets_loaded_inside_the_tinymce_editor_css` + `url_for_high_contrast_tinymce_editor_css` | RCE phase | 2 | array of strings (empty) |
| `LTI_LAUNCH_FRAME_ALLOWANCES` | star-lti launch phase | 2 | array of strings |
| `comment_library_suggestions_enabled` | comment library | 2 | boolean (false) |
| `FEATURES` + remaining `SETTINGS` (`open_registration`, `can_add_pronouns`, `show_sections_in_course_tray`, `enable_content_a11y_checker`, `suppress_assignments`) | feature-flag registry | 2 | object<string, boolean> |

Rendering rule (Canvas parity): `ENV` is emitted once in `<head>` as JSON,
before any script include; no key may be added outside this table without
bumping the contract and updating `star-ui` types. SENTRY/PENDO/usage-metrics
keys are permanently omitted (no telemetry in star-lms).

## 4. First journey definition — dashboard → course home

Page-by-page contract. Endpoint status is cross-checked against
`migration/parity/coverage.json` (committed scoreboard) and the crate source;
deviations from the issue research notes are flagged.

| # | Page | Shell route | Data required | Endpoint used | Status |
|---|---|---|---|---|---|
| 1 | Login | `GET/POST /login` | user record, password verify | `POST /api/v1/auth/login` internals (same `verify_password` + `create_token`) | **exists** (star-auth) |
| 2 | Dashboard | `GET /dashboard` | course cards | `GET /api/v1/dashboard/dashboard_cards` | **exists** — `users` controller match, ui-referenced |
| 3 | Dashboard sidebar | `GET /dashboard` | todo items | `GET /api/v1/users/self/todo` | **exists** — ui-referenced |
| 4 | Dashboard sidebar | `GET /dashboard` | upcoming events | `GET /api/v1/users/self/upcoming_events` | **exists** — not ui-referenced |
| 5 | Dashboard | `GET /dashboard` | favorite courses | `GET /api/v1/users/self/favorites/courses` (+ POST/DELETE variants) | **exists** — star-dashboard "Phase 4 favorites"; note: parity tooling currently shows no Canvas match for these paths (manifest gap), routes verified in `crates/star-dashboard/src/lib.rs` |
| 6 | User menu | `GET /dashboard` | profile | `GET /api/v1/users/:user_id/profile` | **exists** — `profile` controller match, ui-referenced |
| 7 | Course home | `GET /courses/:id` | course object | `GET /api/v1/courses/:id` | **exists** — ui-referenced |
| 8 | Course home | `GET /courses/:id` | section tabs | `GET /api/v1/courses/:course_id/tabs` | **exists** — ui-referenced |
| 9 | Course home | `GET /courses/:id` | my enrollment | `GET /api/v1/courses/:course_id/enrollments` | **exists** — ui-referenced |
| 10 | Course home (modules view) | `GET /courses/:id` | modules | `GET /api/v1/courses/:course_id/modules` | **exists** — ui-referenced |
| 11 | Course home (assignments view) | `GET /courses/:id` | assignments | `GET /api/v1/courses/:course_id/assignments` | **exists** — ui-referenced |
| 12 | Course home (announcements) | `GET /courses/:id` | announcements | `GET /api/v1/courses/:course_id/announcements` | **exists** — star-announcements; parity lists it as a star extra (Canvas exposes course announcements via the `announcements` controller layout) |
| 13 | Course home (wiki view) | `GET /courses/:id` | front page | `GET /api/v1/courses/:course_id/front_page` | **exists** — `wiki_pages_api` match in coverage.json. **Correction:** the issue research listed this as missing; coverage.json and `crates/star-pages/src/lib.rs` (GET/PUT `/courses/{courseId}/front_page`, migration 0075 `pages.front_page`) show it implemented |
| 14 | Course home (activity) | `GET /courses/:id` | missing submissions | `GET /api/v1/users/self/missing_submissions` | **missing — phase 2** |
| 15 | Dashboard (activity view) | `GET /dashboard` | planner items | `GET /api/v1/planner/items` | **missing — phase 2** (only `planner_notes` CRUD exists, 5/5) |
| 16 | Dashboard (activity view) | `GET /dashboard` | stream items | `GET /api/v1/dashboard/stream_items` | **missing — phase 2** |

Phase-1 journey rendering rule: the course home page branches on
`course.default_view` (`wiki` → front page from row 13, `modules` → modules
list from row 10, `assignments`/`syllabus` → respective lists from rows 11).
The activity-stream dashboard view (rows 14–16) is phase 2; phase 1 renders
the card view only, with `PREFERENCES.dashboard_view` fixed to `"cards"`.

### Acceptance criteria (9)

1. Logged-out `GET /dashboard` responds 302 to `/login` with a `next`
   query parameter; `/login` renders 200 with the base layout and
   `ENV.current_user_id` absent.
2. `POST /login` with valid credentials sets the `star_session` HttpOnly
   cookie (JWT minted by `star-auth::create_token`) and responds 303 to
   `/dashboard`; invalid credentials re-render the form with an error and
   without leaking whether the email exists.
3. `GET /dashboard` renders 200 with the global nav and exactly the 18
   phase-1 `ENV` keys of section 3, each with the specified type.
4. One course card renders per published course returned by
   `GET /api/v1/dashboard/dashboard_cards` (verified: the star-dashboard
   `DashboardCard` payload), each card linking to `/courses/:id`; unpublished
   courses render with the unpublished style, matching the API's
   `published` flag.
5. `GET /courses/:id` for an enrolled user renders 200 with section tabs in
   the same order as `GET /api/v1/courses/:id/tabs`,
   `ENV.context_asset_string == "course_{id}"`, and
   `ENV.active_context_tab == "home"`.
6. Course home content follows `course.default_view`: a course with
   `default_view = "wiki"` renders its front page body; `default_view =
   "modules"` renders the module list; the chosen branch is asserted in
   tests for both settings.
7. `GET /courses/:id` for a user with no enrollment and no admin role does
   not render course data (401/404 page), matching the API-side course
   visibility gate — the shell must not become a bypass around `/api/v1`
   authorization.
8. Every shell page references only content-hashed assets under
   `/static/{hash}/` (the sole inline `<script>` is the `ENV` block), and
   the journey is completable with JavaScript disabled (cards, tabs, and
   front page visible in the server-rendered HTML).
9. Hitting a shell URL while logged out, then logging in, lands the user on
   the originally requested path (`next` round-trip), and `POST /logout`
   invalidates the session server-side (updates `users.last_logged_out`,
   which the existing `AuthUser` logic already honors) and redirects to
   `/login`.

## 5. Auth/session

**Login flow.** `GET /login` renders a form (CSRF token, see below).
`POST /login` form-decodes email/password, reuses `star-auth`'s
`verify_password` (argon2) against `users.password_hash`, and on success
mints a JWT with the **existing** `star-auth::create_token(user_id, role,
secret)` — same HS256 secret (`STAR_JWT_SECRET`), same `Claims { sub, role,
exp }`, same 7-day expiry — and sets it as a cookie:

```
Set-Cookie: star_session=<jwt>; HttpOnly; SameSite=Lax; Path=/; Secure
```

(`Secure` whenever the deployment terminates TLS; omitted for local/CI
plain-HTTP.)

**Two extractors, one token format.**

- `/api/v1/*` keeps `AuthUser` unchanged: `Authorization: Bearer <token>`
  only. No REST client breaks.
- The shell gets a new `SessionUser` extractor that reads the
  `star_session` cookie and reuses the exact same verification path as
  `AuthUser`: `verify_token` against `AppState.jwt_secret`, role reload
  from the DB by `sub` (token role claim never trusted), and rejection of
  tokens minted before `users.last_logged_out`. The shared logic should be
  factored so the two extractors cannot drift.

**CSRF — double-submit token.** Cookie-based auth re-opens CSRF that Bearer
auth closes. The shell issues a random 128-bit `star_csrf` cookie
(non-HttpOnly so future JS can echo it) plus a hidden form field on every
shell POST form; shell POST handlers reject with 403 when they differ
(constant-time compare). `/api/v1` is exempt (still Bearer, not
cookie-bearing). Logout is a POST, not a GET.

**Logout.** `POST /logout` sets `users.last_logged_out = now()` (reusing the
existing token-invalidation mechanism in `AuthUser`), clears the
`star_session` cookie, redirects to `/login`.

## 6. CI browser smoke-test plan

One new job (`browser-smoke`) — a new workflow
`.github/workflows/browser-smoke.yml` triggered on `migration/**` and
`specs/**` PRs, modeled on the existing `rust-migration.yml` test job:

- **Services:** `postgres:16` with the same env/options block as the
  existing test job (`POSTGRES_USER: star`, `POSTGRES_DB: star_lms_test`,
  `pg_isready` health check).
- **Env:** `DATABASE_URL` pointing at the service,
  `STAR_JWT_SECRET: github-actions-test-secret`, `STAR_PORT` on a free port.
- **Steps:** checkout → rust-toolchain + `Swatinem/rust-cache` →
  `cargo build --locked -p star-http` (debug build is enough; cache makes
  this ~3–4 min) → `npx playwright install --with-deps chromium`
  (~1.5 min, cache the browser via `actions/cache`) → start the binary in
  the background and wait on `/health` → run Playwright.
- **Three journeys** (one spec file each, plain Playwright test-runner — no
  codegen, no fixtures beyond a seeded SQL fixture that creates one user,
  one published course with `default_view = "wiki"`, a front page, and one
  enrollment):
  1. **Login → dashboard:** load `/login`, submit credentials, assert
     redirect to `/dashboard`, assert course card present, assert
     `window.ENV.current_user_id` set.
  2. **Dashboard → course home:** click the card, assert section tabs
     render (count/order vs. the tabs API response fetched in-test), assert
     front-page body text on the course home.
  3. **Access control + logout:** logged-out `GET /dashboard` redirects to
     `/login`; `POST /logout` then `GET /dashboard` redirects again; an
     unenrolled second user gets no course data on `GET /courses/:id`.
- **Budget: ~10 minutes** wall-clock for the whole job (build ~4 min cached,
  browser install ~1.5 min cached, tests <1 min, service boot ~30 s,
  slack for cold caches). `timeout-minutes: 15` hard stop. Failures must
  attach the Playwright trace and a screenshot per failed assertion as CI
  artifacts.

## 7. Dependencies, risks, open questions

1. **User preferences storage does not exist yet.** `PREFERENCES.*` and
   `SETTINGS.collapse_global_nav` / `use_high_contrast` have no backing
   column (no `users.preferences`). A follow-up issue must add one
   (next migration number is 0079+); until then the shell serves documented
   defaults (`dashboard_view: "cards"`, `custom_colors: {}`, both booleans
   false) and criterion 3 tests the defaults.
2. **Parity-tooling drift on journey endpoints.** `users/self/favorites/*`
   and `courses/:course_id/announcements` exist in code but appear in the
   scoreboard's unmatched/extras lists, and the issue's own research
   mis-listed `front_page` as missing. The parity manifest refresh
   (`tools/parity/parity.py manifest`) and a scoreboard re-run should
   accompany the shell work so CI gates reflect reality.
3. **askama compile-cycle friction.** Compile-time templates mean every
   layout edit recompiles. Acceptable for a shell of this size, but the dev
   story needs `cargo watch` documented; if iteration pain grows, minijinja
   with the same typed contexts is the fallback (contract in §3 is
   engine-agnostic).
4. **Session secret is shared with the API.** The cookie JWT uses the same
   `STAR_JWT_SECRET` as Bearer tokens, so a leaked cookie is as powerful as
   a leaked API token for its 7-day life. Mitigations to specify in
   implementation: `HttpOnly` + `SameSite=Lax` everywhere, and consider a
   shorter shell-token expiry minted at login. Open: do we eventually want
   distinct audiences (`aud` claim) for shell vs API tokens?
5. **ENV contract governance.** Canvas's js_env grew to 124 files / 490 call
   sites of uncoordinated additions. Open question: should
   `star-ui` enforce the §3 table with a unit test that diffs the rendered
   ENV keys against a checked-in list (recommended), so any new key is a
   deliberate spec change rather than an accident?

## Cross-references

- Baseline scoreboard: `migration/parity/report.md`, endpoint statuses:
  `migration/parity/coverage.json`.
- Router assembly: `migration/crates/star-http/src/main.rs`.
- Auth primitives to reuse: `migration/crates/star-auth/src/lib.rs`
  (`create_token`, `AuthUser`, `verify_password`, `last_logged_out`
  invalidation).
- Dashboard payload: `migration/crates/star-dashboard/src/lib.rs`
  (`DashboardCard`, favorites routes).
- Canvas reference: `/home/z/my-project/repos/canvas-lms` —
  `app/views/layouts/application.html.erb`, `_head.html.erb`,
  `app/controllers/application_controller.rb` (js_env, line 239),
  `users_controller.rb` (`user_dashboard`, `dashboard_cards`),
  `courses_controller.rb` (`show`), `config/routes.rb`.
