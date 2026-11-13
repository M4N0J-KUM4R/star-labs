# P5 REST Spec — Assessments Tail (submissions, moderation, audit, rubrics, peer reviews, quiz tail)

- **Issue**: #47 (epic `specs`) — "Spec extension: assessments tail"
- **Status**: COMPLETE spec, sections 1–8 plus the implementation-slicing appendix for #30/#31/#32
- **Scope**: every Canvas controller in the assessments family that is not yet at 100% REST parity: `submissions_api`, `gradebook_history_api`, `grade_change_audit_api`, `rubrics_api`, `peer_reviews_api` (+ legacy `assignments#peer_reviews` routes), `assignment_overrides`, `submission_comments_api`, `assignments_api` remainder, `quizzes/quiz_submissions_api`, `quizzes/quiz_reports`, `quizzes/quiz_groups`
- **Baseline**: measured at `main` `3dc34f6` (worktree `wt-i47`, branch `p5/specs-47`). Canvas source read from the local `canvas-lms` checkout; route inventory extracted from `migration/parity/canvas_manifest.json`; implemented-route status from `tools/parity/parity.py report` (regenerated for this spec).
- **Deliverable type**: documentation only. No Rust code, no migrations, no route registrations in this spec.

---

## 1. Overview and fresh coverage

The assessments family is the last large REST block with significant per-route permission and anonymity semantics. It splits into three conceptual halves:

1. **Grading surface** — submissions (core, anonymous, moderated/provisional), gradeable students, read states.
2. **Gradebook accountability** — gradebook history (submission versions per day/grader/assignment) and the grade-change audit trail.
3. **Assessment tail** — rubrics import/export, peer reviews, assignment override batch operations and aliases, submission comment remainder, quiz submission/report/group remainder.

Fresh coverage regenerated at `3dc34f6` with `python3 tools/parity/parity.py report` from `migration/`:

| Metric | Implemented | Total | Coverage |
|---|---:|---:|---:|
| Workspace REST routes | 471 | 1403 | 33.6% |
| UI-referenced REST routes | 258 | 503 | 51.3% |
| Star schema tables | 119 | 688 Canvas models | n/a |

Per-controller coverage for every controller in this spec:

| Controller | Done | Total | Coverage | Slice(s) |
|---|---:|---:|---:|---|
| `submissions_api` | 14 | 44 | 31.8% | A, E, F, B/G |
| `grade_change_audit_api` | 5 | 12 | 41.7% | B |
| `gradebook_history_api` | 0 | 4 | 0.0% | B |
| `rubrics_api` | 6 | 15 | 40.0% | C |
| `rubrics` (legacy, non-API) | 3 | 3 | 100.0% | C (landed) |
| `peer_reviews_api` | 4 | 9 | 44.4% | D |
| `assignment_overrides` | 5 | 10 | 50.0% | G |
| `submission_comments_api` | 3 | 4 | 75.0% | G |
| `assignments_api` | 5 | 12 | 41.7% | G |
| `quizzes/quiz_submissions_api` | 5 | 7 | 71.4% | G |
| `quizzes/quiz_reports` | 2 | 4 | 50.0% | G |
| `quizzes/quiz_groups` | 5 | 6 | 83.3% | G |
| `provisional_grades` | 0 | 4 | 0.0% | F |
| `anonymous_provisional_grades` | 0 | 1 | 0.0% | F |
| `moderation_set` | 0 | 2 | 0.0% | F |

Notes on the numbers:

- The issue's `submissions_api 14/44` is confirmed by the fresh run.
- Legacy peer review routes (`assignments#peer_reviews`, `#assign_peer_reviews`, `#remind_peer_review`/`#delete_peer_review` under `courses/:course_id/assignments/...`, non-`/api/v1`) are fully covered by `star-assignments` (3/3); they are tracked here for context only and do not appear in the parity manifest totals.
- `gradebook_history_api`, `provisional_grades`, `anonymous_provisional_grades`, and `moderation_set` are at zero — these are the primary blockers behind issues #30, #31 and #32.

---

## 2. Slice map A–G

| Slice | Name | Contents | Route count (todo/total) | Owning issue |
|---|---|---|---|---|
| **A** | Submissions core & multi-submission | index/show/update/for_students/create_file/bulk_update (assignment + course level)/submission_summary/gradeable_students/multiple_gradeable_students, submission read states (read/unread/bulk/clear), item + rubric + document-annotation read states, all `/sections/:section_id/...` twins | 30 todo / 44 (14 done); 2 of the 30 go to #30 (slice E) and 12 to #32 (slice B/G), leaving 16 | #32 (remainder) |
| **B** | Gradebook history & audit | 4 `gradebook_history_api` routes; 7 remaining `grade_change_audit_api` routes | 11 todo / 16 (5 done) | #32 |
| **C** | Rubrics | `rubrics_api` account-context routes, upload/upload_status/rubrics_by_import_id for both contexts | 9 todo / 15 (6 done) | future issue |
| **D** | Peer reviews | `peer_reviews_api` section twins + `allocate` (legacy 3/3 already landed) | 5 todo / 9 (4 done) | future issue |
| **E** | Anonymous submissions/grading | `show_anonymous`/`update_anonymous` section twins, `gradeable_students?allow_new_anonymous_id=true`, anonymized response semantics on the already-landed course routes | 2 todo routes + semantics on 3 landed routes | **#30** |
| **F** | Provisional / moderated grading | all 7 routes of `moderation_set`, `provisional_grades`, `anonymous_provisional_grades`; `submission[provisional]`/`submission[final]` params on submissions update; `include[]=provisional_grades` on submissions index/show and gradeable_students | 7 todo / 7 + param work on landed routes | **#31** |
| **G** | Tail | quiz submissions (`complete`, `time`), quiz reports (`show`, `abort`), quiz groups (`reorder`), assignment overrides batch + aliases (5), submission comments remainder (`annotation_notification`), assignments_api remainder (7) | 18 todo across 6 controllers | future issue (none of G is in #32; see appendix) |

Slice D is "mostly landed": the four course-context routes plus the legacy non-API trio are done; only the section twins and `allocate` remain. Slices E and F are almost entirely semantics (E) or greenfield (F).

---

## 3. Per-controller route inventory

Full route inventory for all 15 controllers, extracted from `migration/parity/canvas_manifest.json` (Canvas route inventory) and cross-referenced against a fresh `python3 tools/parity/parity.py report --manifest parity/canvas_manifest.json --star-src . --out <dir>` run at `3dc34f6` (137 route rows total). Status column: **done** = a matching Star route is registered; **todo** = no Star route yet. Slices reference §2.

Route-table conventions: `…` abbreviates the shared prefixes shown in each table header. The parity tool normalizes `/api/v1` away when matching Star registrations, so the Star side registers these as `/courses/{courseId}/assignments/{assignmentId}/submissions` etc. (see `crates/star-assignments/src/lib.rs` route table for the landed pattern).

### 3.1 `submissions_api` — 14/44 done (slices A, E, F, B/G)

Course-context prefix `…/courses/:course_id/assignments/:assignment_id`; section twins replace the course segment with `/sections/:section_id`. Canvas registers every course route except `gradeable_students`, `multiple_gradeable_students`, and the course-level `update_grades`/`bulk_mark_read`/`clear_unread` twins for sections too (routes.rb `[%w[course course], %w[section course_section]].each` loop).

| # | Verb | Route (suffix after course/section prefix) | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `/submissions` | index | done | A |
| 2 | GET | `/submissions/:user_id` | show | done | A |
| 3 | PUT | `/submissions/:user_id` | update | done | A |
| 4 | GET | `/anonymous_submissions/:anonymous_id` | show_anonymous | done | A |
| 5 | PUT | `/anonymous_submissions/:anonymous_id` | update_anonymous | done | A |
| 6 | POST | `/submissions/:user_id/files` | create_file | done | A |
| 7 | POST | `/submissions/update_grades` | bulk_update | done | A |
| 8 | GET | `/submission_summary` | submission_summary | done | A |
| 9 | GET | `…/courses/:course_id/students/submissions` | for_students | done | A |
| 10 | GET | `…/courses/:course_id/assignments/:assignment_id/gradeable_students` | gradeable_students | done | E |
| 11 | PUT | `…/courses/:course_id/assignments/:assignment_id/submissions/:user_id/read` | mark_submission_read | done | A |
| 12 | DELETE | `…/courses/:course_id/assignments/:assignment_id/submissions/:user_id/read` | mark_submission_unread | done | A |
| 13 | PUT | `…/courses/:course_id/submissions/bulk_mark_read` | mark_bulk_submissions_as_read | done | A |
| 14 | PUT | `…/courses/:course_id/submissions/:user_id/clear_unread` | submissions_clear_unread | done | A |
| 15 | GET | `/sections/:section_id/…/submissions` | index (section twin) | todo | B/G (#32) |
| 16 | GET | `/sections/:section_id/…/submissions/:user_id` | show (section twin) | todo | B/G (#32) |
| 17 | PUT | `/sections/:section_id/…/submissions/:user_id` | update (section twin) | todo | B/G (#32) |
| 18 | GET | `/sections/:section_id/…/anonymous_submissions/:anonymous_id` | show_anonymous (section twin) | todo | E (#30) |
| 19 | PUT | `/sections/:section_id/…/anonymous_submissions/:anonymous_id` | update_anonymous (section twin) | todo | E (#30) |
| 20 | POST | `/sections/:section_id/…/submissions/:user_id/files` | create_file (section twin) | todo | A |
| 21 | POST | `/sections/:section_id/…/submissions/update_grades` | bulk_update (section twin) | todo | A |
| 22 | GET | `/sections/:section_id/…/submission_summary` | submission_summary (section twin) | todo | A |
| 23 | GET | `/sections/:section_id/students/submissions` | for_students (section twin) | todo | A |
| 24 | PUT | `/sections/:section_id/…/submissions/:user_id/read` | mark_submission_read (section twin) | todo | A |
| 25 | DELETE | `/sections/:section_id/…/submissions/:user_id/read` | mark_submission_unread (section twin) | todo | A |
| 26 | PUT | `/sections/:section_id/…/submissions/:user_id/read/:item` | mark_submission_item_read (section twin) | todo | A |
| 27 | PUT | `/sections/:section_id/submissions/bulk_mark_read` | mark_bulk_submissions_as_read (section twin) | todo | A |
| 28 | PUT | `/sections/:section_id/submissions/:user_id/clear_unread` | submissions_clear_unread (section twin) | todo | A |
| 29 | GET | `/sections/:section_id/…/submissions/:user_id/rubric_assessments/read` | rubric_assessments_read_state (section twin) | todo | A |
| 30 | GET | `/sections/:section_id/…/submissions/:user_id/rubric_comments/read` | rubric_assessments_read_state (section twin) | todo | A |
| 31 | GET | `/sections/:section_id/…/submissions/:user_id/document_annotations/read` | document_annotations_read_state (section twin) | todo | A |
| 32 | PUT | `/sections/:section_id/…/submissions/:user_id/rubric_assessments/read` | mark_rubric_assessments_read (section twin) | todo | A |
| 33 | PUT | `/sections/:section_id/…/submissions/:user_id/rubric_comments/read` | mark_rubric_assessments_read (section twin) | todo | A |
| 34 | PUT | `/sections/:section_id/…/submissions/:user_id/document_annotations/read` | mark_document_annotations_read (section twin) | todo | A |
| 35 | GET | `…/courses/:course_id/assignments/:assignment_id/submissions/:user_id/read/:item` | mark_submission_item_read (item read) | todo | B/G (#32) |
| 36 | GET | `…/courses/:course_id/assignments/:assignment_id/submissions/:user_id/rubric_assessments/read` | rubric_assessments_read_state | todo | B/G (#32) |
| 37 | GET | `…/courses/:course_id/assignments/:assignment_id/submissions/:user_id/rubric_comments/read` | rubric_assessments_read_state | todo | B/G (#32) |
| 38 | PUT | `…/courses/:course_id/assignments/:assignment_id/submissions/:user_id/rubric_assessments/read` | mark_rubric_assessments_read | todo | B/G (#32) |
| 39 | PUT | `…/courses/:course_id/assignments/:assignment_id/submissions/:user_id/rubric_comments/read` | mark_rubric_assessments_read | todo | B/G (#32) |
| 40 | GET | `…/courses/:course_id/assignments/:assignment_id/submissions/:user_id/document_annotations/read` | document_annotations_read_state | todo | B/G (#32) |
| 41 | PUT | `…/courses/:course_id/assignments/:assignment_id/submissions/:user_id/document_annotations/read` | mark_document_annotations_read | todo | B/G (#32) |
| 42 | GET | `…/courses/:course_id/assignments/gradeable_students` | multiple_gradeable_students | todo | B/G (#32) |
| 43 | POST | `…/courses/:course_id/submissions/update_grades` | bulk_update (course-level) | todo | B/G (#32) |
| 44 | POST | `…/sections/:section_id/submissions/update_grades` | bulk_update (section-level) | todo | A |

Note on the 44 rows: they are exactly the manifest's 44 `submissions_api` (verb, path) pairs as Canvas routes.rb registers them — the 14 done course-context routes (rows 1–14) plus the 30 todo rows (rows 15–44). The non-API route `POST /courses/:course_id/assignments/:assignment_id/submissions` (`submissions#create`, a different controller) is NOT in this controller's manifest entry. Canvas generates the section twins via the routes.rb `[%w[course course], %w[section course_section]].each` loop, so every course route in the loop body has a section twin; the course-only routes are `gradeable_students` and `multiple_gradeable_students` (`/courses/:course_id/assignments/...` prefix only).

**The 30 todo routes, exactly** (each row of the table above marked todo, grouped here for readability; verified against the fresh parity run):

- Section twins of the 13 done course routes (13): index, show, update, show_anonymous, update_anonymous, create_file, assignment-level bulk_update, submission_summary, for_students, mark_submission_read, mark_submission_unread, mark_bulk_submissions_as_read, submissions_clear_unread.
- Item/rubric/document-annotation read states, course context (7): mark_submission_item_read (`read/:item` PUT), rubric_assessments_read_state GET (`rubric_assessments/read`), rubric_assessments_read_state GET (`rubric_comments/read`), mark_rubric_assessments_read PUT (`rubric_assessments/read`), mark_rubric_assessments_read PUT (`rubric_comments/read`), document_annotations_read_state GET, mark_document_annotations_read PUT.
- The same seven read-state routes under section context (7).
- Course-level `POST /courses/:course_id/submissions/update_grades` and section-level `POST /sections/:section_id/submissions/update_grades` (2).
- `GET /courses/:course_id/assignments/gradeable_students` (multiple_gradeable_students) (1).

Count: 13 + 7 + 7 + 2 + 1 = 30. The authoritative machine-readable list is the extraction script `/home/z/my-project/scripts/i47_route_inventory.py` logic (manifest routes minus the fresh-coverage done-set); the table above reproduces it verbatim.

### 3.2 `moderation_set` — 0/2 done (slice F, #31)

Prefix `…/courses/:course_id/assignments/:assignment_id`.

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `/moderated_students` | index | todo | F (#31) |
| 2 | POST | `/moderated_students` | create | todo | F (#31) |

### 3.3 `provisional_grades` — 0/4 done (slice F, #31)

Prefix `…/courses/:course_id/assignments/:assignment_id`.

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `/provisional_grades/status` | status | todo | F (#31) |
| 2 | PUT | `/provisional_grades/:provisional_grade_id/select` | select | todo | F (#31) |
| 3 | PUT | `/provisional_grades/bulk_select` | bulk_select | todo | F (#31) |
| 4 | POST | `/provisional_grades/publish` | publish | todo | F (#31) |

### 3.4 `anonymous_provisional_grades` — 0/1 done (slice F, #31)

Prefix `…/courses/:course_id/assignments/:assignment_id`.

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `/anonymous_provisional_grades/status` | status | todo | F (#31) |

### 3.5 `gradebook_history_api` — 0/4 done (slice B, #32)

Prefix `…/courses/:course_id/gradebook_history`.

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `/days` | days | todo | B (#32) |
| 2 | GET | `/:date` | day_details | todo | B (#32) |
| 3 | GET | `/:date/graders/:grader_id/assignments/:assignment_id/submissions` | submissions | todo | B (#32) |
| 4 | GET | `/feed` | feed | todo | B (#32) |

### 3.6 `grade_change_audit_api` — 5/12 done (slice B, #32)

Prefix `…/audit/grade_change`.

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `` (root) | query | done | B |
| 2 | GET | `/courses/:course_id` | for_course | done | B |
| 3 | GET | `/courses/:course_id/assignments/:assignment_id` | for_course_and_other_parameters | done | B |
| 4 | GET | `/courses/:course_id/graders/:grader_id` | for_course_and_other_parameters | done | B |
| 5 | GET | `/courses/:course_id/students/:student_id` | for_course_and_other_parameters | done | B |
| 6 | GET | `/assignments/:assignment_id` | for_assignment | todo | B (#32) |
| 7 | GET | `/students/:student_id` | for_student | todo | B (#32) |
| 8 | GET | `/graders/:grader_id` | for_grader | todo | B (#32) |
| 9 | GET | `/courses/:course_id/assignments/:assignment_id/graders/:grader_id` | for_course_and_other_parameters | todo | B (#32) |
| 10 | GET | `/courses/:course_id/assignments/:assignment_id/students/:student_id` | for_course_and_other_parameters | todo | B (#32) |
| 11 | GET | `/courses/:course_id/graders/:grader_id/students/:student_id` | for_course_and_other_parameters | todo | B (#32) |
| 12 | GET | `/courses/:course_id/assignments/:assignment_id/graders/:grader_id/students/:student_id` | for_course_and_other_parameters | todo | B (#32) |

### 3.7 `rubrics_api` — 6/15 done (slice C, future issue)

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `…/courses/:course_id/rubrics` | index | done | C |
| 2 | GET | `…/courses/:course_id/rubrics/:id` | show | done | C |
| 3 | GET | `…/courses/:course_id/rubrics/:id/used_locations` | used_locations | done | C |
| 4 | GET | `…/rubrics/upload_template` | upload_template | done | C |
| 5 | POST | `…/courses/:course_id/rubrics/download_rubrics` | download_rubrics | done | C |
| 6 | POST | `…/accounts/:account_id/rubrics/download_rubrics` | download_rubrics | done | C |
| 7 | GET | `…/accounts/:account_id/rubrics` | index | todo | C |
| 8 | GET | `…/accounts/:account_id/rubrics/:id` | show | todo | C |
| 9 | GET | `…/accounts/:account_id/rubrics/:id/used_locations` | used_locations | todo | C |
| 10 | GET | `…/courses/:course_id/rubrics/upload/:id` | upload_status | todo | C |
| 11 | GET | `…/accounts/:account_id/rubrics/upload/:id` | upload_status | todo | C |
| 12 | GET | `…/courses/:course_id/rubrics/upload/:id/rubrics` | rubrics_by_import_id | todo | C |
| 13 | GET | `…/accounts/:account_id/rubrics/upload/:id/rubrics` | rubrics_by_import_id | todo | C |
| 14 | POST | `…/courses/:course_id/rubrics/upload` | upload | todo | C |
| 15 | POST | `…/accounts/:account_id/rubrics/upload` | upload | todo | C |

### 3.8 `rubrics` (legacy CRUD under `/api/v1`) — 3/3 done (slice C, landed)

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | POST | `…/courses/:course_id/rubrics` | create | done | C (landed) |
| 2 | PUT | `…/courses/:course_id/rubrics/:id` | update | done | C (landed) |
| 3 | DELETE | `…/courses/:course_id/rubrics/:id` | destroy | done | C (landed) |

### 3.9 `peer_reviews_api` — 4/9 done (slice D, future issue; context only)

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `…/courses/:course_id/assignments/:assignment_id/peer_reviews` | index | done | D |
| 2 | GET | `…/courses/:course_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews` | index | done | D |
| 3 | POST | `…/courses/:course_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews` | create | done | D |
| 4 | DELETE | `…/courses/:course_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews` | destroy | done | D |
| 5 | GET | `…/sections/:section_id/assignments/:assignment_id/peer_reviews` | index (section twin) | todo | D |
| 6 | GET | `…/sections/:section_id/…/submissions/:submission_id/peer_reviews` | index (section twin) | todo | D |
| 7 | POST | `…/sections/:section_id/…/submissions/:submission_id/peer_reviews` | create (section twin) | todo | D |
| 8 | DELETE | `…/sections/:section_id/…/submissions/:submission_id/peer_reviews` | destroy (section twin) | todo | D |
| 9 | POST | `…/courses/:course_id/assignments/:assignment_id/allocate` | allocate | todo | D |

### 3.10 `assignment_overrides` — 5/10 done (slice G)

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `…/courses/:course_id/assignments/:assignment_id/overrides` | index | done | G |
| 2 | GET | `…/courses/:course_id/assignments/:assignment_id/overrides/:id` | show | done | G |
| 3 | POST | `…/courses/:course_id/assignments/:assignment_id/overrides` | create | done | G |
| 4 | PUT | `…/courses/:course_id/assignments/:assignment_id/overrides/:id` | update | done | G |
| 5 | DELETE | `…/courses/:course_id/assignments/:assignment_id/overrides/:id` | destroy | done | G |
| 6 | GET | `…/courses/:course_id/assignments/overrides` | batch_retrieve | todo | G |
| 7 | POST | `…/courses/:course_id/assignments/overrides` | batch_create | todo | G |
| 8 | PUT | `…/courses/:course_id/assignments/overrides` | batch_update | todo | G |
| 9 | GET | `…/groups/:group_id/assignments/:assignment_id/override` | group_alias | todo | G |
| 10 | GET | `…/sections/:course_section_id/assignments/:assignment_id/override` | section_alias | todo | G |

### 3.11 `submission_comments_api` — 3/4 done (slice G)

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | POST | `/courses/:course_id/assignments/:assignment_id/submissions/:user_id/comments/files` (non-`/api/v1`) | create_file | done | G |
| 2 | PUT | `…/api/v1/courses/:course_id/…/submissions/:user_id/comments/:id` | update | done | G |
| 3 | DELETE | `…/api/v1/courses/:course_id/…/submissions/:user_id/comments/:id` | destroy | done | G |
| 4 | POST | `/courses/:course_id/assignments/:assignment_id/submissions/:user_id/annotation_notification` (non-`/api/v1`) | annotation_notification | todo | G |

### 3.12 `assignments_api` — 5/12 done (slice G)

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `…/courses/:course_id/assignments` | index | done | G |
| 2 | GET | `…/courses/:course_id/assignment_groups/:assignment_group_id/assignments` | index | done | G |
| 3 | GET | `…/courses/:course_id/assignments/:id` | show | done | G |
| 4 | POST | `…/courses/:course_id/assignments` | create | done | G |
| 5 | PUT | `…/courses/:course_id/assignments/:id` | update | done | G |
| 6 | GET | `…/users/:user_id/courses/:course_id/assignments` | user_index | todo | G |
| 7 | GET | `…/courses/:course_id/assignments/:assignment_id/users/:user_id/group_members` | student_group_members | todo | G |
| 8 | POST | `…/courses/:course_id/assignments/:assignment_id/duplicate` | duplicate | todo | G |
| 9 | PUT | `…/courses/:course_id/assignments/bulk_update` | bulk_update | todo | G |
| 10 | POST | `…/courses/:course_id/assignments/:assignment_id/retry_alignment_clone` | retry_alignment_clone | todo | G |
| 11 | POST | `…/courses/:course_id/assignments/:assignment_id/accessibility/scan` | accessibility_scan | todo | G |
| 12 | POST | `…/courses/:course_id/assignments/:assignment_id/accessibility/queue_scan` | accessibility_queue_scan | todo | G |

### 3.13 `quizzes/quiz_submissions_api` — 5/7 done (slice G)

Prefix `…/courses/:course_id/quizzes/:quiz_id`.

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `/submissions` | index | done | G |
| 2 | GET | `/submission` | submission | done | G |
| 3 | GET | `/submissions/:id` | show | done | G |
| 4 | POST | `/submissions` | create | done | G |
| 5 | PUT | `/submissions/:id` | update | done | G |
| 6 | POST | `/submissions/:id/complete` | complete | todo | G |
| 7 | GET | `/submissions/:id/time` | time | todo | G |

### 3.14 `quizzes/quiz_reports` — 2/4 done (slice G)

Prefix `…/courses/:course_id/quizzes/:quiz_id`.

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `/reports` | index | done | G |
| 2 | POST | `/reports` | create | done | G |
| 3 | GET | `/reports/:id` | show | todo | G |
| 4 | DELETE | `/reports/:id` | abort | todo | G |

### 3.15 `quizzes/quiz_groups` — 5/6 done (slice G)

Prefix `…/courses/:course_id/quizzes/:quiz_id`.

| # | Verb | Route | Action | Status | Slice |
|---|---|---|---|---|---|
| 1 | GET | `/groups` | index | done | G |
| 2 | GET | `/groups/:id` | show | done | G |
| 3 | POST | `/groups` | create | done | G |
| 4 | PUT | `/groups/:id` | update | done | G |
| 5 | DELETE | `/groups/:id` | destroy | done | G |
| 6 | POST | `/groups/:id/reorder` | reorder | todo | G |

### 3.16 Row-count reconciliation

Sum of the 15 controller tables: 44 + 2 + 4 + 1 + 4 + 12 + 15 + 3 + 9 + 10 + 4 + 12 + 7 + 4 + 6 = **137 route rows** (the manifest's exact totals per controller). Done rows: 14 + 0 + 0 + 0 + 0 + 5 + 6 + 3 + 4 + 5 + 3 + 5 + 5 + 2 + 5 = **57**; todo rows: **80** (both verified against the fresh parity run's implemented-route set for these 15 controllers). The §2 slice map distributes those 80 todo rows across slices A–G plus per-route semantic work on already-landed routes (slices E and F).

---

## 4. Permission matrices

All checks below are transcribed from the Canvas controllers read for this spec (`app/controllers/{...}_controller.rb` and `app/models/abstract_assignment.rb` in the local `canvas-lms` checkout). Star names follow the existing `star-permissions` crate conventions; where a Canvas right is course-scoped, Star evaluates it against the course context reached from the route (section twins additionally scope enrollment visibility to the section, per Canvas's `get_course_from_section` + `apply_enrollment_visibility(..., section_ids)`).

### 4.1 Grading-surface gates (`submissions_api`)

| Route(s) | Canvas check | Star equivalent |
|---|---|---|
| `index`, `submission_summary`, `gradeable_students`, `multiple_gradeable_students` | `authorized_action(@context, @current_user, [:manage_grades, :view_all_grades])` | teacher-ish: `manage_grades` OR `view_all_grades` on the course |
| `for_students` | `grants_any_right?(:manage_grades, :view_all_grades)` to widen to "all"/arbitrary ids; otherwise observer-scope: active observer enrollments' `associated_user_id`s plus self if `read_grades`; non-visible requested id → 401 | same tri-level logic (teacher / observer / student-self) |
| `show` | `authorized_action(@submission, :read)` **and** (`read_as_admin` OR `manage_grades` OR `assignment_visible_to_user?`) | submission-level read + assignment visibility |
| `show_anonymous`, `update_anonymous` | same as `show`/`update` after resolving the submission by `anonymous_id` (no separate right; the anonymous id is unguessable) | reuse show/update handler with `anonymize_user_id` response flag set |
| `update` | `params[:submission]` or `rubric_assessment` present → `authorized_action(@submission, :grade)`; comment-only → `:comment`. On a moderated, unpublished assignment, a non-provisional `posted_grade` is rejected with 401 (`render_unauthorized_action`) | keep the 401-on-final-grade-without-moderation-right behavior (slice F) |
| `create_file` | `:submit` on the assignment when uploading for self; `:grade` when `@user != @current_user`; `check_limited_access_for_students` before_action | permission depends on target user |
| `bulk_update` (assignment-level, course-level, section-level) | all target assignments published AND course `:manage_grades`; unknown assignment ids leak only after the right is confirmed (400 "Some assignments could not be found") | same two-step ordering |
| `mark_submission_read`/`unread`, `mark_bulk_submissions_as_read` | implicit: only the submission owner's own rows are touched (`submission&.user_id == @current_user.id`) | owner check inside handler, 204 otherwise |
| `mark_submission_item_read` (`read/:item`) | `authorized_action(@submission, @current_user, :mark_item_read)`; item ∈ {`grade`, `comment`, `rubric`} | submission-level `mark_item_read` |
| `rubric_assessments_read_state` (GET), `document_annotations_read_state` (GET) | `authorized_action(@submission, @current_user, :read)` | submission read |
| `mark_rubric_assessments_read` (PUT), `mark_document_annotations_read` (PUT) | hard `@user == @current_user` or 401 (only the assessed student) | owner-only |
| `submissions_clear_unread` | `authorized_action(Account.site_admin, @current_user, :manage_students)` — site-admin-only | site admin gate |

### 4.2 Moderated / provisional grading (slices F, `moderation_set`, `provisional_grades`, `anonymous_provisional_grades`)

Canvas `AbstractAssignment#permits_moderation?(user)`:

```ruby
def permits_moderation?(user)
  return false unless user
  final_grader_id == user.id || context.account_membership_allows(user, :select_final_grade)
end
```

| Route(s) | Canvas check |
|---|---|
| `moderation_set#index`, `#create` | `@assignment.permits_moderation?(@current_user)`; `create` additionally: blank `student_ids` → 400; students filtered to `@context.students_visible_to(@current_user, include: :inactive)` |
| `provisional_grades#select`, `#bulk_select` | `permits_moderation?` |
| `provisional_grades#publish` | `@context.grants_right?(:manage_grades)` **AND** `permits_moderation?`; then 400 unless `moderated_grading?`; 400 if `grades_published?` |
| `provisional_grades#status`, `anonymous_provisional_grades#status` | `[:manage_grades, :view_all_grades]` on the course; then 400 unless `moderated_grading?`; 400 if `grades_published?`; returning the full grade list further requires `permits_moderation?` (401 otherwise) |
| `submissions update` with `submission[final]=true` | `value_to_boolean(final) && permits_moderation?` — the final flag is silently downgraded to false without the right; but a bare `posted_grade` without `provisional` on an unpublished moderated assignment is a 401 |
| `include[]=provisional_grades` on submissions index/show and `gradeable_students` | grading data included only when `assignment.moderated_grading?`; on `gradeable_students` the include additionally requires `permits_moderation?` (401 otherwise) |

Related display gates (all from `abstract_assignment.rb`):

- `anonymize_students?` — false unless `anonymous_grading?`; for moderated assignments, anonymize only while `!grades_published?`; otherwise anonymize iff any active student has an unposted submission (`unposted_anonymous_submissions?`).
- `can_view_student_names?(user)` — false if `anonymize_students?`; otherwise `grants_any_right?(user, :manage_grades, :view_all_grades)`.
- `can_view_other_grader_identities?(user)` — requires `manage_grades`/`view_all_grades`; then true unless moderated-and-unpublished, in which case: final grader sees names iff `grader_names_visible_to_final_grader`; `select_final_grade` admins always see names; other graders see names only if `grader_comments_visible_to_graders` and not `graders_anonymous_to_graders`.
- `can_be_moderated_grader?(user)` (used by `status`) — `manage_grades`/`view_all_grades`, and if the grader limit is reached, only the final grader or an existing moderation grader.

### 4.3 Gradebook history & audit (slice B)

| Route(s) | Canvas check |
|---|---|
| all 4 `gradebook_history_api` routes | `before_action :require_manage_grades` → `authorized_action(@context, @current_user, :manage_grades)` on the course — all four routes, no exceptions |
| `grade_change_audit_api#for_assignment` | `course_authorized?(@assignment.course)` = `grants_any_right?(:manage_grades, :view_all_grades)`; assignment's root account must match the request's domain account, else 404 |
| `#for_course`, `#for_course_and_other_parameters` (all 7 combos) | `course_authorized?(course)`; unknown course → 404 raised |
| `#for_student`, `#for_grader` | `admin_authorized?` = root-account `:view_grade_changes` (admin-only) |
| `#query` (root) | branch-dependent: with `course_id`/`assignment_id` present → `course_authorized?`; with neither → `admin_authorized?` |

### 4.4 Assessment tail (slices C, D, G)

| Controller / route(s) | Canvas check |
|---|---|
| `rubrics_api` all routes except `upload_template` | `authorized_action(@context, :manage_rubrics)` |
| `rubrics_api#download_rubrics` | `authorized_action(@context, [:read_rubrics, :manage_rubrics])` (either right suffices) |
| `rubrics_api#upload_template` | none beyond authentication (context-free GET) |
| `peer_reviews_api#index` | if `@assignment.grants_any_right?(:grade)` → all requests; otherwise only requests where the caller is the assessee |
| `peer_reviews_api#create`, `#destroy` | `authorized_action(@assignment, @current_user, :grade)`; `create` also 400s when reviewer == student |
| `peer_reviews_api#allocate` | assignment visibility for the caller (`AssignmentVisibility` check), else 403 (self-allocation, not teacher-driven) |
| `assignment_overrides` create/update/destroy | `authorized_action(@assignment, :update)` (`require_assignment_edit`) |
| `assignment_overrides` `batch_create`/`batch_update` | `authorized_action(@course, :manage_assignments_edit)` (`require_all_assignments_edit`) |
| `assignment_overrides#batch_retrieve` | `authorized_action(@course, :read)` (`require_course`) |
| `assignment_overrides#group_alias`, `#section_alias` | same `require_course`/`require_assignment` chain — read-level; 404 when no override exists for the group/section |
| `submission_comments_api#update` | `authorized_action(submission_comment, :update)` (comment author or editor) |
| `submission_comments_api#destroy` | `authorized_action(submission_comment, :delete)` |
| `submission_comments_api#create_file` | `authorized_action?(@assignment, :submit)` / `:grade` depending on target user (same shape as submissions `create_file`) |
| `submission_comments_api#annotation_notification` | `authorized_action?(Account.site_admin, :send_messages)` — site-admin-only broadcast helper |
| `assignments_api#user_index` | `require_user_visibility` (self/observer/admin visibility of the target user) |
| `assignments_api#student_group_members` | `authorized_action(assignment, :read)`; student resolved from `@context.students` |
| `assignments_api#duplicate` | `:create` on the source assignment; `:manage_assignments_add` on a target course when cross-course |
| `assignments_api#bulk_update` | course `:manage_assignments_edit`, then per-assignment `user_can_update?` |
| `assignments_api#accessibility_scan`, `#queue_scan` | granular manage-course-content rights AND `a11y_checker_enabled?` on the account |
| `assignments_api#retry_alignment_clone` | follows `duplicate`'s gates (alignment-copy retry path) |
| `quizzes/quiz_reports` index/show/create/abort | `authorized_action(@quiz, :read_statistics)` — all four |
| `quizzes/quiz_submissions_api#index` | `manage_grades`/`view_all_grades` sees all; submit-only users see their own |
| `…#submission` | `@quiz.grants_right?(:submit)` |
| `…#show` | `authorized_action(@quiz_submission, :read)` |
| `…#create` | implicit via `prepare_service` + module-lock/access-code checks (student flow); preview path for teachers |
| `…#update` | teacher regrade flow (`fudge_points`, per-question scores) — documented 403 when not a teacher |
| `…#complete` | validation-token check (403 on invalid token/access code/IP filter) |
| `…#time` | `authorized_action(@quiz_submission, :record_events)` |
| `quizzes/quiz_groups#reorder` | `authorized_action(@quiz, :update)` |

---

## 5. Data model and migration needs

### 5.1 What already exists (no new migration needed)

| Migration | Provides | Used by |
|---|---|---|
| `0080_submission_read_states.sql` | `submissions.anonymous_id` (+index), `assignments.anonymous_id`, `assignments.anonymous_grading`, `submission_reads.read_at`/`.source` | slices A/E (anonymous submissions, read states, `clear_unread`, `bulk_mark_read`), slice F status/JSON `anonymous_id` fields |
| `0092_grade_change_audit.sql` | `grade_change_events` (uuid, course/assignment/student/grader ids, before/after grade+score, `graded_anonymously`, `version_number`, `request_id`, time-descending indexes per filter column) | slice B audit routes — all 12 can be served from this table; the 7 todo routes only add query combinations and the admin/student/grader entry points |
| `0104_peer_reviews.sql` | `assessment_requests` (assessor/assessee/asset, workflow_state, unique(asset, assessor)) | slice D; peer-review index/create/destroy and (with the allocation service) `allocate` |

Also existing from earlier migrations: `submission_reads` itself (0060, extended by 0080), `rubrics`/`rubric_criteria` (0094), quiz statistics (0102). The `assignments` table already carries the flags needed for moderated grading decisions except those in §5.2.

**Migration reservation protocol**: new numbers are assigned by the coordinator via `orchestration/migration-reservations.json` (currently `max_merged: 0107`, `next_available: 0108`, `active: {}`). Agents never self-assign. The implementing issues (#30/#31/#32) must request a reservation at dispatch time; this spec does not reserve numbers.

### 5.2 New tables required (additive only)

Three tables back slice F. Canvas models: `ModeratedGrading::ProvisionalGrade` (`app/models/moderated_grading/provisional_grade.rb`), `ModeratedGrading::Selection`, `ModerationGrader`.

**`provisional_grades`** (Canvas `ModeratedGrading::ProvisionalGrade`):

| Column | Type / constraint | Notes |
|---|---|---|
| `id` | BIGSERIAL PK | |
| `submission_id` | BIGINT NOT NULL FK → submissions | `belongs_to :submission` |
| `scorer_id` | BIGINT NOT NULL FK → users | `belongs_to :scorer`; `validates :scorer, presence: true` |
| `score` | NUMERIC(10,2) | |
| `grade` | TEXT | |
| `graded_at` | TIMESTAMPTZ | null when the provisional grade is comment-only |
| `final` | BOOLEAN NOT NULL DEFAULT FALSE | the moderator's final provisional grade |
| `source_provisional_grade_id` | BIGINT FK → provisional_grades | for the final grade's provenance |
| `grade_matches_current_submission` | BOOLEAN | |
| `created_at`/`updated_at` | TIMESTAMPTZ | |

Index: `(submission_id)`; consider `(scorer_id)`. Grade attributes surface in `provisional_grade_json` as `provisional_grade_id`, `score`, `grade`, `grade_matches_current_submission`, `graded_at`, `final`, `speedgrader_url`.

**`moderated_grading_selections`** (Canvas `ModeratedGrading::Selection`; API name "moderation set"):

| Column | Type / constraint | Notes |
|---|---|---|
| `id` | BIGSERIAL PK | |
| `assignment_id` | BIGINT NOT NULL FK → assignments | `belongs_to :assignment` |
| `student_id` | BIGINT NOT NULL FK → users | `belongs_to :student`; `validates :student_id, uniqueness: { scope: :assignment_id }` |
| `selected_provisional_grade_id` | BIGINT FK → provisional_grades | the chosen grade; null until selected |

Index: unique `(assignment_id, student_id)`; `(selected_provisional_grade_id)`.

**`moderation_graders`** (Canvas `ModerationGrader` — grader slots for limited moderated grading):

| Column | Type / constraint | Notes |
|---|---|---|
| `id` | BIGSERIAL PK | |
| `assignment_id` | BIGINT NOT NULL FK → assignments | |
| `user_id` | BIGINT NOT NULL FK → users | `validates :user, uniqueness: { scope: :assignment_id }` |
| `anonymous_id` | TEXT | grader-side anonymous id (`grader_ids_to_anonymous_ids`) |
| `slot_taken` | BOOLEAN NOT NULL DEFAULT FALSE | |
| `created_at`/`updated_at` | TIMESTAMPTZ | |

Index: unique `(assignment_id, user_id)`; `(anonymous_id)`.

**Assignments additions** for slice F: `final_grader_id` (FK → users, nullable), `grader_names_visible_to_final_grader` (BOOLEAN), `grader_comments_visible_to_graders` (BOOLEAN), `graders_anonymous_to_graders` (BOOLEAN), `moderated_grading` (BOOLEAN), `grades_published_at` (TIMESTAMPTZ). Check whether `moderated_grading`/`grades_published_at` already exist in the snapshot's `assignments` table before adding (`ALTER TABLE ... ADD COLUMN IF NOT EXISTS` is the established pattern in this repo). One candidate extension `moderated_grading_max_grader_count` (INTEGER) is referenced by `moderated_grader_limit_reached?`; include if the snapshot models the moderation grader limit.

### 5.3 Routes needing no schema change

- All slice B routes: `gradebook_history_api` reads submission **versions**. The snapshot already versions submissions for the gradebook/history family — verify the `versions`/submission-history mechanism used by the landed gradebook (`star-assignments` `gradebook` handler) covers `version_id`-ordered pagination; if Star's submission history table lacks a `version_id` monotonically increasing key, a small additive migration (new index or column) may be needed and must be requested through the reservation ledger.
- Slice E: purely permission/serialization semantics on top of 0080 columns.
- Slices C, D, G: `rubrics_api` upload/import needs a `rubric_imports` table (Canvas `RubricImport`: context, user, attachment, workflow_state, import_id) — that belongs to the future rubrics issue, noted here for completeness. Quiz tail, overrides batch/aliases, assignments remainder, and submission-comments `annotation_notification` need no new tables (progress objects already exist for async jobs).

---

## 6. Behavior contracts

### 6.1 Anonymous grading display rules (slice E, #30)

Contract sources: `submissions_api_controller#show_anonymous/#update_anonymous/#gradeable_students`, `lib/api/v1/user.rb#anonymous_user_display_json`, `lib/api/v1/submission.rb` (`submission_json`, `submission_attempt_json`), `abstract_assignment.rb` gates in §4.2.

1. **Addressing by anonymous id.** `show_anonymous`/`update_anonymous` resolve the submission via `@assignment.submissions.find_by!(anonymous_id: params[:anonymous_id])` — a miss is a 404, and no oracle exists to enumerate ids. Both set an internal `anonymize_user_id` flag that propagates into every serialized submission in the response.
2. **`anonymize_user_id` response semantics.** With the flag set (or when the caller holds account-level membership, `context.account_membership_allows(current_user)`): the top-level submission JSON drops `user_id` and adds `anonymous_id`; in attempt/history JSON (`submission_attempt_json`) the same swap applies per version (`json_fields -= ["user_id"]; json_fields << "anonymous_id"`). The `anonymous_id` field is documented as "only included if the caller has administrator access for the current account" — implement exactly this dual rule.
3. **`html_url` under anonymization.** When `assignment.anonymize_students?` is true, `html_url` points at the SpeedGrader-by-anonymous-id URL (`.../gradebook/speed_grader?...&anonymous_id=...`) instead of the user-addressed submission URL.
4. **`gradeable_students?allow_new_anonymous_id=true`** (already landed, needs semantics hardening for #30):
   - Response shape switches from `UserDisplay` to `AnonymousUserDisplay`: `{ anonymous_id, avatar_image_url (default fallback), display_name }`, where `display_name` comes from `assignment.anonymous_student_identities[user_id][:name]` when present.
   - The gate is **not** "anonymous_grading flag on": it is `allow_new_anonymous_id ? @assignment.can_view_student_names?(@current_user) : true`. `can_view_student_names?` is false when `anonymize_students?` (which for moderated assignments means grades-not-yet-published, and otherwise means any-unposted-anonymous-submission-exists). So the caller sees names only when Canvas would show them.
   - **Sort semantics**: with `sort=name`, Canvas cannot sort by real name when names are hidden, so it falls back to `ORDER BY anonymous_id` (`submission_scope.order("anonymous_id #{order}")`); default (no `sort`) orders by `user_id` when names are visible and `anonymous_id` when hidden. The current Star implementation orders by `u.name` — must switch to `sortable_name` semantics and the anonymous fallback.
   - `include[]=provisional_grades` on this route is gated by `permits_moderation?` (401 otherwise) — see §6.2.
5. **Anonymous graders.** In provisional-grade JSON, when `can_view_other_grader_identities?` is false, `scorer_id` is replaced by `anonymous_grader_id = assignment.grader_ids_to_anonymous_ids[scorer_id]` (backed by `moderation_graders.anonymous_id`), and rubric assessments are omitted from the payload. `grader_id` on the submission itself may be negative for autograded quizzes (negative quiz id) or LTI tools (negative tool id) — preserve that convention.
6. **Audit-side anonymization** (slice B): events whose assignment is `anonymous_grading? && muted?` get `student_id` nulled in course-scoped audit responses (`anonymize_events`); `for_student`-style responses drop such events entirely (`remove_anonymous_events`). See §6.3.

### 6.2 Provisional grade lifecycle (slice F, #31)

Contract sources: `provisional_grades_controller.rb`, `anonymous_provisional_grades_controller.rb`, `provisional_grades_base_controller.rb`, `moderation_set_controller.rb`, `submissions_api_controller#update/#gradeable_students`, `lib/api/v1/submission.rb#submission_provisional_grades_json`.

1. **Creation** — provisional grades are created via the ordinary submissions update route (`PUT .../submissions/:user_id` or the anonymous twin) with `submission[provisional]=true`. `final` is honored only when `permits_moderation?` holds (`submission[:final] = value_to_boolean(...) && permits_moderation?` — silently downgraded otherwise). A non-provisional `posted_grade` on an unpublished moderated assignment is rejected 401 before any grading happens.
2. **Moderation set membership** — `POST .../moderated_students` with `student_ids[]`: 400 on blank list; only ids visible as students (incl. inactive) are returned/created; creation is incremental (existing selections kept). `GET .../moderated_students` returns plain user JSON ordered by id, paginated.
3. **Selection** — `PUT .../provisional_grades/:provisional_grade_id/select`: 400 `{message: "student not in moderation set"}` when the submission's student has no selection row; on success returns `{assignment_id, student_id, selected_provisional_grade_id}` with `student_id` swapped for `anonymous_id` when `can_view_student_names?` is false. `PUT .../provisional_grades/bulk_select` takes `provisional_grade_ids[]`, updates each student's selection in one transaction, returns an array of the same selection shape (only changed selections are included), touches affected submissions (SpeedGrader live-reload), and emits a moderation event per changed selection.
4. **Status polling** — `GET .../provisional_grades/status?student_id=` (and the `anonymous_provisional_grades/status?anonymous_id=` twin, which differs only in how the submission is located): returns `{needs_provisional_grade: <bool>}` = `can_be_moderated_grader?(current_user)`. With `last_updated_at` present and equal to the submission's `updated_at`, that short-circuit response is the whole payload. Otherwise (and only when `permits_moderation?`, else 401) the response embeds `provisional_grades[]` and `final_provisional_grade`, each entry carrying `selected` (matches the moderation-set selection) and `readonly` (`!final && scorer != current_user`), plus `scorer_name` only when `can_view_other_grader_identities?`. Non-moderated → 400; already published → 400.
5. **Publishing** — `POST .../provisional_grades/publish` (irreversible): requires `manage_grades` + `permits_moderation?`; 400 when not `moderated_grading?` or already `grades_published?`. Per submission: use the selected provisional grade; else, if exactly one graded provisional grade exists, use it; else the single ungraded (comment-only) provisional grade; else 422 `{message: "All submissions must have a selected grade"}`. On success: publish each grade (skipping per-submission score recompute), recompute student scores once for the course, set `assignments.grades_published_at`, respond `{message: "OK"}`.
6. **Reading provisional grades** — `include[]=provisional_grades` on submissions index/show is only honored for `moderated_grading?` assignments; the list is filtered by role: moderators see all provisional grades (final sorted last), other graders only their own. On `gradeable_students` the include requires `permits_moderation?` and adds `in_moderation_set`, `selected_provisional_grade_id`, and the `provisional_grades` array per student.

### 6.3 Gradebook history query semantics (slice B, #32)

Contract source: `gradebook_history_api_controller.rb` + `lib/api/v1/gradebook_history.rb`.

1. `GET .../gradebook_history/days` — for every graded submission in the course (`graded_at` not null), group by calendar **day in the course's time zone** (`graded_at.in_time_zone.to_date`); each day entry lists graders (by grader id) and, per grader, the assignment ids they graded that day; result sorted by date descending. Autograded submissions count under grader id 0's bucket via the `grader_id <= 0` rule.
2. `GET .../gradebook_history/:date` — same shape scoped to one day (`graded_at` within `beginning_of_day..end_of_day` in course TZ); `:date` parses as `%Y-%m-%d`.
3. `GET .../gradebook_history/:date/graders/:grader_id/assignments/:assignment_id/submissions` — all submission **versions** for that grader/assignment/day, grouped per submission (`{submission_id, versions: [...]}`). Only versions whose `score` changed (or the first scored version) are kept; each carries `previous_*` (prior scored version's grade/score/graded_at/grader) and `new_*` aliases; versions are returned newest-first within each submission.
4. `GET .../gradebook_history/feed` — paginated, **uncollated** submission versions for the course, optionally filtered by `assignment_id`/`user_id`, ordered by `version_id` descending unless `ascending=true`; these `SubmissionVersion` objects carry plain `grade`/`graded_at`/`grader`, not the `new_`/`previous_` collated fields.
5. All four require course `manage_grades` (§4.3); all lists paginate via the standard Canvas page/Link-header protocol.

### 6.4 Override date/section semantics (slice G)

1. **Batch retrieve** (`GET .../assignments/overrides` with `assignment_overrides[][{id, assignment_id}]`): 400 unless every entry has both keys; unknown assignments are silently dropped (nulls in the reordered response), and the response preserves **request order**.
2. **Batch create/update**: transactional — all-or-nothing; per-entry errors surface in an `errors` array aligned to inputs. Specificity rule on create: `student_ids` > `group_id` > `course_section_id`; more than one present → the most specific wins silently.
3. **Update semantics**: for non-adhoc overrides, omitted date fields **clear** the override for that date; for adhoc (student) overrides, omitted `student_ids` leaves the set unchanged (documented Canvas quirk — preserve it).
4. **Aliases**: `GET /groups/:group_id/assignments/:assignment_id/override` and `GET /sections/:course_section_id/assignments/:assignment_id/override` respond **302 redirect** to the canonical override URL when one exists, 404 otherwise. They never render a body.
5. **Date fields**: `due_at`, `unlock_at`, `lock_at` per override; `all_day`/`all_day_date` are derived on save in Canvas and appear in override JSON.

### 6.5 Quiz report generation states (slice G)

Contract source: `quizzes/quiz_reports_controller.rb`.

1. `POST .../reports` (`report_type` = `student_analysis` | `item_analysis`, optional `include[]=[progress]`, `quiz[...]` filters) starts asynchronous CSV generation via a Progress object; the report JSON embeds `progress_url` and, once finished, `file_url`.
2. `GET .../reports/:id` returns the report with its generation state (progress workflow: `queued` → `running` → `completed`/`failed`) and download links when the CSV exists.
3. `DELETE .../reports/:id` (action `abort`) is two-cased: if the CSV was already generated, destroy the attachment (and its progress) permanently; if generation never started, 422 "report is not being generated"; if underway but not abortable, 422 "report generation can not be aborted"; otherwise abort in-flight generation. Success → 204.
4. `POST .../quizzes/:quiz_id/groups/:id/reorder` (quiz groups) takes `order[]` of question ids and reorders within the group; 204 on success; requires quiz `:update`.
5. Quiz submission `complete` requires the latest `attempt` and the `validation_token` issued at creation (403 on mismatch, 400 when already complete or attempt is stale); `time` returns `{end_at, time_left}` under `:record_events`.

---

## 7. Test plan (per controller, permission negatives included)

Each crate follows the existing pattern: unit tests beside handlers plus DB-backed integration tests where a `DATABASE_URL` is available, and (optionally, gated by `STAR_PARITY_URL`) live Canvas-parity comparisons. The negatives below are the ones most likely to regress; every route also needs its happy path, pagination behavior, and error-shape assertions.

**`submissions_api` (slices A/E/F, #30/#31/#32)**
- Section twins: teacher restricted to section A cannot list/show/update section-B students via `/sections/:section_id/...` (404/401 per Canvas behavior); same handler reuse for course and section paths.
- Anonymous twins (#30): 404 on unknown `anonymous_id`; response drops `user_id`, exposes `anonymous_id`; `html_url` points at the SpeedGrader anonymous URL when `anonymize_students?`.
- `gradeable_students` (#30): with `allow_new_anonymous_id=true` + names hidden → `AnonymousUserDisplay` shape and `anonymous_id` ordering (incl. `sort=name` fallback); names visible → `UserDisplay` + `sortable_name`/`user_id` ordering. Negative: student caller → 401.
- `for_students` negatives: student requesting another student's ids → 401; observer outside scope → 401; `student_ids=all` as submit-only user → limited set, not 401.
- `update` negatives: comment-only as unprivileged user → 401; non-provisional `posted_grade` on unpublished moderated assignment → 401; `submission[final]=true` without `permits_moderation?` → silently non-final (assert no final row created).
- Read states: only submission owner's rows affected; `mark_rubric_assessments_read`/`mark_document_annotations_read` as non-owner → 401; `submissions_clear_unread` as non-site-admin → 401; `read/:item` with invalid item → 400.
- `bulk_update`: unpublished target assignment → 401; missing `grade_data` → 400; unknown assignment ids → 400 with the id list (and only after the right is confirmed — assert a non-teacher gets 401 without the id leak).
- `multiple_gradeable_students` (#32): per-user `assignment_ids` reflect visibility; student caller → 401.

**`moderation_set` / `provisional_grades` / `anonymous_provisional_grades` (slice F, #31)**
- Negatives: non-final-grader without `select_final_grade` on `index`/`create`/`select`/`bulk_select`/`publish` → 401; `publish` without `manage_grades` → 401; `status` by plain grader → only `needs_provisional_grade`, 401 when the full list is requested implicitly via changed `last_updated_at`; non-moderated assignment → 400; already-published → 400.
- `create` with blank `student_ids` → 400 and `[]`; incremental create keeps prior selections.
- `select` for a student outside the moderation set → 400 "student not in moderation set".
- `publish` 422 when a submission has neither a selection nor exactly one graded/comment-only provisional grade; verify `grades_published_at` set, scores recomputed once, irreversibility (second publish → 400).
- Anonymity: with names hidden, selection JSON carries `anonymous_id` instead of `student_id`; provisional grades of other scorers show `anonymous_grader_id`.

**`gradebook_history_api` (slice B, #32)** — all 4 routes: non-grader (`view_all_grades` only) → 401; student → 401. `days` buckets by course-TZ day; `:date` with a bad format → error; `submissions` only reports score-changing versions with `previous_*` populated; `feed` paginates by `version_id` desc and honors `ascending`; filters `assignment_id`/`user_id`.

**`grade_change_audit_api` (slice B, #32)** — negatives: `for_student`/`for_grader` as non-admin → 401; `query` with no conditions → 400; `query` with neither course nor assignment as non-admin → 401; cross-root-account assignment → 404. Positives: compound document shape (events + linked assignments/courses/students/graders), `start_time`/`end_time` windows, `include[]=current_grade`; anonymous+muted assignments null out `student_id` in course-scoped queries and drop events in student-scoped queries; section-limited graders see only visible students' events (`events_visible_to_current_user`).

**`rubrics_api` (slice C, future)** — account-context routes 401 for course-only teachers; `download_rubrics` succeeds with `read_rubrics` alone (negative: no rights → 401); `upload` without attachment → 400; `upload_status` accepts `latest` and import ids, 404 on unknown; `rubrics_by_import_id` returns rubrics created by that import.

**`peer_reviews_api` (slice D, future)** — submit-only caller sees only own assessee requests; `create`/`destroy` without `:grade` → 401; reviewer==student on `create` → 400; `allocate` as student not assigned the assignment → 403.

**`assignment_overrides` (slice G)** — negatives: `batch_create`/`batch_update` without `manage_assignments_edit` → 401; single-override write without assignment `:update` → 401; `batch_retrieve` without course `:read` → 401. Positives: transactional batch (all-or-nothing), request-order preservation, alias 302/404 behavior, date-clearing on partial update, adhoc `student_ids` omission keeps the set.

**`submission_comments_api` (slice G)** — `update`/`destroy` by non-author → 401; `annotation_notification` by non-site-admin → 401.

**`assignments_api` (slice G)** — `user_index` visibility for self/observer/admin only; `duplicate` cross-course without `manage_assignments_add` on target → 401; `bulk_update` without `manage_assignments_edit` → 401 and with a non-updatable assignment → 401; accessibility scans without granular rights or with the checker disabled → 401; `student_group_members` on a non-group assignment → empty list.

**`quizzes/quiz_submissions_api`, `quiz_reports`, `quiz_groups` (slice G)** — `complete` with wrong/missing `validation_token` → 403, stale attempt → 400, already complete → 400; `time` without `:record_events` → 401; reports: all four routes 401 without `read_statistics`; `abort` 422 branches (not generating / not abortable); `reorder` with duplicate or foreign question ids → 400/404, without quiz `:update` → 401.

---

## 8. Open questions

1. **Submission versions storage for gradebook history.** Canvas stores collated history via the `versions` table plus a `submission_versions` index table. Does the Star snapshot's existing submission-history mechanism provide a `version_id`-ordered, filterable, paginatable source for §6.3's `feed`? If not, the additive migration needed (and its reservation) must be settled before #32 starts.
2. **`anonymous_student_identities`.** Canvas's `AnonymousUserDisplay.display_name` reads `assignment.anonymous_student_identities` (per-student pseudonyms). The snapshot has no such mapping; the landed Star handler emits `"Student"`. Confirm the intended parity level for #30: shape parity only (current behavior) or full identity generation (needs an assignments×users pseudonym table).
3. **`can_view_student_names?` scope.** The landed `gradeable_students` gates on the `anonymous_grading` flag; Canvas gates on `anonymize_students?`, which additionally depends on grades-published state and unposted submissions. #30 must switch the gate; confirm no UI currently depends on the looser behavior.
4. **Moderation events / live SpeedGrader reload.** Canvas's `create_moderation_event` and submission `touch` drive websocket updates. Out of scope for #31 unless the coordinator says otherwise; the spec's slice F omits them (API responses don't expose them).
5. **Audit `page_view` links.** `grade_change_events_compound_json` links a `page_view` id when one exists. The Star audit table has no page-view association; decide whether to always omit the link or accept a null (current 0092-based handlers presumably omit).
6. **Non-API submissions create route.** `POST /courses/:course_id/assignments/:assignment_id/submissions` belongs to the non-API `submissions` controller; it is outside this spec's manifest totals but overlaps slice A handlers. Confirm with the coordinator whether the #32 slice should reuse the update handler or skip it.
7. **`submissions_clear_unread` semantics.** Canvas's handler reports a warning to error capture while returning 204 with an empty body (no explicit render). Star should return 204; the Canvas-side observability call is not parity-relevant.
8. **Rubric import ownership.** §5.3's `rubric_imports` table and the account-context rubrics routes are left to a future issue; confirm the coordinator wants a separate spec rather than folding them into #32.

---

## Implementation slicing for #30 / #31 / #32

This appendix is the authoritative route assignment for the three Ready issues this spec unblocks. It is derived from the slice map (§2) and the verified route inventory (§3). Every route is listed with its full Canvas path; `todo` status per §3. Issues #30 and #31 are single-slice; #32 takes the slice-B and slice-A/G submissions remainder. Slices C (rubrics account-context + upload) and D (peer-review section twins + allocate) and the non-submissions part of G are intentionally left for future issues.

### #30 — Slice E: anonymous submissions/grading semantics

**New routes (2, both todo):**

1. `GET /api/v1/sections/:section_id/assignments/:assignment_id/anonymous_submissions/:anonymous_id` — `show_anonymous` (section twin)
2. `PUT /api/v1/sections/:section_id/assignments/:assignment_id/anonymous_submissions/:anonymous_id` — `update_anonymous` (section twin)

Both reuse the landed course-context handlers (`show_anonymous_submission` / `update_anonymous_submission` in `star-assignments`) with the section-scoped context resolution and enrollment visibility.

**Semantics work on landed routes (no new route):**

3. `GET /api/v1/courses/:course_id/assignments/:assignment_id/gradeable_students` — `allow_new_anonymous_id=true` semantics:
   - gate on `can_view_student_names?` (via `anonymize_students?`: for moderated assignments, grades-not-yet-published; otherwise any active student with an unposted submission) instead of the raw `anonymous_grading` flag;
   - `AnonymousUserDisplay` response shape `{anonymous_id, avatar_image_url, display_name}` when names are hidden, `UserDisplay` otherwise;
   - sort: `sort=name` → `sortable_name` when names visible, **`anonymous_id` fallback ordering when hidden**; default order `user_id` visible / `anonymous_id` hidden.
4. Anonymized response semantics on the landed course twins (`GET`/`PUT .../anonymous_submissions/:anonymous_id`): `user_id` dropped, `anonymous_id` present (also for account-membership callers), `html_url` → SpeedGrader anonymous URL when `anonymize_students?`, same swap inside `submission_history` attempt JSON.

### #31 — Slice F: provisional / moderated grading (all 7 routes new + param work)

**New routes (7, all todo):**

1. `GET /api/v1/courses/:course_id/assignments/:assignment_id/moderated_students` — `moderation_set#index`
2. `POST /api/v1/courses/:course_id/assignments/:assignment_id/moderated_students` — `moderation_set#create`
3. `PUT /api/v1/courses/:course_id/assignments/:assignment_id/provisional_grades/:provisional_grade_id/select` — `provisional_grades#select`
4. `PUT /api/v1/courses/:course_id/assignments/:assignment_id/provisional_grades/bulk_select` — `provisional_grades#bulk_select`
5. `POST /api/v1/courses/:course_id/assignments/:assignment_id/provisional_grades/publish` — `provisional_grades#publish`
6. `GET /api/v1/courses/:course_id/assignments/:assignment_id/provisional_grades/status?student_id=` — `provisional_grades#status`
7. `GET /api/v1/courses/:course_id/assignments/:assignment_id/anonymous_provisional_grades/status?anonymous_id=` — `anonymous_provisional_grades#status`

**Param/include work on landed routes:**

8. `PUT /api/v1/courses/:course_id/assignments/:assignment_id/submissions/:user_id` (and the anonymous twin): `submission[provisional]` creates a provisional grade; `submission[final]` honored only under `permits_moderation?` (final-grader or account `select_final_grade`); **401 when posting a final/non-provisional grade on an unpublished moderated assignment without moderation rights** (§4.2/§6.2).
9. `include[]=provisional_grades` on submissions `index`/`show` (course + section contexts as they land) and on `gradeable_students` (there gated by `permits_moderation?`, 401 otherwise), with the role-filtered provisional-grade list (moderators see all, final last; graders see their own) and `anonymous_grader_id` substitution when `can_view_other_grader_identities?` is false.

**Schema prerequisite:** the three tables of §5.2 (`provisional_grades`, `moderated_grading_selections`, `moderation_graders`) plus the assignments columns; migration number to be reserved by the coordinator.

### #32 — Slices B + submissions remainder of A/G

**`gradebook_history_api` (4 routes, all todo):**

1. `GET /api/v1/courses/:course_id/gradebook_history/days` — `days`
2. `GET /api/v1/courses/:course_id/gradebook_history/:date` — `day_details`
3. `GET /api/v1/courses/:course_id/gradebook_history/:date/graders/:grader_id/assignments/:assignment_id/submissions` — `submissions`
4. `GET /api/v1/courses/:course_id/gradebook_history/feed` — `feed`

**`grade_change_audit_api` (7 routes, all todo; query semantics per §6.3 note and §4.3):**

5. `GET /api/v1/audit/grade_change/assignments/:assignment_id` — `for_assignment`
6. `GET /api/v1/audit/grade_change/students/:student_id` — `for_student`
7. `GET /api/v1/audit/grade_change/graders/:grader_id` — `for_grader`
8. `GET /api/v1/audit/grade_change/courses/:course_id/assignments/:assignment_id/graders/:grader_id` — `for_course_and_other_parameters`
9. `GET /api/v1/audit/grade_change/courses/:course_id/assignments/:assignment_id/students/:student_id` — `for_course_and_other_parameters`
10. `GET /api/v1/audit/grade_change/courses/:course_id/graders/:grader_id/students/:student_id` — `for_course_and_other_parameters`
11. `GET /api/v1/audit/grade_change/courses/:course_id/assignments/:assignment_id/graders/:grader_id/students/:student_id` — `for_course_and_other_parameters`

**`submissions_api` remainder (12 routes, all todo):**

12. `GET /api/v1/sections/:section_id/assignments/:assignment_id/submissions` — `index` (section twin)
13. `GET /api/v1/sections/:section_id/assignments/:assignment_id/submissions/:user_id` — `show` (section twin)
14. `PUT /api/v1/sections/:section_id/assignments/:assignment_id/submissions/:user_id` — `update` (section twin)
15. `PUT /api/v1/courses/:course_id/assignments/:assignment_id/submissions/:user_id/read/:item` — `mark_submission_item_read` (item read state; items `grade`/`comment`/`rubric`)
16. `GET /api/v1/courses/:course_id/assignments/:assignment_id/submissions/:user_id/rubric_assessments/read` — `rubric_assessments_read_state`
17. `GET /api/v1/courses/:course_id/assignments/:assignment_id/submissions/:user_id/rubric_comments/read` — `rubric_assessments_read_state`
18. `PUT /api/v1/courses/:course_id/assignments/:assignment_id/submissions/:user_id/rubric_assessments/read` — `mark_rubric_assessments_read`
19. `PUT /api/v1/courses/:course_id/assignments/:assignment_id/submissions/:user_id/rubric_comments/read` — `mark_rubric_assessments_read`
20. `GET /api/v1/courses/:course_id/assignments/:assignment_id/submissions/:user_id/document_annotations/read` — `document_annotations_read_state`
21. `PUT /api/v1/courses/:course_id/assignments/:assignment_id/submissions/:user_id/document_annotations/read` — `mark_document_annotations_read`
22. `GET /api/v1/courses/:course_id/assignments/gradeable_students?assignment_ids[]=` — `multiple_gradeable_students`
23. `POST /api/v1/courses/:course_id/submissions/update_grades` — `bulk_update` (course-level)

**Explicitly NOT in #32** (deferred, per §2): the other submissions section twins (`create_file`, `submission_summary`, `for_students`, read PUT/DELETE, `bulk_mark_read`, `clear_unread`, section-level `update_grades`, and the section twins of the item/rubric/document read-state routes — slice A remainder), slices C and D, and the non-submissions part of G (quiz tail, overrides batch/aliases, submission-comments remainder, assignments remainder). These stay in the future-issue bucket until the coordinator slices them.

**Total for #32: 23 routes** (4 gradebook history + 7 audit + 12 submissions: 3 section twins of index/show/update, the item read state `read/:item`, the 6 rubric/document-annotation read-state GET+PUT routes, `multiple_gradeable_students`, and the course-level `update_grades`).
