# Spec: REST curriculum remainder — outcomes, courses deep, groups

- **Issue:** #24 (epic: `specs`)
- **Branch:** `p5/specs-24` (from wave-1 batch head `526de19`, which carries the six not-yet-merged-to-main specs as landed references)
- **Scope:** curriculum routes NOT owned by open PR #16 (discussion_topics group-context/read-all) or merged PR #19 (users controller). In scope: `outcome_groups_api` (all contexts), `outcomes_api`, `courses` deep gaps, `groups`, `group_categories`
- **Status:** SPEC — documentation only; no code, no migrations, no cargo, no push

---

## 1. Overview and fresh coverage

Numbers below were freshly re-measured for this spec at the worktree head:

```
python3 tools/parity/parity.py report \
  --manifest parity/canvas_manifest.json --star-src . --out /tmp/parity-i24
```

Workspace totals: **REST 471/1403 (33.6%)**, UI-referenced **258/503 (51.3%)**, 624 star route registrations (157 extras).

In-scope controllers (from `/tmp/parity-i24/report.md` + `coverage.json`):

| Controller | Done | Total | Coverage | Target crate |
|---|---:|---:|---:|---|
| `outcome_groups_api` | 13 | 40 | 32.5% | `star-outcomes` |
| `outcomes_api` | 0 | 4 | 0.0% | `star-outcomes` |
| `courses` | 14 | 38 | 36.8% | `star-courses` |
| `groups` | 9 | 17 | 52.9% | `star-groups` |
| `group_categories` | 10 | 16 | 62.5% | `star-groups` |
| **In-scope total (5 controllers)** | **46** | **115** | **40.0%** | |

Missing routes in scope: **69** (`outcome_groups_api` 27, `outcomes_api` 4, `courses` 24, `groups` 8, `group_categories` 6).

Boundary notes:
- `courses#users` / `courses#user` (`GET /courses/:course_id/users[/:id]`) are **courses-controller** routes and are already done; they stay in this inventory for completeness even though the users *controller* belongs to PR #19.
- `discussion_topics` group-context routes (PR #16) are excluded entirely.
- Canvas source: `config/routes.rb` + the five controllers under `app/controllers/`.

Priorities within the gap (UI-referenced first):
1. `outcomes_api` show/update/destroy — 0% controller, blocks every outcomes-management flow.
2. Outcome link write surface: `PUT/DELETE .../outcomes/:outcome_id` (link/unlink) and `POST .../import` across course/account/global contexts.
3. Courses deep-read: `GET /accounts/:account_id/courses/:id`, `include[]` fan-out on show/index (syllabus_body, term, grading_periods, course_progress, needs_grading_count, permissions), `search_users`, `permissions`, `recent_students`, `todo`, `students`.
4. Groups/group_categories tail: account-context index, `bulk_user_tags`, differentiation-tag trio, CSV import/export pair, `users` on category.
5. Feature-gated / legacy last: `youtube_migration` (5 routes), `link_validation`, `dismiss_migration_limitation_message`, `new_quizzes_selection_update`.

---

## 2. Route inventory per controller

Status legend: `done` = registered in a star crate and matched by the parity report; `missing` = Canvas route with no star match. `UI` = route referenced from Canvas `ui/`.

### 2.1 `outcome_groups_api` — 13/40 done (27 missing) → `star-outcomes`

| Method | Canvas path | Status | Notes |
|---|---|---|---|
| GET | `/accounts/:account_id/outcome_group_links` | **missing** | link_index, account context |
| GET | `/accounts/:account_id/outcome_groups` | done UI | index |
| GET | `/accounts/:account_id/outcome_groups/account_chain` | **missing** | undocumented, UI-only (outcomes find modal) |
| DELETE | `/accounts/:account_id/outcome_groups/:id` | done UI | destroy |
| GET | `/accounts/:account_id/outcome_groups/:id` | done UI | show |
| PUT | `/accounts/:account_id/outcome_groups/:id` | done UI | update |
| POST | `/accounts/:account_id/outcome_groups/:id/import` | **missing** | import group tree |
| GET | `/accounts/:account_id/outcome_groups/:id/outcomes` | **missing** | outcomes list (course variant done) |
| POST | `/accounts/:account_id/outcome_groups/:id/outcomes` | **missing** | link (course variant done) |
| DELETE | `/accounts/:account_id/outcome_groups/:id/outcomes/:outcome_id` | **missing** | unlink |
| PUT | `/accounts/:account_id/outcome_groups/:id/outcomes/:outcome_id` | **missing** | link by outcome_id |
| GET | `/accounts/:account_id/outcome_groups/:id/subgroups` | done | subgroups (accounts only today) |
| POST | `/accounts/:account_id/outcome_groups/:id/subgroups` | **missing** | create subgroup |
| GET | `/accounts/:account_id/root_outcome_group` | **missing** | redirect (course variant done) |
| GET | `/courses/:course_id/outcome_group_links` | done UI | link_index |
| GET | `/courses/:course_id/outcome_groups` | done UI | index |
| GET | `/courses/:course_id/outcome_groups/account_chain` | **missing** | undocumented |
| DELETE | `/courses/:course_id/outcome_groups/:id` | done UI | destroy |
| GET | `/courses/:course_id/outcome_groups/:id` | done UI | show |
| PUT | `/courses/:course_id/outcome_groups/:id` | done UI | update |
| POST | `/courses/:course_id/outcome_groups/:id/import` | **missing** | import group tree |
| GET | `/courses/:course_id/outcome_groups/:id/outcomes` | done | outcomes |
| POST | `/courses/:course_id/outcome_groups/:id/outcomes` | done | link |
| DELETE | `/courses/:course_id/outcome_groups/:id/outcomes/:outcome_id` | **missing** | unlink |
| PUT | `/courses/:course_id/outcome_groups/:id/outcomes/:outcome_id` | **missing** | link by outcome_id |
| GET | `/courses/:course_id/outcome_groups/:id/subgroups` | **missing** | subgroups (only `/outcome_groups/:id/subgroups` global-id form exists) |
| POST | `/courses/:course_id/outcome_groups/:id/subgroups` | **missing** | create subgroup |
| GET | `/courses/:course_id/root_outcome_group` | done UI | redirect |
| GET | `/global/outcome_groups/account_chain` | **missing** | global context |
| DELETE | `/global/outcome_groups/:id` | **missing** | global group destroy |
| GET | `/global/outcome_groups/:id` | **missing** | global group show |
| PUT | `/global/outcome_groups/:id` | **missing** | global group update |
| POST | `/global/outcome_groups/:id/import` | **missing** | |
| GET | `/global/outcome_groups/:id/outcomes` | **missing** | |
| POST | `/global/outcome_groups/:id/outcomes` | **missing** | |
| DELETE | `/global/outcome_groups/:id/outcomes/:outcome_id` | **missing** | unlink |
| PUT | `/global/outcome_groups/:id/outcomes/:outcome_id` | **missing** | |
| GET | `/global/outcome_groups/:id/subgroups` | **missing** | |
| POST | `/global/outcome_groups/:id/subgroups` | **missing** | |
| GET | `/global/root_outcome_group` | **missing** | |

Quirk: Canvas exposes both a global-id form (`/outcome_groups/:id`, already done in star-outcomes) and context-scoped forms (`/accounts/:account_id/outcome_groups/:id`, `/courses/:course_id/outcome_groups/:id`, `/global/outcome_groups/:id`) that address the SAME rows. The context-scoped forms additionally enforce that the group belongs to that context (or is visible through it). Star already has both forms for show/update/destroy on accounts and courses; the tail is link/unlink/import/subgroups-create/account_chain/link_index(account)/global-context.

### 2.2 `outcomes_api` — 0/4 done (4 missing) → `star-outcomes`

| Method | Canvas path | Status | Notes |
|---|---|---|---|
| GET | `/courses/:course_id/outcome_alignments` | **missing** UI | requires `assignment_id` or `student_id` |
| DELETE | `/outcomes/:id` | **missing** | destroy outcome |
| GET | `/outcomes/:id` | **missing** | show, `add_defaults` param |
| PUT | `/outcomes/:id` | **missing** | update incl. ratings/mastery/calculation |

### 2.3 `courses` — 14/38 done (24 missing) → `star-courses`

| Method | Canvas path | Status | Notes |
|---|---|---|---|
| POST | `/accounts/:account_id/courses` | done UI | create |
| PUT | `/accounts/:account_id/courses` | **missing** UI | batch_update (offer/conclude/delete/undelete) |
| GET | `/accounts/:account_id/courses/:id` | **missing** | account-scoped show |
| GET | `/courses` | done UI | index |
| DELETE | `/courses/:id` | done UI | destroy |
| GET | `/courses/:id` | done UI | show — **deep `include[]` gaps, see §5.3** |
| PUT | `/courses/:id` | done UI | update |
| GET | `/courses/:course_id/activity_stream` | **missing** | stream items (0106 exists) |
| GET | `/courses/:course_id/activity_stream/summary` | done UI | |
| GET | `/courses/:course_id/bulk_user_progress` | **missing** | all users' progress CSV/JSON |
| GET | `/courses/:course_id/content_share_users` | done UI | |
| POST | `/courses/:course_id/files` | done UI | create_file |
| POST | `/courses/:id/dismiss_migration_limitation_message` | **missing** | feature-flagged |
| GET | `/courses/:course_id/effective_due_dates` | **missing** | per-student/assignment effective dates |
| GET | `/courses/:course_id/link_validation` | **missing** | read job result |
| POST | `/courses/:course_id/link_validation` | **missing** | start async job |
| GET | `/courses/:course_id/permissions` | **missing** | computed permissions hash |
| POST | `/courses/:course_id/ping` | done UI | |
| POST | `/courses/:course_id/preview_html` | **missing** | sanitized HTML preview |
| PUT | `/courses/:id/quizzes` | **missing** | new_quizzes_selection_update |
| GET | `/courses/:course_id/recent_students` | **missing** | ordered by last_login |
| POST | `/courses/:course_id/reset_content` | **missing** | reset to blueprint state |
| POST | `/courses/:course_id/restore/:version_id` | **missing** UI | restore course version |
| GET | `/courses/:course_id/search_users` | **missing** | users + search params (alias of #users) |
| GET | `/courses/:course_id/settings` | done UI | api_settings |
| PUT | `/courses/:course_id/settings` | done UI | update_settings |
| GET | `/courses/:course_id/student_view_student` | **missing** | Test Student user |
| GET | `/courses/:course_id/students` | **missing** | deprecated, unpaginated |
| GET | `/courses/:course_id/todo` | **missing** | needing-submit/grade items |
| GET | `/courses/:course_id/users` | done UI | users (courses controller) |
| GET | `/courses/:course_id/users/:id` | done UI | user |
| GET | `/courses/:course_id/users/:user_id/progress` | **missing** | single-user progress |
| GET | `/courses/:course_id/youtube_migration/conversion_status` | **missing** UI | legacy YT migration tool |
| POST | `/courses/:course_id/youtube_migration/convert` | **missing** UI | |
| GET | `/courses/:course_id/youtube_migration/scan` | **missing** UI | |
| POST | `/courses/:course_id/youtube_migration/scan` | **missing** UI | |
| PUT | `/courses/:course_id/youtube_migration/scan/:scan_id` | **missing** | |
| GET | `/users/:user_id/courses` | done UI | user_index |

### 2.4 `groups` — 9/17 done (8 missing) → `star-groups`

| Method | Canvas path | Status | Notes |
|---|---|---|---|
| GET | `/accounts/:account_id/groups` | **missing** | context_index (account) |
| GET | `/courses/:course_id/bulk_user_tags` | **missing** UI | differentiation tags bulk |
| GET | `/courses/:course_id/groups` | done UI | context_index (course) |
| POST | `/group_categories/:group_category_id/groups` | **missing** UI | create in category |
| POST | `/groups` | done UI | create (standalone/category) |
| DELETE | `/groups/:id` | done UI | destroy |
| GET | `/groups/:id` | done UI | show |
| PATCH | `/groups/:id` | done UI | update |
| PUT | `/groups/:id` | done UI | update |
| GET | `/groups/:group_id/activity_stream` | **missing** | group stream |
| GET | `/groups/:group_id/activity_stream/summary` | **missing** | |
| POST | `/groups/:group_id/files` | **missing** UI | create_file (group upload step 1) |
| POST | `/groups/:group_id/invite` | done | invite |
| GET | `/groups/:group_id/permissions` | **missing** | permissions hash |
| POST | `/groups/:group_id/preview_html` | **missing** | |
| GET | `/groups/:group_id/users` | done UI | users |
| GET | `/users/self/groups` | done UI | index (self) |

### 2.5 `group_categories` — 10/16 done (6 missing) → `star-groups`

| Method | Canvas path | Status | Notes |
|---|---|---|---|
| GET | `/accounts/:account_id/group_categories` | done UI | index |
| POST | `/accounts/:account_id/group_categories` | done UI | create |
| GET | `/courses/:course_id/group_categories` | done UI | index |
| POST | `/courses/:course_id/group_categories` | done UI | create |
| POST | `/courses/:course_id/group_categories/bulk_manage_differentiation_tag` | **missing** UI | create/destroy tags in bulk |
| GET | `/courses/:course_id/group_categories/export_tags` | **missing** | tag CSV export |
| POST | `/courses/:course_id/group_categories/import_tags` | **missing** | tag CSV import |
| DELETE | `/group_categories/:id` | done UI | destroy |
| GET | `/group_categories/:id` | done UI | show |
| PATCH | `/group_categories/:id` | done UI | update |
| PUT | `/group_categories/:id` | done UI | update |
| POST | `/group_categories/:group_category_id/assign_unassigned_members` | done UI | |
| GET | `/group_categories/:group_category_id/export` | **missing** UI | groups CSV export |
| GET | `/group_categories/:group_category_id/groups` | done UI | groups |
| POST | `/group_categories/:group_category_id/import` | **missing** UI | groups CSV import |
| GET | `/group_categories/:group_category_id/users` | **missing** | category users |

---

## 3. Permission matrix per controller

Role legend (Canvas RoleOverride semantics): **SA** site admin, **AA** account admin, **T** teacher-in-context, **SD** student (enrolled), **O** observer, **X** unauthenticated/unenrolled → 401; `-` → 401/403 by `authorized_action`. Ability names are Canvas's, as read from the controllers.

### 3.1 `outcome_groups_api` + `outcomes_api`

Guard helpers in the controller: `can_read_outcomes` / `can_manage_outcomes` dispatch on context presence — with a context they check `read_outcomes` / `manage_outcomes` on the context; in the global context they check `read_global_outcomes` / `manage_global_outcomes` on `Account.site_admin`. `link_index` requires `require_context` (courses/accounts) + read side.

| Action (route group) | Ability checked (on) | SA | AA | T | SD | O | Notes |
|---|---|---|---|---|---|---|---|
| index/show/subgroups/outcomes/link_index/redirect (GET) | `read_outcomes` (context) | ✓ | ✓ | ✓ | ✓ | ✓ | enrolled users read course outcomes |
| global-context GET (`/global/...`) | `read_global_outcomes` (site_admin) | ✓ | – | – | – | – | |
| create/update/destroy group, create subgroup, link/unlink, import | `manage_outcomes` (context) | ✓ | ✓ | ✓ | – | – | T gets it via course role |
| global-context writes | `manage_global_outcomes` (site_admin) | ✓ | – | – | – | – | |
| `account_chain` (undocumented) | `manage_outcomes` (context) | ✓ | ✓ | ✓ | – | – | writer-side only |
| outcomes#show | `read` (on the LearningOutcome) | ✓ | ✓ | ✓ | ✓ | ✓ | |
| outcomes#update / #destroy | `update` (on the LearningOutcome) | ✓ | ✓ | ✓* | – | – | *teacher only for course-owned outcomes |
| outcome_alignments (assignment_id present) | `manage_grades` OR `view_all_grades` (course) | ✓ | ✓ | ✓ | – | – | 401 otherwise |
| outcome_alignments (student_id present) | student self, or grader rights | ✓ | ✓ | ✓ | self | self | see §5.2 |

Special cases:
- destroy group: root groups (no `learning_outcome_group_id`/parent) return **400** `"error"` before any delete — parity must keep this quirk.
- import: **400** when the source group does not exist, is a root group, or its context is not global / same context / an associated account.
- outcomes#update under `account_level_mastery_scales` FF: params `mastery_points`, `ratings`, `calculation_method`, `calculation_int` are rejected with **403** `{"error": "Individual outcome ... cannot be modified."}` even for admins.

### 3.2 `courses`

| Action | Ability checked (on) | SA | AA | T | SD | O | X | Notes |
|---|---|---|---|---|---|---|---|---|
| index `GET /courses` | visibility filter (enrollments + observed + admin scope) | ✓ | ✓ | ✓ | ✓ | ✓ | – | `enrollment_type/state/role`, `include[]` |
| show | `read` (course) | ✓ | ✓ | ✓ | ✓ | ✓ | public if open | include[] fan-out gated per-field (below) |
| show `include[]=permissions` | `read` | ✓ | ✓ | ✓ | ✓ | ✓ | – | permission values differ by role |
| account-scoped show `GET /accounts/:a/courses/:id` | `read` + account context | ✓ | ✓ | ✓ | ✓ | – | – | |
| create | `create_courses` (account) | ✓ | ✓ | ✓† | – | – | – | †if allowed by account settings |
| batch_update | `manage_courses_admin`, or `undelete_courses` when `event=undelete` | ✓ | ✓ | – | – | – | – | |
| update/destroy | `update` / `delete` (course) | ✓ | ✓ | ✓ | – | – | – | done already |
| users / search_users | `read_roster` OR `view_all_grades` OR `manage_grades` | ✓ | ✓ | ✓ | – | – | – | `include_inactive` additionally needs `read_as_admin` |
| students (deprecated) | `read_roster` | ✓ | ✓ | ✓ | – | – | – | unpaginated |
| recent_students | `read_reports` | ✓ | ✓ | ✓ | – | – | – | |
| permissions | `read` (course) | ✓ | ✓ | ✓ | ✓ | ✓ | – | returns grants hash |
| preview_html | `read` (course) | ✓ | ✓ | ✓ | ✓ | ✓ | – | |
| activity_stream(+summary) | `read` (course) for course-scoped stream | ✓ | ✓ | ✓ | ✓ | ✓ | – | summary already done |
| todo | `read` (course) | ✓ | ✓ | ✓ | ✓ | ✓ | – | grading half needs grader |
| user_progress / bulk_user_progress | `read` (course) + target visibility (`view_all_grades` or target `read`) | ✓ | ✓ | ✓ | self | self | – | |
| student_view_student | `use_student_view` (course) | ✓ | ✓ | ✓ | – | – | – | |
| reset_content | `manage_course_content_edit`-family (blueprint) | ✓ | ✓ | ✓ | – | – | – | |
| effective_due_dates | `read` + grade visibility (`manage_grades`/`view_all_grades`) | ✓ | ✓ | ✓ | – | – | – | |
| settings GET/PUT | `read`/`update` — done | ✓ | ✓ | ✓ | – | – | – | |
| dismiss_migration_limitation_message | `manage_course_content_edit` (course) | ✓ | ✓ | ✓ | – | – | – | FF-gated |
| new_quizzes_selection_update | `manage`-family | ✓ | ✓ | ✓ | – | – | – | FF-gated |
| link_validation GET/POST | `read_reports`/`manage`-family | ✓ | ✓ | ✓ | – | – | – | async job |
| youtube_migration (5) | `manage`-family | ✓ | ✓ | ✓ | – | – | – | legacy FF tool |

Include-parameter gating on show/index (the "deep" part): `syllabus_body` only when caller can `read_syllabus`; `term`, `grading_periods`, `course_progress`, `total_scores` (on enrollments) and `needs_grading_count` (graders only) each require the corresponding read right; `observers` only for graders. `hide_final_grades` (account setting) suppresses student-visible grade totals.

### 3.3 `groups` + `group_categories`

| Action | Ability checked (on) | SA | AA | T | SD | O | Notes |
|---|---|---|---|---|---|---|---|
| context_index (course) | `read_roster` (context) | ✓ | ✓ | ✓ | ✓ | ✓ | done |
| context_index (account) | `read_roster` (account) | ✓ | ✓ | – | – | – | |
| context_index `collaboration_state=non_collaborative` | `manage_tags_add` OR `manage_tags_manage` OR `manage_tags_delete` | ✓ | ✓ | ✓ | – | – | tags are differentiation tags |
| context_index `collaboration_state=all` | non-granular callers silently get collaborative-only | | | | | | fallback filter |
| bulk_user_tags | granular `manage_tags_*` (add/manage/delete) | ✓ | ✓ | ✓ | – | – | |
| create (context) | `manage_groups_add` (context) | ✓ | ✓ | ✓ | – | – | standalone `/groups` create done |
| create (category) | category context + `manage_groups_add` | ✓ | ✓ | ✓ | – | – | |
| show | `read` (group) | ✓ | ✓ | ✓ | ✓ | ✓ | membership-based |
| update / destroy | `update` / `delete` (group) | ✓ | ✓ | ✓ | – | – | done |
| users | `read_roster` (group) | ✓ | ✓ | ✓ | ✓ | ✓ | done |
| invite / manage members | `manage` (group) | ✓ | ✓ | ✓ | – | – | invite done |
| activity_stream(+summary) | `read` (group context) | ✓ | ✓ | ✓ | ✓ | ✓ | |
| permissions | `manage` (group) | ✓ | ✓ | ✓ | – | – | |
| preview_html | `manage` (group) | ✓ | ✓ | ✓ | – | – | courses#preview_html uses `read`; group side is stricter |
| create_file | `create` (attachment) + context | ✓ | ✓ | ✓ | ✓* | – | *students in their own group |
| group_categories index/create | `require_context` + granular `manage_groups_*` + `manage_tags_*` | ✓ | ✓ | ✓ | – | – | done |
| group_categories update/destroy | granular `manage_groups_*` (+`manage_tags_*` for tag categories) | ✓ | ✓ | ✓ | – | – | done |
| assign_unassigned_members | `manage_groups_*` | ✓ | ✓ | ✓ | – | – | done |
| categories#users | Course ctx: `read_roster`; Account ctx: `read` | ✓ | ✓ | ✓ | ✓† | ✓† | †course ctx students |
| bulk_manage_differentiation_tag | granular `manage_tags_*` | ✓ | ✓ | ✓ | – | – | |
| import/export, import_tags/export_tags | granular `manage_groups_*` / `manage_tags_*` | ✓ | ✓ | ✓ | – | – | CSV round-trip |

SIS note (from controller): `sis_*`/integration params on group create/update are only honored when the caller holds `manage_sis` on the root account; `storage_quota_mb` only with `manage_storage_quotas` — mirror both silently-stripped behaviors.

---

## 4. Data model & migration needs (additive only)

Existing coverage (verified in `crates/star-db/migrations/`):
- `outcome_groups` (0044; deepened 0081 with `source_outcome_group_id` + parent/state index), `outcome_links` (0044), `outcome_alignments` (0044), legacy `learning_outcomes`/`learning_outcome_groups`/`learning_outcome_links`/`learning_outcome_results` (0022).
- `groups` + `group_memberships` (0018), `group_categories` (0043, deepened 0096 with `account_id` and `non_collaborative` on categories), `group_invitations`/`group_files` (0064).
- `courses.syllabus_body` (0009), `courses.storage_quota` (0061), `grading_period_groups`/`grading_periods` (0050), enrollment terms (0101), stream items (0106).

Additive needs — **all numbers coordinator-assigned via `orchestration/migration-reservations.json` (next available 0108); never self-assign**:

| Need | Why | Migration |
|---|---|---|
| `learning_outcomes`: add `mastery_points NUMERIC`, `calculation_method TEXT`, `calculation_int INT`, `vendor_guid TEXT`, `ratings JSONB` (embedded rubric criterion: `[{description, points}]`) | `outcomes_api` show/update require the full Outcome JSON incl. ratings + calculation method; 0022 has only title/description/display_name | needs 010x (coordinator-assigned) |
| `outcomes_friendly_descriptions` (learning_outcome_id, account_id, course_id, description, unique per outcome+context) | friendly descriptions on `outcomes` include (FF `outcomes_friendly_description`) — can be deferred | needs 010x (coordinator-assigned), defer acceptable |
| `groups.non_collaborative BOOLEAN DEFAULT false` | `collaboration_state` filter and `bulk_user_tags`; 0096 indexed it on `group_categories` — **verify whether the `groups` column already exists; add only if absent** | needs 010x (coordinator-assigned) if absent |
| `courses.hide_final_grades BOOLEAN DEFAULT false` (if absent — in-memory struct exists in star-courses, column does not appear in migrations) | student-visible totals suppression in show/index/enrollments | needs 010x (coordinator-assigned) |
| `accounts.parent_account_id` — **verify star-accounts schema**; `account_chain` walks it | `outcome_groups_api#account_chain` builds the ancestor list | needs 010x (coordinator-assigned) if absent |
| Assignment overrides / differentiation dates for `effective_due_dates` — **verify whether `assignment_overrides`-equivalent tables exist** | effective due dates are computed from base + student/section/override dates | needs 010x (coordinator-assigned) if absent; defer route otherwise |
| Course versions for `restore_version` — check whether course snapshots exist (likely absent) | `POST /courses/:course_id/restore/:version_id` restores a prior course state | needs 010x (coordinator-assigned); propose deferring route |

No new tables needed for: link/unlink (use `outcome_links`), import (deep-copy rows within existing tables; `source_outcome_group_id` already present), account_chain response (derived), account-context index fan-outs, `permissions`/`preview_html`/`todo`/`recent_students`/`students`/`search_users` (computed), CSV import/export (attachment-based), groups streams (0106), student_view_student (create a user row per course with existing `users`/`enrollments`).

---

## 5. Behavior contracts for missing routes

### 5.1 `outcome_groups_api` — link/unlink, import, subgroups, account_chain, global context

- **`POST /:context/outcome_groups/:id/outcomes` (link)** — body `outcome_id` (or `outcome_ids[]`), `move_from` (optional). Validates the outcome is available in the context (global, same context, or an associated account) via `context_available_outcome`; else **404**. Returns the new `OutcomeLink` JSON (fields: `url`, `context_id`, `context_type`, `outcome_group`, `outcome`, `can_unlink`). Duplicate link in same group → **400** (unique constraint).
- **`PUT /:context/outcome_groups/:id/outcomes/:outcome_id` (link by id)** — same semantics, outcome addressed by path.
- **`DELETE /:context/outcome_groups/:id/outcomes/:outcome_id` (unlink)** — destroys the `outcome_links` row; returns the deleted `OutcomeLink` JSON. Unlinking the outcome's owning/last link is allowed for admins (`can_unlink` governs UI, not the route).
- **`POST /:context/outcome_groups/:id/import`** — body `source_outcome_group_id`. Copies the source subtree (groups + linked outcomes) into the target group within the caller's context. Errors: missing/nonexistent source, source is root, or source context not global/same/associated → **400** with body `"error"` (JSON string, quirk of Canvas). Success returns the updated target `OutcomeGroup` JSON.
- **`POST /:context/outcome_groups/:id/subgroups`** — body `title`, `description`, `vendor_guid`. Parent must be in context (else **404**). Returns the new `OutcomeGroup` (201).
- **`GET /:context/outcome_groups/:id/subgroups`** — course-scoped form missing; account form done. Children ordered by title, active only, paginated.
- **`GET /:context/outcome_groups/account_chain`** — undocumented, UI-only. Returns, for each ancestor account above the context, `{id (root group id), title, description: "Account level outcomes group.", dontImport: true, url, subgroups_url, outcomes_url}`. Requires `manage_outcomes`; paginated. For an Account context the chain excludes the account itself (`account_chain[1..]`).
- **`GET /:context/outcome_group_links` (account form)** — mirrors the done course `link_index`: returns `OutcomeLink`s whose group's context chain includes the account.
- **`GET /:context/root_outcome_group` (account/global)** — 302-style redirect to the context root group show (course variant done).
- **Global context (`/global/...`)** — same contracts as context forms, but permission checks run against `Account.site_admin` with `read_global_outcomes`/`manage_global_outcomes`, and group rows use the global context convention (`context_type` global/NULL).

### 5.2 `outcomes_api` — show, update, destroy, outcome_alignments

- **`GET /outcomes/:id`** — param `add_defaults` (bool): when set and the Account Level Mastery Scales FF is off, injects color/mastery-level defaults into ratings. Response `Outcome` JSON: `id`, `url`, `context_id`, `context_type`, `title`, `display_name`, `description`, `vendor_guid`, `mastery_points`, `ratings[]` (`{description, points}`, plus `color`/`mastery` when defaulted), `calculation_method`, `calculation_int`, `canonical_outcome_uuid` (where applicable), `assessed` (bool). 404 when not visible to caller.
- **`PUT /outcomes/:id`** — params: `title`, `display_name`, `description`, `vendor_guid`, `mastery_points`, `calculation_method` (`weighted_average|decaying_average|n_mastery|latest|highest|average`), `calculation_int`, `ratings[][description]`, `ratings[][points]`, `add_defaults`. Semantics: fields not provided left as-is; unrecognized fields ignored; a provided `ratings[]` **fully replaces** the embedded rubric criterion (no merging); missing rating descriptions default to `"No description"`, missing points to `0`; `mastery_points` defaults to max rating points when ratings given without it. Validation failure → **400** with the model errors object. FF `account_level_mastery_scales` → **403** `{"error": "..."}` for `mastery_points`/`ratings`/`calculation_*`. Success returns the updated `Outcome`.
- **`DELETE /outcomes/:id`** — soft-delete (`workflow_state='deleted'`), returns the deleted `Outcome` JSON. Outcome still linked elsewhere stays linked (Canvas deletes the outcome and its links; mirror soft-delete of both).
- **`GET /courses/:course_id/outcome_alignments`** — requires `assignment_id` XOR-ish `student_id` (either alone or both; **neither → 400** `{"message": "student_id or assignment_id is required"}`). With `assignment_id` only: caller needs `manage_grades` or `view_all_grades` on the course (**401** otherwise); returns `[{learning_outcome_id, title, assignment_id, submission_types, url}]`-shaped alignments. With `student_id`: student-self or grader; returns outcome rollups/alignments for the student. `ActiveRecord::RecordNotFound` → **404** with the message body.

### 5.3 `courses` deep gaps

- **`GET /accounts/:account_id/courses/:id`** — same contract as `GET /courses/:id` (below), resolved through the account.
- **`GET /courses/:id` include fan-out (extends the done route)** — `include[]` values to support: `syllabus_body` (gated by syllabus read), `term` (enrollment term object; terms table exists), `grading_periods` (gated), `course_progress` (`{requirement_completed_count, requirement_count, next_requirement_url, completed_at}`), `total_scores` (enrollment totals; suppressed when `hide_final_grades`), `needs_grading_count` (graders only), `permissions` (single-course only), `observers`, `concluded`, `course_image`, `banner_image`, `sections`, `storage_quota`. Post-FF extras can land later.
- **`GET /courses` index params** — `enrollment_type`, `enrollment_role`, `enrollment_role_id`, `enrollment_state`, `state[]`, `enrollment_term_id`, `search_term`, `include[]` (`syllabus_body`, `term`, `grading_periods`, `course_progress`, `observers`, `needs_grading_count`, `total_scores`, `sections`, `storage_quota`, `concluded`, `post_processing_assignments`, `favorites`, `teachers`, `course_image`, `banner_image`, `current_grading_period_scores`), `published`, `sort`, `order`, `exclude_blueprint_courses`, `homeroom_course_id`. `hide_final_grades` from account settings filters grade fields for student callers.
- **`GET /courses/:course_id/search_users`** — params `search_term`, `enrollment_type`, `enrollment_role`, `enrollment_role_id`, `enrollment_state`, `sort`, `order`, `include[]`, `user_id`, `include_inactive` (needs `read_as_admin`), `section_ids`, `differentiation_tag_id`. Returns paginated user JSON with optional embedded `enrollments`. Distinct from `users` only in defaults; implement as a shared handler.
- **`GET /courses/:course_id/permissions`** — returns `{ "<permission>": bool }` for each requested/computed right (`read`, `manage`, `read_roster`, `view_all_grades`, `manage_grades`, `update`, `delete`, `use_student_view`, ...). `read` on course required.
- **`GET /courses/:course_id/students`** — deprecated, unpaginated list of students visible to caller, ordered by sortable name.
- **`GET /courses/:course_id/recent_students`** — students with last login, ordered `last_login DESC NULLS LAST`, paginated, user JSON with `last_login`.
- **`GET /courses/:course_id/todo`** — current-user items needing submit (assignments/quizzes with due dates, ungraded included) and, for graders, needing grading; bookmarked by `(due_at, created_at, id)`.
- **`GET /courses/:course_id/users/:user_id/progress`** and **`/bulk_user_progress`** — course-progress DTO; bulk returns every enrolled user's progress (grader/`view_all_grades` or self).
- **`GET /courses/:course_id/student_view_student`** — returns (creating if needed) the course's Test Student user JSON; `use_student_view` required.
- **`POST /courses/:course_id/preview_html`** — body `html`; returns `{html: "<sanitized>"}`; group-side variant needs `manage` on the group.
- **`PUT /accounts/:account_id/courses` (batch_update)** — body `course_ids[]`, `event` in `offer|conclude|delete|undelete|publish|unpublish`; permission `manage_courses_admin` (`undelete_courses` for `undelete`); returns `{completion: <progress-id-or-percent>}`-style progress; unknown event → 400.
- **`POST /courses/:course_id/reset_content`** — resets a blueprint-associated course to its template state (deletes non-blueprint content); async; returns reset status.
- **`GET/POST /courses/:course_id/link_validation`** — POST starts an async validation job (202/200 with progress); GET returns the current result `{workflow_state, results: {broken_links_by_asset: {...}}, ...}` or `{} ` when not run. Depends on star-jobs.
- **`GET /courses/:course_id/effective_due_dates`** — per-assignment/section/student effective due dates (`{assignment_id: {user_id|"section_id"|"course_id": {due_at, grading_period_id}}}`); grader rights required.
- **`POST /courses/:course_id/restore/:version_id`** — restore a course snapshot; depends on course versions (deferred, see §4).
- **`youtube_migration` (5 routes)** — legacy YouTube-to-video migration scan/convert/status; FF-gated admin tool. Recommend stubbing with 404-on-disabled-FF first and full behavior behind the flag.
- **`POST /courses/:id/dismiss_migration_limitation_message`, `PUT /courses/:id/quizzes`** — FF-gated single-purpose setters; recommend deferring behind feature-flag stubs.

### 5.4 `groups` / `group_categories` tail

- **`GET /accounts/:account_id/groups`** — same contract as the done course `context_index` (`filter`, `user_id`, `collaboration_state`, `only_own_groups`, `include[]`, `page`), against account groups + account-level categories.
- **`GET /courses/:course_id/bulk_user_tags`** — returns all differentiation tags (non-collaborative categories) with their user memberships in bulk: `{tags: [...], user_tags: {user_id: [tag_id...]}}`-shape for the tags UI.
- **`POST /group_categories/:group_category_id/groups`** — create a group inside the category (name, members); `manage_groups_add` on the context.
- **`GET /groups/:group_id/activity_stream`** + **`/summary`** — group-scoped stream items for the current user (mirror the done course summary handler; stream tables exist via 0106).
- **`GET /groups/:group_id/permissions`** — `{can_manage, ...}` hash derived from `manage` on the group (mirror the courses settings permissions shape).
- **`POST /groups/:group_id/preview_html`** — as courses but `manage` on group.
- **`POST /groups/:group_id/files`** — step 1 of the group file upload contract (mirror the done course `create_file`: returns `{upload_url, upload_params}` / upload session); then `files` crate completes it.
- **`GET /group_categories/:group_category_id/users`** — users in the category's groups, `search_term` filter; for Course contexts forces `enrollment_type=student`; paginated user JSON.
- **`GET /group_categories/:group_category_id/export`** — groups CSV (columns: `group_name,group_id,group_category_id,login_name,user_id`); requires granular manage-groups; streams a CSV attachment.
- **`POST /group_categories/:group_category_id/import`** — CSV upload of the same shape; creates/updates groups and memberships; returns `{imported: <count>, missing: <count>, duplicated: <count>}`-style summary; validation errors → 400 with message list.
- **`GET /courses/:course_id/group_categories/export_tags`** / **`POST .../import_tags`** — same round-trip for differentiation tags (columns add tag/category metadata); granular manage-tags permission.
- **`POST /courses/:course_id/group_categories/bulk_manage_differentiation_tag`** — body is a bulk operation set (create/destroy tag memberships, create/delete tags); granular manage-tags.

---

## 6. Behavioral test plan per controller

Conventions (from `crates/star-outcomes/tests/canvas_parity.rs` and `crates/star-groups/tests/`): live parity tests gated by `STAR_PARITY_TESTS=1`, `reqwest::blocking` client, base `http://127.0.0.1:8080/api/v1`, login-helper fetching a bearer token, plus `*_integration.rs` suites in-crate. New suites: `star-outcomes/tests/outcomes_api_parity.rs`, `star-groups/tests/groups_tail_integration.rs`, `star-courses/tests/courses_deep_integration.rs`.

### 6.1 `star-outcomes` (outcome_groups_api + outcomes_api)

Integration (teacher/admin setup, student negatives):
1. `PUT /courses/:id/outcome_groups/:gid/outcomes/:oid` links an account outcome into a course group → 200 OutcomeLink; repeat link → 400.
2. `DELETE .../outcomes/:oid` unlinks → 200, group outcomes list no longer contains it.
3. `POST .../outcome_groups/:gid/import` with a valid non-root source in an associated account → 200, subtree copied (titles present under target); source root → 400; nonexistent source → 400.
4. `POST .../outcome_groups/:gid/subgroups` creates a child; listing subgroups returns it ordered by title.
5. `GET .../outcome_groups/account_chain` (teacher) returns ancestor root-group descriptors incl. `dontImport: true`; student → 401/403.
6. Global-context smoke: SA token on `/global/outcome_groups/:id` show/update + `root_outcome_group` redirect.
7. `GET/PUT/DELETE /outcomes/:id` — show returns ratings/mastery/calculation fields; PUT replacing ratings swaps the full set and defaults missing descriptions/points; DELETE soft-deletes and the outcome vanishes from lists; PUT with `mastery_points` under FF emulation → 403.
8. `GET /courses/:cid/outcome_alignments` with `assignment_id` (teacher) → alignments list; with neither param → 400 message; student without grader rights → 401.

Permission negatives: student on manage routes (create/update/destroy group, link/unlink, import, subgroups POST) → 401/403; unenrolled user on read routes → 401; non-site-admin on `/global/...` writes → 401/403.

### 6.2 `star-courses` (deep)

1. `GET /courses/:id?include[]=syllabus_body,term,grading_periods,permissions` returns the extra fields; student without syllabus right does not get `syllabus_body`; `needs_grading_count` absent for students.
2. `GET /courses?enrollment_state=active&include[]=term` filters and includes correctly; `hide_final_grades` account setting suppresses student `total_scores`.
3. `GET /courses/:cid/search_users?search_term=x&enrollment_type=TeacherEnrollment` — pagination + filtering; student → 401/403.
4. `GET /courses/:cid/permissions` — teacher gets `manage: true`; student `manage: false`.
5. `students`, `recent_students` (ordering `last_login DESC NULLS LAST`), `todo` (student sees needing-submit, teacher sees needing-grading), `user_progress` (self), `bulk_user_progress` (teacher), `student_view_student` (teacher 200; student 401/403).
6. `POST /courses/:cid/preview_html` sanitizes scripts out of returned `html`.
7. `PUT /accounts/:aid/courses` with `event=offer` on a course id list → progress payload; non-admin → 401/403.
8. `effective_due_dates` (teacher) returns the per-user map; student → 401/403.
9. Stubs: `link_validation` POST then GET returns pending/complete states; `youtube_migration` routes return flag-appropriate 404 when disabled.

### 6.3 `star-groups` (tail)

1. `GET /accounts/:aid/groups` — account groups + categories listed; `collaboration_state=non_collaborative` visible to teachers/admins only; students get collaborative-only even with `all`.
2. `bulk_user_tags` (teacher) returns tag + user-tag maps; student → 401/403.
3. `POST /group_categories/:gcid/groups` creates a group in the category; appears in `GET /group_categories/:gcid/groups`.
4. Group `activity_stream`, `activity_stream/summary`, `permissions`, `preview_html` — member reads, manager-only writes.
5. `POST /groups/:gid/files` returns the upload session; uploaded file appears in `GET /groups/:gid/files` (done route).
6. `GET /group_categories/:gcid/users?search_term=` — course context forces students; account context `read`.
7. CSV round-trip: `export` → parse CSV → `import` reproduces groups/memberships; `import` of malformed CSV → 400 with messages; `export_tags`/`import_tags` round-trip for tags; `bulk_manage_differentiation_tag` create+destroy cases.
8. SIS/storage quota quirk: `storage_quota_mb` silently ignored for callers without `manage_storage_quotas`; `sis_*` ignored without `manage_sis`.

---

## 7. Dependencies and open questions

Dependencies:
- `specs/p5-rest-platform.md` (accounts) — account context resolution for `/accounts/:account_id/{groups,outcome_groups,outcome_group_links}` and `account_chain` (needs `accounts.parent_account_id` confirmed).
- `specs/p5-rest-content.md` (files/content migrations) — group file upload step-2/3, link_validation result attachments, CSV export attachments.
- `specs/p5-rest-assessments.md` (submissions/assignments) — `effective_due_dates`, `todo` grading half, `outcome_alignments` assignment lookup.
- `specs/p5-ui-shell.md` — `account_chain` and `outcome_alignments` are UI-driven despite being undocumented in the public API docs.
- PR #16 (discussion_topics group-context/read-all) — owns group-context *discussion* routes; this spec's groups tail touches the same context-resolution helpers.
- `star-jobs` — link_validation, reset_content, batch_update progress reporting.
- Migration reservations protocol (issue #20): every schema need in §4 is "needs 010x (coordinator-assigned)"; this spec never self-assigns.

Open questions:
1. **Global outcomes context:** does `star-permissions` model `Account.site_admin` (a virtual root) for `read_global_outcomes`/`manage_global_outcomes`, or should global routes check a site-admin flag on the user? Affects all 11 `/global/...` routes.
2. **Import semantics:** Canvas `import` deep-copies the source subtree (groups + outcomes) into the destination context rather than linking. Confirm copy-depth (outcomes duplicated vs. linked) is acceptable for parity tests.
3. **`groups.non_collaborative` / `courses.hide_final_grades` / `accounts.parent_account_id`:** each may already exist in a crate-local schema path not covered by the grep of `star-db/migrations`; implementers must re-verify before requesting a coordinator number.
4. **`effective_due_dates` and `restore_version`** need assignment-override and course-version data that may not exist yet — propose splitting them into a follow-up issue if the tables are absent, rather than blocking the route group.
5. **Undocumented endpoints** (`account_chain`, `outcome_alignments` response details): behavior contracts here are read from controller code, not public API docs; treat the code as source of truth.
6. **youtube_migration:** legacy, feature-gated; recommend stub-level parity (404 when the tool's FF is off) and a follow-up for full scan/convert behavior.
