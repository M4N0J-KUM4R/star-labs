# P5 REST Spec — Account reports and course reports

- **Issue**: #113 (epic `specs`), family: account/course reports. Companion: `p5-rest-misc-index.md` (shared job-model convention §3).
- **Baseline**: fresh parity run at `526de19`; both controllers at **0%**.
- **Target crate**: new `star-account-reports` crate.

## 1. Overview and fresh coverage

| Controller | Done | Total | Coverage | UI-referenced |
|---|---:|---:|---:|---|
| `account_reports` | 0 | 6 | 0.0% | all 6 yes |
| `course_reports` | 0 | 3 | 0.0% | all 3 yes |

Account reports are Canvas's admin CSV export engine: a catalog of named report types, each run spawning a stateful `AccountReport` job that streams CSV into an attachment and exposes progress. Course reports are the same machinery scoped to a course with exactly one type in this checkout (`course_pace_docx`). Unlike most of issue #113, **every route here is UI-referenced** (the account Settings → Reports page), so these 9 routes count directly toward the UI-referenced coverage metric (258/503 today).

## 2. Route inventory (9 routes, all todo)

| # | Verb | Canvas path | Action | Status | Notes |
|---|---|---|---|---|---|
| 1 | GET | `/api/v1/accounts/:account_id/reports` | available_reports | todo | the catalog, not the runs |
| 2 | GET | `/api/v1/accounts/:account_id/reports/:report` | index | todo | runs of one type, `most_recent`, paginated |
| 3 | GET | `/api/v1/accounts/:account_id/reports/:report/:id` | show | todo | |
| 4 | POST | `/api/v1/accounts/:account_id/reports/:report` | create | todo | params[...] vary per type |
| 5 | PUT | `/api/v1/accounts/:account_id/reports/:report/:id/abort` | abort | todo | `created_or_running` only |
| 6 | DELETE | `/api/v1/accounts/:account_id/reports/:report/:id` | destroy | todo | soft delete (`deleted` state) |
| 7 | GET | `/api/v1/courses/:course_id/reports/:report_type` | course_reports#last | todo | most recent **by the caller** |
| 8 | POST | `/api/v1/courses/:course_id/reports/:report_type` | course_reports#create | todo | invalid type → 400 |
| 9 | GET | `/api/v1/courses/:course_id/reports/:report_type/:id` | course_reports#show | todo | |

## 3. Permission matrix (verified)

| Route(s) | Ability | Notes |
|---|---|---|
| account_reports — all six actions | `@context.grants_right?(user, :read_reports)` | one ability guards the whole controller (verified: every action calls `authorized_action(..., :read_reports)`) |
| course_reports create (route 8) | `:read_reports` on the course context | verified line 127 |
| course_reports show (route 9) | `:read` on the report record itself | verified line 100 — a user can read a report they can see even without `read_reports`; the record policy follows the account/course |
| course_reports last (route 7) | implicit: scoped to `user: @current_user` | only your own runs are ever returned |

Negatives: anonymous 401 everywhere; account/course member without `read_reports` 403; unknown report type on routes 2–6 → 404 (type_scope misses); wrong account's report id → 404 (scoped `find`).

## 4. Job/storage dependency model

**Canvas**: `AccountReport`/`CourseReport` walk `created → running → compiling → complete | error | aborted | deleted` (all seven states verified in both models; `running` scope = `running+compiling`, `created_or_running` = `created+running+compiling`, `active` = not-deleted). Generation runs through the `account_reports` **plugin** (`gems/plugins/account_reports`): a `REPORTS` registry keyed by report_type maps to a `proc` and optional `parallel_proc` (parallel runners — `account_report_runners` rows). Output CSV is written to a polymorphic `Attachment`; `progress` and `current_line_count` (bumped every 1000 records) are exposed on the JSON. Aborting flips `created_or_running` rows to `aborted` and also aborts incomplete runners.

**star-lms** (shared convention, index §3): `account_reports`/`course_reports` domain tables with the 7-state CHECK; `star-jobs::enqueue_job(tag = "AccountReport#generate", context_type/context_id = the report row)`; CSV written through `star-files` into an attachment row referenced by `attachment_id`. Route 1 (the catalog) is **static configuration**, not a query: a Rust-side registry (type name, title, parameters, whether implemented) rendering Canvas's shape, so unimplemented types can still be listed with a note. First-cut report implementations: the outcome/grade export family already has close cousins in-repo (`learning_outcome_results`, quiz statistics) — the wave-3 issue should pick 1–2 types (recommend `sis_export_csv`/`provisioning_csv` and `grade_export_csv` since the data is all local) and return `error` state ("not implemented") for the rest.

Verified report types in `gems/plugins/account_reports/lib/account_reports/default.rb`: `student_assignment_outcome_map_csv`, `outcome_results_csv`, `outcome_export_csv`, `grade_export_csv`, `parallel_grade_export_csv`, `mgp_grade_export_csv`, `parallel_mgp_grade_export_csv`; the controller documentation additionally shows `sis_export_csv` / `provisioning_csv` examples — full catalog to be enumerated from the plugin at implementation (§8).

## 5. Data model and migration needs

One migration — **"010x, coordinator-assigned"** — creating:

- `account_reports` — `id`, `account_id NOT NULL REFERENCES accounts(id)`, `user_id NOT NULL REFERENCES users(id)`, `report_type TEXT NOT NULL`, `workflow_state TEXT NOT NULL DEFAULT 'created'` with the 7-value CHECK, `parameters JSONB NOT NULL DEFAULT '{}'`, `attachment_id BIGINT REFERENCES files(id)`, `progress INTEGER NOT NULL DEFAULT 0`, `current_line_count INTEGER`, `start_at`, `end_at`, `runtime REAL`, timestamps. Index `(account_id, report_type, created_at DESC)`.
- `course_reports` — same shape with `course_id NOT NULL REFERENCES courses(id)` plus `root_account_id`; `parameters` limited to `enrollment_ids[]`/`section_ids[]` (verified permit list).

No `account_report_runners` table in the first cut (parallel runners are an optimization; single-job generation covers the API contract).

## 6. Behavior contracts

- **Route 1 (available_reports)**: returns the catalog — entries with report definition plus `last_run` (the Report JSON) or null; `include[]=description_html` / `parameters_html` flags documented.
- **Route 2 (index)**: paginated runs of the given type, `active` scope, `most_recent` ordering; unknown type → 404.
- **Route 4 (create)**: `report[...]` parameters pass through to `parameters` JSONB; creates in `created` and enqueues the job; response is the report JSON (`@returns Report`).
- **Route 5 (abort)**: scoped to `created_or_running` — aborting a `complete`/`error` report → 404 (scope miss); success returns the report JSON with `workflow_state: "aborted"`.
- **Route 6 (destroy)**: soft-delete (`workflow_state = "deleted"`, `active` scope excludes it afterwards); returns the report JSON; destroy failure → 400 with errors.
- **Report JSON shape** (from the controller `@response_field` docs): `id`, `report` (type), `file_url`, `attachment` (only after completion), `status` (the workflow_state), `created_at`, `started_at`, `ended_at`, `runtime`, `parameters`, `progress`, `current_line_count`, `user` (initiator).
- **course_reports#last quirk**: renders **`{}`** (200) when the caller has never run the type — not 404. Pin this.
- **course_reports#create quirk**: unknown type → `400 {"error": "invalid report type <type>"}` (not 404); valid type creates a row (course_pace_docx is the only one in this checkout).
- Pagination: `Api.paginate` with Link headers, per workspace convention.

## 7. Test plan

1. Catalog: GET route 1 lists the configured registry with `last_run` null; after a run, `last_run` is populated.
2. Lifecycle: create → row in `created` + enqueued job; SQL-advance to `running` → show reflects it; complete with a real attachment (small CSV via star-files) → `file_url`/`attachment` present; runtime/line-count fields set.
3. abort on `created`/`running` → `aborted`; abort on `complete` → 404.
4. destroy soft-deletes; subsequent show/index 404/exclude it.
5. Course reports: create with `parameters[enrollment_ids]`; `last` returns `{}` before any run and the report after; invalid type → 400 exact body; show by non-`read_reports` user per the record-policy note in §3.
6. Permission negatives: anonymous 401 ×9; student/teacher without `read_reports` 403; cross-account report id 404; unknown report type 404 (account routes 2–6).

## 8. Open questions

1. Full report-type catalog (the plugin registers types across multiple files — enumerate `configure_account_report` calls at implementation; only the Default module was read for this spec).
2. Whether the wave-3 issue should implement any report generator or only the state machine + one stub type — recommend state machine + `grade_export_csv` as proof.
3. `parameters_html`/`description_html` includes render ERB partials in Canvas; star-lms should return static strings and document the deviation.
4. Course `course_pace_docx` generator depends on course pacing data star-lms does not model yet — likely returns `error` state in the first cut; confirm scope with the wave-3 issue.
