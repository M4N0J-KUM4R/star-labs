# Behavioral parity scoreboard (issue #231)

Route registration (the old "100%") says nothing about behavior. This is the
measurement: the full production router (`star_http::build_app`, extracted
from the binary into `crates/star-http/src/lib.rs`) is driven in-process with
axum-test against a real, migrated PostgreSQL, and every route in
`parity/canvas_manifest.json` (1403 routes) is probed.

## Run it

```sh
mlai-service start postgres   # or any Postgres
cd migration
DATABASE_URL=postgres://ubuntu:ubuntu@127.0.0.1:5432/app \
  cargo run -p star-http --example behavior_parity
```

Writes `report.json` next to this file and prints the scorecard.

## What is probed, per route

| Probe | Question it answers |
|---|---|
| anon | Does a handler answer at all (not axum's empty fall-through 404)? Does it reject anonymous access (401/403/302) unless on `public_routes.json`? |
| malformed-id-0 / -overflow | Do `0` and `> i64::MAX` path params avoid 5xx? |
| pagination-page / -zero | Do `index` actions accept `?per_page=1&page=1` and `?per_page=0` without 5xx? |
| expectations.json | Per-route contracts (authz status today; fields, nullability, soft-delete as entries are added) |

## Scorecard (2026-11-23 run, after leak/missing-route phase)

| Dimension | Score | Detail |
|---|---|---|
| registration | 100% | 4217 pass / 0 fail; the ~100 previously missing routes are registered (custom_data scopes, favorites, submissions comments/files, annotation_notification, learn_platform, ...) |
| authz (anonymous) | 100% | 1403 pass / 0 fail; the 5 leaky handler families now gate on session auth (`app_center/apps`, `learn_platform/*`, `sessionless_launch` x2, ...) |
| robustness (bad ids) | 100% | 2514 pass / 0 fail; `outcome_groups/:id/outcomes` Path-arity 500s fixed via shared cores with per-context wrappers; `sessionless_launch` now 401 anon instead of 503 |
| pagination (index) | 100% | 300 pass / 0 fail |
| behavioral contract | 5 / 1403 specified | only routes with an `expectations.json` entry count as measured; **unspecified ≠ passing** |

### Phase record — "leaky handlers + missing routes" (this phase)

- Changed: `star-outcomes/src/lib.rs` (link/list cores + wrappers), `star-courses/src/misc_gaps_v2.rs` (`app_center_apps` takes `AuthUser`), `star-accounts/src/learn_platform.rs`, `star-users/src/custom_data.rs` (scope routes), `star-lti/src/external_tools.rs` (`sessionless_launch` takes `AuthUser`), `star-http/src/lib.rs` + harness (route_not_found body, optional-segment stripping, allowlist).
- Behaviour preserved exactly: response shapes of all previously-working routes; only auth gating (anonymous → 401) and previously-500/404 routes changed.
- Tests after the phase: `port_c_integration` 4/4 (its `app_center/apps` test updated to authenticate — the anonymous 200 assertion predates the authz fix), `star-lti` 116 pass / 0 fail, harness 0 failures.
- Rollback: revert the working-tree diff on the six files above (`git checkout -- <paths>`); the harness (`cargo run -p star-http --example behavior_parity`) then re-prints the previous scorecard, so a bad revert is visible in one command.

## Still unmeasured (needs seeded fixtures per resource)

Field names, nullability, soft-delete visibility, permission order
(student/teacher/admin matrices). The harness reports these as
`unmeasured_dims`; the honest status of the API is
**"registration 97%, observable behavior 98–100% on mechanical dimensions,
deep Canvas-diff behavior 5/1403 routes specified"** — not 100%.

## Files

- `crates/star-http/examples/behavior_parity.rs` — the harness
- `crates/star-http/src/lib.rs` — router assembly extracted from `main.rs` (binary behavior unchanged)
- `public_routes.json` — reviewed Canvas-public allowlist (extend only with a routes.rb citation)
- `expectations.json` — per-route contracts; grows toward covering all 1403
- `report.json` — machine-readable latest run (failures, unregistered, allowlist)
