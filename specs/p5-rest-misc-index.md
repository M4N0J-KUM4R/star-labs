# P5 REST Spec — Misc Canvas APIs: index and shared conventions

- **Issue**: #113 (epic `specs`) — "Spec: misc Canvas APIs (analytics, search, polling, SIS, reports, collaborations, eportfolios, conferences)"
- **Baseline**: worktree `wt-i113`, branch `p5/specs-113`, based on the wave-1 batch head `526de19` (six not-yet-merged spec branches treated as landed references). Canvas source read from the local `canvas-lms` checkout; route inventory extracted from `migration/parity/canvas_manifest.json`; implemented-route status from a fresh `python3 tools/parity/parity.py report --manifest parity/canvas_manifest.json --star-src . --out /tmp/parity-i113` run.
- **Deliverable type**: documentation only. No Rust code, no migrations, no route registrations.

## 1. Layout decision

The issue allows one file or several. This spec uses **one file per family** because the eight families have almost nothing in common beyond the shared job-model convention (§3): polling is a synchronous CRUD slice, SIS imports and account reports are batch/attachment-driven, conferences wrap an external provider, search is read-only computation, and analytics turns out to have **zero REST routes** (see `p5-rest-search-analytics.md` §1). A single file would exceed 900 lines and mix five unrelated state machines. Files:

| File | Family | Routes (todo/total) | Unblocks |
|---|---|---:|---|
| `p5-rest-polling.md` | `polling/*` (polls, choices, sessions, submissions) | 21/21 | wave-3 polling issue |
| `p5-rest-sis.md` | SIS imports, import errors, SIS assignments, disable_post_to_sis | 12/12 | wave-3 SIS issue |
| `p5-rest-account-reports.md` | account reports + course reports | 9/9 | wave-3 account-reports issue |
| `p5-rest-collaborations.md` | collaborations (+ members, potential collaborators) | 7/7 | wave-3 search/collaborations issue |
| `p5-rest-eportfolios.md` | eportfolios (+ pages, moderation) | 7/7 | later issue |
| `p5-rest-conferences.md` | web conferences (+ recordings) | 7/7 | later issue |
| `p5-rest-search-analytics.md` | search, smart_search, **analytics (zero routes)** | 7/7 + 0 | wave-3 search issue |

Related-but-out-of-scope controllers found next to these families in the manifest (documented in the open questions of the relevant files, not specced here): `study_assist` (POST `/courses/:course_id/study_assist`), `disable_post_to_sis_api` is included in the SIS file, and `analytics_hub` (non-API HTML page) is covered by the analytics finding in `p5-rest-search-analytics.md`.

## 2. Fresh coverage (workspace-wide)

Measured at `526de19` with the parity tool:

| Metric | Implemented | Total | Coverage |
|---|---:|---:|---:|
| REST API routes | 471 | 1403 | 33.6% |
| UI-referenced REST routes | 258 | 503 | 51.3% |
| Star registrations | 624 route/method pairs | — | 157 unmatched extras |
| Schema tables | 119 | 688 Canvas models | n/a |

Every controller in this spec is at **0%** coverage: `polling/polls` 0/5, `polling/poll_choices` 0/5, `polling/poll_sessions` 0/9, `polling/poll_submissions` 0/2, `sis_api` 0/2, `sis_imports_api` 0/7, `sis_import_errors_api` 0/2, `disable_post_to_sis_api` 0/1, `account_reports` 0/6, `course_reports` 0/3, `collaborations` 0/7, `eportfolios_api` 0/7, `conferences` 0/7, `search` 0/4, `smart_search` 0/3, `analytics_api` 0/0 (controller absent from this Canvas checkout — see the analytics finding). Total: **72 todo routes** across the seven REST families.

## 3. Shared job-model convention (applies to SIS, reports, smart search; not polling/collaborations/eportfolios/conferences)

Canvas runs the async parts of these families on **Delayed::Job**:

- SIS imports: `SisBatch` enqueues `Delayed::LOW_PRIORITY` work with `strand: strand_for_account(account)` (a **singleton strand per account**, so at most one import processes per account) and an optional `run_at` delay from the `sis_batch_process_start_delay` setting; the callback is `batch.process_instructure_csv_zip`. Restore-states runs as `restore_states_later` returning a `Progress`.
- Account reports: `AccountReport` walks `created → running → compiling → complete/error/aborted/deleted`, spawning parallel `account_report_runners` for `parallel_*` report types, writing CSV to a polymorphic `Attachment`, and exposing `progress` + `current_line_count` (updated every 1000 records).
- Smart search: course indexing is tracked by `SmartSearch.check_course(course)` returning `(ready, progress)`; the API reports `indexed`/`indexing`.

star-lms has **no worker process**. The two in-repo precedents are:

1. **`star-jobs`** (migrations ~0057): a durable `jobs` table with Delayed::Job-compatible columns (`tag, strand, singleton, priority, attempts, max_attempts, run_at, handler, workflow_state, progress, context_type, context_id, ...`) plus `enqueue_job`, and Jobs V1/V2 read/cancel APIs. The crate doc states the intended pattern explicitly: "No worker runs in this workspace: `enqueue_job` records the durable row and an external worker (or a later phase) is expected to advance `workflow_state` / `progress`."
2. **`content_migrations`** (migration `0062_content_migrations.sql`): domain rows carry a `workflow_state TEXT` column with a `CHECK` constraint enumerating the Canvas states, and "the API layer maps them to the Canvas JSON states".

**Recommendation (uniform across all misc families): the "domain row + jobs row" pattern.**

- Every long-running operation gets its own domain table with a CHECK-constrained `workflow_state` enum copied verbatim from the Canvas model (`sis_batches`, `account_reports`, `course_reports`, …), mirroring the `content_migrations` precedent.
- Creating the operation (SIS import POST, report POST) validates input, persists the domain row in `created`/`initializing`, then calls `star-jobs::enqueue_job` with a descriptive `tag` (e.g. `SisBatch#process_instructure_csv`, `AccountReport#generate`) and, for SIS, `strand = sis_batch:{account_id}` + `singleton = true` to reproduce Canvas's one-import-per-account semantics.
- Progress polling endpoints read the domain row; where Canvas embeds a `Progress` JSON object (SIS import show, restore_states response), the handler serializes the linked jobs row into Canvas's progress shape (`{id, tag, workflow_state, completion, ...}`) rather than adding a new `progresses` table.
- Integration tests that need terminal states flip `workflow_state`/`progress` directly in SQL (the same way content-migration tests do today); no fake worker is required.
- Storage: Canvas attaches the SIS upload CSV and the generated report CSV to polymorphic `Attachment` rows. star-lms already has a `files` table and a `star-files` crate with upload/download plumbing; both families should reuse it with `context_type` = the new domain table (verify the `files` polymorphic columns when implementing; extend additively if needed).

## 4. Migration numbering

All migrations in these specs are **additive only** and use numbers **assigned by the coordinator** through `orchestration/migration-reservations.json` (protocol: issue #20; `next_available: 0108`, `max_merged: 0107` at the time of writing). Every migration listed below is marked **"010x, coordinator-assigned"** — agents must never self-assign. Approximate need: one migration per family (7 total), detailed in each file.

## 5. Testing conventions (shared)

- Every DB-backed test skips cleanly when `DATABASE_URL` is unset and uses a unique-per-run prefix (pid + millis) with full teardown, per the workspace ground rules in `task.md`.
- Tests document actual handler behavior, including Canvas quirks (e.g. SIS import 400-on-non-root-account, course report `last` returning `{}` when never run).
- Permission negatives are mandatory for every route: anonymous → 401, unauthorized enrolled/teacher-of-other-course or plain student → 403, cross-account → 404/403 as Canvas dictates.
- Route tables below use Canvas path notation (`:course_id`); star-lms registers the normalized twin (`/courses/{courseId}/...`) exactly as the existing crates do — the parity tool normalizes `/api/v1` away when matching.
