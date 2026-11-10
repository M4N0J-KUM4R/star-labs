# Canvas to Rust parity scoreboard

Measured, deterministic output. Canvas source is not committed; refresh the manifest locally with `tools/parity/parity.py manifest`.

## Summary

| Area | Implemented | Total | Coverage |
|---|---:|---:|---:|
| REST API routes | 167 | 1403 | 11.9% |
| REST routes referenced from `ui/` | 91 | 503 | 18.1% |
| GraphQL files | 0 | 390 | 0.0% |
| GraphQL object types | 0 | 170 | 0.0% |
| GraphQL mutations | 0 | 110 | 0.0% |
| UI feature directories | 0 | 232 | 0.0% |
| JS/JSX/TS/TSX files | 0 | 5459 | 0.0% |
| ERB views | 0 | 475 | 0.0% |
| js_env files | 0 | 124 | 0.0% |
| Schema tables (vs 688 Canvas models) | 68 | 688 | n/a |

Implemented REST route count: **167 / 1403** (11.9%).

Star registrations: 277 route/method pairs from 253 `.route(...)` calls (0 unparsed); 111 have no Canvas match and are listed as extras below.

Canvas source sizes: 390 GraphQL files (170 object types, 110 mutations), 688 models, 232 UI feature dirs, 5459 JS/TSX files, 475 ERB views, 124 js_env files (490 call sites).

## Controllers with coverage (46)

| Controller | Implemented | Total | Coverage |
|---|---:|---:|---:|
| `discussion_topics_api` | 8 | 58 | 13.8% |
| `wiki_pages_api` | 8 | 29 | 27.6% |
| `folders` | 7 | 34 | 20.6% |
| `conversations` | 6 | 16 | 37.5% |
| `courses` | 6 | 38 | 15.8% |
| `group_categories` | 6 | 16 | 37.5% |
| `sections` | 6 | 9 | 66.7% |
| `assignment_overrides` | 5 | 10 | 50.0% |
| `assignments_api` | 5 | 12 | 41.7% |
| `context_module_items_api` | 5 | 12 | 41.7% |
| `context_modules_api` | 5 | 8 | 62.5% |
| `enrollments_api` | 5 | 14 | 35.7% |
| `groups` | 5 | 17 | 29.4% |
| `planner_notes` | 5 | 5 | 100.0% |
| `quizzes/quiz_groups` | 5 | 6 | 83.3% |
| `quizzes/quiz_questions` | 5 | 5 | 100.0% |
| `quizzes/quizzes_api` | 5 | 7 | 71.4% |
| `assignment_groups_api` | 4 | 5 | 80.0% |
| `calendar_events_api` | 4 | 15 | 26.7% |
| `discussion_topics` | 4 | 10 | 40.0% |
| `files` | 4 | 26 | 15.4% |
| `grading_standards_api` | 4 | 10 | 40.0% |
| `users` | 4 | 39 | 10.3% |
| `account_notifications` | 3 | 8 | 37.5% |
| `accounts` | 3 | 22 | 13.6% |
| `admins` | 3 | 4 | 75.0% |
| `content_exports_api` | 3 | 14 | 21.4% |
| `group_memberships` | 3 | 11 | 27.3% |
| `outcome_groups_api` | 3 | 40 | 7.5% |
| `quizzes/quiz_submissions_api` | 3 | 7 | 42.9% |
| `rubrics` | 3 | 3 | 100.0% |
| `terms` | 3 | 3 | 100.0% |
| `jobs` | 2 | 3 | 66.7% |
| `notification_preferences` | 2 | 10 | 20.0% |
| `rubrics_api` | 2 | 15 | 13.3% |
| `submissions_api` | 2 | 44 | 4.5% |
| `terms_api` | 2 | 2 | 100.0% |
| `assignment_groups` | 1 | 1 | 100.0% |
| `assignments` | 1 | 1 | 100.0% |
| `developer_keys` | 1 | 5 | 20.0% |
| `grading_periods` | 1 | 8 | 12.5% |
| `lti/account_lookup` | 1 | 1 | 100.0% |
| `profile` | 1 | 2 | 50.0% |
| `sub_accounts` | 1 | 2 | 50.0% |
| `submission_comments_api` | 1 | 4 | 25.0% |
| `submissions` | 1 | 2 | 50.0% |

## Star routes with no Canvas match (111)

| Verb | Path | Crate |
|---|---|---|
| DELETE | `/account_notifications/{id}` | `star-accounts` |
| POST | `/accounts/{accountId}/grading_periods` | `star-grading-standards` |
| POST | `/accounts/{id}/users/{userId}` | `star-accounts` |
| DELETE | `/announcements/{id}` | `star-announcements` |
| GET | `/announcements/{id}` | `star-announcements` |
| PUT | `/announcements/{id}` | `star-announcements` |
| DELETE | `/assignment_overrides/{id}` | `star-assignments` |
| GET | `/assignment_overrides/{id}` | `star-assignments` |
| PUT | `/assignment_overrides/{id}` | `star-assignments` |
| DELETE | `/assignments/{id}` | `star-assignments` |
| GET | `/assignments/{id}` | `star-assignments` |
| PUT | `/assignments/{id}` | `star-assignments` |
| GET | `/assignments/{id}/submissions` | `star-assignments` |
| POST | `/assignments/{id}/submissions` | `star-assignments` |
| GET | `/assignments/{id}/submissions/me` | `star-assignments` |
| POST | `/auth/login` | `star-auth` |
| POST | `/auth/logout` | `star-auth` |
| GET | `/auth/me` | `star-auth` |
| POST | `/auth/register` | `star-auth` |
| GET | `/content_exports/{id}` | `star-content` |
| GET | `/conversations/{id}/participants` | `star-conversations` |
| PUT | `/conversations/{id}/read` | `star-conversations` |
| POST | `/courses` | `star-courses` |
| GET | `/courses/{courseId}/announcements` | `star-announcements` |
| POST | `/courses/{courseId}/announcements` | `star-announcements` |
| GET | `/courses/{courseId}/announcements/unread_count` | `star-announcements` |
| DELETE | `/courses/{courseId}/announcements/{id}/read` | `star-announcements` |
| PUT | `/courses/{courseId}/announcements/{id}/read` | `star-announcements` |
| GET | `/courses/{courseId}/assignment_groups/{id}/rules` | `star-assignment-groups` |
| PUT | `/courses/{courseId}/assignment_groups/{id}/rules` | `star-assignment-groups` |
| GET | `/courses/{courseId}/assignments/{id}/submissions/self` | `star-assignments` |
| GET | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments` | `star-submission-comments` |
| POST | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments` | `star-submission-comments` |
| GET | `/courses/{courseId}/calendar_events` | `star-calendar` |
| POST | `/courses/{courseId}/calendar_events` | `star-calendar` |
| GET | `/courses/{courseId}/final_grade` | `star-grading-standards` |
| GET | `/courses/{courseId}/folders/root` | `star-files` |
| GET | `/courses/{courseId}/gradebook` | `star-assignments` |
| POST | `/courses/{courseId}/groups` | `star-groups` |
| POST | `/courses/{courseId}/outcome_groups` | `star-outcomes` |
| GET | `/courses/{courseId}/outcomes` | `star-outcomes` |
| POST | `/courses/{courseId}/outcomes` | `star-outcomes` |
| GET | `/courses/{courseId}/rubrics/{rubricId}/assessments` | `star-rubrics` |
| POST | `/courses/{courseId}/rubrics/{rubricId}/assessments` | `star-rubrics` |
| GET | `/dashboard` | `star-dashboard` |
| GET | `/developer_keys` | `star-auth` |
| POST | `/developer_keys` | `star-auth` |
| GET | `/enrollments` | `star-enrollments` |
| POST | `/enrollments` | `star-enrollments` |
| DELETE | `/enrollments/{id}` | `star-enrollments` |
| GET | `/folders` | `star-files` |
| POST | `/folders` | `star-files` |
| DELETE | `/grading_periods/{id}` | `star-grading-standards` |
| GET | `/grading_periods/{id}` | `star-grading-standards` |
| PUT | `/grading_periods/{id}` | `star-grading-standards` |
| DELETE | `/grading_standards/{id}` | `star-grading-standards` |
| GET | `/grading_standards/{id}` | `star-grading-standards` |
| GET | `/health` | `star-http` |
| POST | `/jobs/{id}/cancel` | `star-jobs` |
| POST | `/login/oauth2/token` | `star-auth` |
| DELETE | `/modules/{id}` | `star-modules` |
| GET | `/modules/{id}` | `star-modules` |
| PUT | `/modules/{id}` | `star-modules` |
| POST | `/modules/{id}/items` | `star-modules` |
| DELETE | `/modules/{id}/items/{itemId}` | `star-modules` |
| GET | `/notifications/unread_count` | `star-notifications` |
| DELETE | `/notifications/{id}` | `star-notifications` |
| PUT | `/notifications/{id}/read` | `star-notifications` |
| DELETE | `/outcome_groups/{id}` | `star-outcomes` |
| GET | `/outcome_groups/{id}` | `star-outcomes` |
| PUT | `/outcome_groups/{id}` | `star-outcomes` |
| GET | `/outcome_groups/{id}/outcomes` | `star-outcomes` |
| POST | `/outcome_groups/{id}/outcomes` | `star-outcomes` |
| GET | `/outcomes/{outcomeId}/results` | `star-outcomes` |
| POST | `/outcomes/{outcomeId}/results` | `star-outcomes` |
| DELETE | `/pages/{id}` | `star-pages` |
| GET | `/pages/{id}` | `star-pages` |
| PUT | `/pages/{id}` | `star-pages` |
| DELETE | `/quizzes/{id}` | `star-quizzes` |
| GET | `/quizzes/{id}` | `star-quizzes` |
| PUT | `/quizzes/{id}` | `star-quizzes` |
| POST | `/quizzes/{id}/start` | `star-quizzes` |
| GET | `/quizzes/{id}/submissions/me` | `star-quizzes` |
| DELETE | `/rubric_assessments/{id}` | `star-rubrics` |
| GET | `/rubric_assessments/{id}` | `star-rubrics` |
| PUT | `/rubric_assessments/{id}` | `star-rubrics` |
| DELETE | `/rubrics/{id}` | `star-rubrics` |
| GET | `/rubrics/{id}` | `star-rubrics` |
| PUT | `/rubrics/{id}` | `star-rubrics` |
| GET | `/rubrics/{rubricId}/assessments` | `star-rubrics` |
| POST | `/rubrics/{rubricId}/assessments` | `star-rubrics` |
| GET | `/submissions/{submissionId}/comments` | `star-submission-comments` |
| POST | `/submissions/{submissionId}/comments` | `star-submission-comments` |
| PUT | `/submissions/{submissionId}/comments/read` | `star-submission-comments` |
| GET | `/submissions/{submissionId}/comments/unread_count` | `star-submission-comments` |
| DELETE | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` |
| PUT | `/submissions/{id}/grade` | `star-assignments` |
| DELETE | `/terms/{id}` | `star-courses` |
| GET | `/terms/{id}` | `star-courses` |
| PUT | `/terms/{id}` | `star-courses` |
| GET | `/users` | `star-users` |
| GET | `/users/self` | `star-users` |
| GET | `/users/self/communication_channels` | `star-notifications` |
| POST | `/users/self/communication_channels` | `star-notifications` |
| DELETE | `/users/self/communication_channels/{id}` | `star-notifications` |
| GET | `/users/self/communication_channels/{communication_channel_id}/notification_preferences` | `star-notifications` |
| GET | `/users/self/notification_preferences` | `star-notifications` |
| PUT | `/users/self/notification_preferences` | `star-notifications` |
| GET | `/users/self/notifications` | `star-notifications` |
| GET | `/users/self/profile` | `star-users` |
| PUT | `/users/self/profile` | `star-users` |

## Parser scope and limitations

- Canvas routes are verb declarations inside `ApiRouteSet.draw` blocks, plus Rails `resources` declarations (API defaults: index/create/show/update/destroy, honoring `only:`, `except:`, `path:`, `controller:`, and nesting) and `%w[...].each` context loops, which are expanded for every value.
- Conditional context prefixes assigned from `var == "literal" ? var : "..."` are evaluated per loop binding, and `if`/`unless var == "literal"` guards filter bindings; this expands the `outcome_groups_api` routes to `courses/...`, `accounts/...`, and `global/...`. Other runtime interpolations are left as `{}` and cannot match concrete Rust paths.
- UI references are `/api/v1/...` string literals in `ui/`; runtime-built paths and hard-coded ids that cannot be normalized are missed.
- Rust paths are matched by `(verb, normalized path)`; axum's `nest("/api/v1", ...)` mount is assumed for all crate routers.
- Extras include deliberate flat-path deviations (for example `/assignments/{id}` where Canvas uses `/courses/{course_id}/assignments/{id}`) and auth/dashboard/health routes Canvas does not expose.
- The schema row counts `CREATE TABLE` statements in `star-db` migrations (36 authored tables); sqlx adds `_sqlx_migrations`, so a live database has 37 tables.
