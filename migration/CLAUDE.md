# star-lms — Rust rewrite of Canvas LMS core

Karpathy principles applied: simplicity first (no speculative abstractions),
surgical scope (core LMS loop only), goal-driven (every crate compiles, tests
pass, live HTTP verified on EC2).

## Stack (fixed — do not deviate)

- axum 0.8 + tokio
- sqlx (postgres, runtime-tokio, rustls) — compile-time checked queries
- serde / serde_json
- argon2 for password hashing
- jsonwebtoken for sessions
- chrono (serde feature)
- No other frameworks. No GraphQL. No gRPC. No speculative abstraction layers.

## Architecture

- `crates/star-http` — the only binary. Owns main(), router, auth middleware.
- `crates/star-common` — AppState (sqlx PgPool), shared error type, JWT claims.
- `crates/star-db` — migrations (plain SQL, sqlx::migrate!).
- `crates/star-auth` — login/logout/me handlers + password/JWT helpers.
- `crates/star-users` — user CRUD + profile handlers.
- `crates/star-courses` — courses + enrollments.
- `crates/star-assignments` — assignments + submissions + grades.
- `crates/star-content` — modules + pages + announcements.
- Domain crates expose `pub fn router() -> axum::Router<AppState>`; no state
  structs in domain crates. star-http merges them and supplies state.

## Conventions

- Table names snake_case (`users`, `courses`, `enrollments`, `assignments`,
  `submissions`, `modules`, `pages`, `announcements`).
- Every table gets `id BIGSERIAL PRIMARY KEY`, `created_at`, `updated_at`
  (timestamptz default now()).
- Errors: one enum in star-common -> IntoResponse with proper status codes.
- Handlers return `Result<Json<T>, ApiError>`.
- Tests: integration tests live next to handlers; DB tests run against
  DATABASE_URL if set, otherwise skipped.
- JSON responses mirror Canvas API v1 shapes (id, name, created_at ...).

## Non-goals (explicitly)

- File uploads/S3, live events, LTI, canvas RCE, quizzes, analytics,
  notifications engine, i18n, webpack frontend — NOT in this scope.
- Canvas API parity beyond the endpoints listed above.

## Verification (definition of done)

1. `cargo build --workspace` green.
2. `cargo test --workspace` green (DB tests skipped cleanly without DATABASE_URL).
3. On EC2: `cargo run` serves :8080; register/login/me/list-courses verified
   with curl against the running Postgres container.
