# App replay

Build: `target/debug/star-http`

Passed: 212/460

Fallback rate: not reported by this build

## Distinct differences

| Cases | Id | Where | Kind |
| ---: | --- | --- | --- |
| 118 | `d04de58a` | `header cache-control` | text |
| 66 | `f7aed968` | `header www-authenticate` | missing key |
| 38 | `7ec02a16` | `body $.errors[].message` | value |
| 28 | `84764fab` | `status` | 404→403 |
| 27 | `96a328ab` | `status` | 404→200 |
| 26 | `4cb102ff` | `status` | 401→404 |
| 22 | `19fe84fe` | `status` | 401→200 |
| 19 | `c99aa66e` | `body $.errors` | missing key |
| 17 | `59f5f078` | `status` | 200→403 |
| 13 | `59f5f378` | `status` | 200→404 |
| 12 | `847351ab` | `status` | 404→415 |
| 9 | `4cb105ff` | `status` | 401→403 |
| 9 | `ea00df59` | `status` | 400→403 |
| 9 | `847652ab` | `status` | 404→400 |
| 9 | `4cb106ff` | `status` | 401→400 |
| 9 | `e06d529c` | `header set-cookie` | text |
| 9 | `862e729b` | `body $` | type |
| 8 | `d62ae6e8` | `body` | lines missing |
| 7 | `0da8054c` | `header content-type` | text |
| 7 | `84139eb4` | `header pragma` | missing key |
| 6 | `bf07e579` | `status` | 403→404 |
| 6 | `ea00de59` | `status` | 400→404 |
| 5 | `8caa558f` | `header content-type` | missing key |
| 5 | `5292d478` | `status` | 200→500 |
| 5 | `59f28678` | `status` | 200→415 |
| 4 | `b61a6028` | `status` | 500→200 |
| 4 | `96a324ab` | `status` | 404→204 |
| 3 | `e9fdd959` | `status` | 400→415 |
| 2 | `7bcd37ab` | `status` | 404→500 |
| 2 | `f810c4ea` | `status` | 302→403 |
| 2 | `ea7fe128` | `status` | 500→403 |
| 2 | `4caa7cff` | `status` | 401→422 |
| 1 | `8e5bed78` | `status` | 200→204 |
| 1 | `e0eac759` | `status` | 400→500 |
| 1 | `45192798` | `status` | 204→415 |
| 1 | `59f5ef78` | `status` | 200→400 |
| 1 | `32efd298` | `status` | 204→200 |
| 1 | `e0eac659` | `status` | 400→503 |
| 1 | `7bcd36ab` | `status` | 404→503 |
| 1 | `ea795828` | `status` | 500→422 |
| 1 | `3ccedb4f` | `status` | 422→403 |
| 1 | `8d8c8fab` | `status` | 404→302 |
| 1 | `451c2498` | `status` | 204→404 |

## Failing cases

### accounts-delete-accounts-users-35

Arguments: `DELETE /api/v1/accounts/6/users/1`

at status (200→403)

status:

```diff
expected 200, got 403
```

### accounts-get-accounts-accessibility-issue-summary

Arguments: `GET /api/v1/accounts/6/accessibility_issue_summary`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### accounts-get-accounts-courses

Arguments: `GET /api/v1/accounts/6/courses`

at status (200→403)

status:

```diff
expected 200, got 403
```

### accounts-get-accounts-settings

Arguments: `GET /api/v1/accounts/6/settings`

at status (200→403)

status:

```diff
expected 200, got 403
```

### accounts-put-accounts

Arguments: `PUT /api/v1/accounts/6`

at status (200→403)

status:

```diff
expected 200, got 403
```

### assignments-delete-courses-assignments

Arguments: `DELETE /api/v1/courses/1/assignments/1`

at status (200→204)

status:

```diff
expected 200, got 204
```

at header content-type (missing key)

header content-type:

```diff
- application/json; charset=utf-8
(missing)
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### calendar-events-api-delete-calendar-events

Arguments: `DELETE /api/v1/calendar_events/1`

at status (403→404)

status:

```diff
expected 403, got 404
```

### calendar-events-api-get-calendar-events-277

Arguments: `GET /api/v1/calendar_events/1`

at status (200→404)

status:

```diff
expected 200, got 404
```

### calendar-events-api-get-calendar-events-participants

Arguments: `GET /api/v1/calendar_events/1/participants`

at status (200→404)

status:

```diff
expected 200, got 404
```

### calendar-events-api-post-calendar-events-reservations-42

Arguments: `POST /api/v1/calendar_events/1/reservations/3`

at status (403→404)

status:

```diff
expected 403, got 404
```

### calendar-events-api-post-calendar-events-reservations

Arguments: `POST /api/v1/calendar_events/1/reservations`

at status (403→404)

status:

```diff
expected 403, got 404
```

### calendar-events-api-post-courses-calendar-events-timetable-events

Arguments: `POST /api/v1/courses/1/calendar_events/timetable_events`

at status (500→200)

status:

```diff
expected 500, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### calendar-events-api-post-courses-calendar-events-timetable

Arguments: `POST /api/v1/courses/1/calendar_events/timetable`

at status (500→200)

status:

```diff
expected 500, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### calendar-events-api-put-calendar-events

Arguments: `PUT /api/v1/calendar_events/1`

at status (403→404)

status:

```diff
expected 403, got 404
```

### conversations-delete-conversations-delete-for-all

Arguments: `DELETE /api/v1/conversations/1/delete_for_all`

at status (200→403)

status:

```diff
expected 200, got 403
```

### conversations-get-conversations-deleted

Arguments: `GET /api/v1/conversations/deleted`

at status (200→403)

status:

```diff
expected 200, got 403
```

### conversations-put-conversations-restore

Arguments: `PUT /api/v1/conversations/restore`

at status (400→403)

status:

```diff
expected 400, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### courses-delete-courses

Arguments: `DELETE /api/v1/courses/1`

at status (400→403)

status:

```diff
expected 400, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### courses-get-courses-bulk-user-progress

Arguments: `GET /api/v1/courses/1/bulk_user_progress`

at status (400→500)

status:

```diff
expected 400, got 500
```

### courses-get-courses-content-share-users

Arguments: `GET /api/v1/courses/1/content_share_users`

at status (400→403)

status:

```diff
expected 400, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### courses-get-courses-recent-students

Arguments: `GET /api/v1/courses/1/recent_students`

at status (200→500)

status:

```diff
expected 200, got 500
```

### courses-get-courses-search-users

Arguments: `GET /api/v1/courses/1/search_users`

at status (200→500)

status:

```diff
expected 200, got 500
```

### courses-get-courses-student-view-student

Arguments: `GET /api/v1/courses/1/student_view_student`

at status (200→404)

status:

```diff
expected 200, got 404
```

### courses-get-courses-users-362

Arguments: `GET /api/v1/courses/1/users/1`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### courses-get-courses-users-progress

Arguments: `GET /api/v1/courses/1/users/1/progress`

at status (404→500)

status:

```diff
expected 404, got 500
```

### courses-get-courses-youtube-migration-conversion-status

Arguments: `GET /api/v1/courses/1/youtube_migration/conversion_status`

at status (500→200)

status:

```diff
expected 500, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### courses-get-courses-youtube-migration-scan

Arguments: `GET /api/v1/courses/1/youtube_migration/scan`

at status (500→200)

status:

```diff
expected 500, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### courses-post-accounts-courses

Arguments: `POST /api/v1/accounts/6/courses`

at status (200→415)

status:

```diff
expected 200, got 415
```

### courses-post-courses-dismiss-migration-limitation-message

Arguments: `POST /api/v1/courses/1/dismiss_migration_limitation_message`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### courses-post-courses-link-validation

Arguments: `POST /api/v1/courses/1/link_validation`

at status (200→415)

status:

```diff
expected 200, got 415
```

### courses-post-courses-preview-html

Arguments: `POST /api/v1/courses/1/preview_html`

at status (200→415)

status:

```diff
expected 200, got 415
```

### courses-post-courses-youtube-migration-convert

Arguments: `POST /api/v1/courses/1/youtube_migration/convert`

at status (404→415)

status:

```diff
expected 404, got 415
```

### courses-post-courses-youtube-migration-scan

Arguments: `POST /api/v1/courses/1/youtube_migration/scan`

at status (404→415)

status:

```diff
expected 404, got 415
```

### courses-put-accounts-courses

Arguments: `PUT /api/v1/accounts/6/courses`

at status (400→403)

status:

```diff
expected 400, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### courses-put-courses-quizzes

Arguments: `PUT /api/v1/courses/1/quizzes`

at status (204→415)

status:

```diff
expected 204, got 415
```

### courses-put-courses-settings

Arguments: `PUT /api/v1/courses/1/settings`

at status (200→415)

status:

```diff
expected 200, got 415
```

### courses-put-courses

Arguments: `PUT /api/v1/courses/1`

at status (200→415)

status:

```diff
expected 200, got 415
```

### discussion-topics-api-put-courses-discussion-topics-migrate-disallow

Arguments: `PUT /api/v1/courses/1/discussion_topics/migrate_disallow`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### discussion-topics-api-put-courses-discussion-topics-read-all-91

Arguments: `PUT /api/v1/courses/1/discussion_topics/read_all`

at status (404→204)

status:

```diff
expected 404, got 204
```

at header content-type (missing key)

header content-type:

```diff
- application/json; charset=utf-8
(missing)
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### discussion-topics-api-put-courses-discussion-topics-update-discussion-types

Arguments: `PUT /api/v1/courses/1/discussion_topics/update_discussion_types`

at status (404→400)

status:

```diff
expected 404, got 400
```

### discussion-topics-api-put-groups-discussion-topics-read-all-186

Arguments: `PUT /api/v1/groups/1/discussion_topics/read_all`

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### enrollments-api-get-accounts-enrollments

Arguments: `GET /api/v1/accounts/6/enrollments/7`

at status (200→404)

status:

```diff
expected 200, got 404
```

### enrollments-api-get-courses-enrollments

Arguments: `GET /api/v1/courses/1/enrollments`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### enrollments-api-get-sections-enrollments

Arguments: `GET /api/v1/sections/2/enrollments`

at status (200→404)

status:

```diff
expected 200, got 404
```

### enrollments-api-get-temporary-enrollment-status

Arguments: `GET /api/v1/temporary_enrollment_status`

at status (200→400)

status:

```diff
expected 200, got 400
```

### enrollments-api-get-users-temporary-enrollment-status

Arguments: `GET /api/v1/users/1/temporary_enrollment_status`

at status (204→200)

status:

```diff
expected 204, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### enrollments-api-post-courses-enrollments

Arguments: `POST /api/v1/courses/1/enrollments`

at status (404→415)

status:

```diff
expected 404, got 415
```

### enrollments-api-post-sections-enrollments

Arguments: `POST /api/v1/sections/2/enrollments`

at status (400→404)

status:

```diff
expected 400, got 404
```

### enrollments-api-put-courses-users-last-attended

Arguments: `PUT /api/v1/courses/1/users/1/last_attended`

at status (404→400)

status:

```diff
expected 404, got 400
```

### external-tools-delete-accounts-external-tools-rce-favorites

Arguments: `DELETE /accounts/6/external_tools/rce_favorites/1`

at header cache-control, line 1, column 4, char 3: expected "no-⟨s⟩tore", got "no-⟨c⟩ache"

header cache-control:

```diff
- no-store
+ no-cache
```

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html
+ application/json; charset=utf-8
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

### external-tools-delete-accounts-external-tools-top-nav-favorites

Arguments: `DELETE /accounts/6/external_tools/top_nav_favorites/1`

at header cache-control, line 1, column 4, char 3: expected "no-⟨s⟩tore", got "no-⟨c⟩ache"

header cache-control:

```diff
- no-store
+ no-cache
```

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html
+ application/json; charset=utf-8
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

### external-tools-get-accounts-external-tools-migration-info

Arguments: `GET /api/v1/accounts/6/external_tools/1/migration_info`

at status (200→403)

status:

```diff
expected 200, got 403
```

### external-tools-get-accounts-external-tools-sessionless-launch

Arguments: `GET /api/v1/accounts/6/external_tools/sessionless_launch`

at status (400→503)

status:

```diff
expected 400, got 503
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### external-tools-get-accounts-external-tools

Arguments: `GET /api/v1/accounts/6/external_tools`

at status (200→403)

status:

```diff
expected 200, got 403
```

### external-tools-get-courses-external-tools-sessionless-launch

Arguments: `GET /api/v1/courses/1/external_tools/sessionless_launch`

at status (404→503)

status:

```diff
expected 404, got 503
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### external-tools-get-courses-external-tools-visible-course-nav-tools

Arguments: `GET /courses/1/external_tools/visible_course_nav_tools`

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html
+ application/json; charset=utf-8
```

### external-tools-get-courses-external-tools

Arguments: `GET /api/v1/courses/1/external_tools`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### external-tools-get-external-tools-visible-course-nav-tools

Arguments: `GET /external_tools/visible_course_nav_tools`

at header cache-control, line 1, column 4, char 3: expected "no-⟨s⟩tore", got "no-⟨c⟩ache"

header cache-control:

```diff
- no-store
+ no-cache
```

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html
+ application/json; charset=utf-8
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

### external-tools-post-accounts-create-tool-with-verification

Arguments: `POST /api/v1/accounts/6/create_tool_with_verification`

at status (400→403)

status:

```diff
expected 400, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### external-tools-post-accounts-external-tools-rce-favorites

Arguments: `POST /accounts/6/external_tools/rce_favorites/1`

at header cache-control, line 1, column 4, char 3: expected "no-⟨s⟩tore", got "no-⟨c⟩ache"

header cache-control:

```diff
- no-store
+ no-cache
```

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html
+ application/json; charset=utf-8
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

### external-tools-post-accounts-external-tools-top-nav-favorites

Arguments: `POST /accounts/6/external_tools/top_nav_favorites/1`

at header cache-control, line 1, column 4, char 3: expected "no-⟨s⟩tore", got "no-⟨c⟩ache"

header cache-control:

```diff
- no-store
+ no-cache
```

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html
+ application/json; charset=utf-8
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

### external-tools-post-accounts-external-tools

Arguments: `POST /api/v1/accounts/6/external_tools`

at status (400→403)

status:

```diff
expected 400, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### external-tools-post-courses-create-tool-with-verification

Arguments: `POST /api/v1/courses/1/create_tool_with_verification`

at status (404→400)

status:

```diff
expected 404, got 400
```

### external-tools-post-courses-external-tools

Arguments: `POST /api/v1/courses/1/external_tools`

at status (404→400)

status:

```diff
expected 404, got 400
```

### external-tools-put-accounts-external-tools

Arguments: `PUT /api/v1/accounts/6/external_tools/1`

at status (404→415)

status:

```diff
expected 404, got 415
```

### external-tools-put-courses-external-tools

Arguments: `PUT /api/v1/courses/1/external_tools/1`

at status (404→415)

status:

```diff
expected 404, got 415
```

### files-delete-files

Arguments: `DELETE /api/v1/files/7`

at status (401→404)

status:

```diff
expected 401, got 404
```

### files-get-courses-files-quota

Arguments: `GET /api/v1/courses/1/files/quota`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### files-get-courses-files

Arguments: `GET /api/v1/courses/1/files`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### files-get-files-create-success

Arguments: `GET /api/v1/files/7/create_success`

at status (400→404)

status:

```diff
expected 400, got 404
```

### files-get-files-icon-metadata

Arguments: `GET /api/v1/files/7/icon_metadata`

at status (400→404)

status:

```diff
expected 400, got 404
```

### files-get-files-public-url

Arguments: `GET /api/v1/files/7/public_url`

at status (200→404)

status:

```diff
expected 200, got 404
```

### files-get-files

Arguments: `GET /api/v1/files/7`

at status (200→404)

status:

```diff
expected 200, got 404
```

### files-get-folders-files

Arguments: `GET /api/v1/folders/7/files`

at status (200→404)

status:

```diff
expected 200, got 404
```

### files-get-users-files-quota

Arguments: `GET /api/v1/users/1/files/quota`

at status (200→500)

status:

```diff
expected 200, got 500
```

### files-post-files-capture

Arguments: `POST /api/v1/files/capture`

at status (404→415)

status:

```diff
expected 404, got 415
```

### files-post-files-create-success

Arguments: `POST /api/v1/files/7/create_success`

at status (400→404)

status:

```diff
expected 400, got 404
```

### files-post-files-reset-verifier

Arguments: `POST /api/v1/files/7/reset_verifier`

at status (200→404)

status:

```diff
expected 200, got 404
```

### files-post-files

Arguments: `POST /api/v1/files/7`

at status (200→404)

status:

```diff
expected 200, got 404
```

### files-put-files

Arguments: `PUT /api/v1/files/7`

at status (403→404)

status:

```diff
expected 403, got 404
```

### folders-get-courses-folders-323

Arguments: `GET /api/v1/courses/1/folders/1`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### folders-get-courses-folders-and-files

Arguments: `GET /api/v1/courses/1/folders_and_files`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-courses-folders-by-path

Arguments: `GET /api/v1/courses/1/folders/by_path`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### folders-get-courses-folders-duplicates

Arguments: `GET /api/v1/courses/1/folders/1/duplicates`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-courses-folders-icon-maker

Arguments: `GET /api/v1/courses/1/folders/icon_maker`

at status (404→500)

status:

```diff
expected 404, got 500
```

### folders-get-courses-folders-media

Arguments: `GET /api/v1/courses/1/folders/media`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### folders-get-courses-folders

Arguments: `GET /api/v1/courses/1/folders`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### folders-get-folders-all

Arguments: `GET /api/v1/folders/1/all`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-groups-folders-and-files

Arguments: `GET /api/v1/groups/1/folders_and_files`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-groups-folders-by-path

Arguments: `GET /api/v1/groups/1/folders/by_path`

at status (200→404)

status:

```diff
expected 200, got 404
```

### folders-get-groups-folders-duplicates

Arguments: `GET /api/v1/groups/1/folders/1/duplicates`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-users-folders-and-files

Arguments: `GET /api/v1/users/1/folders_and_files`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-users-folders-by-path

Arguments: `GET /api/v1/users/1/folders/by_path`

at status (200→404)

status:

```diff
expected 200, got 404
```

### folders-get-users-folders-duplicates

Arguments: `GET /api/v1/users/1/folders/1/duplicates`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-post-accounts-folders

Arguments: `POST /api/v1/accounts/6/folders`

at status (200→403)

status:

```diff
expected 200, got 403
```

### folders-post-courses-folders

Arguments: `POST /api/v1/courses/1/folders`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### folders-post-folders-copy-file

Arguments: `POST /api/v1/folders/1/copy_file`

at status (400→415)

status:

```diff
expected 400, got 415
```

### folders-post-folders-copy-folder

Arguments: `POST /api/v1/folders/1/copy_folder`

at status (400→415)

status:

```diff
expected 400, got 415
```

### folders-post-folders-files

Arguments: `POST /api/v1/folders/1/files`

at status (500→422)

status:

```diff
expected 500, got 422
```

### folders-post-users-folders

Arguments: `POST /api/v1/users/1/folders`

at status (200→500)

status:

```diff
expected 200, got 500
```

### group-categories-get-accounts-group-categories

Arguments: `GET /api/v1/accounts/6/group_categories`

at status (200→403)

status:

```diff
expected 200, got 403
```

### group-categories-get-courses-group-categories-export-tags

Arguments: `GET /api/v1/courses/1/group_categories/export_tags`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header content-type, line 1, column 1, char 0: expected "⟨a⟩pplication/json;…", got "⟨t⟩ext/csv"

header content-type:

```diff
- application/json; charset=utf-8
+ text/csv
```

### group-categories-get-courses-group-categories

Arguments: `GET /api/v1/courses/1/group_categories`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### group-categories-patch-group-categories

Arguments: `PATCH /api/v1/group_categories/1`

at header cache-control, line 1, column 4, char 3: expected "no-⟨s⟩tore", got "no-⟨c⟩ache"

header cache-control:

```diff
- no-store
+ no-cache
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

### group-categories-post-accounts-group-categories

Arguments: `POST /api/v1/accounts/6/group_categories`

at status (400→403)

status:

```diff
expected 400, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### group-categories-post-courses-group-categories-import-tags

Arguments: `POST /api/v1/courses/1/group_categories/import_tags`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### group-categories-post-courses-group-categories

Arguments: `POST /api/v1/courses/1/group_categories`

at status (404→400)

status:

```diff
expected 404, got 400
```

### groups-delete-groups

Arguments: `DELETE /api/v1/groups/1`

at status (404→204)

status:

```diff
expected 404, got 204
```

at header content-type (missing key)

header content-type:

```diff
- application/json; charset=utf-8
(missing)
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### groups-get-accounts-groups

Arguments: `GET /api/v1/accounts/6/groups`

at status (200→403)

status:

```diff
expected 200, got 403
```

### groups-get-courses-bulk-user-tags

Arguments: `GET /api/v1/courses/1/bulk_user_tags`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### groups-get-courses-groups

Arguments: `GET /api/v1/courses/1/groups`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### groups-get-groups-activity-stream-summary

Arguments: `GET /api/v1/groups/1/activity_stream/summary`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### groups-get-groups-activity-stream

Arguments: `GET /api/v1/groups/1/activity_stream`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### groups-get-groups

Arguments: `GET /api/v1/groups/1`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### groups-patch-groups

Arguments: `PATCH /api/v1/groups/1`

at header cache-control, line 1, column 4, char 3: expected "no-⟨s⟩tore", got "no-⟨c⟩ache"

header cache-control:

```diff
- no-store
+ no-cache
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

### groups-post-groups

Arguments: `POST /api/v1/groups`

at status (200→500)

status:

```diff
expected 200, got 500
```

### groups-put-groups

Arguments: `PUT /api/v1/groups/1`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### lti-registrations-delete-accounts-lti-registrations-bind

Arguments: `DELETE /api/v1/accounts/6/lti_registrations/2/bind`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-delete-accounts-lti-registrations

Arguments: `DELETE /api/v1/accounts/6/lti_registrations/2`

at status (200→404)

status:

```diff
expected 200, got 404
```

### lti-registrations-get-accounts-lti-registration-by-client-id

Arguments: `GET /api/v1/accounts/6/lti_registration_by_client_id/4`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-get-accounts-lti-registrations-by-utid

Arguments: `GET /api/v1/accounts/6/lti_registrations/by_utid/:utid`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-get-accounts-lti-registrations-check-domain-duplicates

Arguments: `GET /api/v1/accounts/6/lti_registrations/check_domain_duplicates`

at status (200→403)

status:

```diff
expected 200, got 403
```

### lti-registrations-get-accounts-lti-registrations-history

Arguments: `GET /api/v1/accounts/6/lti_registrations/2/history`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-get-accounts-lti-registrations-install-status

Arguments: `GET /api/v1/accounts/6/lti_registrations/install_status/4`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-get-accounts-lti-registrations-overlay-history

Arguments: `GET /api/v1/accounts/6/lti_registrations/2/overlay_history`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-get-accounts-lti-registrations

Arguments: `GET /api/v1/accounts/6/lti_registrations`

at status (200→403)

status:

```diff
expected 200, got 403
```

### lti-registrations-post-accounts-lti-registrations-bind

Arguments: `POST /api/v1/accounts/6/lti_registrations/2/bind`

at status (404→415)

status:

```diff
expected 404, got 415
```

### lti-registrations-post-accounts-lti-registrations-configuration-validate

Arguments: `POST /api/v1/accounts/6/lti_registrations/configuration/validate`

at status (422→403)

status:

```diff
expected 422, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-post-accounts-lti-registrations-install-from-template

Arguments: `POST /api/v1/accounts/6/lti_registrations/2/install_from_template`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-post-accounts-lti-registrations

Arguments: `POST /api/v1/accounts/6/lti_registrations`

at status (400→415)

status:

```diff
expected 400, got 415
```

### lti-registrations-put-accounts-lti-registrations

Arguments: `PUT /api/v1/accounts/6/lti_registrations/2`

at status (404→415)

status:

```diff
expected 404, got 415
```

### outcome-groups-api-delete-accounts-outcome-groups-outcomes

Arguments: `DELETE /api/v1/accounts/6/outcome_groups/2/outcomes/1`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-delete-accounts-outcome-groups

Arguments: `DELETE /api/v1/accounts/6/outcome_groups/2`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-delete-global-outcome-groups-outcomes

Arguments: `DELETE /api/v1/global/outcome_groups/2/outcomes/1`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-delete-global-outcome-groups

Arguments: `DELETE /api/v1/global/outcome_groups/2`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-get-accounts-outcome-groups-account-chain

Arguments: `GET /api/v1/accounts/6/outcome_groups/account_chain`

at status (200→403)

status:

```diff
expected 200, got 403
```

### outcome-groups-api-get-accounts-outcome-groups-outcomes

Arguments: `GET /api/v1/accounts/6/outcome_groups/2/outcomes`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-get-accounts-root-outcome-group

Arguments: `GET /api/v1/accounts/6/root_outcome_group`

at status (302→403)

status:

```diff
expected 302, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-get-courses-outcome-group-links

Arguments: `GET /api/v1/courses/1/outcome_group_links`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### outcome-groups-api-get-courses-outcome-groups-account-chain

Arguments: `GET /api/v1/courses/1/outcome_groups/account_chain`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### outcome-groups-api-get-courses-outcome-groups

Arguments: `GET /api/v1/courses/1/outcome_groups`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### outcome-groups-api-get-courses-root-outcome-group

Arguments: `GET /api/v1/courses/1/root_outcome_group`

at status (404→302)

status:

```diff
expected 404, got 302
```

### outcome-groups-api-get-global-outcome-groups-account-chain

Arguments: `GET /api/v1/global/outcome_groups/account_chain`

at status (500→403)

status:

```diff
expected 500, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-get-global-root-outcome-group

Arguments: `GET /api/v1/global/root_outcome_group`

at status (302→403)

status:

```diff
expected 302, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-post-accounts-outcome-groups-import

Arguments: `POST /api/v1/accounts/6/outcome_groups/2/import`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-post-accounts-outcome-groups-outcomes

Arguments: `POST /api/v1/accounts/6/outcome_groups/2/outcomes`

at status (404→415)

status:

```diff
expected 404, got 415
```

### outcome-groups-api-post-accounts-outcome-groups-subgroups

Arguments: `POST /api/v1/accounts/6/outcome_groups/2/subgroups`

at status (404→415)

status:

```diff
expected 404, got 415
```

### outcome-groups-api-post-courses-outcome-groups-outcomes

Arguments: `POST /api/v1/courses/1/outcome_groups/2/outcomes`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-post-courses-outcome-groups-subgroups

Arguments: `POST /api/v1/courses/1/outcome_groups/2/subgroups`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-post-global-outcome-groups-import

Arguments: `POST /api/v1/global/outcome_groups/2/import`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-post-global-outcome-groups-outcomes

Arguments: `POST /api/v1/global/outcome_groups/2/outcomes`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-post-global-outcome-groups-subgroups

Arguments: `POST /api/v1/global/outcome_groups/2/subgroups`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-put-accounts-outcome-groups-outcomes

Arguments: `PUT /api/v1/accounts/6/outcome_groups/2/outcomes/1`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-put-accounts-outcome-groups

Arguments: `PUT /api/v1/accounts/6/outcome_groups/2`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-put-global-outcome-groups

Arguments: `PUT /api/v1/global/outcome_groups/2`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### rubrics-api-get-accounts-rubrics-268

Arguments: `GET /api/v1/accounts/6/rubrics/1`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### rubrics-api-get-accounts-rubrics-upload-rubrics

Arguments: `GET /api/v1/accounts/6/rubrics/upload/1/rubrics`

at status (200→403)

status:

```diff
expected 200, got 403
```

### rubrics-api-get-accounts-rubrics-upload

Arguments: `GET /api/v1/accounts/6/rubrics/upload/1`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### rubrics-api-get-accounts-rubrics-used-locations

Arguments: `GET /api/v1/accounts/6/rubrics/1/used_locations`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### rubrics-api-get-accounts-rubrics

Arguments: `GET /api/v1/accounts/6/rubrics`

at status (200→403)

status:

```diff
expected 200, got 403
```

### rubrics-api-get-courses-rubrics-upload-rubrics

Arguments: `GET /api/v1/courses/1/rubrics/upload/1/rubrics`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### rubrics-api-get-courses-rubrics

Arguments: `GET /api/v1/courses/1/rubrics`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### rubrics-api-post-accounts-rubrics-download-rubrics

Arguments: `POST /api/v1/accounts/6/rubrics/download_rubrics`

at status (400→403)

status:

```diff
expected 400, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### rubrics-api-post-accounts-rubrics-upload

Arguments: `POST /api/v1/accounts/6/rubrics/upload`

at status (400→403)

status:

```diff
expected 400, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### rubrics-api-post-courses-rubrics-download-rubrics

Arguments: `POST /api/v1/courses/1/rubrics/download_rubrics`

at status (404→400)

status:

```diff
expected 404, got 400
```

### rubrics-api-post-courses-rubrics-upload

Arguments: `POST /api/v1/courses/1/rubrics/upload`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### submissions-api-get-courses-assignments-gradeable-students-300

Arguments: `GET /api/v1/courses/1/assignments/gradeable_students`

at status (404→200)

status:

```diff
expected 404, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

### submissions-api-get-courses-students-submissions

Arguments: `GET /api/v1/courses/1/students/submissions`

at status (404→403)

status:

```diff
expected 404, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### submissions-api-get-sections-students-submissions

Arguments: `GET /api/v1/sections/2/students/submissions`

at status (403→404)

status:

```diff
expected 403, got 404
```

### submissions-api-post-courses-assignments-submissions-update-grades

Arguments: `POST /api/v1/courses/1/assignments/1/submissions/update_grades`

at status (404→400)

status:

```diff
expected 404, got 400
```

### submissions-api-post-courses-submissions-update-grades

Arguments: `POST /api/v1/courses/1/submissions/update_grades`

at status (404→400)

status:

```diff
expected 404, got 400
```

### submissions-api-post-sections-assignments-submissions-update-grades

Arguments: `POST /api/v1/sections/2/assignments/1/submissions/update_grades`

at status (400→404)

status:

```diff
expected 400, got 404
```

### submissions-api-post-sections-submissions-update-grades

Arguments: `POST /api/v1/sections/2/submissions/update_grades`

at status (400→404)

status:

```diff
expected 400, got 404
```

### submissions-api-put-courses-assignments-anonymous-submissions

Arguments: `PUT /api/v1/courses/1/assignments/1/anonymous_submissions/3`

at status (404→415)

status:

```diff
expected 404, got 415
```

### submissions-api-put-courses-assignments-submissions-read-64

Arguments: `PUT /api/v1/courses/1/assignments/1/submissions/1/read/:item`

at status (404→400)

status:

```diff
expected 404, got 400
```

### submissions-api-put-courses-submissions-bulk-mark-read

Arguments: `PUT /api/v1/courses/1/submissions/bulk_mark_read`

at status (404→204)

status:

```diff
expected 404, got 204
```

at header content-type (missing key)

header content-type:

```diff
- application/json; charset=utf-8
(missing)
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### submissions-api-put-courses-submissions-clear-unread

Arguments: `PUT /api/v1/courses/1/submissions/1/clear_unread`

at status (404→204)

status:

```diff
expected 404, got 204
```

at header content-type (missing key)

header content-type:

```diff
- application/json; charset=utf-8
(missing)
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### submissions-api-put-sections-assignments-anonymous-submissions

Arguments: `PUT /api/v1/sections/2/assignments/1/anonymous_submissions/3`

at status (404→415)

status:

```diff
expected 404, got 415
```

### submissions-api-put-sections-submissions-bulk-mark-read

Arguments: `PUT /api/v1/sections/2/submissions/bulk_mark_read`

at status (204→404)

status:

```diff
expected 204, got 404
```

### submissions-api-put-sections-submissions-clear-unread

Arguments: `PUT /api/v1/sections/2/submissions/1/clear_unread`

at status (500→403)

status:

```diff
expected 500, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### users-delete-users-mobile-sessions-228

Arguments: `DELETE /api/v1/users/mobile_sessions`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-delete-users-mobile-sessions

Arguments: `DELETE /api/v1/users/1/mobile_sessions`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-delete-users-self-activity-stream-230

Arguments: `DELETE /api/v1/users/self/activity_stream/1`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"hidden":true}
```

### users-delete-users-self-activity-stream

Arguments: `DELETE /api/v1/users/self/activity_stream`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"hidden":true}
```

### users-delete-users-self-todo

Arguments: `DELETE /api/v1/users/self/todo/:asset_string/:purpose`

at status (401→400)

status:

```diff
expected 401, got 400
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"ignored":false}
```

### users-delete-users-sessions

Arguments: `DELETE /api/v1/users/1/sessions`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-delete-users

Arguments: `DELETE /api/v1/users/1`

at status (200→403)

status:

```diff
expected 200, got 403
```

### users-get-accounts-users

Arguments: `GET /api/v1/accounts/6/users`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨R⟩oJLofp422QVhl8E3…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D%3D; path=/
+ _csrf_token=RoJLofp422QVhl8E3elaI24xVJ4XzlA2EhXD0YFHbQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-get-dashboard-dashboard-cards

Arguments: `GET /api/v1/dashboard/dashboard_cards`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ [{"longName":"Parity Course - PARITY","shortName":"PARITY","originalName":"Parity Course","courseCode":"PARITY","assetString":"course_1","href":"/courses/1","term":null,"subtitle":"enrolled as Teacher","enrollmentState":"active","enrollmentType":"TeacherEnrollment","observee":null,"id":1,"isFavorited":false,"isK5Subject":false,"isHomeroom":false,"useClassicFont":false,"canManage":true,"canReadAnnouncements":true,"image":null,"color":null,"position":null,"published":true,"links":[{"css_class":"discussions","icon":null,"hidden":false,"path":"/courses/1/discussion_topics","label":"Discussions"},{"css_class":"assignments","icon":null,"hidden":false,"path":"/courses/1/assignments","label":"Assignments"},{"css_class":"announcements","icon":null,"hidden":false,"path":"/courses/1/announcements","label":"Announcements"},{"css_class":"files","icon":null,"hidden":false,"path":"/courses/1/files","label":"Files"}],"canChangeCoursePublishState":true,"defaultView":"modules","pagesUrl":"/courses/1/pages","frontPageTitle":null}]
```

### users-get-show-k5-dashboard

Arguments: `GET /api/v1/show_k5_dashboard`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"show_k5_dashboard":false,"use_classic_font":false}
```

### users-get-users-activity-stream

Arguments: `GET /api/v1/users/activity_stream`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ []
```

### users-get-users-colors-434

Arguments: `GET /api/v1/users/1/colors/:asset_string`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### users-get-users-colors

Arguments: `GET /api/v1/users/1/colors`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"custom_colors":{}}
```

### users-get-users-dashboard-positions

Arguments: `GET /api/v1/users/1/dashboard_positions`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"dashboard_positions":{}}
```

### users-get-users-graded-submissions

Arguments: `GET /api/v1/users/1/graded_submissions`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ []
```

### users-get-users-missing-submissions

Arguments: `GET /api/v1/users/1/missing_submissions`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ []
```

### users-get-users-new-user-tutorial-statuses

Arguments: `GET /api/v1/users/1/new_user_tutorial_statuses`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨m⟩Nxmu%2BWnZ3Mpw%2…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%3D%3D; path=/
+ _csrf_token=mNxmu%2BWnZ3Mpw%2FVb3p6cECjN9B3hB0JHnjVCA%2FaRAg%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"new_user_tutorial_statuses":{"collapsed":{}}}
```

### users-get-users-self-activity-stream-summary

Arguments: `GET /api/v1/users/self/activity_stream/summary`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨r⟩JlZSWg3yLJqIYp8O…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D%3D; path=/
+ _csrf_token=rJlZSWg3yLJqIYp8OTqlCo72auTvYrEXlUxdEy%2FZjA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ []
```

### users-get-users-self-activity-stream

Arguments: `GET /api/v1/users/self/activity_stream`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ []
```

### users-get-users-self-todo-item-count

Arguments: `GET /api/v1/users/self/todo_item_count`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"needs_grading_count":0,"assignments_needing_submitting":0}
```

### users-get-users-self-todo

Arguments: `GET /api/v1/users/self/todo`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ []
```

### users-get-users-self-upcoming-events

Arguments: `GET /api/v1/users/self/upcoming_events`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ []
```

### users-get-users-settings

Arguments: `GET /api/v1/users/1/settings`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨l⟩bOOsHIpm05ynlwjl…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%3D%3D; path=/
+ _csrf_token=lbOOsHIpm05ynlwjl9PNzq%2BFaapJ4WwiTWEJfrNP%2Fg%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"collapse_course_nav":false,"collapse_global_nav":false,"comment_library_suggestions_enabled":false,"default_to_block_editor":false,"elementary_dashboard_disabled":false,"hide_dashcard_color_overlays":false,"manual_mark_as_read":false,"release_notes_badge_disabled":false,"widget_dashboard_dark_mode":false,"widget_dashboard_user_preference":false}
```

### users-get-users

Arguments: `GET /api/v1/users/1`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"id":1,"name":"Parity User","email":"parity@example.com","role":"teacher","created_at":"2026-11-27T13:34:24.091667Z","sortable_name":"Parity User","short_name":"Parity User","first_name":"Parity","last_name":"User","locale":null,"effective_locale":"en","permissions":{"can_update_avatar":false,"can_update_name":true,"limit_parent_app_web_access":false}}
```

### users-post-accounts-self-registration

Arguments: `POST /api/v1/accounts/6/self_registration`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### users-post-accounts-users

Arguments: `POST /api/v1/accounts/6/users`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-post-users-clear-cache

Arguments: `POST /api/v1/users/1/clear_cache`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-post-users-files

Arguments: `POST /api/v1/users/1/files`

at status (401→422)

status:

```diff
expected 401, got 422
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨f⟩ilename is requi…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"filename is required"}]}
```

### users-post-users-self-pandata-events-token

Arguments: `POST /api/v1/users/self/pandata_events_token`

at status (401→400)

status:

```diff
expected 401, got 400
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨2⟩LU%2FJdtj47giJxz…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D%3D; path=/
+ _csrf_token=2LU%2FJdtj47giJxz59l9ySnV97PKbf%2FZix5iIDJzppw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"message":"Access token required"}
```

### users-post-users-split

Arguments: `POST /api/v1/users/1/split`

at status (401→400)

status:

```diff
expected 401, got 400
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"message":"Nothing to split off of this user"}
```

### users-put-users-colors

Arguments: `PUT /api/v1/users/1/colors/:asset_string`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### users-put-users-dashboard-positions

Arguments: `PUT /api/v1/users/1/dashboard_positions`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"dashboard_positions":{}}
```

### users-put-users-files-ui-version-preference

Arguments: `PUT /api/v1/users/1/files_ui_version_preference`

at status (401→400)

status:

```diff
expected 401, got 400
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"message":"Invalid files_ui_version provided"}
```

### users-put-users-merge-into-accounts-users

Arguments: `PUT /api/v1/users/1/merge_into/accounts/6/users/3`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨m⟩WlKiFVQZBm%2BJh1…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D%3D; path=/
+ _csrf_token=mWlKiFVQZBm%2BJh12JuSfDry7NRAGhLgJspvqAG1tHg%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-put-users-merge-into

Arguments: `PUT /api/v1/users/1/merge_into/3`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-put-users-new-user-tutorial-statuses

Arguments: `PUT /api/v1/users/1/new_user_tutorial_statuses/:page_name`

at status (401→400)

status:

```diff
expected 401, got 400
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"message":"Invalid Page Name Provided"}
```

### users-put-users-settings

Arguments: `PUT /api/v1/users/1/settings`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"manual_mark_as_read":false,"collapse_global_nav":false,"collapse_course_nav":false,"hide_dashcard_color_overlays":false,"release_notes_badge_disabled":false,"comment_library_suggestions_enabled":false,"elementary_dashboard_disabled":false,"default_to_block_editor":false,"widget_dashboard_user_preference":false,"widget_dashboard_dark_mode":false}
```

### users-put-users-text-editor-preference

Arguments: `PUT /api/v1/users/1/text_editor_preference`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"text_editor_preference":null}
```

### users-put-users

Arguments: `PUT /api/v1/users/1`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"id":1,"name":"Parity User","email":"parity@example.com","role":"teacher","created_at":"2026-11-27T13:34:24.091667Z","sortable_name":"Parity User","short_name":"Parity User","locale":null,"time_zone":null}
```

### wiki-pages-api-delete-courses-pages

Arguments: `DELETE /api/v1/courses/1/pages/front-page`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-delete-groups-pages

Arguments: `DELETE /api/v1/groups/1/pages/front-page`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-courses-front-page

Arguments: `GET /api/v1/courses/1/front_page`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"message":"No front page has been set"}
```

### wiki-pages-api-get-courses-page-title-availability

Arguments: `GET /api/v1/courses/1/page_title_availability`

at status (401→400)

status:

```diff
expected 401, got 400
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨F⟩ailed to deseria…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"Failed to deserialize query string: missing field `title`"}]}
```

### wiki-pages-api-get-courses-pages-343

Arguments: `GET /api/v1/courses/1/pages/front-page`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-courses-pages-revisions-345

Arguments: `GET /api/v1/courses/1/pages/front-page/revisions/1`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-courses-pages-revisions-latest

Arguments: `GET /api/v1/courses/1/pages/front-page/revisions/latest`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-courses-pages-revisions

Arguments: `GET /api/v1/courses/1/pages/front-page/revisions`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-courses-pages

Arguments: `GET /api/v1/courses/1/pages`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ []
```

### wiki-pages-api-get-groups-front-page

Arguments: `GET /api/v1/groups/1/front_page`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-groups-page-title-availability

Arguments: `GET /api/v1/groups/1/page_title_availability`

at status (401→400)

status:

```diff
expected 401, got 400
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨F⟩ailed to deseria…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"Failed to deserialize query string: missing field `title`"}]}
```

### wiki-pages-api-get-groups-pages-409

Arguments: `GET /api/v1/groups/1/pages/front-page`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-groups-pages-revisions-411

Arguments: `GET /api/v1/groups/1/pages/front-page/revisions/1`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-groups-pages-revisions-latest

Arguments: `GET /api/v1/groups/1/pages/front-page/revisions/latest`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-groups-pages-revisions

Arguments: `GET /api/v1/groups/1/pages/front-page/revisions`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨A⟩HgaHgYYyXMBxKr9y…"

header set-cookie:

```diff
- _csrf_token=[..]%3D%3D; path=/
+ _csrf_token=AHgaHgYYyXMBxKr9yrLQvEnos1D9KroSybDUG7ik0g%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-groups-pages

Arguments: `GET /api/v1/groups/1/pages`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-post-courses-pages-accessibility-queue-scan

Arguments: `POST /api/v1/courses/1/pages/front-page/accessibility/queue_scan`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-post-courses-pages-accessibility-scan

Arguments: `POST /api/v1/courses/1/pages/front-page/accessibility/scan`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-post-courses-pages-ai-alt-text

Arguments: `POST /api/v1/courses/1/pages_ai/alt_text`

at status (401→400)

status:

```diff
expected 401, got 400
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨a⟩ttachment_id is …"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"attachment_id is required"}]}
```

### wiki-pages-api-post-courses-pages-duplicate

Arguments: `POST /api/v1/courses/1/pages/front-page/duplicate`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-post-courses-pages-revisions

Arguments: `POST /api/v1/courses/1/pages/front-page/revisions/1`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-post-courses-pages

Arguments: `POST /api/v1/courses/1/pages`

at status (401→422)

status:

```diff
expected 401, got 422
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨B⟩m%2BT%2FrX6LKJjl…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D%3D; path=/
+ _csrf_token=Bm%2BT%2FrX6LKJjly4AdDorupQq%2BxyCDe6gPAfO2WjSSw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨t⟩itle is required"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"title is required"}]}
```

### wiki-pages-api-post-groups-pages-ai-alt-text

Arguments: `POST /api/v1/groups/1/pages_ai/alt_text`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-post-groups-pages-revisions

Arguments: `POST /api/v1/groups/1/pages/front-page/revisions/1`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-post-groups-pages

Arguments: `POST /api/v1/groups/1/pages`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-put-courses-front-page

Arguments: `PUT /api/v1/courses/1/front_page`

at status (401→400)

status:

```diff
expected 401, got 400
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨t⟩itle is required"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"title is required"}]}
```

### wiki-pages-api-put-courses-pages

Arguments: `PUT /api/v1/courses/1/pages/front-page`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨q⟩IkEhAoMiBExuwMmG…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D%3D; path=/
+ _csrf_token=qIkEhAoMiBExuwMmGufe5uyJU39m45nAN4j9sOErjQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-put-groups-front-page

Arguments: `PUT /api/v1/groups/1/front_page`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-put-groups-pages

Arguments: `PUT /api/v1/groups/1/pages/front-page`

at status (401→404)

status:

```diff
expected 401, got 404
```

at header www-authenticate (missing key)

header www-authenticate:

```diff
- Bearer realm="canvas-lms"
(missing)
```

at body $.errors[0].message, char 0: expected "⟨I⟩nvalid access to…", got "⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"Invalid access token."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```
