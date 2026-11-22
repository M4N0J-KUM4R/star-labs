# Verification receipt — issue #41 "REST: master courses/templates parity"

- **Receipt id:** `p5-41-verify-001`
- **Date:** 2026-11-21
- **Base:** `main` @ `0350dc3` (all CI checks green on this SHA)
- **Verdict:** no implementation work remains — scope already landed via PR #18
  (merge `c16b2e7`, extended by PR #165). This issue can close.

## Why this issue was still open

#41 was written on 2026-11-13 against the stale 2026-11-13 CI parity artifact
(main `e558f6b`), which pre-dated PR #18. Its own body flags this: "Note PR #18
covers blueprint courses — check overlap before starting." The check now has a
definitive answer: **full overlap, 13/13.**

## Acceptance criteria — evidence

### 1. Route parity — 13/13 (`migration/parity/report.md`, `coverage.json`)

Every `master_courses/master_templates` route in `canvas_manifest.json` is
registered in `crates/star-courses/src/lib.rs` (router block "Blueprint courses
/ master courses") and served under `/api/v1` via `crates/star-http`:

| Canvas route | Star handler |
|---|---|
| GET `/api/v1/courses/:course_id/blueprint_templates/:template_id` | `show_blueprint_template` |
| GET `.../blueprint_templates/:template_id/associated_courses` | `blueprint_associated_courses` |
| PUT `.../blueprint_templates/:template_id/update_associations` | `blueprint_update_associations` |
| GET `.../blueprint_templates/:template_id/unsynced_changes` | `blueprint_unsynced_changes` |
| POST `.../blueprint_templates/:template_id/migrations` | `queue_blueprint_migration` |
| GET `.../blueprint_templates/:template_id/migrations` | `list_blueprint_migrations` |
| GET `.../blueprint_templates/:template_id/migrations/:id` | `show_blueprint_migration` |
| GET `.../blueprint_templates/:template_id/migrations/:id/details` | `blueprint_migration_details` |
| PUT `.../blueprint_templates/:template_id/restrict_item` | `blueprint_restrict_item` |
| GET `/api/v1/courses/:course_id/blueprint_subscriptions` | `list_blueprint_subscriptions` |
| GET `.../blueprint_subscriptions/:subscription_id/migrations` | `list_blueprint_imports` |
| GET `.../blueprint_subscriptions/:subscription_id/migrations/:id` | `show_blueprint_import` |
| GET `.../blueprint_subscriptions/:subscription_id/migrations/:id/details` | `blueprint_import_details` |

Scoreboard: `master_courses/master_templates` — **13/13 (100%)**, one of only
13 controllers at full coverage.

### 2. Blueprint/template permission tests

`crates/star-courses/tests/blueprint_integration.rs` (5 `#[tokio::test]` fns,
DB-backed, CI-gated; run under the PostgreSQL service workflow):

- `blueprint_template_show_happy_path_and_non_admin_forbidden` — admin happy
  path + 403 for non-admins + 404 semantics (unknown template id `default`).
- `blueprint_associated_courses_and_update_associations` — association list +
  update round-trip.
- `blueprint_migration_lifecycle_and_subscriptions` — queue → poll → details,
  child-side subscription import views, `restrict_item` behavior, 404 on unknown
  migration.
- `blueprint_unsynced_changes_initial_sync_before_any_migration` — Canvas
  initial-sync edge case.
- `blueprint_routes_unknown_course_or_template_are_404` — negative paths for
  templates, courses, and subscriptions.

### 3. Migration safety

- `crates/star-db/migrations/0107_master_courses.sql`: purely additive —
  `CREATE TABLE IF NOT EXISTS` for `master_courses`, `master_templates`,
  `master_migrations`, `master_course_subscriptions` + indexes; no `DROP`,
  no renumbering.
- Ledger: `orchestration/migration-reservations.json` records `0107` →
  `note: "PR #18", status: "merged"`. Compliant with the additive-only rule.

## CI gate status on base SHA `0350dc3`

`Rust migration validation` ✅ · `Format and lint` (test workflow) ✅ ·
`Production build` ✅ · `Parity scoreboard` ✅ · `Bootstrap validation` ✅ ·
`Browser smoke` ✅ — GitHub Actions runs API, 2026-11-21.

## Conclusion

All three acceptance criteria hold on `main`. #41 closes with this receipt as
the audit record; no code changes are required or made.
