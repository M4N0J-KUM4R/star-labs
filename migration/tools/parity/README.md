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

## Publishing from CI

`report` is deterministic, so CI commits its own output (issue #74). The
`publish` job downloads the `parity-report` artifact from the `parity` job and
commits those exact bytes — nothing is recomputed, so the branch and the
artifact cannot disagree.

- Target: the `parity-data` branch, not `main`. The `Phase 4 main gate` ruleset
  puts `main` behind a `pull_request` rule with no bypass actors, so a workflow
  token cannot write there and every main write stays under the coordinator's
  exact-SHA merge gate (issue #20).
- Trigger: every `push` to `main` (i.e. after each merge). `workflow_dispatch`
  with `publish=yes` rehearses the same code path from a lane branch.
- Committed: `report.md`, `coverage.json`, `baseline.json`.
  `canvas_manifest.json` is an input and is never republished.
- Runs are no-ops when the numbers have not moved; a coverage regression fails
  the `parity` job (baseline guard above), which skips `publish` and leaves the
  last good numbers in place.
- `migration/parity/` on `main` stays as the ratchet input and as the seed for
  the first data-only commit; read `parity-data` for live numbers. As of
  2026-11-18 main's copy said 244/1403 while CI measured 481/1403.

Refresh the branch by hand (normally unnecessary):

```sh
gh workflow run "Parity scoreboard" --ref main -f publish=yes
```

Route totals only move when a human refreshes the manifest from a Canvas
checkout — CI has no Canvas source, so "live" means live against the committed
manifest.

## Baseline policy

`parity/baseline.json` stores the implemented REST route count. `report` exits
non-zero when the current count is below the baseline, so coverage cannot
regress. A higher count ratchets the baseline up. On regression the failing run
still writes `coverage.json` and `report.md` but leaves the baseline untouched.

## Output schema

- `canvas_manifest.json` — `routes[]` (`verb`, `path`, `normalized_path`,
  `controller`, `action`, `ui_referenced`) plus `counts` for API routes,
  controllers, GraphQL files/types/mutations, models, UI feature dirs, JS/TSX
  files, ERB views, js_env files/call sites, and UI `/api/v1` references;
  plus `graphql_type_names[]` and `graphql_mutation_names[]` (the Canvas
  GraphQL operation-name inventories, see below).
- `coverage.json` — `summary` (implemented/total and 0.1%-rounded percentages),
  `controllers` (only controllers with coverage), `implemented_routes`,
  `unmatched_star_routes` (extras), and `graphql` (per-name coverage:
  implemented/missing type and mutation names, the rename map, and star
  names with no Canvas counterpart).
- `report.md` — human-readable summary, per-controller table, GraphQL
  operation coverage (implemented + missing name lists), extras table,
  limitations.
- `baseline.json` — `implemented_rest_routes` and the ratchet policy.

## Measurement rules

- Canvas routes are verb declarations inside `ApiRouteSet.draw` blocks, plus
  Rails `resources` declarations and `%w[...].each` context loops. `resources`
  expands to the API defaults (GET/POST collection, GET member, PATCH/PUT and
  DELETE member), honoring `only:`/`except:`, `path:`, `controller:`, and
  nesting. Context loops expand every value with `#{context}`,
  `#{context.pluralize}`, `:#{context}_id`, and `#{path_prefix}`
  substitutions (including the tuple form).
- Conditional context prefixes of the form
  `prefix = ((context == "global") ? context : "#{context}s/:#{context}_id")`
  are evaluated per loop binding, and `if`/`unless context == "literal"` guards
  filter the bindings. This expands the `outcome_groups_api` routes to
  `courses/...`, `accounts/...`, and `global/...`. Other runtime interpolations
  are left as `{}` and cannot match concrete Rust paths.
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
- GraphQL parity is measured by operation name. Canvas names come from
  `app/graphql/types/*_type.rb` (the `graphql_name` when declared, else the
  class name minus a trailing `Type` — graphql-ruby's default) and from
  `app/graphql/mutations/**` (classes whose superclass chain reaches
  `Mutations::BaseMutation`, camelCased to their mutation field names). A
  Canvas type file can define several GraphQL types, so the inventories are
  per-name while the denominators stay the file counts (170 / 110). Rust names
  come from `crates/star-graphql/src/*.rs`: `#[Object]` impl targets and
  async-graphql derives (`SimpleObject`, `Interface`, `Union`, `InputObject`,
  `Enum`, `MergedObject`), and mutation fields from `#[Object]` impls on the
  `Mutation` root or `*Mutations` containers. Four deliberate star renames are
  mapped to their Canvas graphql_names before matching: `CourseSection` ->
  `Section`, `EnrollmentTerm` -> `Term`, `DiscussionTopic` -> `Discussion`,
  `QuizQuestion` -> `QuizItem` (each documented in the star-graphql module doc
  comments). Canvas's committed `schema.graphql` is NOT used as a source: it
  drifts from the source tree (it still lists the removed
  `*AccountDomainLookup` mutations).
- UI is 0 because the Rust workspace has no frontend; schema tables count
  `CREATE TABLE` in `star-db` migrations (sqlx bookkeeping excluded).
