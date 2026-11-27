# App replay

Build: `target/debug/star-http`

Passed: 265/430

Fallback rate: not reported by this build

## Distinct differences

| Cases | Id | Where | Kind |
| ---: | --- | --- | --- |
| 78 | `d04de58a` | `header cache-control` | text |
| 27 | `e06d529c` | `header set-cookie` | text |
| 26 | `84139eb4` | `header pragma` | missing key |
| 17 | `7ec02a16` | `body $.errors[].message` | value |
| 16 | `d506e5b1` | `body $.status` | missing key |
| 15 | `0da8054c` | `header content-type` | text |
| 10 | `d62ae6e8` | `body` | lines missing |
| 10 | `6b521040` | `body` | text |
| 10 | `862e729b` | `body $` | type |
| 8 | `84764fab` | `status` | 404→403 |
| 8 | `59f5f078` | `status` | 200→403 |
| 8 | `5a352777` | `body` | lines added |
| 7 | `1388bc4e` | `body $` | length |
| 6 | `59f5f378` | `status` | 200→404 |
| 5 | `ea7fe128` | `status` | 500→403 |
| 5 | `96a328ab` | `status` | 404→200 |
| 5 | `847652ab` | `status` | 404→400 |
| 5 | `bf0b6a79` | `status` | 403→415 |
| 4 | `bf07e579` | `status` | 403→404 |
| 4 | `4b262163` | `body $[].id` | value |
| 4 | `c81e8479` | `status` | 403→500 |
| 3 | `ce5768af` | `body $.id` | value |
| 3 | `5292d478` | `status` | 200→500 |
| 3 | `4e1b1758` | `header link` | missing key |
| 3 | `c99aa66e` | `body $.errors` | missing key |
| 3 | `bf07e979` | `status` | 403→400 |
| 3 | `946b26c3` | `header location` | missing key |
| 3 | `ad489379` | `status` | 403→200 |
| 2 | `4cb105ff` | `status` | 401→403 |
| 2 | `f3a2de59` | `body $` | value |
| 2 | `ea7fde28` | `status` | 500→404 |
| 2 | `bb192150` | `body $.status` | extra key |
| 2 | `8e66a14e` | `body $.id` | missing key |
| 2 | `bf07e679` | `status` | 403→405 |
| 2 | `451c2998` | `status` | 204→403 |
| 2 | `e0eac659` | `status` | 400→503 |
| 2 | `f810c9ea` | `status` | 302→404 |
| 2 | `847351ab` | `status` | 404→415 |
| 2 | `ea00de59` | `status` | 400→404 |
| 2 | `16ab684f` | `header access-control-allow-credentials` | missing key |
| 2 | `f39553a0` | `header access-control-allow-headers` | missing key |
| 2 | `0895895f` | `header access-control-allow-methods` | missing key |
| 2 | `d150aa07` | `header access-control-allow-origin` | missing key |
| 2 | `06ffc01c` | `header access-control-request-method` | missing key |
| 2 | `2f857360` | `body $.name` | value |
| 2 | `2867bf27` | `body $.created_at` | value |
| 1 | `37d258d9` | `body $.custom_help_links` | extra key |
| 1 | `ea00df59` | `status` | 400→403 |
| 1 | `363352ad` | `body $.error` | missing key |
| 1 | `16f83452` | `body $.use_default_discussion_settings` | value |
| 1 | `ba6eda7a` | `body $.course_section_id` | type |
| 1 | `59f5ef78` | `status` | 200→400 |
| 1 | `32efd298` | `status` | 204→200 |
| 1 | `ea7fe228` | `status` | 500→400 |
| 1 | `4cb102ff` | `status` | 401→404 |
| 1 | `d088d7d7` | `body $.message` | missing key |
| 1 | `2f763e2b` | `body $.public_url` | missing key |
| 1 | `c37ba3e2` | `body $.quota` | missing key |
| 1 | `16ccb963` | `body $[].name` | value |
| 1 | `19fe84fe` | `status` | 401→200 |
| 1 | `4e38daf3` | `body $[].description` | missing key |
| 1 | `bd14c25e` | `body $.participate` | missing key |
| 1 | `d2959cb2` | `body $.description` | missing key |
| 1 | `171679bc` | `header location` | text |
| 1 | `f810c4ea` | `status` | 302→403 |
| 1 | `c7f2b919` | `header content-disposition` | text |
| 1 | `e5f63a90` | `header content-transfer-encoding` | missing key |
| 1 | `ad489779` | `status` | 403→204 |
| 1 | `8caa558f` | `header content-type` | missing key |
| 1 | `0d525965` | `header link` | text |
| 1 | `ea795828` | `status` | 500→422 |
| 1 | `6591b220` | `body $.message` | value |
| 1 | `b61a6028` | `status` | 500→200 |
| 1 | `0868dc4f` | `status` | 422→200 |

## Failing cases

### accounts-get-accounts-accessibility-issue-summary

Arguments: `GET /api/v1/accounts/6/accessibility_issue_summary`

at status (401→403)

status:

```diff
expected 401, got 403
```

### accounts-get-accounts-help-links

Arguments: `GET /api/v1/accounts/6/help_links`

at body $.custom_help_links (extra key)

body:

```diff
- {}
+ {"custom_help_links":[],"default_help_links":[{"available_to":["student"],"id":"link_1","subtext":"Questions are submitted to your instructor","text":"Ask your instructor a question","type":"default","url":"#teacher_feedback"},{"available_to":["user"],"id":"link_2","subtext":"Find answers to common questions","text":"Search the Canvas Guides","type":"default","url":"http://lms.canvaslms.com/search/"},{"available_to":["user"],"id":"link_0","subtext":"Get help with Canvas!","text":"Report a problem","type":"default","url":"#ticket_create"}],"help_link_icon":"help","help_link_name":"Help"}
```

### accounts-get-accounts-settings

Arguments: `GET /api/v1/accounts/6/settings`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨v⟩s7FHVA2lO6SYg7s9…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]_[..].[..].[..]; path=/; httponly
+ _csrf_token=vs7FHVA2lO6SYg7s9tSaPaNfvXg%2B1YU9pkp6xDOXhQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### accounts-get-accounts-terms-of-service

Arguments: `GET /api/v1/accounts/6/terms_of_service`

at body $ (value)

body:

```diff
- {"id":6,"terms_type":"default","passive":true,"account_id":6,"content":null,"self_registration_type":"none"}
+ {"account_id":6,"content":null,"id":6,"passive":true,"self_registration_type":"none","terms_type":"default"}
```

### accounts-get-manageable-accounts

Arguments: `GET /api/v1/manageable_accounts`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨v⟩UtYtxukQIM1Kyo%2…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D[..]_[..]_[..].[..].[..]; path=/; httponly
+ _csrf_token=vUtYtxukQIM1Kyo%2BE1uqDfaM%2B5fJlvW9O%2FIFwyPUrw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### accounts-get-manually-created-courses-account

Arguments: `GET /api/v1/manually_created_courses_account`

at body $.id (value): expected 9, got 2

body:

```diff
- {"id":9,"name":"Manually-Created Courses","parent_account_id":4,"root_account_id":4,"workflow_state":"active","uuid":"Hp6ITN1nNVCwGH4CJSgSm0pNkOaFWesKyzH8Uca2","default_time_zone":"America/Denver"}
+ {"id":2,"name":"Manually-Created Courses","parent_account_id":1,"root_account_id":1,"workflow_state":"active","uuid":null,"default_storage_quota_mb":null,"default_user_storage_quota_mb":null,"default_group_storage_quota_mb":null,"default_time_zone":"UTC","sis_account_id":null,"sis_import_id":null,"integration_id":null,"course_template_id":null}
```

### assignments-delete-courses-assignments

Arguments: `DELETE /api/v1/courses/1/assignments/1`

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

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### calendar-events-api-get-calendar-events-participants

Arguments: `GET /api/v1/calendar_events/1/participants`

at body (lines missing), line 1, length 65 expected, 0 got

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected false, got true)
```

### calendar-events-api-get-courses-calendar-events-timetable

Arguments: `GET /api/v1/courses/1/calendar_events/timetable`

at status (200→404)

status:

```diff
expected 200, got 404
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### calendar-events-api-post-calendar-events

Arguments: `POST /api/v1/calendar_events`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨P⟩QVeiO5i8fjZtfjo1…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%[..]_[..].[..]; path=/; httponly
+ _csrf_token=PQVeiO5i8fjZtfjo1qWbol%2FDy0xI3UgQ5eXXBjsOvg%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### calendar-events-api-post-courses-calendar-events-timetable-events

Arguments: `POST /api/v1/courses/1/calendar_events/timetable_events`

at status (500→404)

status:

```diff
expected 500, got 404
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### calendar-events-api-post-courses-calendar-events-timetable

Arguments: `POST /api/v1/courses/1/calendar_events/timetable`

at status (500→404)

status:

```diff
expected 500, got 404
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### calendar-events-api-put-calendar-events

Arguments: `PUT /api/v1/calendar_events/1`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨U⟩yFbRf5tZaXgr%2FU…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D[..]_[..]_[..].[..]_[..]_[..].[..]; path=/; httponly
+ _csrf_token=UyFbRf5tZaXgr%2FUB8rWSyMDyUBagU3RfSUdEkq57yg%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### conversations-get-conversations-find-recipients

Arguments: `GET /api/v1/conversations/find_recipients`

at body $ (length), length 0 expected, 1 got

body:

```diff
- []
+ [{"avatar_url":null,"common_name":{"id":1,"model":"User","name":"Parity User","short_name":"Parity User","sortable_name":"Parity User"},"full_name":"Parity User","id":1,"name":"Parity User","short_name":"Parity User","sortable_name":"Parity User"}]
```

### conversations-get-conversations

Arguments: `GET /api/v1/conversations`

at body $ (length), length 0 expected, 1 got

body:

```diff
- []
+ [{"id":2,"subject":"hi","workflow_state":"active","last_message_preview":"there","participant_count":1,"last_message_at":"2026-11-27T09:27:13.573359Z","created_at":"2026-11-27T09:27:13.573359Z"}]
```

### conversations-post-conversations

Arguments: `POST /api/v1/conversations`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨%⟩2Bshn%2BJGLNxsIO…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]-[..]-[..].[..]; path=/; httponly
+ _csrf_token=%2Bshn%2BJGLNxsIOnrxEPemZpMLDTKtGuwxvgJbYKa8sw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### courses-get-accounts-courses

Arguments: `GET /api/v1/accounts/6/courses/1`

at status (200→404)

status:

```diff
expected 200, got 404
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### courses-get-courses-bulk-user-progress

Arguments: `GET /api/v1/courses/1/bulk_user_progress`

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

at body $.error (missing key)

body:

```diff
- {"error":{"message":"No progress available because this course is not module based (meaning, it does not have modules and module completion requirements)."}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-get-courses-effective-due-dates

Arguments: `GET /api/v1/courses/1/effective_due_dates`

at status (200→403)

status:

```diff
expected 200, got 403
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-store"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-store
```

at body $.status (extra key)

body:

```diff
- {}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-get-courses-link-validation

Arguments: `GET /api/v1/courses/1/link_validation`

at status (200→403)

status:

```diff
expected 200, got 403
```

### courses-get-courses-permissions

Arguments: `GET /api/v1/courses/1/permissions`

at body, line 1, column 20, char 19: expected "…ead\":true,\"read_⟨o⟩utcomes\":true,\"r…", got "…ead\":true,\"read_⟨r⟩oster\":true,\"vie…"

body:

```diff
- {"read":true,"read_outcomes":true,"read_syllabus":true,"read_files":true,"site_admin_self_token_create":false,"become_user":false,"import_sis":false,"create_access_tokens":false,"delete_access_tokens":false,"view_user_generated_access_tokens":false,"manage_account_memberships":false,"manage_account_settings":false,"manage_alerts":false,"manage_catalog":false,"manage_courses_admin":false,"manage_courses_add":false,"manage_courses_publish":[..],"manage_courses_conclude":[..],"manage_courses_reset":false,"manage_courses_delete":false,"undelete_courses":false,"view_archived_courses":false,"manage_data_services":false,"manage_course_visibility":[..],"manage_developer_keys":false,"modify_site_admin_developer_keys":false,"moderate_user_content":false,"view_feature_flags":[..],"manage_feature_flags":[..],"manage_frozen_assignments":false,"manage_global_outcomes":false,"manage_jobs":false,"manage_lti_registrations":false,"manage_release_notes":false,"manage_master_courses":false,"manage_nav_menu_links":false,"manage_role_overrides":false,"manage_storage_quotas":false,"manage_sis":false,"manage_site_settings":false,"manage_rate_limiting":false,"manage_internal_settings":false,"manage_user_logins":false,"manage_dsr_requests":false,"manage_user_observers":false,"read_course_content":false,"read_course_list":false,"read_messages":false,"reset_any_mfa":false,"view_course_changes":false,"view_error_reports":false,"view_grade_changes":false,"view_jobs":false,"view_notifications":false,"view_statistics":false,"create_collaborations":[..],"create_conferences":[..],"create_forum":[..],"generate_observer_pairing_code":false,"import_outcomes":[..],"manage_lti_add":[..],"manage_lti_edit":[..],"manage_lti_delete":[..],"allow_course_admin_actions":[..],"add_teacher_to_course":[..],"remove_teacher_from_course":[..],"add_ta_to_course":[..],"remove_ta_from_course":[..],"add_observer_to_course":[..],"remove_observer_from_course":[..],"add_designer_to_course":[..],"remove_designer_from_course":[..],"manage_assignments_add":[..],"manage_assignments_edit":[..],"manage_assignments_delete":[..],"manage_account_calendar_visibility":false,"manage_account_calendar_events":false,"manage_calendar":[..],"manage_course_content_add":[..],"manage_course_content_edit":[..],"manage_course_content_delete":[..],"add_course_template":false,"edit_course_template":false,"delete_course_template":false,"manage_account_banks":false,"share_banks_with_subaccounts":false,"manage_files_add":[..],"manage_files_edit":[..],"manage_files_delete":[..],"manage_grades":[..],"manage_groups_add":[..],"manage_groups_manage":[..],"manage_groups_delete":[..],"manage_tags_add":[..],"manage_tags_manage":[..],"manage_tags_delete":[..],"manage_interaction_alerts":[..],"manage_outcomes":[..],"manage_proficiency_calculations":false,"manage_proficiency_scales":false,"manage_sections_add":[..],"manage_sections_edit":[..],"manage_sections_delete":[..],"manage_students":[..],"add_student_to_course":[..],"remove_student_from_course":[..],"temporary_enrollments_add":false,"temporary_enrollments_edit":false,"temporary_enrollments_delete":false,"manage_rubrics":[..],"manage_wiki_create":[..],"manage_wiki_update":[..],"manage_wiki_delete":[..],"moderate_forum":[..],"new_quizzes_view_ip_address":false,"post_to_forum":true,"proxy_assignment_submission":false,"read_announcements":true,"read_email_addresses":true,"read_forum":true,"read_question_banks":true,"read_reports":[..],"read_roster":true,"read_sis":true,"select_final_grade":[..],"send_messages":[..],"send_messages_all":[..],"view_audit_trail":false,"view_all_grades":true,"view_group_pages":true,"view_quiz_answer_audits":false,"view_user_logins":true,"view_admin_analytics":false,"view_analytics_hub":false,"view_ask_questions_pinboards":false,"view_ask_questions_analytics":false,"manage_ask_questions_analytics_context":false,"view_students_in_need":false,"view_students_in_need_in_course":false,"view_course_readiness":false,"view_title_iv_financial_aid_report":false,"view_rsi_report":false,"view_accessibility_insights":false,"manage_impact":false,"manage_institutional_tags_view":false,"manage_institutional_tags_create":false,"manage_institutional_tags_edit":false,"access_oak":false,"access_oak_teacher":false,"block_editor_template_editor":false,"block_editor_global_template_editor":false,"new_quizzes_multiple_session_detection":false,"manage_users_in_bulk":false,"manage_rules_view":false,"manage_rules_add":false,"manage_rules_edit":false,"manage_rules_delete":false,"manage_course_details":false,"manage_course_navigation":false,"manage_course_feature_options":false,"edit_discussion_anonymity":false,"edit_discussion_options":false,"edit_discussion_views":false,"apply_default_discussion_options":false,"read_as_member":true,"participate_as_student":false,"read_grades":false,"reset_what_if_grades":false,"read_as_admin":true,"manage":[..],"update":[..],"view_unpublished_items":true,"read_rubrics":true,"use_student_view":true,"set_grading_scheme":[..],"manage_grading_schemes":[..],"reset_content":false,"delete":false,"read_prior_roster":true,"direct_share":[..],"edit_course_availability":[..],"update_nav":[..],"update_course_details":[..]}
+ {"read":true,"read_roster":true,"view_all_grades":true,"manage_grades":true,"update":true,"delete":false,"manage":true,"use_student_view":true}
```

### courses-get-courses-recent-students

Arguments: `GET /api/v1/courses/1/recent_students`

at status (200→403)

status:

```diff
expected 200, got 403
```

### courses-get-courses-search-users

Arguments: `GET /api/v1/courses/1/search_users`

at status (200→500)

status:

```diff
expected 200, got 500
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/courses/1/users?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/courses/1/users?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/courses/1/users?page=1&per_page=10>; rel="last"
(missing)
```

### courses-get-courses-settings

Arguments: `GET /api/v1/courses/1/settings`

at body $.use_default_discussion_settings (value): expected false, got true

body:

```diff
- {"allow_final_grade_override":false,"allow_student_discussion_topics":true,"allow_student_forum_attachments":true,"allow_student_discussion_editing":true,"allow_student_discussion_reporting":true,"allow_student_anonymous_discussion_topics":false,"use_default_discussion_settings":false,"default_discussion_settings":{},"filter_speed_grader_by_student_group":false,"grading_standard_enabled":false,"grading_standard_id":null,"grade_passback_setting":null,"allow_student_organized_groups":true,"hide_final_grades":false,"hide_distribution_graphs":false,"hide_sections_on_course_users_page":false,"lock_all_announcements":false,"usage_rights_required":false,"restrict_student_past_view":false,"restrict_student_future_view":false,"restrict_quantitative_data":false,"show_announcements_on_home_page":false,"home_page_announcement_limit":null,"syllabus_course_summary":true,"homeroom_course":false,"image_url":null,"image_id":null,"image":null,"banner_image_url":null,"banner_image_id":null,"banner_image":null,"course_color":null,"friendly_name":null,"default_due_time":"23:59:59","conditional_release":false,"default_student_gradebook_view":false}
+ {"allow_final_grade_override":false,"allow_student_discussion_topics":true,"allow_student_forum_attachments":true,"allow_student_discussion_editing":true,"allow_student_discussion_reporting":true,"allow_student_anonymous_discussion_topics":false,"use_default_discussion_settings":true,"default_discussion_settings":{},"filter_speed_grader_by_student_group":false,"grading_standard_enabled":false,"grading_standard_id":null,"grade_passback_setting":null,"allow_student_organized_groups":true,"hide_final_grades":false,"hide_distribution_graphs":false,"hide_sections_on_course_users_page":false,"lock_all_announcements":false,"usage_rights_required":false,"restrict_student_past_view":false,"restrict_student_future_view":false,"restrict_quantitative_data":null,"show_announcements_on_home_page":false,"home_page_announcement_limit":null,"syllabus_course_summary":true,"homeroom_course":false,"image_url":null,"image_id":null,"image":null,"banner_image_url":null,"banner_image_id":null,"banner_image":null,"course_color":null,"friendly_name":null,"default_due_time":"23:59:59","conditional_release":false,"default_student_gradebook_view":null}
```

### courses-get-courses-student-view-student

Arguments: `GET /api/v1/courses/1/student_view_student`

at status (200→403)

status:

```diff
expected 200, got 403
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-store"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-store
```

at body $.id (missing key)

body:

```diff
- {"id":2,"name":"Test Student","created_at":"2026-11-26T20:48:21Z","sortable_name":"Student, Test","short_name":"Test Student","sis_user_id":null,"integration_id":null,"login_id":"539353094aa82e6b2a46ffe81f8518fb57a4407e"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-get-courses-students

Arguments: `GET /api/v1/courses/1/students`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":2,"name":"Test Student","created_at":"2026-11-26T20:48:21Z","sortable_name":"Student, Test","short_name":"Test Student","sis_user_id":null,"integration_id":null,"login_id":"539353094aa82e6b2a46ffe81f8518fb57a4407e"}]
+ []
```

### courses-get-courses-users

Arguments: `GET /api/v1/courses/1/users`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨i⟩jyKF%2B766lI0OWF…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]_[..].[..]; path=/; httponly
+ _csrf_token=ijyKF%2B766lI0OWFVkFPO0lTU3fjv9rs9POCR8revxw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### courses-get-courses-youtube-migration-conversion-status

Arguments: `GET /api/v1/courses/1/youtube_migration/conversion_status`

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

at body $.errors[0].message, char 0: expected "⟨A⟩n error occurred…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-get-courses-youtube-migration-scan

Arguments: `GET /api/v1/courses/1/youtube_migration/scan`

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

at body $.errors[0].message, char 0: expected "⟨A⟩n error occurred…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-post-courses-link-validation

Arguments: `POST /api/v1/courses/1/link_validation`

at status (200→403)

status:

```diff
expected 200, got 403
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨1⟩qQ4rGMjV9BQE%2B8…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=1qQ4rGMjV9BQE%2B88j7m1EXVWk%2FL9hlIj0M4AVagW9w%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### courses-post-courses-youtube-migration-convert

Arguments: `POST /api/v1/courses/1/youtube_migration/convert`

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

at body $.errors[0].message, char 0: expected "⟨A⟩n error occurred…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-post-courses-youtube-migration-scan

Arguments: `POST /api/v1/courses/1/youtube_migration/scan`

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

at body $.errors[0].message, char 0: expected "⟨A⟩n error occurred…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-put-accounts-courses

Arguments: `PUT /api/v1/accounts/6/courses`

at status (403→405)

status:

```diff
expected 403, got 405
```

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

at body (lines missing), line 1, length 93 expected, 0 got

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
(trailing newline: expected false, got true)
```

### courses-put-courses-quizzes

Arguments: `PUT /api/v1/courses/1/quizzes`

at status (204→403)

status:

```diff
expected 204, got 403
```

### courses-put-courses-settings

Arguments: `PUT /api/v1/courses/1/settings`

at status (200→403)

status:

```diff
expected 200, got 403
```

### courses-put-courses

Arguments: `PUT /api/v1/courses/1`

at status (200→403)

status:

```diff
expected 200, got 403
```

### discussion-topics-api-delete-groups-discussion-topics-subscribed

Arguments: `DELETE /api/v1/groups/1/discussion_topics/1/subscribed`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨V⟩4LN%2BapzGGr4FPZ…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D[..]_[..]_[..]_[..]-[..]-[..].[..]; path=/; httponly
+ _csrf_token=V4LN%2BapzGGr4FPZ4pXLr%2FrtXaSswKMafFXijQr8q5Q%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### discussion-topics-api-get-groups-discussion-topics-summaries

Arguments: `GET /api/v1/groups/1/discussion_topics/1/summaries`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨c⟩t2qvxPVvnhlIUcEy…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..] [..]_[..]_[..].[..].[..]; path=/; httponly
+ _csrf_token=ct2qvxPVvnhlIUcEy7rjPLLLCGwz4WoLhv1lSS9HzQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
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

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"update_count":0}
```

### discussion-topics-api-put-courses-discussion-topics-update-discussion-types

Arguments: `PUT /api/v1/courses/1/discussion_topics/update_discussion_types`

at status (404→400)

status:

```diff
expected 404, got 400
```

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"error":"Missing data"}
```

### enrollments-api-delete-courses-enrollments

Arguments: `DELETE /api/v1/courses/1/enrollments/1`

at status (200→403)

status:

```diff
expected 200, got 403
```

### enrollments-api-get-accounts-enrollments

Arguments: `GET /api/v1/accounts/6/enrollments/1`

at body $.course_section_id (type), expected number, got null

body:

```diff
- {"id":1,"root_account_id":6,"user_id":1,"course_id":1,"course_section_id":1,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T05:26:00Z","created_at":"2026-11-26T20:15:52Z","start_at":null,"end_at":null,"type":"TeacherEnrollment","enrollment_state":"completed","role":"TeacherEnrollment","role_id":44,"last_activity_at":"2026-11-27T05:25:53Z","last_attended_at":null,"total_activity_time":120,"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/1/users/1"}
+ {"associated_user_id":null,"course_id":1,"course_integration_id":null,"course_section_id":null,"created_at":"2026-11-27T09:15:28Z","end_at":null,"enrollment_state":"completed","html_url":"/courses/1/users/1","id":1,"last_activity_at":null,"last_attended_at":null,"limit_privileges_to_course_section":false,"role":"teacher","role_id":2,"root_account_id":6,"section_integration_id":null,"sis_account_id":null,"sis_course_id":null,"sis_section_id":null,"sis_user_id":null,"start_at":null,"total_activity_time":0,"type":"TeacherEnrollment","updated_at":"2026-11-27T10:26:18Z","user_id":1}
```

### enrollments-api-get-courses-enrollments

Arguments: `GET /api/v1/courses/1/enrollments`

at body $ (type), expected array, got object

body:

```diff
- [{"id":2,"root_account_id":6,"user_id":2,"course_id":1,"course_section_id":1,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T05:25:16Z","created_at":"2026-11-26T20:48:22Z","start_at":null,"end_at":null,"type":"StudentViewEnrollment","enrollment_state":"active","role":"StudentEnrollment","role_id":43,"grades":{"html_url":"http://127.0.0.1:[..]/courses/1/grades/2","current_grade":null,"current_score":null,"final_grade":null,"final_score":null,"unposted_current_score":null,"unposted_current_grade":null,"unposted_final_score":null,"unposted_final_grade":null},"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/1/users/2","user":{"id":2,"name":"Test Student","created_at":"2026-11-26T20:48:21Z","sortable_name":"Student, Test","short_name":"Test Student","sis_user_id":null,"integration_id":null,"login_id":"539353094aa82e6b2a46ffe81f8518fb57a4407e"}}]
+ {"data":[{"id":1,"role":"teacher","user":{"id":1,"name":"Parity User","email":"parity@example.com"}}],"page":1,"per_page":50,"total":1}
```

### enrollments-api-get-sections-enrollments

Arguments: `GET /api/v1/sections/1/enrollments`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":2,"root_account_id":6,"user_id":2,"course_id":1,"course_section_id":1,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T05:25:16Z","created_at":"2026-11-26T20:48:22Z","start_at":null,"end_at":null,"type":"StudentViewEnrollment","enrollment_state":"active","role":"StudentEnrollment","role_id":43,"grades":{"html_url":"http://127.0.0.1:[..]/courses/1/grades/2","current_grade":null,"current_score":null,"final_grade":null,"final_score":null,"unposted_current_score":null,"unposted_current_grade":null,"unposted_final_score":null,"unposted_final_grade":null},"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/1/users/2","user":{"id":2,"name":"Test Student","created_at":"2026-11-26T20:48:21Z","sortable_name":"Student, Test","short_name":"Test Student","sis_user_id":null,"integration_id":null,"login_id":"539353094aa82e6b2a46ffe81f8518fb57a4407e"}}]
+ []
```

### enrollments-api-get-temporary-enrollment-status

Arguments: `GET /api/v1/temporary_enrollment_status`

at status (200→400)

status:

```diff
expected 200, got 400
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
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

at body (lines added), line 1, length 0 expected, 62 got

body:

```diff
+ {"can_provide":false,"is_provider":false,"is_recipient":false}
(trailing newline: expected true, got false)
```

### enrollments-api-post-accounts-bulk-enrollment

Arguments: `POST /api/v1/accounts/6/bulk_enrollment`

at body, line 1, column 82, char 81: expected "…_server_error\"}]⟨,⟩\"error_report_id…", got "…_server_error\"}]⟨}⟩"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
```

### enrollments-api-put-courses-enrollments-reactivate

Arguments: `PUT /api/v1/courses/1/enrollments/1/reactivate`

at status (403→400)

status:

```diff
expected 403, got 400
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"error":"enrollment not inactive"}
```

### enrollments-api-put-courses-users-last-attended

Arguments: `PUT /api/v1/courses/1/users/1/last_attended`

at status (500→400)

status:

```diff
expected 500, got 400
```

at body, line 1, column 3, char 2: expected "{\"⟨e⟩rrors\":[{\"messag…", got "{\"⟨m⟩essage\":\"Invalid…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"message":"Invalid date time input"}
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

### external-tools-delete-accounts-external-tools

Arguments: `DELETE /api/v1/accounts/6/external_tools/1`

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨i⟩S3XdCwxmO1Iibfiv…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D[..]_[..]_[..]-[..].[..]; path=/; httponly
+ _csrf_token=iS3XdCwxmO1Iibfiv98N0CXgSlKrMPjR6%2F%2BJ8n%2Firg%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### external-tools-get-accounts-external-tools-225

Arguments: `GET /api/v1/accounts/6/external_tools/1`

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

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### external-tools-get-accounts-external-tools-migration-info

Arguments: `GET /api/v1/accounts/6/external_tools/1/migration_info`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨z⟩VBHBzGkZee9y%2FD…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D[..]_[..]_[..]-[..]_[..]-[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=zVBHBzGkZee9y%2FDDTbn37a6MGlLd9jeT%2BOAUI%2B4WWA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### external-tools-get-accounts-external-tools-sessionless-launch

Arguments: `GET /api/v1/accounts/6/external_tools/sessionless_launch`

at status (400→503)

status:

```diff
expected 400, got 503
```

at body (lines missing), line 1, length 936 expected, 0 got

body:

```diff
- {"errors":{"id":[{"attribute":"id","message":"A tool id, tool url, module item id, or resource link lookup uuid must be provided","type":"A tool id, tool url, module item id, or resource link lookup uuid must be provided"}],"url":[{"attribute":"url","message":"A tool id, tool url, module item id, or resource link lookup uuid must be provided","type":"A tool id, tool url, module item id, or resource link lookup uuid must be provided"}],"module_item_id":[{"attribute":"module_item_id","message":"A tool id, tool url, module item id, or resource link lookup uuid must be provided","type":"A tool id, tool url, module item id, or resource link lookup uuid must be provided"}],"resource_link_lookup_uuid":[{"attribute":"resource_link_lookup_uuid","message":"A tool id, tool url, module item id, or resource link lookup uuid must be provided","type":"A tool id, tool url, module item id, or resource link lookup uuid must be provided"}]}}
(trailing newline: expected false, got true)
```

### external-tools-get-courses-external-tools-sessionless-launch

Arguments: `GET /api/v1/courses/1/external_tools/sessionless_launch`

at status (400→503)

status:

```diff
expected 400, got 503
```

at body (lines missing), line 1, length 936 expected, 0 got

body:

```diff
- {"errors":{"id":[{"attribute":"id","message":"A tool id, tool url, module item id, or resource link lookup uuid must be provided","type":"A tool id, tool url, module item id, or resource link lookup uuid must be provided"}],"url":[{"attribute":"url","message":"A tool id, tool url, module item id, or resource link lookup uuid must be provided","type":"A tool id, tool url, module item id, or resource link lookup uuid must be provided"}],"module_item_id":[{"attribute":"module_item_id","message":"A tool id, tool url, module item id, or resource link lookup uuid must be provided","type":"A tool id, tool url, module item id, or resource link lookup uuid must be provided"}],"resource_link_lookup_uuid":[{"attribute":"resource_link_lookup_uuid","message":"A tool id, tool url, module item id, or resource link lookup uuid must be provided","type":"A tool id, tool url, module item id, or resource link lookup uuid must be provided"}]}}
(trailing newline: expected false, got true)
```

### external-tools-get-courses-external-tools-visible-course-nav-tools

Arguments: `GET /courses/1/external_tools/visible_course_nav_tools`

at status (302→404)

status:

```diff
expected 302, got 404
```

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html; charse…", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html; charset=utf-8
+ application/json; charset=utf-8
```

at header location (missing key)

header location:

```diff
- http://127.0.0.1:[..]/login
(missing)
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

at status (403→415)

status:

```diff
expected 403, got 415
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
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

at status (403→415)

status:

```diff
expected 403, got 415
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### external-tools-post-courses-create-tool-with-verification

Arguments: `POST /api/v1/courses/1/create_tool_with_verification`

at status (403→415)

status:

```diff
expected 403, got 415
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### external-tools-post-courses-external-tools

Arguments: `POST /api/v1/courses/1/external_tools`

at status (403→415)

status:

```diff
expected 403, got 415
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### external-tools-put-accounts-external-tools

Arguments: `PUT /api/v1/accounts/6/external_tools/1`

at status (404→415)

status:

```diff
expected 404, got 415
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨E⟩xpected request …"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### external-tools-put-courses-external-tools

Arguments: `PUT /api/v1/courses/1/external_tools/1`

at status (404→415)

status:

```diff
expected 404, got 415
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨E⟩xpected request …"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### files-delete-files

Arguments: `DELETE /api/v1/files/1`

at status (401→404)

status:

```diff
expected 401, got 404
```

at body $.message (missing key)

body:

```diff
- {"message":"Unauthorized to delete this file"}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### files-get-courses-files

Arguments: `GET /api/v1/courses/1/files`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨b⟩Jl6oF6DshmVS01cw…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D[..]_[..]_[..].[..]_[..].[..]; path=/; httponly
+ _csrf_token=bJl6oF6DshmVS01cwQ5VuPGsRxIxIuwSsfACmB6TUQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### files-get-files-create-success

Arguments: `GET /api/v1/files/1/create_success`

at status (400→404)

status:

```diff
expected 400, got 404
```

at header access-control-allow-credentials (missing key)

header access-control-allow-credentials:

```diff
- true
(missing)
```

at header access-control-allow-headers (missing key)

header access-control-allow-headers:

```diff
- Origin, X-Requested-With, Content-Type, Accept, Authorization, Accept-Encoding
(missing)
```

at header access-control-allow-methods (missing key)

header access-control-allow-methods:

```diff
- POST, PUT, DELETE, GET, OPTIONS
(missing)
```

at header access-control-allow-origin (missing key)

header access-control-allow-origin:

```diff
- 
(missing)
```

at header access-control-request-method (missing key)

header access-control-request-method:

```diff
- *
(missing)
```

at header content-type, line 1, column 17, char 16: expected "application/json⟨⟩", got "application/json⟨;⟩ charset=utf-8"

header content-type:

```diff
- application/json
+ application/json; charset=utf-8
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### files-get-files-public-url

Arguments: `GET /api/v1/files/1/public_url`

at status (200→404)

status:

```diff
expected 200, got 404
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body $.public_url (missing key)

body:

```diff
- {"public_url":"http://54.252.48.186/courses/1/files/1/download?verifier=3dtpvsYjTYBROrRYKt6gGxrBsVGGHNIZ7lHXluAv"}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### files-get-users-files-quota

Arguments: `GET /api/v1/users/1/files/quota`

at status (200→500)

status:

```diff
expected 200, got 500
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body $.quota (missing key)

body:

```diff
- {"quota":100000000,"quota_used":0}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
```

### files-post-files-capture

Arguments: `POST /api/v1/files/capture`

at status (404→400)

status:

```diff
expected 404, got 400
```

at header content-type, line 1, column 17, char 16: expected "application/json⟨⟩", got "application/json⟨;⟩ charset=utf-8"

header content-type:

```diff
- application/json
+ application/json; charset=utf-8
```

at body (lines added), line 1, length 0 expected, 73 got

body:

```diff
+ {"errors":[{"message":"Invalid URL: Cannot parse `capture` to a `i64`"}]}
(trailing newline: expected true, got false)
```

### files-post-files-create-success

Arguments: `POST /api/v1/files/1/create_success`

at status (400→404)

status:

```diff
expected 400, got 404
```

at header access-control-allow-credentials (missing key)

header access-control-allow-credentials:

```diff
- true
(missing)
```

at header access-control-allow-headers (missing key)

header access-control-allow-headers:

```diff
- Origin, X-Requested-With, Content-Type, Accept, Authorization, Accept-Encoding
(missing)
```

at header access-control-allow-methods (missing key)

header access-control-allow-methods:

```diff
- POST, PUT, DELETE, GET, OPTIONS
(missing)
```

at header access-control-allow-origin (missing key)

header access-control-allow-origin:

```diff
- 
(missing)
```

at header access-control-request-method (missing key)

header access-control-request-method:

```diff
- *
(missing)
```

at header content-type, line 1, column 17, char 16: expected "application/json⟨⟩", got "application/json⟨;⟩ charset=utf-8"

header content-type:

```diff
- application/json
+ application/json; charset=utf-8
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### files-post-files-reset-verifier

Arguments: `POST /api/v1/files/1/reset_verifier`

at status (403→404)

status:

```diff
expected 403, got 404
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### files-post-files-update-word-count

Arguments: `POST /api/v1/files/update_word_count`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨%⟩2FOuMHwaGC9tvuuc…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]_[..].[..].[..]; path=/; httponly
+ _csrf_token=%2FOuMHwaGC9tvuucvufebFyWlTXvQFr3jTlIcaqyAEw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### files-put-files

Arguments: `PUT /api/v1/files/1`

at status (403→404)

status:

```diff
expected 403, got 404
```

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨e⟩mYWdQ%2FwGhhiIul…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]_[..]-[..].[..]; path=/; httponly
+ _csrf_token=emYWdQ%2FwGhhiIulBQ5PmBM7cfCytySibIER4fpSTTQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### folders-get-courses-folders-302

Arguments: `GET /api/v1/courses/1/folders/1`

at body $.name, char 0: expected "⟨c⟩ourse files", got "⟨P⟩arity Folder"

body:

```diff
- {"id":1,"name":"course files","full_name":"course files","position":null,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-26T20:29:16Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/1/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/1/files","files_count":0,"folders_count":2,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}
+ {"id":1,"name":"Parity Folder","full_name":"course files/Parity Folder","position":1,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T06:15:10.722372Z","updated_at":"2026-11-27T06:15:10.722372Z","category":null,"locked":false,"all_url":"/api/v1/folders/1/all","folders_url":"/api/v1/folders/1/folders","files_url":"/api/v1/folders/1/files","files_count":0,"folders_count":2,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}
```

### folders-get-courses-folders-and-files

Arguments: `GET /api/v1/courses/1/folders_and_files`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-courses-folders-by-path

Arguments: `GET /api/v1/courses/1/folders/by_path`

at body $[0].name, char 0: expected "⟨c⟩ourse files", got "⟨P⟩arity Folder"

body:

```diff
- [{"id":1,"name":"course files","full_name":"course files","position":null,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-26T20:29:16Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/1/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/1/files","files_count":0,"folders_count":2,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}]
+ [{"id":1,"name":"Parity Folder","full_name":"course files/Parity Folder","position":1,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T06:15:10.722372Z","updated_at":"2026-11-27T06:15:10.722372Z","category":null,"locked":false,"all_url":"/api/v1/folders/1/all","folders_url":"/api/v1/folders/1/folders","files_url":"/api/v1/folders/1/files","files_count":0,"folders_count":2,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}]
```

### folders-get-courses-folders-duplicates

Arguments: `GET /api/v1/courses/1/folders/1/duplicates`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-courses-folders-icon-maker

Arguments: `GET /api/v1/courses/1/folders/icon_maker`

at status (204→403)

status:

```diff
expected 204, got 403
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at body (lines added), line 1, length 0 expected, 93 got

body:

```diff
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
(trailing newline: expected true, got false)
```

### folders-get-courses-folders-media

Arguments: `GET /api/v1/courses/1/folders/media`

at body $.id (value): expected 6, got 10

body:

```diff
- {"id":6,"name":"Uploaded Media","full_name":"my files/Uploaded Media","position":1,"parent_folder_id":5,"context_type":"User","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:48:29Z","updated_at":"2026-11-26T20:48:29Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/6/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/6/files","files_count":0,"folders_count":0,"hidden":true,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"id":10,"name":"Uploaded Media","full_name":"course files/Parity Folder/Uploaded Media","position":2,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T10:26:19.434491Z","updated_at":"2026-11-27T10:26:19.434491Z","category":null,"locked":false,"all_url":"/api/v1/folders/10/all","folders_url":"/api/v1/folders/10/folders","files_url":"/api/v1/folders/10/files","files_count":0,"folders_count":0,"hidden":true,"locked_for_user":true,"hidden_for_user":true,"for_submissions":false,"can_upload":false}
```

### folders-get-courses-folders

Arguments: `GET /api/v1/courses/1/folders`

at body $[0].id (value): expected 1, got 8

body:

```diff
- [{"id":1,"name":"course files","full_name":"course files","position":null,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-26T20:29:16Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/1/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/1/files","files_count":0,"folders_count":2,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false},{"id":2,"name":"Parity Folder","full_name":"course files/Parity Folder","position":1,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-26T20:29:16Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/2/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/2/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false},{"id":3,"name":"unfiled","full_name":"course files/unfiled","position":2,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:48:22Z","updated_at":"2026-11-26T20:48:22Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/3/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/3/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}]
+ [{"id":8,"name":"New Folder","full_name":"course files/Parity Folder/unfiled/New Folder","position":1,"parent_folder_id":7,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T08:55:29.524599Z","updated_at":"2026-11-27T08:55:29.524599Z","category":null,"locked":false,"all_url":"/api/v1/folders/8/all","folders_url":"/api/v1/folders/8/folders","files_url":"/api/v1/folders/8/files","files_count":0,"folders_count":0,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false},{"id":1,"name":"Parity Folder","full_name":"course files/Parity Folder","position":1,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T06:15:10.722372Z","updated_at":"2026-11-27T06:15:10.722372Z","category":null,"locked":false,"all_url":"/api/v1/folders/1/all","folders_url":"/api/v1/folders/1/folders","files_url":"/api/v1/folders/1/files","files_count":0,"folders_count":2,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false},{"id":7,"name":"unfiled","full_name":"course files/Parity Folder/unfiled","position":1,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T08:55:29.494115Z","updated_at":"2026-11-27T08:55:29.494115Z","category":null,"locked":false,"all_url":"/api/v1/folders/7/all","folders_url":"/api/v1/folders/7/folders","files_url":"/api/v1/folders/7/files","files_count":0,"folders_count":1,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}]
```

### folders-get-folders-all

Arguments: `GET /api/v1/folders/1/all`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-folders-folders

Arguments: `GET /api/v1/folders/1/folders`

at body $[0].id (value): expected 2, got 7

body:

```diff
- [{"id":2,"name":"Parity Folder","full_name":"course files/Parity Folder","position":1,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-26T20:29:16Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/2/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/2/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false},{"id":3,"name":"unfiled","full_name":"course files/unfiled","position":2,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:48:22Z","updated_at":"2026-11-26T20:48:22Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/3/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/3/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}]
+ [{"id":7,"name":"unfiled","full_name":"course files/Parity Folder/unfiled","position":1,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T08:55:29.494115Z","updated_at":"2026-11-27T08:55:29.494115Z","category":null,"locked":false,"all_url":"/api/v1/folders/7/all","folders_url":"/api/v1/folders/7/folders","files_url":"/api/v1/folders/7/files","files_count":0,"folders_count":1,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}]
```

### folders-get-folders

Arguments: `GET /api/v1/folders/1`

at body $.name, char 0: expected "⟨c⟩ourse files", got "⟨P⟩arity Folder"

body:

```diff
- {"id":1,"name":"course files","full_name":"course files","position":null,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-26T20:29:16Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/1/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/1/files","files_count":0,"folders_count":2,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}
+ {"id":1,"name":"Parity Folder","full_name":"course files/Parity Folder","position":1,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T06:15:10.722372Z","updated_at":"2026-11-27T06:15:10.722372Z","category":null,"locked":false,"all_url":"/api/v1/folders/1/all","folders_url":"/api/v1/folders/1/folders","files_url":"/api/v1/folders/1/files","files_count":0,"folders_count":2,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}
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

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body $ (type), expected array, got object

body:

```diff
- [{"id":4,"name":"files","full_name":"files","position":null,"parent_folder_id":null,"context_type":"Group","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:48:28Z","updated_at":"2026-11-26T20:48:28Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/4/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/4/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}]
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### folders-get-groups-folders-duplicates

Arguments: `GET /api/v1/groups/1/folders/1/duplicates`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-groups-folders-media

Arguments: `GET /api/v1/groups/1/folders/media`

at body $.id (value): expected 6, got 12

body:

```diff
- {"id":6,"name":"Uploaded Media","full_name":"my files/Uploaded Media","position":1,"parent_folder_id":5,"context_type":"User","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:48:29Z","updated_at":"2026-11-26T20:48:29Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/6/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/6/files","files_count":0,"folders_count":0,"hidden":true,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"id":12,"name":"Uploaded Media","full_name":"my files/Uploaded Media","position":1,"parent_folder_id":11,"context_type":"User","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T10:26:19.564755Z","updated_at":"2026-11-27T10:26:19.564755Z","category":null,"locked":false,"all_url":"/api/v1/folders/12/all","folders_url":"/api/v1/folders/12/folders","files_url":"/api/v1/folders/12/files","files_count":0,"folders_count":0,"hidden":true,"locked_for_user":true,"hidden_for_user":true,"for_submissions":false,"can_upload":false}
```

### folders-get-groups-folders

Arguments: `GET /api/v1/groups/1/folders`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":4,"name":"files","full_name":"files","position":null,"parent_folder_id":null,"context_type":"Group","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:48:28Z","updated_at":"2026-11-26T20:48:28Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/4/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/4/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}]
+ []
```

### folders-get-users-folders-and-files

Arguments: `GET /api/v1/users/1/folders_and_files`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-users-folders-by-path-415

Arguments: `GET '/api/v1/users/1/folders/by_path/*full_path'`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨y⟩Y4d0V02fGAd39PCW…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D[..]_[..]_[..]-[..]_[..].[..]-[..].[..]; path=/; httponly
+ _csrf_token=yY4d0V02fGAd39PCWexFsOBm5AvcwD47tuzj3weePQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### folders-get-users-folders-by-path

Arguments: `GET /api/v1/users/1/folders/by_path`

at body $[0].id (value): expected 5, got 11

body:

```diff
- [{"id":5,"name":"my files","full_name":"my files","position":null,"parent_folder_id":null,"context_type":"User","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:48:28Z","updated_at":"2026-11-26T20:48:28Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/5/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/5/files","files_count":0,"folders_count":2,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}]
+ [{"id":11,"name":"my files","full_name":"my files","position":1,"parent_folder_id":null,"context_type":"User","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T10:26:19.553352Z","updated_at":"2026-11-27T10:26:19.553352Z","category":null,"locked":false,"all_url":"/api/v1/folders/11/all","folders_url":"/api/v1/folders/11/folders","files_url":"/api/v1/folders/11/files","files_count":0,"folders_count":2,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}]
```

### folders-get-users-folders-duplicates

Arguments: `GET /api/v1/users/1/folders/1/duplicates`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-post-users-folders

Arguments: `POST /api/v1/users/1/folders`

at body, line 1, column 7, char 6: expected "{\"id\":⟨⟩", got "{\"id\":⟨1⟩5,\"name\":\"New Fo…"

body:

```diff
- {"id":[..],"name":"New Folder [..]","full_name":"my files/unfiled/New Folder [..]","position":[..],"parent_folder_id":7,"context_type":"User","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T05:26:[..]","updated_at":"2026-11-27T05:26:[..]","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/[..]/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/[..]/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"id":15,"name":"New Folder 2","full_name":"my files/unfiled/New Folder 2","position":2,"parent_folder_id":13,"context_type":"User","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T10:28:26.035109Z","updated_at":"2026-11-27T10:28:26.035109Z","category":null,"locked":false,"all_url":"/api/v1/folders/15/all","folders_url":"/api/v1/folders/15/folders","files_url":"/api/v1/folders/15/files","files_count":0,"folders_count":0,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
```

### group-categories-get-courses-group-categories-export-tags

Arguments: `GET /api/v1/courses/1/group_categories/export_tags`

at status (401→403)

status:

```diff
expected 401, got 403
```

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/plain; chars…", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/plain; charset=utf-8
+ application/json; charset=utf-8
```

at body, line 1, column 1, char 0: expected "⟨U⟩nauthorized", got "⟨{⟩\"status\":\"unauth…"

body:

```diff
- Unauthorized
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### group-categories-get-group-categories-export

Arguments: `GET /api/v1/group_categories/1/export`

at status (401→200)

status:

```diff
expected 401, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-store", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-store
+ max-age=0, private, must-revalidate
```

at header content-type, line 1, column 6, char 5: expected "text/⟨p⟩lain; charset=ut…", got "text/⟨c⟩sv"

header content-type:

```diff
- text/plain; charset=utf-8
+ text/csv
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

at body, line 1, column 1, char 0: expected "⟨U⟩nauthorized", got "⟨u⟩ser_id,name,emai…"

body:

```diff
- Unauthorized
+ user_id,name,email
(trailing newline: expected false, got true)
```

### group-categories-get-group-categories-groups

Arguments: `GET /api/v1/group_categories/1/groups`

at status (403→200)

status:

```diff
expected 403, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-store", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-store
+ max-age=0, private, must-revalidate
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ [{"id":1,"course_id":1,"name":"Parity Group","workflow_state":"active","join_level":"invitation_only","group_category_id":1,"created_at":"2026-11-27T06:15:10.715590Z","updated_at":"2026-11-27T06:15:10.715590Z"}]
```

### group-categories-get-group-categories

Arguments: `GET /api/v1/group_categories/1`

at status (403→200)

status:

```diff
expected 403, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-store", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-store
+ max-age=0, private, must-revalidate
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"id":1,"name":"Parity Category","role":null,"self_signup":null,"self_signup_end_at":null,"group_limit":null,"auto_leader":null,"context_type":"Course","course_id":1,"account_id":6,"non_collaborative":false,"protected":false,"allows_multiple_memberships":false,"is_member":false,"created_at":"2026-11-27T06:15:10.705575Z"}
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

### group-categories-post-courses-group-categories-bulk-manage-differentiation-tag

Arguments: `POST /api/v1/courses/1/group_categories/bulk_manage_differentiation_tag`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨U⟩8zr9Z4NdN1Kj535Q…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]-[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=U8zr9Z4NdN1Kj535Qhao9D%2B94fNMCDyo%2FHQpm%2FUM4Q%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### groups-get-courses-bulk-user-tags

Arguments: `GET /api/v1/courses/1/bulk_user_tags`

at status (403→200)

status:

```diff
expected 403, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-store", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-store
+ max-age=0, private, must-revalidate
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

at body $ (type), expected object, got array

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ []
```

### groups-get-courses-groups

Arguments: `GET /api/v1/courses/1/groups`

at body $[0].description (missing key)

body:

```diff
- [{"id":1,"name":"Parity Group","description":null,"is_public":false,"join_level":"parent_context_only","group_category_id":1,"max_membership":10,"created_at":"2026-11-26T20:29:16Z","members_count":0,"storage_quota_mb":50,"context_type":"Course","course_id":1,"context_name":"Parity Course","avatar_url":null,"role":null,"leader":null,"has_submission":false,"concluded":false,"non_collaborative":false}]
+ [{"id":1,"course_id":1,"name":"Parity Group","workflow_state":"active","join_level":"invitation_only","group_category_id":1,"created_at":"2026-11-27T06:15:10.715590Z","updated_at":"2026-11-27T06:15:10.715590Z"}]
```

### groups-get-groups-activity-stream-summary

Arguments: `GET /api/v1/groups/1/activity_stream/summary`

at body $ (type), expected array, got object

body:

```diff
- []
+ {"entries":[],"unread_count":0}
```

### groups-get-groups-activity-stream

Arguments: `GET /api/v1/groups/1/activity_stream`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨N⟩MZ3OVFi%2B9rTcJW…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]-[..]_[..]-[..].[..]; path=/; httponly
+ _csrf_token=NMZ3OVFi%2B9rTcJWjnKRS5c0dBF%2Fxuv6OvBWHhTHB7w%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### groups-get-groups-permissions

Arguments: `GET /api/v1/groups/1/permissions`

at body $.participate (missing key)

body:

```diff
- {"participate":false,"manage_calendar":false,"manage_course_content_add":false,"manage_course_content_edit":false,"manage_course_content_delete":false,"manage_files_add":false,"manage_files_edit":false,"manage_files_delete":false,"manage_wiki_create":false,"manage_wiki_delete":false,"manage_wiki_update":false,"post_to_forum":true,"create_collaborations":false,"create_forum":false,"read_forum":true,"read":true,"read_announcements":true,"read_roster":true,"view_unpublished_items":false,"read_files":true,"send_messages":false,"send_messages_all":false,"moderate_forum":false,"delete":false,"manage":false,"allow_course_admin_actions":false,"manage_students":false,"update":false,"create":false,"participate_as_student":false,"join":false,"leave":false,"create_conferences":false,"read_as_admin":true,"read_sis":true,"view_user_logins":true,"read_email_addresses":true}
+ {"create_content":false,"delete":false,"delete_content":false,"manage_roster":false,"modify_roster":false,"post_messages":false,"read_directory":false,"read_replies":false,"update_content":false}
```

### groups-get-groups

Arguments: `GET /api/v1/groups/1`

at body $.description (missing key)

body:

```diff
- {"id":1,"name":"Parity Group","description":null,"is_public":false,"join_level":"parent_context_only","group_category_id":1,"max_membership":10,"created_at":"2026-11-26T20:29:16Z","members_count":0,"storage_quota_mb":50,"context_type":"Course","course_id":1,"context_name":"Parity Course","avatar_url":null,"role":null,"leader":null,"has_submission":false,"concluded":false,"non_collaborative":false}
+ {"id":1,"course_id":1,"name":"Parity Group","workflow_state":"active","join_level":"invitation_only","group_category_id":1,"created_at":"2026-11-27T06:15:10.715590Z","updated_at":"2026-11-27T06:15:10.715590Z"}
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

at body, line 1, column 7, char 6: expected "{\"id\":⟨⟩", got "{\"id\":⟨2⟩,\"course_id\":nul…"

body:

```diff
- {"id":[..],"name":"[..]","description":null,"is_public":false,"join_level":"invitation_only","group_category_id":2,"max_membership":null,"created_at":"2026-11-27T05:26:[..]","members_count":1,"storage_quota_mb":50,"permissions":{"create_discussion_topic":true,"join":false,"create_announcement":true},"context_type":"Account","account_id":4,"context_name":"Default Account","avatar_url":null,"role":"communities","leader":null,"users":[{"id":1,"name":"Parity User","created_at":"2026-11-26T20:10:15Z","sortable_name":"User, Parity","short_name":"Parity User"}],"group_category":{"id":2,"name":"Communities","role":"communities","self_signup":null,"self_signup_end_at":null,"group_limit":null,"auto_leader":null,"created_at":"2026-11-26T20:48:31Z","context_type":"Account","account_id":4,"protected":true,"allows_multiple_memberships":true,"is_member":true},"has_submission":false,"concluded":false,"non_collaborative":false}
+ {"id":2,"course_id":null,"name":"5b58af14-1f13-4eec-9a78-24430058fca3","workflow_state":"available","join_level":"invitation_only","created_at":"2026-11-27T10:28:26.358692Z","updated_at":"2026-11-27T10:28:26.358692Z"}
```

### lti-registrations-delete-accounts-lti-registrations

Arguments: `DELETE /api/v1/accounts/6/lti_registrations/1`

at status (403→404)

status:

```diff
expected 403, got 404
```

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### lti-registrations-get-accounts-lti-registrations-232

Arguments: `GET /api/v1/accounts/6/lti_registrations/1`

at status (403→404)

status:

```diff
expected 403, got 404
```

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### lti-registrations-get-accounts-lti-registrations-by-utid

Arguments: `GET /api/v1/accounts/6/lti_registrations/by_utid/:utid`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-get-accounts-lti-registrations-check-domain-duplicates

Arguments: `GET /api/v1/accounts/6/lti_registrations/check_domain_duplicates`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-get-accounts-lti-registrations-latest-update-request

Arguments: `GET /api/v1/accounts/6/lti_registrations/1/latest_update_request`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-get-accounts-lti-registrations

Arguments: `GET /api/v1/accounts/6/lti_registrations`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-post-accounts-lti-registrations-bind

Arguments: `POST /api/v1/accounts/6/lti_registrations/1/bind`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-post-accounts-lti-registrations-configuration-validate

Arguments: `POST /api/v1/accounts/6/lti_registrations/configuration/validate`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-post-accounts-lti-registrations-install-from-template

Arguments: `POST /api/v1/accounts/6/lti_registrations/1/install_from_template`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### lti-registrations-post-accounts-lti-registrations

Arguments: `POST /api/v1/accounts/6/lti_registrations`

at status (403→415)

status:

```diff
expected 403, got 415
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨S⟩Rz5CcGR25ssndLWM…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%[..]-[..].[..]; path=/; httponly
+ _csrf_token=SRz5CcGR25ssndLWMxOngSFwr8QV6JHEPjdYUoKE%2Bw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### lti-registrations-put-accounts-lti-registrations

Arguments: `PUT /api/v1/accounts/6/lti_registrations/1`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### outcome-groups-api-get-accounts-outcome-groups-240

Arguments: `GET /api/v1/accounts/6/outcome_groups/1`

at status (200→404)

status:

```diff
expected 200, got 404
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨X⟩QHqf6zU7cLP4648X…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D[..]_[..]_[..]-[..]-[..].[..]-[..].[..]; path=/; httponly
+ _csrf_token=XQHqf6zU7cLP4648XLKLtT1bAXxWqgfSnxmZ7nSVOQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.id (missing key)

body:

```diff
- {"id":1,"title":"Parity Account","vendor_guid":null,"url":"/api/v1/accounts/6/outcome_groups/1","subgroups_url":"/api/v1/accounts/6/outcome_groups/1/subgroups","outcomes_url":"/api/v1/accounts/6/outcome_groups/1/outcomes","can_edit":false,"import_url":"/api/v1/accounts/6/outcome_groups/1/import","context_id":6,"context_type":"Account","description":null}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-get-accounts-outcome-groups-outcomes

Arguments: `GET /api/v1/accounts/6/outcome_groups/1/outcomes`

at status (200→500)

status:

```diff
expected 200, got 500
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/outcome_groups/1/outcomes?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/outcome_groups/1/outcomes?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/outcome_groups/1/outcomes?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
```

### outcome-groups-api-get-accounts-outcome-groups-subgroups

Arguments: `GET /api/v1/accounts/6/outcome_groups/1/subgroups`

at status (200→404)

status:

```diff
expected 200, got 404
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/outcome_groups/1/subgroups?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/outcome_groups/1/subgroups?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/outcome_groups/1/subgroups?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-get-accounts-outcome-groups

Arguments: `GET /api/v1/accounts/6/outcome_groups`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":1,"title":"Parity Account","vendor_guid":null,"url":"/api/v1/accounts/6/outcome_groups/1","subgroups_url":"/api/v1/accounts/6/outcome_groups/1/subgroups","outcomes_url":"/api/v1/accounts/6/outcome_groups/1/outcomes","can_edit":false,"import_url":"/api/v1/accounts/6/outcome_groups/1/import","context_id":6,"context_type":"Account","description":null}]
+ []
```

### outcome-groups-api-get-accounts-root-outcome-group

Arguments: `GET /api/v1/accounts/6/root_outcome_group`

at status (302→404)

status:

```diff
expected 302, got 404
```

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html; charse…", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html; charset=utf-8
+ application/json; charset=utf-8
```

at header location (missing key)

header location:

```diff
- http://127.0.0.1:[..]/api/v1/accounts/6/outcome_groups/1
(missing)
```

### outcome-groups-api-get-courses-outcome-groups-316

Arguments: `GET /api/v1/courses/1/outcome_groups/1`

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

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"can_edit":false,"context_id":1,"context_type":"Course","course_id":1,"created_at":"2026-11-27T10:26:20.294492Z","description":null,"id":1,"import_url":"/api/v1/courses/1/outcome_groups/1/import","outcomes_url":"/api/v1/courses/1/outcome_groups/1/outcomes","parent_outcome_group_id":null,"subgroups_url":"/api/v1/courses/1/outcome_groups/1/subgroups","title":"Parity Course","updated_at":"2026-11-27T10:26:20.294492Z","url":"/api/v1/courses/1/outcome_groups/1","vendor_guid":null,"workflow_state":"active"}
```

### outcome-groups-api-get-courses-outcome-groups-outcomes

Arguments: `GET /api/v1/courses/1/outcome_groups/1/outcomes`

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

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ []
```

### outcome-groups-api-get-courses-outcome-groups-subgroups

Arguments: `GET /api/v1/courses/1/outcome_groups/1/subgroups`

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

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ []
```

### outcome-groups-api-get-courses-outcome-groups

Arguments: `GET /api/v1/courses/1/outcome_groups`

at body $[0].id (value): expected 2, got 1

body:

```diff
- [{"id":2,"title":"Parity Course","vendor_guid":null,"url":"/api/v1/courses/1/outcome_groups/2","subgroups_url":"/api/v1/courses/1/outcome_groups/2/subgroups","outcomes_url":"/api/v1/courses/1/outcome_groups/2/outcomes","can_edit":false,"import_url":"/api/v1/courses/1/outcome_groups/2/import","context_id":1,"context_type":"Course","description":null}]
+ [{"id":1,"course_id":1,"context_type":"Course","context_id":1,"title":"Parity Course","workflow_state":"active","created_at":"2026-11-27T10:26:20.294492Z","updated_at":"2026-11-27T10:26:20.294492Z"}]
```

### outcome-groups-api-get-courses-root-outcome-group

Arguments: `GET /api/v1/courses/1/root_outcome_group`

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html; charse…", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html; charset=utf-8
+ application/json; charset=utf-8
```

at header location, line 1, column 1, char 0: expected "⟨h⟩ttp://127.0.0.1:", got "⟨/⟩api/v1/courses/1…"

header location:

```diff
- http://127.0.0.1:[..]/api/v1/courses/1/outcome_groups/2
+ /api/v1/courses/1/outcome_groups/1
```

at body (lines added), line 1, length 0 expected, 507 got

body:

```diff
+ {"can_edit":false,"context_id":1,"context_type":"Course","course_id":1,"created_at":"2026-11-27T10:26:20.294492Z","description":null,"id":1,"import_url":"/api/v1/courses/1/outcome_groups/1/import","outcomes_url":"/api/v1/courses/1/outcome_groups/1/outcomes","parent_outcome_group_id":null,"subgroups_url":"/api/v1/courses/1/outcome_groups/1/subgroups","title":"Parity Course","updated_at":"2026-11-27T10:28:26.616506Z","url":"/api/v1/courses/1/outcome_groups/1","vendor_guid":null,"workflow_state":"active"}
(trailing newline: expected true, got false)
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

at body, line 1, column 3, char 2: expected "{\"⟨e⟩rrors\":[{\"messag…", got "{\"⟨s⟩tatus\":\"unauthor…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### outcome-groups-api-get-global-outcome-groups-outcomes

Arguments: `GET /api/v1/global/outcome_groups/1/outcomes`

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

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ []
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

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/html; charse…", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/html; charset=utf-8
+ application/json; charset=utf-8
```

at header location (missing key)

header location:

```diff
- http://127.0.0.1:[..]/api/v1/global/outcome_groups/3
(missing)
```

at body (lines added), line 1, length 0 expected, 93 got

body:

```diff
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
(trailing newline: expected true, got false)
```

### outcome-groups-api-post-accounts-outcome-groups-outcomes

Arguments: `POST /api/v1/accounts/6/outcome_groups/1/outcomes`

at status (403→500)

status:

```diff
expected 403, got 500
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
```

### outcome-groups-api-post-accounts-outcome-groups-subgroups

Arguments: `POST /api/v1/accounts/6/outcome_groups/1/subgroups`

at status (403→405)

status:

```diff
expected 403, got 405
```

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

at body (lines missing), line 1, length 93 expected, 0 got

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
(trailing newline: expected false, got true)
```

### outcome-groups-api-put-accounts-outcome-groups-outcomes

Arguments: `PUT /api/v1/accounts/6/outcome_groups/1/outcomes/1`

at status (403→500)

status:

```diff
expected 403, got 500
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
```

### outcome-groups-api-put-courses-outcome-groups-outcomes

Arguments: `PUT /api/v1/courses/1/outcome_groups/1/outcomes/1`

at status (403→500)

status:

```diff
expected 403, got 500
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
```

### outcome-groups-api-put-global-outcome-groups-outcomes

Arguments: `PUT /api/v1/global/outcome_groups/1/outcomes/1`

at status (403→500)

status:

```diff
expected 403, got 500
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
```

### rubrics-api-get-accounts-rubrics

Arguments: `GET /api/v1/accounts/6/rubrics`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨x⟩AneJB5psXlvhb8Y%…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..] [..]_[..].[..]; path=/; httponly
+ _csrf_token=xAneJB5psXlvhb8Y%2BWoolLZaS01saUbwz0ZokcOYlg%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### rubrics-api-get-courses-rubrics

Arguments: `GET /api/v1/courses/1/rubrics`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨n⟩9oJ6s%2Bvge8I1rR…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..].[..]-[..]_[..]-[..].[..]; path=/; httponly
+ _csrf_token=n9oJ6s%2Bvge8I1rRVbEnoZNafJO9r92fJZyja9bG3eQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### rubrics-api-get-rubrics-upload-template

Arguments: `GET /api/v1/rubrics/upload_template`

at header content-disposition, line 1, column 50, char 49: expected "…ic_template.csv\"⟨;⟩ filename*=UTF-8…", got "…ic_template.csv\"⟨⟩"

header content-disposition:

```diff
- attachment; filename="import_rubric_template.csv"; filename*=UTF-8''import_rubric_template.csv
+ attachment; filename="import_rubric_template.csv"
```

at header content-transfer-encoding (missing key)

header content-transfer-encoding:

```diff
- binary
(missing)
```

### submissions-api-get-courses-assignments-gradeable-students

Arguments: `GET /api/v1/courses/1/assignments/1/gradeable_students`

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨u⟩l4a9ABQr32qtjrpJ…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=ul4a9ABQr32qtjrpJSp2r%2FiN0AXFJ0TSjIAYaDpV3w%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### submissions-api-get-courses-assignments-submission-summary

Arguments: `GET /api/v1/courses/1/assignments/1/submission_summary`

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨z⟩pfZS4df%2FS%2FNh…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]-[..].[..]-[..].[..]; path=/; httponly
+ _csrf_token=zpfZS4df%2FS%2FNhUyMA2WUEhdReHYj3nc271f7G0c0GA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### submissions-api-get-courses-assignments-submissions

Arguments: `GET /api/v1/courses/1/assignments/1/submissions`

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

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### submissions-api-get-sections-assignments-submission-summary

Arguments: `GET /api/v1/sections/1/assignments/1/submission_summary`

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

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### submissions-api-get-sections-assignments-submissions-rubric-comments-read

Arguments: `GET /api/v1/sections/1/assignments/1/submissions/1/rubric_comments/read`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨g⟩Pi16Z7khlLoll%2B…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..].[..].[..].[..]; path=/; httponly
+ _csrf_token=gPi16Z7khlLoll%2B6BzWpLOCSgko6QEcRaEeSI%2FbS%2BQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### submissions-api-get-sections-assignments-submissions

Arguments: `GET /api/v1/sections/1/assignments/1/submissions`

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨k⟩s668En%2FGXEKJzB…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D[..]_[..]_[..].[..]_[..]-[..].[..]; path=/; httponly
+ _csrf_token=ks668En%2FGXEKJzB7kfniw9Y9L7HilF4KogoDkkmMmA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### submissions-api-put-courses-assignments-submissions-read-62

Arguments: `PUT /api/v1/courses/1/assignments/1/submissions/1/read/:item`

at status (404→400)

status:

```diff
expected 404, got 400
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩nknown item: :it…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"errors":[{"message":"unknown item: :item"}]}
```

### submissions-api-put-courses-submissions-clear-unread

Arguments: `PUT /api/v1/courses/1/submissions/1/clear_unread`

at status (403→204)

status:

```diff
expected 403, got 204
```

at header cache-control, line 1, column 4, char 3: expected "no-⟨s⟩tore", got "no-⟨c⟩ache"

header cache-control:

```diff
- no-store
+ no-cache
```

at header content-type (missing key)

header content-type:

```diff
- application/json; charset=utf-8
(missing)
```

at header pragma (missing key)

header pragma:

```diff
- no-cache
(missing)
```

at body (lines missing), line 1, length 93 expected, 0 got

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
(trailing newline: expected false, got true)
```

### users-get-dashboard-dashboard-cards

Arguments: `GET /api/v1/dashboard/dashboard_cards`

at body $ (length), length 0 expected, 1 got

body:

```diff
- []
+ [{"longName":"Parity Course - PARITY","shortName":"PARITY","originalName":"Parity Course","courseCode":"PARITY","assetString":"course_1","href":"/courses/1","term":null,"subtitle":"enrolled as Teacher","enrollmentState":"completed","enrollmentType":"TeacherEnrollment","observee":null,"id":1,"isFavorited":false,"isK5Subject":false,"isHomeroom":false,"useClassicFont":false,"canManage":true,"canReadAnnouncements":true,"image":null,"color":null,"position":null,"published":true,"links":[{"css_class":"discussions","icon":null,"hidden":false,"path":"/courses/1/discussion_topics","label":"Discussions"},{"css_class":"assignments","icon":null,"hidden":false,"path":"/courses/1/assignments","label":"Assignments"},{"css_class":"announcements","icon":null,"hidden":false,"path":"/courses/1/announcements","label":"Announcements"},{"css_class":"files","icon":null,"hidden":false,"path":"/courses/1/files","label":"Files"}],"canChangeCoursePublishState":true,"defaultView":"modules","pagesUrl":"/courses/1/pages","frontPageTitle":null}]
```

### users-get-users-activity-stream

Arguments: `GET /api/v1/users/activity_stream`

at header link, line 1, column 19, char 18: expected "…ttp://127.0.0.1:⟨⟩", got "…ttp://127.0.0.1:⟨5⟩4743/api/v1/user…"

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/users/self/activity_stream?page=1&per_page=21>; rel="current",<http://127.0.0.1:[..]/api/v1/users/self/activity_stream?page=1&per_page=21>; rel="first",<http://127.0.0.1:[..]/api/v1/users/self/activity_stream?page=1&per_page=21>; rel="last"
+ <http://127.0.0.1:54743/api/v1/users/activity_stream?page=1&per_page=21>; rel="current",<http://127.0.0.1:54743/api/v1/users/activity_stream?page=1&per_page=21>; rel="first",<http://127.0.0.1:54743/api/v1/users/activity_stream?page=1&per_page=21>; rel="last"
```

### users-get-users-settings

Arguments: `GET /api/v1/users/1/settings`

at body $ (value)

body:

```diff
- {"manual_mark_as_read":false,"collapse_global_nav":false,"collapse_course_nav":false,"hide_dashcard_color_overlays":false,"release_notes_badge_disabled":false,"comment_library_suggestions_enabled":false,"elementary_dashboard_disabled":false,"default_to_block_editor":false,"widget_dashboard_user_preference":false,"widget_dashboard_dark_mode":false}
+ {"collapse_course_nav":false,"collapse_global_nav":false,"comment_library_suggestions_enabled":false,"default_to_block_editor":false,"elementary_dashboard_disabled":false,"hide_dashcard_color_overlays":false,"manual_mark_as_read":false,"release_notes_badge_disabled":false,"widget_dashboard_dark_mode":false,"widget_dashboard_user_preference":false}
```

### users-get-users

Arguments: `GET /api/v1/users/1`

at body $.created_at, char 9: expected "2026-11-2⟨6⟩T20:10:15Z", got "2026-11-2⟨7⟩T06:15:10.683720…"

body:

```diff
- {"id":1,"name":"Parity User","created_at":"2026-11-26T20:10:15Z","sortable_name":"User, Parity","short_name":"Parity User","last_name":"User","first_name":"Parity","locale":null,"effective_locale":"en","permissions":{"can_update_name":true,"can_update_avatar":false,"limit_parent_app_web_access":false}}
+ {"id":1,"name":"Parity User","sortable_name":"Parity User","email":"parity@example.com","role":"AccountAdmin","created_at":"2026-11-27T06:15:10.683720Z"}
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

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at body $.status (extra key)

body:

```diff
- {}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-post-users-files

Arguments: `POST /api/v1/users/1/files`

at status (500→422)

status:

```diff
expected 500, got 422
```

at body, line 1, column 24, char 23: expected "…s\":[{\"message\":\"⟨A⟩n error occurred…", got "…s\":[{\"message\":\"⟨f⟩ilename is requi…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"errors":[{"message":"filename is required"}]}
```

### users-post-users-self-pandata-events-token

Arguments: `POST /api/v1/users/self/pandata_events_token`

at status (403→400)

status:

```diff
expected 403, got 400
```

at body $.message, char 0: expected "⟨D⟩eveloper key not…", got "⟨A⟩ccess token requ…"

body:

```diff
- {"message":"Developer key not authorized"}
+ {"message":"Access token required"}
```

### users-put-users-dashboard-positions

Arguments: `PUT /api/v1/users/1/dashboard_positions`

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨i⟩udnKGF6s22zoiOrR…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]_[..]-[..].[..]_[..].[..]; path=/; httponly
+ _csrf_token=iudnKGF6s22zoiOrRgtxfMWFZnE1qvjh1OCbBxpZoQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body, line 1, column 3, char 2: expected "{\"⟨e⟩rrors\":[{\"messag…", got "{\"⟨d⟩ashboard_positio…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"dashboard_positions":{}}
```

### users-put-users-text-editor-preference

Arguments: `PUT /api/v1/users/1/text_editor_preference`

at status (422→200)

status:

```diff
expected 422, got 200
```

at header cache-control, line 1, column 1, char 0: expected "⟨n⟩o-cache", got "⟨m⟩ax-age=0, privat…"

header cache-control:

```diff
- no-cache
+ max-age=0, private, must-revalidate
```

at header content-type, line 1, column 17, char 16: expected "application/json⟨⟩", got "application/json⟨;⟩ charset=utf-8"

header content-type:

```diff
- application/json
+ application/json; charset=utf-8
```

at body (lines added), line 1, length 0 expected, 31 got

body:

```diff
+ {"text_editor_preference":null}
(trailing newline: expected true, got false)
```

### users-put-users

Arguments: `PUT /api/v1/users/1`

at body $.created_at, char 9: expected "2026-11-2⟨6⟩T20:10:15Z", got "2026-11-2⟨7⟩T06:15:10.683720…"

body:

```diff
- {"id":1,"name":"Parity User","created_at":"2026-11-26T20:10:15Z","sortable_name":"User, Parity","short_name":"Parity User","locale":null,"time_zone":"Mountain Time (US \u0026 Canada)"}
+ {"id":1,"name":"Parity User","sortable_name":"Parity User","email":"parity@example.com","role":"AccountAdmin","created_at":"2026-11-27T06:15:10.683720Z"}
```

### wiki-pages-api-get-courses-page-title-availability

Arguments: `GET /api/v1/courses/1/page_title_availability`

at status (404→400)

status:

```diff
expected 404, got 400
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨F⟩ailed to deseria…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"errors":[{"message":"Failed to deserialize query string: missing field `title`"}]}
```

### wiki-pages-api-get-groups-page-title-availability

Arguments: `GET /api/v1/groups/1/page_title_availability`

at status (404→400)

status:

```diff
expected 404, got 400
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨F⟩ailed to deseria…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"errors":[{"message":"Failed to deserialize query string: missing field `title`"}]}
```

### wiki-pages-api-post-courses-pages-ai-alt-text

Arguments: `POST /api/v1/courses/1/pages_ai/alt_text`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨%⟩2FfCqcqghpEKY2f4…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..].[..].[..]; path=/; httponly
+ _csrf_token=%2FfCqcqghpEKY2f4LlOT1KDjraJ%2BzQT2xH2BuA1nJ4A%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### wiki-pages-api-post-groups-pages-ai-alt-text

Arguments: `POST /api/v1/groups/1/pages_ai/alt_text`

at status (403→400)

status:

```diff
expected 403, got 400
```

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
+ {"errors":[{"message":"attachment_id is required"}]}
```
