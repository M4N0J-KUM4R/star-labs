# P5 REST Spec — SIS (imports, import errors, SIS assignments, disable_post_to_sis)

- **Issue**: #113 (epic `specs`), family: SIS. Companion: `p5-rest-misc-index.md` (shared job-model convention §3).
- **Baseline**: fresh parity run at `526de19`; all four SIS controllers at **0%**.
- **Target crate**: new `star-sis` crate (hosts sis_imports_api, sis_import_errors_api, sis_api, disable_post_to_sis_api).

## 1. Overview and fresh coverage

| Controller | Done | Total | Coverage |
|---|---:|---:|---:|
| `sis_imports_api` | 0 | 7 | 0.0% |
| `sis_import_errors_api` | 0 | 2 | 0.0% |
| `sis_api` | 0 | 2 | 0.0% |
| `disable_post_to_sis_api` | 0 | 1 | 0.0% |

SIS imports are the classic batch-sync surface: a root-account admin uploads a CSV (or a zip of CSVs) and a singleton, account-stranded background job walks it through a 13-state workflow, recording per-row errors. `sis_api#sis_assignments` is a separate grade-export feed; `disable_post_to_sis` is a one-shot course maintenance action. Note the paths: the import/error routes are under `/api/v1/accounts/...`, but `sis_api` and `disable_post_to_sis_api` are under **`/api/sis/...`** (no `v1`) — the parity manifest records them as such and the star router must register the un-versioned prefix.

## 2. Route inventory (12 routes, all todo)

| # | Verb | Canvas path | Action | Status | Notes |
|---|---|---|---|---|---|
| 1 | GET | `/api/v1/accounts/:account_id/sis_imports` | index | todo | filters: `created_since`, `created_before`, `workflow_state[]` |
| 2 | GET | `/api/v1/accounts/:account_id/sis_imports/:id` | show | todo | embeds errors + progress |
| 3 | GET | `/api/v1/accounts/:account_id/sis_imports/importing` | importing | todo | batches currently importing |
| 4 | POST | `/api/v1/accounts/:account_id/sis_imports` | create | todo | multipart or pre_attachment flow (§6) |
| 5 | PUT | `/api/v1/accounts/:account_id/sis_imports/:id/abort` | abort | todo | |
| 6 | PUT | `/api/v1/accounts/:account_id/sis_imports/:id/restore_states` | restore_states | todo | returns a Progress object |
| 7 | PUT | `/api/v1/accounts/:account_id/sis_imports/abort_all_pending` | abort_all_pending | todo | account-wide |
| 8 | GET | `/api/v1/accounts/:account_id/sis_import_errors` | errors#index | todo | `failure` boolean filter |
| 9 | GET | `/api/v1/accounts/:account_id/sis_imports/:id/errors` | errors#index (per batch) | todo | |
| 10 | GET | `/api/sis/accounts/:account_id/assignments` | sis_api#sis_assignments | todo | **/api/sis** prefix |
| 11 | GET | `/api/sis/courses/:course_id/assignments` | sis_api#sis_assignments | todo | **/api/sis** prefix |
| 12 | PUT | `/api/sis/courses/:course_id/disable_post_to_sis` | disable_post_to_sis | todo | **/api/sis** prefix |

## 3. Permission matrix (Canvas abilities, all verified in controllers)

| Route(s) | Ability | Failure mode |
|---|---|---|
| sis_imports index / show / importing / abort | `[:import_sis, :manage_sis]` (grants_any) | 401 anonymous; 403 without either |
| sis_imports create | `:import_sis` | 403 |
| sis_imports restore_states | `:manage_sis` | 403 |
| sis_imports abort_all_pending | no explicit `authorized_action` found in this checkout (see §8); enforce `[:import_sis, :manage_sis]` in star-lms and pin it | — |
| sis_import_errors (both) | `[:import_sis, :manage_sis]` | 401/403 |
| sis_api sis_assignments | `require_view_all_grades` (`:view_all_grades`) + `require_grade_export` (400 when the account's SIS grade export is not enabled) + `require_published_course` | 401/403; **400** for grade-export-disabled |
| disable_post_to_sis | `context.grants_right?(user, :manage_assignments_edit)` | 401 (Canvas renders `head :unauthorized`) |

Additional **context guards** (verified, run before abilities on every sis_imports route): non-root account → `400 {"errors": ["SIS imports can only be executed on root accounts"]}`; account with SIS imports disabled → `403 {"errors": ["SIS imports are not enabled for this account"]}`.

## 4. Job/storage dependency model

**Canvas**: `SisBatch` enqueues `Delayed::LOW_PRIORITY` work with `strand: strand_for_account(account)` — a **singleton per account**, so one import processes at a time per account — plus an optional `run_at` delay (`sis_batch_process_start_delay` setting). The import-type callback table has exactly one entry in this checkout: `"instructure_csv" => batch.process_instructure_csv_zip` (the default). Restore-states runs via `restore_states_later`, which returns a `Progress` the API serializes as the whole response body.

**star-lms** (per the shared convention, index §3): `sis_batches` domain rows with the full 13-value `workflow_state` CHECK (`initializing, created, importing, cleanup_batch, imported, imported_with_messages, aborted, failed, failed_with_messages, restoring, partially_restored, restored, restore_failed` — verified in `app/models/sis_batch.rb`); `star-jobs::enqueue_job` with `tag = "SisBatch#process_instructure_csv"`, `strand = "sis_batch:{account_id}"`, `singleton = true`. The upload CSV is stored through the existing `files`/`star-files` plumbing with `context_type='SisBatch'` (verify the polymorphic columns at implementation). Route 6 returns a Canvas-`Progress`-shaped JSON built from the linked jobs row. No worker is required for parity: create returns the batch in `created`/`initializing`, and tests advance states with direct SQL updates.

## 5. Data model and migration needs

One migration — **"010x, coordinator-assigned"** — creating two tables:

- `sis_batches` — `id`, `account_id NOT NULL REFERENCES accounts(id)`, `user_id REFERENCES users(id)`, `workflow_state TEXT NOT NULL DEFAULT 'created'` with the 13-value CHECK, `import_type TEXT NOT NULL DEFAULT 'instructure_csv'`, `data JSONB NOT NULL DEFAULT '{}'` (batch_mode, term, diffing options, thresholds — the full documented param set from §6), `attachment_id BIGINT REFERENCES files(id)` (uploaded CSV/zip), `progress` fields sourced from the linked job, counters (`imported_count`, `workflow_state_counts JSONB`, `error_count`, `warning_count`), `started_at`, `ended_at`, timestamps.
- `sis_import_errors` — `id`, `sis_batch_id NOT NULL REFERENCES sis_batches(id) ON DELETE CASCADE`, `file TEXT`, `line INTEGER`, `message TEXT NOT NULL`, `failure BOOLEAN NOT NULL DEFAULT false`, `created_at` (index on `(sis_batch_id, created_at DESC)`).

`sis_api#sis_assignments` and `disable_post_to_sis` need **no new tables** (they read `assignments`, `course_sections`, `grading_periods` — all present).

## 6. Behavior contracts

- **create (route 4)**: `import_type` defaults to `instructure_csv`; any other value is invalid (Canvas raises `invalid import type parameter`). Two upload modes: (a) multipart `attachment` with the CSV body; (b) the 4-step `pre_attachment` flow (send metadata, receive file-upload target, upload, confirm) with `extension` parameter. Documented params (from the controller `@argument` list, all optional except the data itself): `batch_mode`, `batch_mode_term_id`, `multi_term_batch_mode`, `skip_deletes`, `override_sis_stickiness`, `add_sis_stickiness`, `clear_sis_stickiness`, `update_sis_id_if_login_claimed`, `diffing_data_set_identifier`, `diffing_remaster_data_set`, `diffing_drop_status` (`deleted|completed|inactive`), `diffing_user_remove_status` (`deleted|suspended`), `batch_mode_enrollment_drop_status` (`deleted|completed|inactive`), `change_threshold`, `diff_row_count_threshold`. Response: the batch JSON with embedded `progress` object and (via show) `errors` include. First wave-3 implementation may persist the full param set into `data` JSONB without acting on every flag — flag unimplemented ones in the response `data` verbatim and note them as accepted-but-inert.
- **index (route 1)**: `workflow_state[]` filter intersects against the 13 states; `created_since`/`created_before` ISO8601 bounds; paginated `{"sis_imports": [...]}` envelope.
- **show (route 2)**: `includes: ["errors"]` by default in Canvas; 404 for a batch on another account (scoped `@account.sis_batches.find`).
- **abort (route 5)**: transitions an importing batch toward `aborted`.
- **restore_states (route 6)**: params `batch_mode`, `undelete_only`, `unconclude_only`; response is a bare Progress JSON (`@returns Progress`).
- **importing (route 3)**: `{"sis_imports": [...]}` of batches in `importing`.
- **import errors (routes 8–9)**: `failure=true` keeps only failed rows; response `{"sis_import_errors": [...]}` ordered `created_at DESC`, paginated; different pagination base URL when scoped to a batch.
- **sis_assignments (routes 10–11)**: `starts_before`/`ends_after` window on the account variant; `include[]=student_overrides`; 400 `GRADE_EXPORT_NOT_ENABLED_ERROR` when grade export is off; requires published courses; assignment JSON carries SIS ids (`sis_assignment_id` etc.) — reuse the existing assignment serialization in `star-assignments` and add the SIS-id fields.
- **disable_post_to_sis (route 12)**: body may carry `grading_period_id`; unknown grading period → `400 {"code": "not_found", "error": "The Grading Period cannot be found"}`; success flips `post_to_sis` off for the course's published assignments.

## 7. Test plan

1. Import lifecycle: POST with a small instructure_csv attachment → 200, batch `created`, a `jobs` row enqueued with the account strand/singleton; advance state via SQL → show reflects `imported` with counts; index filters by `workflow_state[]` and date bounds.
2. Context guards: sub-account → 400 root-account error; `allow_sis_import` off → 403; invalid `import_type` → error.
3. abort / abort_all_pending / restore_states: state transitions and the Progress-shaped response for restore_states.
4. Import errors: seed `sis_import_errors`, verify both routes, `failure` filter, envelope, per-batch scope 404 for other account.
5. sis_assignments: teacher with `view_all_grades` on published course → 200 with SIS fields; grade-export disabled → 400; unpublished course → error; `include[]=student_overrides` shape.
6. disable_post_to_sis: authorized teacher → assignments' `post_to_sis` cleared; unknown grading_period_id → 400; plain student → 401.
7. Permission negatives on every route: anonymous 401; non-admin 403; wrong-account ids 404 (scoped queries).

## 8. Open questions

1. `abort_all_pending` has no explicit `authorized_action` in this Canvas checkout — verify against `routes.rb`/policy defaults; spec recommends `[:import_sis, :manage_sis]`.
2. Does wave-3 need the full `pre_attachment` 4-step flow, or is multipart-only acceptable for the first cut? (Recommend: multipart first, pre_attachment documented as a follow-up; the Canvas UI uses pre_attachment.)
3. SIS CSV parsing itself (users/accounts/courses/enrollments CSV dialect) is a large sub-project — this spec scopes the **API surface and state machine**, with the parser behind the same job tag; the wave-3 issue should slice parser support by CSV type.
4. Exact JSON shape of `sis_import_json` counts (`workflow_state_counts` etc.) — pin against a live Canvas response or the serializer at implementation.
