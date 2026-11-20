# star-ui Integration Guide (from research agent A, 2026-11-20, main @ a403ee2)

Authoritative spec: `specs/p5-ui-shell.md` (merged; issue #22 CLOSED). Companion: `canvas-reference.md` in this dir. **No local cargo build/test/clippy/run — CI is the only gate. `cargo fmt` allowed. Lockfile refresh must go through the "CI assist" workflow (`gh workflow run "CI assist" --ref <branch> -f branch=<branch> -f task=fmt+lock`).**

## 1. star-auth (migration/crates/star-auth/src/lib.rs)
- `AuthUser { id: i64, role: String }` extractor (lib.rs:38-100): reads `Authorization: Bearer`, `decode::<Claims>` vs `app.jwt_secret`, then **reloads role from DB** (`SELECT role, last_logged_out FROM users WHERE id = $1`), rejects if `issued_at = claims.exp - TOKEN_EXPIRY_SECS < last_logged_out.timestamp()`. Imports: `axum_core::extract::FromRequestParts`, `http::request::Parts`, `axum::extract::FromRef`, `jsonwebtoken::{decode, DecodingKey, Validation}`, `star_common::{ApiError, AppState, Claims}`.
- `pub fn hash_password(&str) -> Result<String,String>` (:107), `pub fn verify_password(plain,&hash) -> bool` (:116), `const TOKEN_EXPIRY_SECS: usize = 7*24*60*60` (:130), `pub fn create_token(user_id: i64, role: &str, secret: &str) -> Result<String,String>` (:133, HS256, exp=now+7d), `pub fn verify_token(token,&secret) -> Option<Claims>` (:154).
- Router (lib.rs:169+): `/auth/register` POST, `/auth/login` POST, `/auth/me` GET, `/auth/logout` POST (nested under /api/v1 by star-http).
- `LoginRequest { email, password }`; `UserResponse { id, name, sortable_name?, email, role, created_at }` (pub, FromRow).
- login handler (lib.rs:338-370): `SELECT id, password_hash, role FROM users WHERE email = $1` → 401 if none → `verify_password` else 401 (no user enumeration) → `create_token` → `Json(LoginResponse { token, user })`.
- Logout precedent (star-users terminate_sessions, lib.rs:1650): `UPDATE users SET last_logged_out = now(), updated_at = now() WHERE id = $1`.

## 2. star-common (73 lines) + star-api helpers
- `AppState { db: PgPool, jwt_secret: String }` (Clone) — exactly 2 fields.
- `ApiError` enum: NotFound("not found")/Unauthorized("unauthorized")/Forbidden("forbidden")/Conflict(String)/Validation(String)/BadRequest(String)/Db(String); `IntoResponse` = plain text; `From<sqlx::Error>` → `ApiError::Db`.
- `Claims { sub: i64, role: String, exp: usize }`.
- star-api: `PaginationParams` (?page/&per_page clamp [1,100]), `Paginated<T>::new(data,page,per_page,total)`, `check_enrollment(user_id, course_id, role, pool)` (admin bypass), `require_course_teacher(...)`.

## 3. star-http (src/main.rs, 169 lines)
- Env: `DATABASE_URL`, `STAR_JWT_SECRET` (both fail-fast), `STAR_PORT` (default 8080). `PgPoolOptions::new().max_connections(5).acquire_timeout(10s)`; `star_db::MIGRATOR.run(&pool)`.
- App router (main.rs:101-107): `Router::new().route("/health", get(health)).nest("/api/v1", api_router).nest("/api/graphql", star_graphql::router()).with_state(state).layer(TraceLayer::new_for_http())`. **star-ui merges at TOP LEVEL (sibling of /health), not under /api/v1.**
- star-http/Cargo.toml lists every crate as path dep — add `star-ui = { path = "../star-ui" }`.

## 4. star-dashboard
- Routes: `/dashboard` GET, `/dashboard/dashboard_cards` GET, `/users/self/todo`, `/users/self/upcoming_events`, `/users/self/favorites/courses` GET/DELETE, `/users/self/favorites/courses/{id}` POST/DELETE.
- `DashboardCard` (lib.rs:617-657, camelCase): long_name, short_name, original_name, course_code, asset_string ("course_{id}"), href ("/courses/{id}"), term?, subtitle ("enrolled as Student"), enrollment_state, enrollment_type ("StudentEnrollment"|"TeacherEnrollment"|...), observee?, id, is_favorited, is_k5_subject, is_homeroom, use_classic_font, can_manage, can_read_announcements, image?, color?, position?, published (workflow_state ∈ {available,completed}), links (Vec: discussions/assignments/announcements/files tabs), can_change_course_publish_state, default_view (**HARDCODED "modules"** — no column exists), pages_url, front_page_title?.
- cards SQL: DISTINCT ON (c.id) courses JOIN enrollments (non-deleted, this user), role-rank teacher<ta<designer<student, ORDER BY position NULLS LAST, name ASC LIMIT 50. `can_manage = role in (teacher,ta,designer) || auth.role == admin`.

## 5. star-courses
- `Course` (lib.rs:24-40): id, name, course_code, description?, workflow_state ("available"|"unpublished"|"completed"|"deleted"), start_at?, end_at?, syllabus_body?, created_at, updated_at. **No default_view, no published field.**
- GET /courses/{id} (lib.rs:646): 404 before 403; `check_enrolled_or_admin` gate → shell must reuse same gate (spec criterion 7).
- Tabs GET /courses/{courseId}/tabs (lib.rs:3046): `CourseTab { id, html_url, full_url, position (renumbered 1..n), hidden?, visibility ("public"|"admins"), label, tab_type: "internal" }`. DEFAULT_COURSE_TABS order: home, announcements, assignments, discussions, grades, people, pages, files, syllabus, modules, outcomes, quizzes; Settings last, teacher/admin only. Handler: 404 if missing → 403 if not enrolled/admin → include_settings for teacher/admin → reads `course_tabs` table → renumber. Bare JSON array.

## 6. Other crates
- star-pages: GET/PUT `/courses/{courseId}/front_page`; `FrontPage { id, page_id, course_id, title, body?, url?, workflow_state, editing_roles, published, hide_from_students, front_page, html_url, created_at, updated_at }`; 404 `{"message": "No front page has been set"}` BEFORE enrolled check. pages.front_page bool column (partial unique index).
- star-modules: GET `/courses/{courseId}/modules?limit&offset` (limit default 100 max 100); `Module { id, course_id, name, position, workflow_state, created_at, updated_at }`; `WHERE course_id AND workflow_state <> 'deleted' ORDER BY position, id`.
- star-enrollments: `EnrollmentWithUser { id, user_id, course_id, course_section_id?, role (String: student/teacher/ta/observer/designer), workflow_state, created_at, name, email }`. Note: GET/POST /courses/{id}/enrollments is served by star-courses.
- star-users: GET /users/self → `{ id, name, sortable_name, email, role, created_at }`; profile → `{ id, name, short_name, sortable_name, title, bio, primary_email, avatar_url, time_zone, locale }`; `UserSettings` booleans; **user_preferences JSONB table exists (migration 0123)** with helpers `set_user_preference(pool, user_id, key, Option<&Value>)` / `get_user_preference(pool, user_id, key) -> Value` (star-users lib.rs:3038-3078) — use for PREFERENCES.* (no new migration needed). custom_colors via GET/PUT /users/{userId}/colors.
- star-announcements: GET `/courses/{courseId}/announcements` (enrolled-or-admin gate, `workflow_state <> 'deleted' ORDER BY created_at DESC`, local Pagination clamp [1,50]); `Announcement { id, course_id, title, message, workflow_state, posted_at?, created_at, updated_at }`.

## 7. Test template (copy from star-pages/tests/pages_integration.rs:19-111)
```rust
const JWT_SECRET: &str = "test-jwt-secret";
fn run_prefix() -> String { /* "ui-tests-{pid}-{millis+atomic}" */ }
async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = std::env::var("DATABASE_URL").ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10)).connect(&db_url).await.ok()?;
    star_db::MIGRATOR.run(&pool).await.expect("run migrations");
    let state = AppState { db: pool.clone(), jwt_secret: JWT_SECRET.to_string() };
    let server = TestServer::new(star_ui::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}
// seed_user: INSERT INTO users (name, email, password_hash, role) VALUES ($1,$2,$3,$4) RETURNING id  (email {prefix}-{name}@test.local, hash "not-a-real-hash")
// seed_course: INSERT INTO courses (name, course_code) VALUES ($1,$2) RETURNING id  (code {prefix}-{name})
// seed_enrollment: INSERT INTO enrollments (user_id, course_id, role) VALUES ($1,$2,$3)
// auth(user_id, role) -> "Bearer {star_auth::create_token(user_id, role, JWT_SECRET)}"
// cleanup: DELETE FROM courses WHERE course_code LIKE '{prefix}%'; DELETE FROM users WHERE email LIKE '{prefix}%@test.local';
```
Every test: `let Some((server, pool, prefix)) = setup().await else { println!("SKIP: DATABASE_URL not set"); return; };`. For login tests seed a REAL argon2 hash via `star_auth::hash_password("pw")` (dev-dep on star-auth). Assertions: `assert_eq!(resp.status_code(), 200, "body: {}", resp.text());`. star-http tests spawn the real binary via `env!("CARGO_BIN_EXE_star-http")` + free port + /health poll — model for full-stack smoke if needed.

## 8. CI wiring (must-do for a new crate)
1. `migration/Cargo.toml` members += `"crates/star-ui"`.
2. star-http Cargo.toml path dep; main.rs top-level `.merge(star_ui::router())`.
3. `.github/workflows/rust-migration.yml` test-shard matrix — add `-p star-ui` to EXACTLY ONE shard (shard-coverage job fails otherwise).
4. `.github/workflows/nightly-compat.yml` shard map too (preflight guard).
5. Cargo.lock refresh via CI assist (`task=fmt+lock`) — askama/axum-extra NOT in lock yet.
- Required PR checks: `test` (fan-in), `Format and lint` (fmt --check + clippy --workspace --all-targets --locked -D warnings + migration filename hygiene), `shell-and-lab-constraints` (every PR; don't touch infra/**, scripts/**).
- parity.yml runs on migration/** PRs: counts `.route()` calls in `crates/*/src/*.rs`; new HTML routes = harmless "extras". **Do NOT create `migration/ui/` dir** (parity counts it as UI denominators). Keep assets under `migration/crates/star-ui/static/`.
- change class: migration/** (except *.md) → rust=true → full matrix.
- New workflow files (browser-smoke) trigger only per their own `on:`; keep `migration/**` + `specs/**` paths, model services block on rust-migration.yml test job (postgres:16, POSTGRES_USER star / PASSWORD star-test-only / DB star_lms_test, DATABASE_URL postgres://star:star-test-only@localhost:5432/star_lms_test, STAR_JWT_SECRET github-actions-test-secret).

## 9. Dependency pins
- tower-http workspace line currently `features = ["trace"]` — ServeDir needs `fs`: make it `["trace", "fs"]`.
- askama latest stable 0.16.1 (templates in `templates/` next to src/; optional serde_json feature). axum-extra 0.12.6 (`features = ["form", "cookie"]`; cookie 0.18.2 already locked). sha2 0.10.9 already in lock. rand workspace 0.8 (CSRF 128-bit token). axum::Form needs axum `form` feature — prefer axum-extra `Form` + `CookieJar`.
- Crate Cargo.toml template: copy star-pages (workspace = true for externals, path deps for star-*; dev-deps star-db, axum-test, tokio, serde_json, + star-auth).

## 10. Protocol (issue #20) + PR conventions
- Branch `p5/ui-shell-<issue#>`; assign issue + In Progress BEFORE coding; PR body `Closes #<issue>`; title `<area>: <summary>`; body: Summary → routes/pages table → Tests → Verification ("Lint passes. GitHub Actions CI validates the build server-side.").
- Only the merge coordinator merges to main. Never force-push. No local cargo/db/services; `cargo fmt` OK.
- Migrations: coordinator-assigned numbers only — star-ui phase 1 needs NONE (user_preferences table exists).

## 11. Known deviations to document in PR
- `course.default_view` column does not exist (API hardcodes "modules"). Phase-1 course home: render front page if one exists, else modules list (spec §4 wiki/modules branch approximated; note deviation).
- published = workflow_state ∈ {available, completed} (same derivation as DashboardCard).
- `Secure` cookie flag omitted on plain-HTTP CI/dev (spec allows).

## 12. Recommended skeleton
```
migration/crates/star-ui/
├── Cargo.toml
├── templates/
│   ├── layouts/{base.html, authenticated.html, course.html}
│   ├── login.html, dashboard.html, course_home.html, error.html, (journeys: assignments.html, gradebook.html, discussion.html, inbox.html, settings.html, people.html, speedgrader.html, quizzes.html, quiz_take.html, files.html, announcements.html, syllabus.html, calendar.html)
├── static/            # app.css, course.css (NOT migration/ui/)
└── src/
    ├── lib.rs         # pub fn router() -> Router<AppState>
    ├── session.rs     # SessionUser extractor (star_session cookie → same verify path as AuthUser) + CSRF issue/compare (star_csrf cookie + hidden field, constant-time)
    ├── env.rs         # typed js_env: 18 phase-1 keys + phase-2 constants; unit test diffs rendered keys vs checked-in list
    ├── assets.rs      # /static/{sha256-prefix}/{file}, Cache-Control immutable
    ├── queries.rs     # thin SQL loaders (cards, course+tabs, front page, modules, enrollments...)
    └── handlers/{login.rs, dashboard.rs, course.rs, ...}
└── tests/ui_integration.rs  # 9 acceptance criteria from spec §4
```
Routes: `/` redirect→/dashboard; `/login` GET/POST; `/logout` POST; `/dashboard` GET; `/courses/{id}` GET; `/courses/{id}/{tab}` GET; static service. ENV emitted once in <head> as JSON before any script. Set-Cookie: `star_session=<jwt>; HttpOnly; SameSite=Lax; Path=/`.
