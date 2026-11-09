# Parity scoreboard

Deterministic measurement of Canvas LMS -> Rust migration coverage. Stdlib-only
Python 3; no network, timestamps, or Canvas source in this repository.

## Commands

Refresh the committed manifest from a local Canvas checkout (not committed, keep
the path outside this repo):

```sh
cd migration
python3 tools/parity/parity.py manifest \
  --canvas-src /path/to/canvas-lms \
  --out parity/canvas_manifest.json
```

Regenerate the scoreboard from the current Rust tree:

```sh
cd migration
python3 tools/parity/parity.py report \
  --manifest parity/canvas_manifest.json \
  --star-src . \
  --out parity
```

CI (`.github/workflows/parity.yml`) runs only `report`; it never checks out
Canvas.

## Baseline policy

`parity/baseline.json` stores the implemented REST route count. `report` exits
non-zero when the current count is below the baseline, so coverage cannot
regress. A higher count ratchets the baseline up. On regression the failing run
still writes `coverage.json` and `report.md` but leaves the baseline untouched.

## Output schema

- `canvas_manifest.json` — `routes[]` (`verb`, `path`, `normalized_path`,
  `controller`, `action`, `ui_referenced`) plus `counts` for API routes,
  controllers, GraphQL files/types/mutations, models, UI feature dirs, JS/TSX
  files, ERB views, js_env files/call sites, and UI `/api/v1` references.
- `coverage.json` — `summary` (implemented/total and 0.1%-rounded percentages),
  `controllers` (only controllers with coverage), `implemented_routes`, and
  `unmatched_star_routes` (extras).
- `report.md` — human-readable summary, per-controller table, extras table,
  limitations.
- `baseline.json` — `implemented_rest_routes` and the ratchet policy.

## Measurement rules

- Canvas routes are explicit verb declarations inside `ApiRouteSet.draw`
  blocks. `resources` expansions are not enumerated; dynamic `#{...}` paths are
  kept as `{}` placeholders and cannot match concrete Rust paths.
- Normalization strips `/api/v1` and collapses `:param`, `{param}`, and
  interpolations to `{}`; UI literals additionally collapse numeric segments.
- Rust routes are `.route("path", get|post|put|delete|patch(...))` calls in
  `crates/*/src/*.rs`, including multi-line calls and chained methods (one call
  can register several verb/method pairs). Axum's `nest("/api/v1", ...)` mount
  in `star-http` is assumed.
- Matching is by `(verb, normalized path)`. Star paths with no Canvas match are
  reported as extras; many are deliberate flat-path deviations (for example
  `/assignments/{id}` where Canvas uses `/courses/{course_id}/assignments/{id}`)
  or auth/dashboard/health routes Canvas does not expose.
- UI is 0 because the Rust workspace has no frontend; schema tables count
  `CREATE TABLE` in `star-db` migrations (sqlx bookkeeping excluded).
