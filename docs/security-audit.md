# Security audit — issue #106

Scope: CORS policy, CSRF protection on cookie-auth routes, session
fixation/expiry, dependency vulnerability scanning, secret scanning in CI.
Canvas source references are to `instructure/canvas-lms` `master`.

## Findings and resolutions

| # | Severity | Finding | Resolution in this PR | Canvas reference |
|---|---|---|---|---|
| 1 | HIGH | No secret scanning in CI — a leaked PAT or key would merge unnoticed | New `security-scan.yml` workflow runs gitleaks on every PR and push (full history on push), with a narrow allowlist for the CI test fixtures | — |
| 2 | HIGH | `/api/v1/files/:id/create_success` answered no `Access-Control-*` headers on GET/POST and a bare `Allow` on OPTIONS, so a browser completing an S3 upload could not read the finalisation reply cross-origin | `open_cors` helper mirrors Canvas: reflect request `Origin`, `Allow-Credentials: true`, the Canvas method/header lists; applied to GET/POST/OPTIONS | `app/controllers/files_controller.rb:178-181` (`before_action :open_cors`), `:1812-1818` (`open_cors`), `config/routes.rb:2238` (OPTIONS → `api_create_success_cors`) |
| 3 | MED | No limit on failed logins — the API `/api/v1/auth/login` and the UI `POST /login` allowed unlimited password guessing | `LoginRateLimiter` (in-process, shared by both surfaces): after **10** consecutive failures the account is locked for **30 minutes**; a success clears the counter; unknown emails count too (no account enumeration via lockout timing); locked requests are refused *before* the password check | `lib/canvas/security/password_policy.rb:25,28` (`MAX_LOGIN_ATTEMPTS=20`, `DEFAULT_LOGIN_ATTEMPTS=10`), `lib/canvas/security/login_registry.rb:60-110` (`audit_login`, `too_many_attempts`, Redis TTL windows) |
| 4 | LOW | Session lifetime is 7 days; Canvas expires sessions after 1 day | Documented deviation, unchanged: the 7-day JWT lifetime is the workspace-wide spec §5 contract shared by the API and the shell; shortening only the cookie would split the two surfaces for no demonstrated threat | `config/initializers/session_store.rb:49` (`expire_after` 1 day) |

## Verified as already sound (no change needed)

- **CSRF on cookie-auth routes.** Every UI write route (17 of 17:
  login, logout, register, forgot/reset password, discussion create/reply,
  conversation create/reply/mark-all-read, quiz new/take-start/submit,
  assignment create/update/submit, speedgrader grade) checks the
  double-submit pair (`star_csrf` cookie + `_csrf` form field, constant-time
  compare) and answers 403 on mismatch. `/api/v1` is Bearer-only and is
  exempt by construction — same shape as Canvas, where
  `protect_from_forgery with: :exception` (`application_controller.rb:43`)
  guards the HTML routes and the API authenticates by token.
- **Session fixation.** Each login mints a fresh JWT; logout stamps
  `users.last_logged_out`, which both session paths (`SessionUser`,
  `AuthUser`) check, invalidating every pre-logout token. This is the
  JWT equivalent of Canvas' `reset_session` on login
  (`application_controller.rb:1900,2252,2956-2958`).
- **Cookie attributes.** `star_session`: `HttpOnly`, `SameSite=Lax`,
  `Path=/` (no `Secure` while CI serves plain HTTP — deployment concern,
  noted in code). CSRF cookie deliberately non-HttpOnly for the
  double-submit pattern.
- **Password hashing.** Argon2id via the `argon2` crate on register and
  both login surfaces.
- **Login error shape.** Unknown email and wrong password render the same
  generic failure (no user enumeration); the lockout adds the same
  non-enumeration property to failure *counting*.
- **Dependency audit.** `production-build.yml` already runs
  `cargo audit --file Cargo.lock` (RustSec) as the `Dependency audit`
  check on PRs touching `migration/**` and on main pushes.

## Known limitations (documented, accepted)

- The login limiter is **process-local**. star-lms deploys a single
  `star-http` binary, so the budget is shared across all requests to that
  process; a restart clears counters (fail-open, never fail-closed). A
  Redis- or DB-backed counter (`users.failed_login_count`,
  `users.locked_until`) is the durable upgrade — migration number
  requested from the coordinator on the issue (0229 proposed) and NOT
  self-assigned.
- Canvas' `remaining_attempts_2/1` / `final_attempt` warning ladder and
  account suspension (`suspend_with_notification!`) are not replicated;
  the workspace answers the same generic 401 during counting and a 403
  while locked.
- gitleaks allowlists the CI fixture files; real secrets in any other
  path fail the check.
