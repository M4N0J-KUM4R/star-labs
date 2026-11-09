# Canvas to Rust parity scoreboard

Measured, deterministic output. Canvas source is not committed; refresh the manifest locally with `tools/parity/parity.py manifest`.

## Summary

| Area | Implemented | Total | Coverage |
|---|---:|---:|---:|
| REST API routes | 93 | 1403 | 6.6% |
| REST routes referenced from `ui/` | 62 | 503 | 12.3% |
| GraphQL files | 0 | 390 | 0.0% |
| GraphQL object types | 0 | 170 | 0.0% |
| GraphQL mutations | 0 | 110 | 0.0% |
| UI feature directories | 0 | 232 | 0.0% |
| JS/JSX/TS/TSX files | 0 | 5459 | 0.0% |
| ERB views | 0 | 475 | 0.0% |
| js_env files | 0 | 124 | 0.0% |
| Schema tables (vs 688 Canvas models) | 36 | 688 | n/a |

Implemented REST route count: **93 / 1403** (6.6%).

Star registrations: 160 route/method pairs from 155 `.route(...)` calls (0 unparsed); 68 have no Canvas match and are listed as extras below.

Canvas source sizes: 390 GraphQL files (170 object types, 110 mutations), 688 models, 232 UI feature dirs, 5459 JS/TSX files, 475 ERB views, 124 js_env files (490 call sites).

## Controllers with coverage (29)

| Controller | Implemented | Total | Coverage |
|---|---:|---:|---:|
| `courses` | 6 | 38 | 15.8% |
| `assignments_api` | 5 | 12 | 41.7% |
| `context_modules_api` | 5 | 8 | 62.5% |
| `conversations` | 5 | 16 | 31.2% |
| `groups` | 5 | 17 | 29.4% |
| `quizzes/quizzes_api` | 5 | 7 | 71.4% |
| `wiki_pages_api` | 5 | 29 | 17.2% |
| `assignment_groups_api` | 4 | 5 | 80.0% |
| `calendar_events_api` | 4 | 15 | 26.7% |
| `discussion_topics` | 4 | 10 | 40.0% |
| `grading_standards_api` | 4 | 10 | 40.0% |
| `users` | 4 | 39 | 10.3% |
| `accounts` | 3 | 22 | 13.6% |
| `context_module_items_api` | 3 | 12 | 25.0% |
| `discussion_topics_api` | 3 | 58 | 5.2% |
| `enrollments_api` | 3 | 14 | 21.4% |
| `files` | 3 | 26 | 11.5% |
| `group_memberships` | 3 | 11 | 27.3% |
| `outcome_groups_api` | 3 | 40 | 7.5% |
| `quizzes/quiz_submissions_api` | 3 | 7 | 42.9% |
| `rubrics` | 3 | 3 | 100.0% |
| `rubrics_api` | 2 | 15 | 13.3% |
| `submissions_api` | 2 | 44 | 4.5% |
| `assignment_groups` | 1 | 1 | 100.0% |
| `assignments` | 1 | 1 | 100.0% |
| `lti/account_lookup` | 1 | 1 | 100.0% |
| `sub_accounts` | 1 | 2 | 50.0% |
| `submission_comments_api` | 1 | 4 | 25.0% |
| `submissions` | 1 | 2 | 50.0% |

## Star routes with no Canvas match (68)

| Verb | Path | Crate |
|---|---|---|
| POST | `/accounts/{id}/users/{userId}` | `star-accounts` |
| DELETE | `/announcements/{id}` | `star-announcements` |
| GET | `/announcements/{id}` | `star-announcements` |
| PUT | `/announcements/{id}` | `star-announcements` |
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
| POST | `/courses` | `star-courses` |
| GET | `/courses/{courseId}/announcements` | `star-announcements` |
| POST | `/courses/{courseId}/announcements` | `star-announcements` |
| GET | `/courses/{courseId}/assignments/{id}/submissions/self` | `star-assignments` |
| GET | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments` | `star-submission-comments` |
| POST | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments` | `star-submission-comments` |
| GET | `/courses/{courseId}/calendar_events` | `star-calendar` |
| POST | `/courses/{courseId}/calendar_events` | `star-calendar` |
| GET | `/courses/{courseId}/final_grade` | `star-grading-standards` |
| GET | `/courses/{courseId}/gradebook` | `star-assignments` |
| POST | `/courses/{courseId}/groups` | `star-groups` |
| POST | `/courses/{courseId}/outcome_groups` | `star-outcomes` |
| GET | `/courses/{courseId}/outcomes` | `star-outcomes` |
| POST | `/courses/{courseId}/outcomes` | `star-outcomes` |
| GET | `/dashboard` | `star-dashboard` |
| GET | `/enrollments` | `star-enrollments` |
| POST | `/enrollments` | `star-enrollments` |
| DELETE | `/enrollments/{id}` | `star-enrollments` |
| DELETE | `/grading_standards/{id}` | `star-grading-standards` |
| GET | `/grading_standards/{id}` | `star-grading-standards` |
| GET | `/health` | `star-http` |
| DELETE | `/modules/{id}` | `star-modules` |
| GET | `/modules/{id}` | `star-modules` |
| PUT | `/modules/{id}` | `star-modules` |
| POST | `/modules/{id}/items` | `star-modules` |
| DELETE | `/modules/{id}/items/{itemId}` | `star-modules` |
| GET | `/notifications/unread_count` | `star-notifications` |
| DELETE | `/notifications/{id}` | `star-notifications` |
| PUT | `/notifications/{id}/read` | `star-notifications` |
| DELETE | `/outcome_groups/{id}` | `star-outcomes` |
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
| DELETE | `/rubrics/{id}` | `star-rubrics` |
| GET | `/rubrics/{id}` | `star-rubrics` |
| PUT | `/rubrics/{id}` | `star-rubrics` |
| GET | `/rubrics/{rubricId}/assessments` | `star-rubrics` |
| POST | `/rubrics/{rubricId}/assessments` | `star-rubrics` |
| GET | `/submissions/{submissionId}/comments` | `star-submission-comments` |
| POST | `/submissions/{submissionId}/comments` | `star-submission-comments` |
| DELETE | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` |
| PUT | `/submissions/{id}/grade` | `star-assignments` |
| GET | `/users` | `star-users` |
| GET | `/users/self` | `star-users` |
| GET | `/users/self/notification_preferences` | `star-notifications` |
| PUT | `/users/self/notification_preferences` | `star-notifications` |
| GET | `/users/self/notifications` | `star-notifications` |

## Parser scope and limitations

- Canvas routes are verb declarations inside `ApiRouteSet.draw` blocks, plus Rails `resources` declarations (API defaults: index/create/show/update/destroy, honoring `only:`, `except:`, `path:`, `controller:`, and nesting) and `%w[...].each` context loops, which are expanded for every value.
- Conditional context prefixes assigned from `var == "literal" ? var : "..."` are evaluated per loop binding, and `if`/`unless var == "literal"` guards filter bindings; this expands the `outcome_groups_api` routes to `courses/...`, `accounts/...`, and `global/...`. Other runtime interpolations are left as `{}` and cannot match concrete Rust paths.
- UI references are `/api/v1/...` string literals in `ui/`; runtime-built paths and hard-coded ids that cannot be normalized are missed.
- Rust paths are matched by `(verb, normalized path)`; axum's `nest("/api/v1", ...)` mount is assumed for all crate routers.
- Extras include deliberate flat-path deviations (for example `/assignments/{id}` where Canvas uses `/courses/{course_id}/assignments/{id}`) and auth/dashboard/health routes Canvas does not expose.
- The schema row counts `CREATE TABLE` statements in `star-db` migrations (36 authored tables); sqlx adds `_sqlx_migrations`, so a live database has 37 tables.
