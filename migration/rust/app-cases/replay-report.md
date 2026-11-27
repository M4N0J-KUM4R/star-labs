# App replay

Build: `target/debug/star-http`

Passed: 166/460

Fallback rate: not reported by this build

## Distinct differences

| Cases | Id | Where | Kind |
| ---: | --- | --- | --- |
| 181 | `d04de58a` | `header cache-control` | text |
| 42 | `59f5f378` | `status` | 200→404 |
| 34 | `59f5f078` | `status` | 200→403 |
| 30 | `4e1b1758` | `header link` | missing key |
| 29 | `84139eb4` | `header pragma` | missing key |
| 28 | `862e729b` | `body $` | type |
| 28 | `6b521040` | `body` | text |
| 27 | `e06d529c` | `header set-cookie` | text |
| 27 | `7ec02a16` | `body $.errors[].message` | value |
| 24 | `d506e5b1` | `body $.status` | missing key |
| 20 | `bf07e579` | `status` | 403→404 |
| 20 | `84764fab` | `status` | 404→403 |
| 17 | `8e66a14e` | `body $.id` | missing key |
| 17 | `5a352777` | `body` | lines added |
| 16 | `d088d7d7` | `body $.message` | missing key |
| 15 | `0da8054c` | `header content-type` | text |
| 13 | `1388bc4e` | `body $` | length |
| 13 | `ea00df59` | `status` | 400→403 |
| 13 | `9c827749` | `body $.errors` | type |
| 9 | `847351ab` | `status` | 404→415 |
| 8 | `451c2498` | `status` | 204→404 |
| 8 | `ea00de59` | `status` | 400→404 |
| 7 | `c99aa66e` | `body $.errors` | missing key |
| 6 | `59f28678` | `status` | 200→415 |
| 6 | `d62ae6e8` | `body` | lines missing |
| 6 | `96a328ab` | `status` | 404→200 |
| 5 | `363352ad` | `body $.error` | missing key |
| 4 | `b61a6028` | `status` | 500→200 |
| 4 | `847652ab` | `status` | 404→400 |
| 4 | `e9fdd959` | `status` | 400→415 |
| 4 | `7a1c3f36` | `body $.errors[].code` | missing key |
| 4 | `fbfed8ad` | `body $.url` | missing key |
| 3 | `ce5768af` | `body $.id` | value |
| 3 | `8caa558f` | `header content-type` | missing key |
| 3 | `946b26c3` | `header location` | missing key |
| 3 | `19fe84fe` | `status` | 401→200 |
| 3 | `7ad71360` | `body $.revision_id` | missing key |
| 2 | `bb192150` | `body $.status` | extra key |
| 2 | `8e5bed78` | `status` | 200→204 |
| 2 | `2867bf27` | `body $.created_at` | value |
| 2 | `e0eac759` | `status` | 400→500 |
| 2 | `5292d478` | `status` | 200→500 |
| 2 | `ea836b28` | `status` | 500→415 |
| 2 | `0191ac8c` | `status` | 201→404 |
| 2 | `59f5ef78` | `status` | 200→400 |
| 2 | `7a19789b` | `body $.migration_running` | missing key |
| 2 | `e0eac659` | `status` | 400→503 |
| 2 | `4cb102ff` | `status` | 401→404 |
| 2 | `16ab684f` | `header access-control-allow-credentials` | missing key |
| 2 | `f39553a0` | `header access-control-allow-headers` | missing key |
| 2 | `0895895f` | `header access-control-allow-methods` | missing key |
| 2 | `d150aa07` | `header access-control-allow-origin` | missing key |
| 2 | `06ffc01c` | `header access-control-request-method` | missing key |
| 2 | `e5f63a90` | `header content-transfer-encoding` | missing key |
| 2 | `3ccedb4f` | `status` | 422→403 |
| 2 | `34dd3427` | `body $.context_type` | missing key |
| 2 | `f810c4ea` | `status` | 302→403 |
| 2 | `0d525965` | `header link` | text |
| 2 | `ea7fe128` | `status` | 500→403 |
| 2 | `6591b220` | `body $.message` | value |
| 2 | `efc6d93d` | `body $.manual_mark_as_read` | missing key |
| 2 | `bf07e979` | `status` | 403→400 |
| 1 | `4cb105ff` | `status` | 401→403 |
| 1 | `04edf02b` | `body $.site_admin_self_token_create` | missing key |
| 1 | `5f045dfa` | `body $.quiz_ip_filters` | missing key |
| 1 | `187da715` | `body $.calendar_contexts_limit` | extra key |
| 1 | `8a77b5c6` | `body $.timetable` | extra key |
| 1 | `f8a8bd64` | `body $.account_id` | missing key |
| 1 | `8e791948` | `body $.status` | value |
| 1 | `f3bc1db5` | `body $["10"]` | missing key |
| 1 | `1c47c326` | `body $.id` | type |
| 1 | `e05b6599` | `body $.read_outcomes` | missing key |
| 1 | `16f83452` | `body $.use_default_discussion_settings` | value |
| 1 | `9a628c86` | `body $.created_at` | missing key |
| 1 | `4b262163` | `body $[].id` | value |
| 1 | `2fd06ba3` | `body $.success` | missing key |
| 1 | `7e12c68a` | `body $.html` | missing key |
| 1 | `ad489379` | `status` | 403→200 |
| 1 | `45192798` | `status` | 204→415 |
| 1 | `139c3d9d` | `body $.allow_final_grade_override` | missing key |
| 1 | `7d67a581` | `body $.unread_entries` | missing key |
| 1 | `5c20b18d` | `header x-request-context-id` | text |
| 1 | `59f5ee78` | `status` | 200→401 |
| 1 | `451c2998` | `status` | 204→403 |
| 1 | `ea7fe228` | `status` | 500→400 |
| 1 | `f810c9ea` | `status` | 302→404 |
| 1 | `2e708528` | `body $.quota` | value |
| 1 | `ea7fde28` | `status` | 500→404 |
| 1 | `00c26cb1` | `header content-disposition` | missing key |
| 1 | `fa3db4ca` | `body $.duplicates` | missing key |
| 1 | `bfa50d61` | `body $.total` | missing key |
| 1 | `e2775cc2` | `body $.errors[]` | type |
| 1 | `171679bc` | `header location` | text |
| 1 | `c7f2b919` | `header content-disposition` | text |
| 1 | `ad489779` | `status` | 403→204 |
| 1 | `2b07d2b3` | `body $[].shortName` | value |
| 1 | `c1869d73` | `body $.custom_colors` | missing key |
| 1 | `7e18373e` | `body $.dashboard_positions` | missing key |
| 1 | `ea07f059` | `status` | 400→422 |

## Failing cases

### accounts-get-accounts-237

Arguments: `GET /api/v1/accounts/6`

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
- {"id":6,"name":"Parity Account","parent_account_id":null,"root_account_id":null,"workflow_state":"active","uuid":"Cv5tdSkArkSNXmHkjaMkdL3BVtJoHONeyaMv2cC4","default_storage_quota_mb":500,"default_user_storage_quota_mb":50,"default_group_storage_quota_mb":50,"default_time_zone":"America/Denver","course_template_id":null}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
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

at body $.error (missing key)

body:

```diff
- {"error":"Not authorized to view accessibility data"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### accounts-get-accounts-courses

Arguments: `GET /api/v1/accounts/6/courses`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/courses?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/courses?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/courses?page=1&per_page=10>; rel="last"
(missing)
```

### accounts-get-accounts-help-links

Arguments: `GET /api/v1/accounts/6/help_links`

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

### accounts-get-accounts-permissions

Arguments: `GET /api/v1/accounts/6/permissions`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨I⟩%2BBG%2Bzu5cVluy…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%[..]%[..]_[..]-[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=I%2BBG%2Bzu5cVluy7XU%2BqYyZhejCUcUu%2BeE%2BeL70SBiQA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.site_admin_self_token_create (missing key)

body:

```diff
- {"site_admin_self_token_create":false,"become_user":true,"import_sis":true,"create_access_tokens":true,"delete_access_tokens":true,"view_user_generated_access_tokens":true,"manage_account_memberships":true,"manage_account_settings":true,"manage_alerts":true,"manage_catalog":false,"manage_courses_admin":true,"manage_courses_add":true,"create_courses":true,"manage_courses_publish":true,"manage_courses_conclude":true,"manage_courses_reset":false,"manage_courses_delete":false,"undelete_courses":true,"view_archived_courses":false,"manage_data_services":true,"manage_course_visibility":true,"manage_developer_keys":true,"modify_site_admin_developer_keys":false,"moderate_user_content":true,"view_feature_flags":true,"manage_feature_flags":true,"manage_frozen_assignments":true,"manage_global_outcomes":false,"manage_jobs":false,"manage_lti_registrations":true,"manage_release_notes":false,"manage_master_courses":true,"manage_nav_menu_links":false,"manage_role_overrides":true,"manage_storage_quotas":true,"manage_sis":true,"manage_site_settings":false,"manage_rate_limiting":false,"manage_internal_settings":false,"manage_user_logins":true,"manage_dsr_requests":false,"manage_user_observers":true,"read_course_content":true,"read_course_list":true,"read_messages":false,"reset_any_mfa":false,"view_course_changes":true,"view_error_reports":false,"view_grade_changes":true,"view_jobs":false,"view_notifications":false,"view_statistics":true,"create_collaborations":true,"create_conferences":true,"create_forum":true,"generate_observer_pairing_code":true,"import_outcomes":true,"manage_lti_add":true,"manage_lti_edit":true,"manage_lti_delete":true,"allow_course_admin_actions":true,"add_teacher_to_course":true,"remove_teacher_from_course":true,"add_ta_to_course":true,"remove_ta_from_course":true,"add_observer_to_course":true,"remove_observer_from_course":true,"add_designer_to_course":true,"remove_designer_from_course":true,"manage_assignments_add":true,"manage_assignments_edit":true,"manage_assignments_delete":false,"manage_account_calendar_visibility":true,"manage_account_calendar_events":true,"manage_calendar":true,"manage_course_content_add":true,"manage_course_content_edit":true,"manage_course_content_delete":true,"add_course_template":true,"edit_course_template":true,"delete_course_template":true,"manage_account_banks":true,"share_banks_with_subaccounts":false,"manage_files_add":true,"manage_files_edit":true,"manage_files_delete":true,"manage_grades":true,"manage_groups_add":true,"manage_groups_manage":true,"manage_groups_delete":true,"manage_tags_add":true,"manage_tags_manage":true,"manage_tags_delete":true,"manage_interaction_alerts":true,"manage_outcomes":true,"manage_proficiency_calculations":true,"manage_proficiency_scales":true,"manage_sections_add":true,"manage_sections_edit":true,"manage_sections_delete":true,"manage_students":true,"add_student_to_course":true,"remove_student_from_course":true,"temporary_enrollments_add":false,"temporary_enrollments_edit":false,"temporary_enrollments_delete":false,"manage_rubrics":true,"manage_wiki_create":true,"manage_wiki_update":true,"manage_wiki_delete":true,"moderate_forum":true,"new_quizzes_view_ip_address":true,"post_to_forum":true,"proxy_assignment_submission":false,"read_announcements":true,"read_email_addresses":true,"read_forum":true,"read_question_banks":true,"read_reports":true,"read_roster":true,"read_sis":true,"select_final_grade":true,"send_messages":true,"send_messages_all":true,"view_audit_trail":true,"view_all_grades":true,"view_group_pages":true,"view_quiz_answer_audits":true,"view_user_logins":true,"view_admin_analytics":false,"view_analytics_hub":false,"view_ask_questions_pinboards":false,"view_ask_questions_analytics":false,"manage_ask_questions_analytics_context":false,"view_students_in_need":false,"view_students_in_need_in_course":false,"view_course_readiness":false,"view_title_iv_financial_aid_report":false,"view_rsi_report":false,"view_accessibility_insights":false,"manage_impact":true,"manage_institutional_tags_view":false,"manage_institutional_tags_create":false,"manage_institutional_tags_edit":false,"access_oak":false,"access_oak_teacher":false,"block_editor_template_editor":false,"block_editor_global_template_editor":false,"new_quizzes_multiple_session_detection":true,"manage_users_in_bulk":false,"manage_rules_view":false,"manage_rules_add":false,"manage_rules_edit":false,"manage_rules_delete":false,"manage_course_details":false,"manage_course_navigation":false,"manage_course_feature_options":false,"edit_discussion_anonymity":false,"edit_discussion_options":false,"edit_discussion_views":false,"apply_default_discussion_options":false,"read":true,"read_as_admin":true,"manage":true,"update":true,"delete":true,"read_outcomes":true,"read_terms":true,"read_files":true,"launch_external_tool":true,"read_global_outcomes":false,"view_bounced_emails":false,"view_account_calendar_details":true,"make_submission_comments":false,"manage_grading_schemes":true}
+ {"become_user":false,"create_courses":false,"manage_account_memberships":false,"manage_admin_roles":false,"manage_alerts":false,"manage_courses_admin":false,"manage_feature_flags":false,"manage_settings":false,"manage_sis":false,"view_statistics":false}
```

### accounts-get-accounts-quiz-ip-filters

Arguments: `GET /api/v1/accounts/6/quiz_ip_filters`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/quiz_ip_filters?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/quiz_ip_filters?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/quiz_ip_filters?page=1&per_page=10>; rel="last"
(missing)
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨4⟩S3hLNaztJcegSHum…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..] [..].[..].[..]; path=/; httponly
+ _csrf_token=4S3hLNaztJcegSHumA2ZsnLw%2B%2BXobeqENGe0IyqTYA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.quiz_ip_filters (missing key)

body:

```diff
- {"quiz_ip_filters":[]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### accounts-get-accounts-settings

Arguments: `GET /api/v1/accounts/6/settings`

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

### accounts-get-accounts-sub-accounts

Arguments: `GET /api/v1/accounts/6/sub_accounts`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/sub_accounts?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/sub_accounts?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/sub_accounts?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- [{"id":8,"name":"Manually-Created Courses","parent_account_id":6,"root_account_id":null,"workflow_state":"active","uuid":"RJWXBiMSmjeSEzdeTKLvGvs7zSVmjxwEl2J3uP2i","default_storage_quota_mb":500,"default_user_storage_quota_mb":50,"default_group_storage_quota_mb":50,"default_time_zone":"America/Denver","course_template_id":null}]
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### accounts-get-accounts-terms-of-service

Arguments: `GET /api/v1/accounts/6/terms_of_service`

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
- {"id":6,"terms_type":"default","passive":true,"account_id":6,"content":null,"self_registration_type":"none"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### accounts-get-accounts

Arguments: `GET /api/v1/accounts`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- [{"id":6,"name":"Parity Account","parent_account_id":null,"root_account_id":null,"workflow_state":"active","uuid":"Cv5tdSkArkSNXmHkjaMkdL3BVtJoHONeyaMv2cC4","default_storage_quota_mb":500,"default_user_storage_quota_mb":50,"default_group_storage_quota_mb":50,"default_time_zone":"America/Denver","course_template_id":null}]
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### accounts-get-course-accounts

Arguments: `GET /api/v1/course_accounts`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":6,"name":"Parity Account","parent_account_id":null,"root_account_id":null,"workflow_state":"active","uuid":"Cv5tdSkArkSNXmHkjaMkdL3BVtJoHONeyaMv2cC4","default_time_zone":"America/Denver"}]
+ []
```

### accounts-get-course-creation-accounts

Arguments: `GET /api/v1/course_creation_accounts`

at body $ (length), length 2 expected, 0 got

body:

```diff
- [{"id":6,"name":"Parity Account","parent_account_id":null,"root_account_id":null,"workflow_state":"active","uuid":"Cv5tdSkArkSNXmHkjaMkdL3BVtJoHONeyaMv2cC4","default_storage_quota_mb":500,"default_user_storage_quota_mb":50,"default_group_storage_quota_mb":50,"default_time_zone":"America/Denver","course_template_id":null},{"id":8,"name":"Manually-Created Courses","parent_account_id":6,"root_account_id":null,"workflow_state":"active","uuid":"RJWXBiMSmjeSEzdeTKLvGvs7zSVmjxwEl2J3uP2i","default_storage_quota_mb":500,"default_user_storage_quota_mb":50,"default_group_storage_quota_mb":50,"default_time_zone":"America/Denver","course_template_id":null}]
+ []
```

### accounts-get-manageable-accounts

Arguments: `GET /api/v1/manageable_accounts`

at body $ (length), length 2 expected, 0 got

body:

```diff
- [{"id":6,"name":"Parity Account","parent_account_id":null,"root_account_id":null,"workflow_state":"active","uuid":"Cv5tdSkArkSNXmHkjaMkdL3BVtJoHONeyaMv2cC4","default_storage_quota_mb":500,"default_user_storage_quota_mb":50,"default_group_storage_quota_mb":50,"default_time_zone":"America/Denver","course_template_id":null},{"id":8,"name":"Manually-Created Courses","parent_account_id":6,"root_account_id":null,"workflow_state":"active","uuid":"RJWXBiMSmjeSEzdeTKLvGvs7zSVmjxwEl2J3uP2i","default_storage_quota_mb":500,"default_user_storage_quota_mb":50,"default_group_storage_quota_mb":50,"default_time_zone":"America/Denver","course_template_id":null}]
+ []
```

### accounts-get-manually-created-courses-account

Arguments: `GET /api/v1/manually_created_courses_account`

at body $.id (value): expected 9, got 2

body:

```diff
- {"id":9,"name":"Manually-Created Courses","parent_account_id":4,"root_account_id":null,"workflow_state":"active","uuid":"Hp6ITN1nNVCwGH4CJSgSm0pNkOaFWesKyzH8Uca2","default_time_zone":"America/Denver"}
+ {"id":2,"name":"Manually-Created Courses","parent_account_id":1,"root_account_id":1,"workflow_state":"active"}
```

### accounts-get-settings-environment

Arguments: `GET /api/v1/settings/environment`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨0⟩h1djBX72R8sLmCFF…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D[..]_[..]_[..]-[..]-[..]-[..].[..]; path=/; httponly
+ _csrf_token=0h1djBX72R8sLmCFFm0%2F5zRJvY924lt0BTFppSihqw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.calendar_contexts_limit (extra key)

body:

```diff
- {}
+ {"calendar_contexts_limit":3,"open_registration":true}
```

### accounts-put-accounts

Arguments: `PUT /api/v1/accounts/6`

at status (200→415)

status:

```diff
expected 200, got 415
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body $.id (missing key)

body:

```diff
- {"id":6,"name":"Parity Account","parent_account_id":null,"root_account_id":null,"workflow_state":"active","uuid":"Cv5tdSkArkSNXmHkjaMkdL3BVtJoHONeyaMv2cC4","default_storage_quota_mb":500,"default_user_storage_quota_mb":50,"default_group_storage_quota_mb":50,"default_time_zone":"America/Denver","course_template_id":null}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
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

### calendar-events-api-get-calendar-events-277

Arguments: `GET /api/v1/calendar_events/1`

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

at body $.id (missing key)

body:

```diff
- {"id":1,"created_at":"2026-11-27T08:49:56Z","updated_at":"2026-11-27T12:32:46Z","start_at":"2026-11-28T08:49:56Z","end_at":"2026-11-28T09:49:56Z","all_day":false,"all_day_date":null,"title":"Parity Event","workflow_state":"deleted","comments":null,"series_uuid":null,"rrule":null,"blackout_date":false,"location_address":null,"location_name":null,"type":"event","description":null,"child_events_count":0,"all_context_codes":"course_1","context_code":"course_1","context_name":"Parity Course","context_color":null,"parent_event_id":null,"hidden":false,"child_events":[],"url":"http://127.0.0.1:[..]/api/v1/calendar_events/1","html_url":"http://127.0.0.1:[..]/calendar?event_id=1\u0026include_contexts=course_1","duplicates":[],"important_dates":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### calendar-events-api-get-calendar-events-participants

Arguments: `GET /api/v1/calendar_events/1/participants`

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
- []
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### calendar-events-api-get-courses-calendar-events-timetable

Arguments: `GET /api/v1/courses/1/calendar_events/timetable`

at body $.timetable (extra key)

body:

```diff
- {}
+ {"timetable":[]}
```

### calendar-events-api-get-users-calendar-events

Arguments: `GET /api/v1/users/3/calendar_events`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/users/3/calendar_events?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/users/3/calendar_events?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/users/3/calendar_events?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### calendar-events-api-post-calendar-events-reservations-42

Arguments: `POST /api/v1/calendar_events/1/reservations/3`

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

### calendar-events-api-post-calendar-events-reservations

Arguments: `POST /api/v1/calendar_events/1/reservations`

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

at body, line 1, column 3, char 2: expected "{\"⟨e⟩rrors\":[{\"messag…", got "{\"⟨t⟩imetable_events\"…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"timetable_events":[]}
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

at body, line 1, column 3, char 2: expected "{\"⟨e⟩rrors\":[{\"messag…", got "{\"⟨t⟩imetable\":[]}"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"timetable":[]}
```

### calendar-events-api-put-calendar-events

Arguments: `PUT /api/v1/calendar_events/1`

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

### conversations-get-conversations-find-recipients

Arguments: `GET /api/v1/conversations/find_recipients`

at body $ (length), length 0 expected, 1 got

body:

```diff
- []
+ [{"id":"course_1","name":"Parity Course","permissions":{},"type":"context","user_count":3}]
```

### conversations-post-conversations-remove-messages

Arguments: `POST /api/v1/conversations/1/remove_messages`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨u⟩HgwF8ZY7qGhWjlHt…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%3D[..]_[..]_[..]-[..].[..]_[..].[..]; path=/; httponly
+ _csrf_token=uHgwF8ZY7qGhWjlHtM5fDzrNE7f2WAFKbEMGtCBvgQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### conversations-put-conversations-restore

Arguments: `PUT /api/v1/conversations/restore`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨Z⟩G07qVCoFoSJHWouf…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D[..]_[..]_[..]_[..]-[..]_[..]_[..]_[..].[..].[..]; path=/; httponly
+ _csrf_token=ZG07qVCoFoSJHWoufDpWI4ztQ9kbCpSE2PApSoZiYA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
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

at body $.message (missing key)

body:

```diff
- {"message":"Only \"delete\" and \"conclude\" events are allowed."}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-get-accounts-courses

Arguments: `GET /api/v1/accounts/6/courses/1`

at body $.account_id (missing key)

body:

```diff
- {"id":1,"name":"Parity Course","course_code":"PARITY","account_id":6,"created_at":"2026-11-26T20:10:16Z","start_at":null,"default_view":"modules","enrollment_term_id":6,"is_public":null,"grading_standard_id":null,"root_account_id":6,"uuid":"vjyH5dTcmA5kYHx39sliWmwHoWQjDUPZxcwwNz8f","license":null,"grade_passback_setting":null,"end_at":null,"public_syllabus":false,"public_syllabus_to_auth":false,"storage_quota_mb":500,"is_public_to_auth_users":false,"homeroom_course":false,"course_color":null,"friendly_name":null,"hide_final_grades":false,"apply_assignment_group_weights":false,"calendar":{"ics":"http://127.0.0.1:[..]/feeds/calendars/course_vjyH5dTcmA5kYHx39sliWmwHoWQjDUPZxcwwNz8f.ics"},"time_zone":"America/Denver","blueprint":false,"template":false,"sis_course_id":null,"sis_import_id":null,"integration_id":null,"enrollments":[{"type":"teacher","role":"TeacherEnrollment","role_id":44,"user_id":1,"enrollment_state":"active","limit_privileges_to_course_section":false}],"workflow_state":"available","restrict_enrollments_to_course_dates":false}
+ {"id":1,"name":"Parity Course","course_code":"PARITY","workflow_state":"available","created_at":"2026-11-27T14:23:43.613643Z","updated_at":"2026-11-27T14:23:43.613643Z"}
```

### courses-get-courses-289

Arguments: `GET /api/v1/courses/1`

at body $.created_at, char 9: expected "2026-11-2⟨6⟩T20:10:16Z", got "2026-11-2⟨7⟩T14:23:43.613643…"

body:

```diff
- {"id":1,"name":"Parity Course","course_code":"PARITY","account_id":6,"created_at":"2026-11-26T20:10:16Z","start_at":null,"default_view":"modules","enrollment_term_id":6,"is_public":null,"grading_standard_id":null,"root_account_id":6,"uuid":"vjyH5dTcmA5kYHx39sliWmwHoWQjDUPZxcwwNz8f","license":null,"grade_passback_setting":null,"end_at":null,"public_syllabus":false,"public_syllabus_to_auth":false,"storage_quota_mb":500,"is_public_to_auth_users":false,"homeroom_course":false,"course_color":null,"friendly_name":null,"hide_final_grades":false,"apply_assignment_group_weights":false,"calendar":{"ics":"http://127.0.0.1:[..]/feeds/calendars/course_vjyH5dTcmA5kYHx39sliWmwHoWQjDUPZxcwwNz8f.ics"},"time_zone":"America/Denver","blueprint":false,"template":false,"sis_course_id":null,"sis_import_id":null,"integration_id":null,"enrollments":[{"type":"teacher","role":"TeacherEnrollment","role_id":44,"user_id":1,"enrollment_state":"active","limit_privileges_to_course_section":false}],"workflow_state":"available","restrict_enrollments_to_course_dates":false}
+ {"account_id":6,"apply_assignment_group_weights":false,"blueprint":false,"calendar":{"ics":"http://127.0.0.1:51468/feeds/calendars/course_Course_44e9fbc419cd4e2091f0ccad7fd39c5681dc5158.ics"},"course_code":"PARITY","course_color":null,"created_at":"2026-11-27T14:23:43.613643Z","default_view":"modules","end_at":null,"enrollment_term_id":null,"friendly_name":null,"grade_passback_setting":null,"grading_standard_id":null,"hide_final_grades":false,"homeroom_course":false,"id":1,"is_public":null,"is_public_to_auth_users":null,"license":null,"name":"Parity Course","public_syllabus":null,"public_syllabus_to_auth":null,"restrict_enrollments_to_course_dates":false,"root_account_id":null,"start_at":null,"storage_quota_mb":null,"template":false,"time_zone":null,"uuid":"44e9fbc419cd4e2091f0ccad7fd39c5681dc5158","workflow_state":"available"}
```

### courses-get-courses-bulk-user-progress

Arguments: `GET /api/v1/courses/1/bulk_user_progress`

at status (400→500)

status:

```diff
expected 400, got 500
```

at body $.error (missing key)

body:

```diff
- {"error":{"message":"No progress available because this course is not module based (meaning, it does not have modules and module completion requirements)."}}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
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

at body $.status, char 0: expected "⟨b⟩ad_request", got "⟨u⟩nauthorized"

body:

```diff
- {"status":"bad_request","message":"Search term required"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-get-courses-effective-due-dates

Arguments: `GET /api/v1/courses/1/effective_due_dates`

at body $["10"] (missing key)

body:

```diff
- {"10":{"3":{"due_at":"2026-12-27T12:16:12Z","grading_period_id":null,"in_closed_grading_period":false},"8":{"due_at":"2026-12-27T12:16:12Z","grading_period_id":null,"in_closed_grading_period":false}}}
+ {}
```

### courses-get-courses-link-validation

Arguments: `GET /api/v1/courses/1/link_validation`

at body $.id (type), expected number, got string

body:

```diff
- {"id":2,"context_id":1,"context_type":"Course","user_id":null,"tag":"link_validation","completion":0.0,"workflow_state":"queued","created_at":"2026-11-26T20:48:22Z","updated_at":"2026-11-26T20:48:22Z","message":null,"url":"http://127.0.0.1:[..]/api/v1/progress/2"}
+ {"id":"lv-1-1","workflow_state":"completed","data":{"broken_links":[]}}
```

### courses-get-courses-permissions

Arguments: `GET /api/v1/courses/1/permissions`

at body $.read_outcomes (missing key)

body:

```diff
- {"read":true,"read_outcomes":true,"read_syllabus":true,"read_files":true,"site_admin_self_token_create":false,"become_user":true,"import_sis":true,"create_access_tokens":true,"delete_access_tokens":true,"view_user_generated_access_tokens":true,"manage_account_memberships":true,"manage_account_settings":true,"manage_alerts":true,"manage_catalog":false,"manage_courses_admin":true,"manage_courses_add":true,"manage_courses_publish":true,"manage_courses_conclude":true,"manage_courses_reset":false,"manage_courses_delete":false,"undelete_courses":true,"view_archived_courses":false,"manage_data_services":true,"manage_course_visibility":true,"manage_developer_keys":true,"modify_site_admin_developer_keys":false,"moderate_user_content":true,"view_feature_flags":true,"manage_feature_flags":true,"manage_frozen_assignments":true,"manage_global_outcomes":false,"manage_jobs":false,"manage_lti_registrations":true,"manage_release_notes":false,"manage_master_courses":true,"manage_nav_menu_links":false,"manage_role_overrides":true,"manage_storage_quotas":true,"manage_sis":true,"manage_site_settings":false,"manage_rate_limiting":false,"manage_internal_settings":false,"manage_user_logins":true,"manage_dsr_requests":false,"manage_user_observers":true,"read_course_content":true,"read_course_list":true,"read_messages":false,"reset_any_mfa":false,"view_course_changes":true,"view_error_reports":false,"view_grade_changes":true,"view_jobs":false,"view_notifications":false,"view_statistics":true,"create_collaborations":true,"create_conferences":true,"create_forum":true,"generate_observer_pairing_code":true,"import_outcomes":true,"manage_lti_add":true,"manage_lti_edit":true,"manage_lti_delete":true,"allow_course_admin_actions":true,"add_teacher_to_course":true,"remove_teacher_from_course":true,"add_ta_to_course":true,"remove_ta_from_course":true,"add_observer_to_course":true,"remove_observer_from_course":true,"add_designer_to_course":true,"remove_designer_from_course":true,"manage_assignments_add":true,"manage_assignments_edit":true,"manage_assignments_delete":true,"manage_account_calendar_visibility":true,"manage_account_calendar_events":true,"manage_calendar":true,"manage_course_content_add":true,"manage_course_content_edit":true,"manage_course_content_delete":true,"add_course_template":true,"edit_course_template":true,"delete_course_template":true,"manage_account_banks":true,"share_banks_with_subaccounts":false,"manage_files_add":true,"manage_files_edit":true,"manage_files_delete":true,"manage_grades":true,"manage_groups_add":true,"manage_groups_manage":true,"manage_groups_delete":true,"manage_tags_add":true,"manage_tags_manage":true,"manage_tags_delete":true,"manage_interaction_alerts":true,"manage_outcomes":true,"manage_proficiency_calculations":true,"manage_proficiency_scales":true,"manage_sections_add":true,"manage_sections_edit":true,"manage_sections_delete":true,"manage_students":true,"add_student_to_course":true,"remove_student_from_course":true,"temporary_enrollments_add":false,"temporary_enrollments_edit":false,"temporary_enrollments_delete":false,"manage_rubrics":true,"manage_wiki_create":true,"manage_wiki_update":true,"manage_wiki_delete":true,"moderate_forum":true,"new_quizzes_view_ip_address":true,"post_to_forum":true,"proxy_assignment_submission":false,"read_announcements":true,"read_email_addresses":true,"read_forum":true,"read_question_banks":true,"read_reports":true,"read_roster":true,"read_sis":true,"select_final_grade":true,"send_messages":true,"send_messages_all":true,"view_audit_trail":true,"view_all_grades":true,"view_group_pages":true,"view_quiz_answer_audits":true,"view_user_logins":true,"view_admin_analytics":false,"view_analytics_hub":false,"view_ask_questions_pinboards":false,"view_ask_questions_analytics":false,"manage_ask_questions_analytics_context":false,"view_students_in_need":false,"view_students_in_need_in_course":false,"view_course_readiness":false,"view_title_iv_financial_aid_report":false,"view_rsi_report":false,"view_accessibility_insights":false,"manage_impact":true,"manage_institutional_tags_view":false,"manage_institutional_tags_create":false,"manage_institutional_tags_edit":false,"access_oak":false,"access_oak_teacher":false,"block_editor_template_editor":false,"block_editor_global_template_editor":false,"new_quizzes_multiple_session_detection":true,"manage_users_in_bulk":false,"manage_rules_view":false,"manage_rules_add":false,"manage_rules_edit":false,"manage_rules_delete":false,"manage_course_details":false,"manage_course_navigation":false,"manage_course_feature_options":false,"edit_discussion_anonymity":false,"edit_discussion_options":false,"edit_discussion_views":false,"apply_default_discussion_options":false,"read_as_member":true,"participate_as_student":false,"read_grades":false,"reset_what_if_grades":false,"read_as_admin":true,"manage":true,"update":true,"view_unpublished_items":true,"read_rubrics":true,"use_student_view":true,"set_grading_scheme":true,"manage_grading_schemes":true,"reset_content":false,"delete":false,"read_prior_roster":true,"direct_share":true,"edit_course_availability":true,"update_nav":true,"update_course_details":true}
+ {"read":true,"read_roster":true,"view_all_grades":true,"manage_grades":true,"update":true,"delete":false,"manage":true,"use_student_view":true}
```

### courses-get-courses-recent-students

Arguments: `GET /api/v1/courses/1/recent_students`

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
- <http://127.0.0.1:[..]/api/v1/courses/1/recent_students?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/courses/1/recent_students?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/courses/1/recent_students?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- [{"id":3,"name":"Parity Student","created_at":"2026-11-27T05:29:37Z","sortable_name":"Student, Parity","short_name":"Parity Student","sis_user_id":null,"integration_id":null,"login_id":"student@example.com","last_login":null}]
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
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

at body $.created_at (missing key)

body:

```diff
- {"id":8,"name":"Test Student","created_at":"2026-11-27T13:51:40Z","sortable_name":"Student, Test","short_name":"Test Student","sis_user_id":null,"integration_id":null,"login_id":"4c96f8dbf95063694a3d07b5f26dd8587cba9abb"}
+ {"id":8,"name":"Test Student","type":"student"}
```

### courses-get-courses-todo

Arguments: `GET /api/v1/courses/1/todo`

at body $ (type), expected array, got object

body:

```diff
- []
+ {"data":[],"page":1,"per_page":50,"total":0}
```

### courses-get-courses-users-362

Arguments: `GET /api/v1/courses/1/users/1`

at body $.created_at, char 9: expected "2026-11-2⟨6⟩T20:10:15Z", got "2026-11-2⟨7⟩T14:23:43.613643…"

body:

```diff
- {"id":1,"name":"Parity User","created_at":"2026-11-26T20:10:15Z","sortable_name":"User, Parity","short_name":"Parity User","sis_user_id":null,"integration_id":null,"login_id":"parity@example.com"}
+ {"id":1,"name":"Parity User","sortable_name":"User, Parity","short_name":"Parity User","email":"parity@example.com","role":"teacher","created_at":"2026-11-27T14:23:43.613643Z"}
```

### courses-get-courses-users-progress

Arguments: `GET /api/v1/courses/1/users/3/progress`

at status (400→500)

status:

```diff
expected 400, got 500
```

at body $.error (missing key)

body:

```diff
- {"error":{"message":"no progress available because this course is not module based (has modules and module completion requirements) or the user is not enrolled as a student in this course"}}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
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

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
+ {"status":"not_started","youtube_video_ids":[]}
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

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
+ {"pages_found":0,"youtube_video_ids":[]}
```

### courses-get-courses

Arguments: `GET /api/v1/courses`

at body $[0].id (value): expected 1, got 5

body:

```diff
- [{"id":1,"name":"Parity Course","course_code":"PARITY","account_id":6,"created_at":"2026-11-26T20:10:16Z","start_at":null,"default_view":"modules","enrollment_term_id":6,"is_public":null,"grading_standard_id":null,"root_account_id":6,"uuid":"vjyH5dTcmA5kYHx39sliWmwHoWQjDUPZxcwwNz8f","license":null,"grade_passback_setting":null,"end_at":null,"public_syllabus":false,"public_syllabus_to_auth":false,"storage_quota_mb":500,"is_public_to_auth_users":false,"homeroom_course":false,"course_color":null,"friendly_name":null,"apply_assignment_group_weights":false,"calendar":{"ics":"http://127.0.0.1:[..]/feeds/calendars/course_vjyH5dTcmA5kYHx39sliWmwHoWQjDUPZxcwwNz8f.ics"},"time_zone":"America/Denver","blueprint":false,"template":false,"sis_course_id":null,"sis_import_id":null,"integration_id":null,"enrollments":[{"type":"teacher","role":"TeacherEnrollment","role_id":44,"user_id":1,"enrollment_state":"active","limit_privileges_to_course_section":false}],"hide_final_grades":false,"workflow_state":"available","restrict_enrollments_to_course_dates":false}]
+ [{"account_id":6,"apply_assignment_group_weights":false,"blueprint":false,"calendar":{"ics":"http://127.0.0.1:51468/feeds/calendars/course_Course_f45ef8d7b8aa40a39de04c293078d957194d2033.ics"},"course_code":"PARITY","course_color":null,"created_at":"2026-11-27T14:23:43.613643Z","default_view":"modules","end_at":null,"enrollment_term_id":null,"friendly_name":null,"grade_passback_setting":null,"grading_standard_id":null,"hide_final_grades":false,"homeroom_course":false,"id":5,"is_public":null,"is_public_to_auth_users":null,"license":null,"name":"Parity Course","public_syllabus":null,"public_syllabus_to_auth":null,"restrict_enrollments_to_course_dates":false,"root_account_id":null,"start_at":null,"storage_quota_mb":null,"template":false,"time_zone":null,"uuid":"f45ef8d7b8aa40a39de04c293078d957194d2033","workflow_state":"available"},{"account_id":6,"apply_assignment_group_weights":false,"blueprint":false,"calendar":{"ics":"http://127.0.0.1:51468/feeds/calendars/course_Course_38f5ca9f52c649b9a78ea5628545b1ef1aa0f58c.ics"},"course_code":"PARITY","course_color":null,"created_at":"2026-11-27T14:23:43.613643Z","default_view":"modules","end_at":null,"enrollment_term_id":null,"friendly_name":null,"grade_passback_setting":null,"grading_standard_id":null,"hide_final_grades":false,"homeroom_course":false,"id":3,"is_public":null,"is_public_to_auth_users":null,"license":null,"name":"Parity Course","public_syllabus":null,"public_syllabus_to_auth":null,"restrict_enrollments_to_course_dates":false,"root_account_id":null,"start_at":null,"storage_quota_mb":null,"template":false,"time_zone":null,"uuid":"38f5ca9f52c649b9a78ea5628545b1ef1aa0f58c","workflow_state":"available"},{"account_id":6,"apply_assignment_group_weights":false,"blueprint":false,"calendar":{"ics":"http://127.0.0.1:51468/feeds/calendars/course_Course_44e9fbc419cd4e2091f0ccad7fd39c5681dc5158.ics"},"course_code":"PARITY","course_color":null,"created_at":"2026-11-27T14:23:43.613643Z","default_view":"modules","end_at":null,"enrollment_term_id":null,"friendly_name":null,"grade_passback_setting":null,"grading_standard_id":null,"hide_final_grades":false,"homeroom_course":false,"id":1,"is_public":null,"is_public_to_auth_users":null,"license":null,"name":"Parity Course","public_syllabus":null,"public_syllabus_to_auth":null,"restrict_enrollments_to_course_dates":false,"root_account_id":null,"start_at":null,"storage_quota_mb":null,"template":false,"time_zone":null,"uuid":"44e9fbc419cd4e2091f0ccad7fd39c5681dc5158","workflow_state":"available"}]
```

### courses-get-users-courses

Arguments: `GET /api/v1/users/3/courses`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/users/3/courses?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/users/3/courses?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/users/3/courses?page=1&per_page=10>; rel="last"
(missing)
```

### courses-post-accounts-courses

Arguments: `POST /api/v1/accounts/6/courses`

at status (200→415)

status:

```diff
expected 200, got 415
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":[..],"name":"Unnamed Course","course_code":"Unnamed","account_id":6,"created_at":"2026-11-27T14:12:[..]","start_at":null,"default_view":"modules","enrollment_term_id":6,"is_public":null,"grading_standard_id":null,"root_account_id":6,"uuid":"[..]","license":null,"grade_passback_setting":null,"conclude_at":null,"allow_wiki_comments":null,"allow_student_forum_attachments":true,"open_enrollment":null,"self_enrollment":null,"restrict_enrollments_to_course_dates":false,"end_at":null,"public_syllabus":false,"public_syllabus_to_auth":false,"storage_quota_mb":500,"is_public_to_auth_users":false,"homeroom_course":false,"course_color":null,"friendly_name":null,"hide_final_grades":false,"apply_assignment_group_weights":false,"calendar":{"ics":"http://127.0.0.1:[..]/feeds/calendars/course_[..].ics"},"time_zone":"America/Denver","blueprint":false,"template":false,"sis_course_id":null,"sis_import_id":null,"integration_id":null,"workflow_state":"unpublished"}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
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

at body $.message (missing key)

body:

```diff
- {"message":"Quiz migration alert not found"}
+ {"success":true}
```

### courses-post-courses-files

Arguments: `POST /api/v1/courses/1/files`

at body, line 1, column 82, char 81: expected "…_server_error\"}]⟨,⟩\"error_report_id…", got "…_server_error\"}]⟨}⟩"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
```

### courses-post-courses-link-validation

Arguments: `POST /api/v1/courses/1/link_validation`

at status (200→415)

status:

```diff
expected 200, got 415
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body $.success (missing key)

body:

```diff
- {"success":true}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### courses-post-courses-preview-html

Arguments: `POST /api/v1/courses/1/preview_html`

at status (200→415)

status:

```diff
expected 200, got 415
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body $.html (missing key)

body:

```diff
- {"html":""}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### courses-post-courses-reset-content

Arguments: `POST /api/v1/courses/1/reset_content`

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
+ {"success":true}
```

### courses-post-courses-youtube-migration-convert

Arguments: `POST /api/v1/courses/1/youtube_migration/convert`

at status (500→415)

status:

```diff
expected 500, got 415
```

at body $.errors[0].message, char 0: expected "⟨A⟩n error occurred…", got "⟨E⟩xpected request …"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### courses-post-courses-youtube-migration-scan

Arguments: `POST /api/v1/courses/1/youtube_migration/scan`

at status (500→415)

status:

```diff
expected 500, got 415
```

at body $.errors[0].message, char 0: expected "⟨A⟩n error occurred…", got "⟨E⟩xpected request …"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
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

at body $.message (missing key)

body:

```diff
- {"message":"must specify course_ids[]"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### courses-put-courses-quizzes

Arguments: `PUT /api/v1/courses/1/quizzes`

at status (204→415)

status:

```diff
expected 204, got 415
```

at body (lines added), line 1, length 0 expected, 81 got

body:

```diff
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
(trailing newline: expected true, got false)
```

### courses-put-courses-settings

Arguments: `PUT /api/v1/courses/1/settings`

at status (200→415)

status:

```diff
expected 200, got 415
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body $.allow_final_grade_override (missing key)

body:

```diff
- {"allow_final_grade_override":false,"allow_student_discussion_topics":true,"allow_student_forum_attachments":true,"allow_student_discussion_editing":true,"allow_student_discussion_reporting":true,"allow_student_anonymous_discussion_topics":false,"use_default_discussion_settings":false,"default_discussion_settings":{},"filter_speed_grader_by_student_group":false,"grading_standard_enabled":false,"grading_standard_id":null,"grade_passback_setting":null,"allow_student_organized_groups":true,"hide_final_grades":false,"hide_distribution_graphs":false,"hide_sections_on_course_users_page":false,"lock_all_announcements":false,"usage_rights_required":false,"restrict_student_past_view":false,"restrict_student_future_view":false,"restrict_quantitative_data":false,"show_announcements_on_home_page":false,"home_page_announcement_limit":null,"syllabus_course_summary":true,"homeroom_course":false,"image_url":null,"image_id":null,"image":null,"banner_image_url":null,"banner_image_id":null,"banner_image":null,"course_color":null,"friendly_name":null,"default_due_time":"23:59:59","conditional_release":false,"default_student_gradebook_view":false}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### courses-put-courses

Arguments: `PUT /api/v1/courses/1`

at status (200→415)

status:

```diff
expected 200, got 415
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body $.id (missing key)

body:

```diff
- {"id":1,"name":"Parity Course","course_code":"PARITY","account_id":6,"created_at":"2026-11-26T20:10:16Z","start_at":null,"default_view":"modules","enrollment_term_id":6,"is_public":null,"grading_standard_id":null,"root_account_id":6,"uuid":"vjyH5dTcmA5kYHx39sliWmwHoWQjDUPZxcwwNz8f","license":null,"grade_passback_setting":null,"end_at":null,"public_syllabus":false,"public_syllabus_to_auth":false,"storage_quota_mb":500,"is_public_to_auth_users":false,"homeroom_course":false,"course_color":null,"friendly_name":null,"hide_final_grades":false,"apply_assignment_group_weights":false,"calendar":{"ics":"http://127.0.0.1:[..]/feeds/calendars/course_vjyH5dTcmA5kYHx39sliWmwHoWQjDUPZxcwwNz8f.ics"},"time_zone":"America/Denver","blueprint":false,"template":false,"sis_course_id":null,"sis_import_id":null,"integration_id":null,"workflow_state":"available","restrict_enrollments_to_course_dates":false}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### discussion-topics-api-delete-courses-discussion-topics-entries-read

Arguments: `DELETE /api/v1/courses/1/discussion_topics/1/entries/1/read`

at status (204→404)

status:

```diff
expected 204, got 404
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### discussion-topics-api-delete-courses-discussion-topics-read-all

Arguments: `DELETE /api/v1/courses/1/discussion_topics/1/read_all`

at status (204→404)

status:

```diff
expected 204, got 404
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### discussion-topics-api-delete-courses-discussion-topics-read

Arguments: `DELETE /api/v1/courses/1/discussion_topics/1/read`

at status (204→404)

status:

```diff
expected 204, got 404
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨m⟩L24QFEKlUOjhFfHe…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D[..]_[..]_[..].[..].[..]; path=/; httponly
+ _csrf_token=mL24QFEKlUOjhFfHeqxWEZ%2Fd61ugJtEDdaI11f97Rg%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### discussion-topics-api-delete-courses-discussion-topics-subscribed

Arguments: `DELETE /api/v1/courses/1/discussion_topics/1/subscribed`

at status (204→404)

status:

```diff
expected 204, got 404
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### discussion-topics-api-get-courses-discussion-topics-entries-replies

Arguments: `GET /api/v1/courses/1/discussion_topics/1/entries/1/replies`

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
- <http://127.0.0.1:[..]/api/v1/courses/1/discussion_topics/1/entries/1/replies?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/courses/1/discussion_topics/1/entries/1/replies?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/courses/1/discussion_topics/1/entries/1/replies?page=1&per_page=10>; rel="last"
(missing)
```

### discussion-topics-api-get-courses-discussion-topics-entries

Arguments: `GET /api/v1/courses/1/discussion_topics/1/entries`

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
- <http://127.0.0.1:[..]/api/v1/courses/1/discussion_topics/1/entries?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/courses/1/discussion_topics/1/entries?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/courses/1/discussion_topics/1/entries?page=1&per_page=10>; rel="last"
(missing)
```

### discussion-topics-api-get-courses-discussion-topics-entry-list

Arguments: `GET /api/v1/courses/1/discussion_topics/1/entry_list`

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
- <http://127.0.0.1:[..]/api/v1/courses/1/discussion_topics/1/entries?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/courses/1/discussion_topics/1/entries?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/courses/1/discussion_topics/1/entries?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### discussion-topics-api-get-courses-discussion-topics-insights-entries

Arguments: `GET /api/v1/courses/1/discussion_topics/1/insights/entries`

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

### discussion-topics-api-get-courses-discussion-topics-insights

Arguments: `GET /api/v1/courses/1/discussion_topics/1/insights`

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

### discussion-topics-api-get-courses-discussion-topics-summaries

Arguments: `GET /api/v1/courses/1/discussion_topics/1/summaries`

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

### discussion-topics-api-get-courses-discussion-topics-view

Arguments: `GET /api/v1/courses/1/discussion_topics/1/view`

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

at body $.unread_entries (missing key)

body:

```diff
- { "unread_entries": [], "forced_entries": [], "entry_ratings": {}, "participants": [], "view": [], "new_entries": [] }
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### discussion-topics-api-get-courses-discussion-topics

Arguments: `GET /api/v1/courses/1/discussion_topics/1`

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

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":1,\"title\":\"Pa…", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":1,"title":"Parity Topic","assignment_id":null,"delayed_post_at":null,"lock_at":null,"created_at":"2026-11-27T06:20:54Z","last_reply_at":"2026-11-[..]:[..]:[..]","posted_at":"2026-11-27T06:20:54Z","root_topic_id":null,"podcast_has_student_posts":false,"discussion_type":"threaded","position":null,"allow_rating":false,"only_graders_can_rate":false,"sort_by_rating":false,"is_section_specific":false,"anonymous_state":null,"summary_enabled":false,"user_name":"Parity User","discussion_subentry_count":[..],"permissions":{"attach":true,"update":true,"reply":true,"delete":true,"manage_assign_to":true},"require_initial_post":null,"user_can_see_posts":true,"podcast_url":null,"read_state":"unread","unread_count":[..],"subscribed":false,"attachments":[],"published":true,"can_unpublish":false,"locked":false,"can_lock":true,"comments_disabled":false,"author":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null},"html_url":"http://127.0.0.1:[..]/courses/1/discussion_topics/1","url":"http://127.0.0.1:[..]/courses/1/discussion_topics/1","pinned":false,"group_category_id":null,"can_group":false,"topic_children":[],"group_topic_children":[],"ungraded_discussion_overrides":[],"locked_for_user":false,"message":null,"todo_date":null,"is_announcement":false,"sort_order":"desc","sort_order_locked":false,"expanded":false,"expanded_locked":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### discussion-topics-api-post-courses-discussion-topics-accessibility-queue-scan

Arguments: `POST /api/v1/courses/1/discussion_topics/1/accessibility/queue_scan`

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

### discussion-topics-api-post-courses-discussion-topics-accessibility-scan

Arguments: `POST /api/v1/courses/1/discussion_topics/1/accessibility/scan`

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

### discussion-topics-api-post-courses-discussion-topics-duplicate

Arguments: `POST /api/v1/courses/1/discussion_topics/1/duplicate`

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

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":[..],"title":"Parity Topic Copy [..]","assignment_id":null,"delayed_post_at":null,"lock_at":null,"created_at":"2026-11-27T14:12:[..]","last_reply_at":null,"posted_at":"2026-11-27T14:12:[..]","root_topic_id":null,"podcast_has_student_posts":false,"discussion_type":"threaded","position":null,"allow_rating":false,"only_graders_can_rate":false,"sort_by_rating":false,"is_section_specific":false,"anonymous_state":null,"summary_enabled":false,"user_name":"Parity User","discussion_subentry_count":0,"permissions":{"attach":true,"update":true,"reply":true,"delete":true,"manage_assign_to":true},"require_initial_post":null,"user_can_see_posts":true,"podcast_url":null,"read_state":"read","unread_count":0,"subscribed":true,"attachments":[],"published":false,"can_unpublish":true,"locked":false,"can_lock":true,"comments_disabled":false,"author":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null},"html_url":"http://127.0.0.1:[..]/courses/1/discussion_topics/[..]","url":"http://127.0.0.1:[..]/courses/1/discussion_topics/[..]","pinned":false,"group_category_id":null,"can_group":true,"topic_children":[],"group_topic_children":[],"ungraded_discussion_overrides":[],"locked_for_user":false,"message":null,"todo_date":null,"is_announcement":false,"sort_order":"desc","sort_order_locked":false,"expanded":false,"expanded_locked":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### discussion-topics-api-post-courses-discussion-topics-entries-rating

Arguments: `POST /api/v1/courses/1/discussion_topics/1/entries/1/rating`

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

### discussion-topics-api-post-courses-discussion-topics-entries-replies

Arguments: `POST /api/v1/courses/1/discussion_topics/1/entries/1/replies`

at status (201→404)

status:

```diff
expected 201, got 404
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":[..],"created_at":"2026-11-27T14:12:[..]","updated_at":"2026-11-27T14:12:[..]","parent_id":1,"rating_count":null,"rating_sum":null,"user_id":1,"user_name":"Parity User","message":null,"user":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null},"read_state":"read","forced_read_state":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### discussion-topics-api-post-courses-discussion-topics-entries

Arguments: `POST /api/v1/courses/1/discussion_topics/1/entries`

at status (201→404)

status:

```diff
expected 201, got 404
```

at header cache-control, line 1, column 1, char 0: expected "⟨m⟩ax-age=0, privat…", got "⟨n⟩o-cache"

header cache-control:

```diff
- max-age=0, private, must-revalidate
+ no-cache
```

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":[..],"created_at":"2026-11-27T14:12:[..]","updated_at":"2026-11-27T14:12:[..]","parent_id":null,"rating_count":null,"rating_sum":null,"user_id":1,"user_name":"Parity User","message":null,"user":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null},"read_state":"read","forced_read_state":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### discussion-topics-api-post-courses-discussion-topics-insights

Arguments: `POST /api/v1/courses/1/discussion_topics/1/insights`

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

### discussion-topics-api-post-courses-discussion-topics-summaries-feedback

Arguments: `POST /api/v1/courses/1/discussion_topics/1/summaries/1/feedback`

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

### discussion-topics-api-post-courses-discussion-topics-summaries

Arguments: `POST /api/v1/courses/1/discussion_topics/1/summaries`

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

### discussion-topics-api-put-courses-discussion-topics-entries-read

Arguments: `PUT /api/v1/courses/1/discussion_topics/1/entries/1/read`

at status (204→404)

status:

```diff
expected 204, got 404
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### discussion-topics-api-put-courses-discussion-topics-insights-entries

Arguments: `PUT /api/v1/courses/1/discussion_topics/1/insights/entries/1`

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

### discussion-topics-api-put-courses-discussion-topics-read-all-91

Arguments: `PUT /api/v1/courses/1/discussion_topics/read_all`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨p⟩VZot7FpTvns8usWZ…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]-[..].[..]; path=/; httponly
+ _csrf_token=pVZot7FpTvns8usWZ54adqtJCldE2nKBvBCn%2BE0yTA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at header x-request-context-id, line 1, column 1, char 0: expected "⟨⟩", got "⟨b⟩fc092d2-7745-403…"

header x-request-context-id:

```diff
- [..]-[..]-444c-[..]-[..]
+ bfc092d2-7745-403d-ab4e-d6e8e540d507
```

### discussion-topics-api-put-courses-discussion-topics-read-all

Arguments: `PUT /api/v1/courses/1/discussion_topics/1/read_all`

at status (204→404)

status:

```diff
expected 204, got 404
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### discussion-topics-api-put-courses-discussion-topics-read

Arguments: `PUT /api/v1/courses/1/discussion_topics/1/read`

at status (204→404)

status:

```diff
expected 204, got 404
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### discussion-topics-api-put-courses-discussion-topics-subscribed

Arguments: `PUT /api/v1/courses/1/discussion_topics/1/subscribed`

at status (204→404)

status:

```diff
expected 204, got 404
```

at body (lines added), line 1, length 0 expected, 65 got

body:

```diff
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### discussion-topics-api-put-courses-discussion-topics-summaries-disable

Arguments: `PUT /api/v1/courses/1/discussion_topics/1/summaries/disable`

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

### discussion-topics-api-put-groups-discussion-topics-read-all-186

Arguments: `PUT /api/v1/groups/1/discussion_topics/read_all`

at body (lines missing), line 1, length 4 expected, 0 got

body:

```diff
- [..]
(trailing newline: expected false, got true)
```

### discussion-topics-api-put-groups-discussion-topics-subscribed

Arguments: `PUT /api/v1/groups/1/discussion_topics/1/subscribed`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨q⟩WcK70nViQZLrW4UB…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]-[..]_[..].[..]; path=/; httponly
+ _csrf_token=qWcK70nViQZLrW4UBElYx0uMELf7JvAXTRle%2FUSz4Q%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### enrollments-api-delete-courses-enrollments

Arguments: `DELETE /api/v1/courses/1/enrollments/10`

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

at body $.id (missing key)

body:

```diff
- {"id":10,"root_account_id":6,"user_id":3,"course_id":1,"course_section_id":3,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T14:12:[..]","created_at":"2026-11-27T13:37:59Z","start_at":null,"end_at":null,"type":"StudentEnrollment","enrollment_state":"completed","role":"StudentEnrollment","role_id":43,"last_activity_at":null,"last_attended_at":null,"total_activity_time":0,"sis_import_id":null,"grades":{"html_url":"http://127.0.0.1:[..]/courses/1/grades/3","current_grade":null,"current_score":null,"final_grade":null,"final_score":null,"unposted_current_score":null,"unposted_current_grade":null,"unposted_final_score":null,"unposted_final_grade":null},"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/1/users/3"}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### enrollments-api-get-accounts-enrollments

Arguments: `GET /api/v1/accounts/6/enrollments/10`

at status (200→401)

status:

```diff
expected 200, got 401
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
- {"id":10,"root_account_id":6,"user_id":3,"course_id":1,"course_section_id":3,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T14:12:[..]","created_at":"2026-11-27T13:37:59Z","start_at":null,"end_at":null,"type":"StudentEnrollment","enrollment_state":"completed","role":"StudentEnrollment","role_id":43,"last_activity_at":null,"last_attended_at":null,"total_activity_time":0,"sis_import_id":null,"grades":{"html_url":"http://127.0.0.1:[..]/courses/1/grades/3","current_grade":null,"current_score":null,"final_grade":null,"final_score":null,"unposted_current_score":null,"unposted_current_grade":null,"unposted_final_score":null,"unposted_final_grade":null},"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/1/users/3"}
+ {"status":"unauthenticated","errors":[{"message":"user authorization required"}]}
```

### enrollments-api-get-courses-enrollments

Arguments: `GET /api/v1/courses/1/enrollments`

at body $ (type), expected array, got object

body:

```diff
- [{"id":11,"root_account_id":6,"user_id":8,"course_id":1,"course_section_id":3,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T13:51:40Z","created_at":"2026-11-27T13:51:40Z","start_at":null,"end_at":null,"type":"StudentViewEnrollment","enrollment_state":"active","role":"StudentEnrollment","role_id":43,"last_activity_at":null,"last_attended_at":null,"total_activity_time":0,"sis_import_id":null,"grades":{"html_url":"http://127.0.0.1:[..]/courses/1/grades/8","current_grade":null,"current_score":null,"final_grade":null,"final_score":null,"unposted_current_score":null,"unposted_current_grade":null,"unposted_final_score":null,"unposted_final_grade":null},"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/1/users/8","user":{"id":8,"name":"Test Student","created_at":"2026-11-27T13:51:40Z","sortable_name":"Student, Test","short_name":"Test Student","sis_user_id":null,"integration_id":null,"login_id":"4c96f8dbf95063694a3d07b5f26dd8587cba9abb"}},{"id":9,"root_account_id":6,"user_id":1,"course_id":1,"course_section_id":3,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T13:37:59Z","created_at":"2026-11-27T13:37:59Z","start_at":null,"end_at":null,"type":"TeacherEnrollment","enrollment_state":"active","role":"TeacherEnrollment","role_id":44,"last_activity_at":"2026-11-27T14:11:58Z","last_attended_at":null,"total_activity_time":0,"sis_import_id":null,"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/1/users/1","user":{"id":1,"name":"Parity User","created_at":"2026-11-26T20:10:15Z","sortable_name":"User, Parity","short_name":"Parity User","sis_user_id":null,"integration_id":null,"login_id":"parity@example.com"}}]
+ {"data":[{"id":9,"role":"teacher","user":{"id":1,"name":"Parity User","email":"parity@example.com"}},{"id":10,"role":"student","user":{"id":3,"name":"Parity Student","email":"student@example.com"}},{"id":11,"role":"student","user":{"id":8,"name":"Test Student","email":"4c96f8dbf95063694a3d07b5f26dd8587cba9abb"}}],"page":1,"per_page":50,"total":3}
```

### enrollments-api-get-sections-enrollments

Arguments: `GET /api/v1/sections/3/enrollments`

at body $ (length), length 2 expected, 0 got

body:

```diff
- [{"id":11,"root_account_id":6,"user_id":8,"course_id":1,"course_section_id":3,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T13:51:40Z","created_at":"2026-11-27T13:51:40Z","start_at":null,"end_at":null,"type":"StudentViewEnrollment","enrollment_state":"active","role":"StudentEnrollment","role_id":43,"last_activity_at":null,"last_attended_at":null,"total_activity_time":0,"sis_import_id":null,"grades":{"html_url":"http://127.0.0.1:[..]/courses/1/grades/8","current_grade":null,"current_score":null,"final_grade":null,"final_score":null,"unposted_current_score":null,"unposted_current_grade":null,"unposted_final_score":null,"unposted_final_grade":null},"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/1/users/8","user":{"id":8,"name":"Test Student","created_at":"2026-11-27T13:51:40Z","sortable_name":"Student, Test","short_name":"Test Student","sis_user_id":null,"integration_id":null,"login_id":"4c96f8dbf95063694a3d07b5f26dd8587cba9abb"}},{"id":9,"root_account_id":6,"user_id":1,"course_id":1,"course_section_id":3,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T13:37:59Z","created_at":"2026-11-27T13:37:59Z","start_at":null,"end_at":null,"type":"TeacherEnrollment","enrollment_state":"active","role":"TeacherEnrollment","role_id":44,"last_activity_at":"2026-11-27T14:11:58Z","last_attended_at":null,"total_activity_time":0,"sis_import_id":null,"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/1/users/1","user":{"id":1,"name":"Parity User","created_at":"2026-11-26T20:10:15Z","sortable_name":"User, Parity","short_name":"Parity User","sis_user_id":null,"integration_id":null,"login_id":"parity@example.com"}}]
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

### enrollments-api-get-users-enrollments

Arguments: `GET /api/v1/users/3/enrollments`

at body $ (length), length 2 expected, 0 got

body:

```diff
- [{"id":3,"root_account_id":6,"user_id":3,"course_id":3,"course_section_id":1,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T12:32:51Z","created_at":"2026-11-27T05:33:48Z","start_at":null,"end_at":null,"type":"StudentEnrollment","enrollment_state":"active","role":"StudentEnrollment","role_id":43,"last_activity_at":"2026-11-27T09:48:46Z","last_attended_at":null,"total_activity_time":580,"sis_import_id":null,"grades":{"html_url":"http://127.0.0.1:[..]/courses/3/grades/3","current_grade":null,"current_score":null,"final_grade":null,"final_score":null,"unposted_current_score":null,"unposted_current_grade":null,"unposted_final_score":null,"unposted_final_grade":null},"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/3/users/3","user":{"id":3,"name":"Parity Student","created_at":"2026-11-27T05:29:37Z","sortable_name":"Student, Parity","short_name":"Parity Student","login_id":"student@example.com"}},{"id":7,"root_account_id":6,"user_id":3,"course_id":5,"course_section_id":2,"associated_user_id":null,"limit_privileges_to_course_section":false,"updated_at":"2026-11-27T13:00:45Z","created_at":"2026-11-27T12:55:33Z","start_at":null,"end_at":null,"type":"StudentEnrollment","enrollment_state":"active","role":"StudentEnrollment","role_id":43,"last_activity_at":null,"last_attended_at":null,"total_activity_time":0,"sis_import_id":null,"grades":{"html_url":"http://127.0.0.1:[..]/courses/5/grades/3","current_grade":null,"current_score":null,"final_grade":null,"final_score":null,"unposted_current_score":null,"unposted_current_grade":null,"unposted_final_score":null,"unposted_final_grade":null},"sis_account_id":null,"sis_course_id":null,"course_integration_id":null,"sis_section_id":null,"section_integration_id":null,"sis_user_id":null,"html_url":"http://127.0.0.1:[..]/courses/5/users/3","user":{"id":3,"name":"Parity Student","created_at":"2026-11-27T05:29:37Z","sortable_name":"Student, Parity","short_name":"Parity Student","login_id":"student@example.com"}}]
+ []
```

### enrollments-api-get-users-temporary-enrollment-status

Arguments: `GET /api/v1/users/3/temporary_enrollment_status`

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

### enrollments-api-post-accounts-bulk-enrollment

Arguments: `POST /api/v1/accounts/6/bulk_enrollment`

at body, line 1, column 82, char 81: expected "…_server_error\"}]⟨,⟩\"error_report_id…", got "…_server_error\"}]⟨}⟩"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}
```

### enrollments-api-post-courses-enrollments

Arguments: `POST /api/v1/courses/1/enrollments`

at status (400→415)

status:

```diff
expected 400, got 415
```

at body $.message (missing key)

body:

```diff
- {"message":"No parameters given"}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### enrollments-api-put-courses-users-last-attended

Arguments: `PUT /api/v1/courses/1/users/3/last_attended`

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

### external-tools-get-accounts-external-tools-migration-info

Arguments: `GET /api/v1/accounts/6/external_tools/1/migration_info`

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

at body $.migration_running (missing key)

body:

```diff
- {"migration_running":false}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
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

### external-tools-get-accounts-external-tools

Arguments: `GET /api/v1/accounts/6/external_tools`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/external_tools?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/external_tools?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/external_tools?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### external-tools-get-courses-external-tools-migration-info

Arguments: `GET /api/v1/courses/1/external_tools/1/migration_info`

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

at body $.migration_running (missing key)

body:

```diff
- {"migration_running":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨V⟩K7%2FCrrrUYPejcK…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D[..]_[..]_[..]+[..].[..].[..]; path=/; httponly
+ _csrf_token=VK7%2FCrrrUYPejcKqMLV7sUo4RhFJ7pgzpqbPLNjMgw%3D%3D; path=/, log_session_id=225a972484c7473051339d977a312dbf; path=/; httponly, _normandy_session=225a972484c7473051339d977a312dbf.225a972484c7473051339d977a312dbf; path=/; httponly
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

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"name":[{"attribute":"name","message":"can't be blank","type":"blank"}],"consumer_key":[{"attribute":"consumer_key","message":"can't be blank","type":"blank"}],"shared_secret":[{"attribute":"shared_secret","message":"can't be blank","type":"blank"}],"config_url":[{"attribute":"config_url","message":"can't be blank","type":"blank"}],"url":[{"attribute":"url","message":"Either the url or domain should be set.","type":"Either the url or domain should be set."}],"domain":[{"attribute":"domain","message":"Either the url or domain should be set.","type":"Either the url or domain should be set."}]}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
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

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"name":[{"attribute":"name","message":"can't be blank","type":"blank"}],"consumer_key":[{"attribute":"consumer_key","message":"can't be blank","type":"blank"}],"shared_secret":[{"attribute":"shared_secret","message":"can't be blank","type":"blank"}],"url":[{"attribute":"url","message":"Either the url or domain should be set.","type":"Either the url or domain should be set."}],"domain":[{"attribute":"domain","message":"Either the url or domain should be set.","type":"Either the url or domain should be set."}]}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### external-tools-post-courses-create-tool-with-verification

Arguments: `POST /api/v1/courses/1/create_tool_with_verification`

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"name":[{"attribute":"name","message":"can't be blank","type":"blank"}],"consumer_key":[{"attribute":"consumer_key","message":"can't be blank","type":"blank"}],"shared_secret":[{"attribute":"shared_secret","message":"can't be blank","type":"blank"}],"config_url":[{"attribute":"config_url","message":"can't be blank","type":"blank"}],"url":[{"attribute":"url","message":"Either the url or domain should be set.","type":"Either the url or domain should be set."}],"domain":[{"attribute":"domain","message":"Either the url or domain should be set.","type":"Either the url or domain should be set."}]}}
+ {"errors":["Name can't be blank"]}
```

### external-tools-post-courses-external-tools

Arguments: `POST /api/v1/courses/1/external_tools`

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"name":[{"attribute":"name","message":"can't be blank","type":"blank"}],"consumer_key":[{"attribute":"consumer_key","message":"can't be blank","type":"blank"}],"shared_secret":[{"attribute":"shared_secret","message":"can't be blank","type":"blank"}],"url":[{"attribute":"url","message":"Either the url or domain should be set.","type":"Either the url or domain should be set."}],"domain":[{"attribute":"domain","message":"Either the url or domain should be set.","type":"Either the url or domain should be set."}]}}
+ {"errors":["Name can't be blank"]}
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

Arguments: `DELETE /api/v1/files/7`

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

### files-get-courses-files-quota

Arguments: `GET /api/v1/courses/1/files/quota`

at body $.quota (value): expected 500000000, got 50000000

body:

```diff
- {"quota":500000000,"quota_used":0}
+ {"quota":50000000,"quota_used":0}
```

### files-get-files-create-success

Arguments: `GET /api/v1/files/7/create_success`

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

### files-get-files-icon-metadata

Arguments: `GET /api/v1/files/7/icon_metadata`

at status (400→404)

status:

```diff
expected 400, got 404
```

at body $.errors[0].message, char 4: expected "The ⟨r⟩equested attachm…", got "The ⟨s⟩pecified resourc…"

body:

```diff
- {"errors":[{"message":"The requested attachment does not support viewing metadata."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### files-get-files-public-url

Arguments: `GET /api/v1/files/7/public_url`

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

### files-get-files

Arguments: `GET /api/v1/files/7`

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

at body $.id (missing key)

body:

```diff
- {"id":7,"folder_id":null,"display_name":"parity-file.txt","filename":"parity-file.txt","uuid":"0GJ6OIpz3lyY0Ik8NvR80eY1jKpdBbQMMhiOymoa","upload_status":"success","content-type":"text/plain","url":"http://127.0.0.1:[..]/files/7/download?download_frd=1\u0026verifier=0GJ6OIpz3lyY0Ik8NvR80eY1jKpdBbQMMhiOymoa","size":21,"created_at":"2026-11-27T12:26:02Z","updated_at":"2026-11-27T13:00:59Z","unlock_at":null,"locked":false,"hidden":false,"lock_at":null,"hidden_for_user":false,"thumbnail_url":null,"modified_at":"2026-11-27T12:26:02Z","mime_class":"text","media_entry_id":null,"category":"uncategorized","locked_for_user":false,"canvadoc_session_url":null}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### files-get-folders-files

Arguments: `GET /api/v1/folders/7/files`

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
- <http://127.0.0.1:[..]/api/v1/folders/7/files?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/folders/7/files?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/folders/7/files?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### files-get-users-files-439

Arguments: `GET /api/v1/users/3/files/7`

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

### files-get-users-files

Arguments: `GET /api/v1/users/3/files`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/users/3/files?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/users/3/files?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/users/3/files?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### files-post-files-capture

Arguments: `POST /api/v1/files/capture`

at status (404→415)

status:

```diff
expected 404, got 415
```

at header content-type, line 1, column 17, char 16: expected "application/json⟨⟩", got "application/json⟨;⟩ charset=utf-8"

header content-type:

```diff
- application/json
+ application/json; charset=utf-8
```

at body (lines added), line 1, length 0 expected, 81 got

body:

```diff
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
(trailing newline: expected true, got false)
```

### files-post-files-create-success

Arguments: `POST /api/v1/files/7/create_success`

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

Arguments: `POST /api/v1/files/7/reset_verifier`

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

### files-post-files

Arguments: `POST /api/v1/files/7`

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

at body $.id (missing key)

body:

```diff
- {"id":7,"folder_id":null,"display_name":"parity-file.txt","filename":"parity-file.txt","uuid":"0GJ6OIpz3lyY0Ik8NvR80eY1jKpdBbQMMhiOymoa","upload_status":"success","content-type":"text/plain","url":"http://127.0.0.1:[..]/files/7/download?download_frd=1\u0026verifier=0GJ6OIpz3lyY0Ik8NvR80eY1jKpdBbQMMhiOymoa","size":21,"created_at":"2026-11-27T12:26:02Z","updated_at":"2026-11-27T13:00:59Z","unlock_at":null,"locked":false,"hidden":false,"lock_at":null,"hidden_for_user":false,"thumbnail_url":null,"modified_at":"2026-11-27T12:26:02Z","mime_class":"text","media_entry_id":null,"category":"uncategorized","locked_for_user":false,"canvadoc_session_url":null}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### files-put-files

Arguments: `PUT /api/v1/files/7`

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

### folders-delete-folders

Arguments: `DELETE /api/v1/folders/1`

at status (400→404)

status:

```diff
expected 400, got 404
```

at body $.message (missing key)

body:

```diff
- {"message":"Can't delete the root folder"}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### folders-get-courses-folders-323

Arguments: `GET /api/v1/courses/1/folders/1`

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

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":1,\"name\":\"cou…", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":1,"name":"course files","full_name":"course files","position":null,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-27T13:00:59Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/1/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/1/files","files_count":0,"folders_count":[..],"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"errors":[{"message":"The specified resource does not exist."}]}
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

at body, line 1, column 8, char 7: expected "[{\"id\":⟨1⟩,\"name\":\"course …", got "[{\"id\":⟨2⟩,\"name\":\"Parity …"

body:

```diff
- [{"id":1,"name":"course files","full_name":"course files","position":null,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-27T13:00:59Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/1/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/1/files","files_count":0,"folders_count":[..],"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}]
+ [{"id":2,"name":"Parity Folder","full_name":"course files/Parity Folder","position":1,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T14:23:43.613643Z","updated_at":"2026-11-27T14:23:43.613643Z","category":null,"locked":false,"all_url":"/api/v1/folders/2/all","folders_url":"/api/v1/folders/2/folders","files_url":"/api/v1/folders/2/files","files_count":0,"folders_count":0,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}]
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

at body $.id (value): expected 24, got 19

body:

```diff
- {"id":24,"name":"Icon Maker Icons","full_name":"course files/Icon Maker Icons","position":5,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T13:51:51Z","updated_at":"2026-11-27T13:51:51Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/24/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/24/files","files_count":0,"folders_count":0,"hidden":true,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"id":19,"name":"Icon Maker Icons","full_name":"course files/Parity Folder/Icon Maker Icons","position":1,"parent_folder_id":2,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T14:23:47.879104Z","updated_at":"2026-11-27T14:23:47.879104Z","category":null,"locked":false,"all_url":"/api/v1/folders/19/all","folders_url":"/api/v1/folders/19/folders","files_url":"/api/v1/folders/19/files","files_count":0,"folders_count":0,"hidden":true,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
```

### folders-get-courses-folders-media

Arguments: `GET /api/v1/courses/1/folders/media`

at body $.id (value): expected 25, got 20

body:

```diff
- {"id":25,"name":"Uploaded Media","full_name":"course files/Uploaded Media","position":6,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T13:51:51Z","updated_at":"2026-11-27T13:51:51Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/25/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/25/files","files_count":0,"folders_count":0,"hidden":true,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"id":20,"name":"Uploaded Media","full_name":"course files/Parity Folder/Uploaded Media","position":2,"parent_folder_id":2,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T14:23:47.894169Z","updated_at":"2026-11-27T14:23:47.894169Z","category":null,"locked":false,"all_url":"/api/v1/folders/20/all","folders_url":"/api/v1/folders/20/folders","files_url":"/api/v1/folders/20/files","files_count":0,"folders_count":0,"hidden":true,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
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
- <http://127.0.0.1:[..]/api/v1/folders/1/folders?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/folders/1/folders?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/folders/1/folders?page=1&per_page=10>; rel="last"
(missing)
```

### folders-get-folders

Arguments: `GET /api/v1/folders/1`

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

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":1,\"name\":\"cou…", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":1,"name":"course files","full_name":"course files","position":null,"parent_folder_id":null,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-27T13:00:59Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/1/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/1/files","files_count":0,"folders_count":[..],"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### folders-get-groups-folders-and-files

Arguments: `GET /api/v1/groups/1/folders_and_files`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-groups-folders-duplicates

Arguments: `GET /api/v1/groups/1/folders/1/duplicates`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-users-folders-442

Arguments: `GET /api/v1/users/3/folders/1`

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

### folders-get-users-folders-and-files

Arguments: `GET /api/v1/users/3/folders_and_files`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-users-folders-by-path-445

Arguments: `GET '/api/v1/users/3/folders/by_path/*full_path'`

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

### folders-get-users-folders-by-path

Arguments: `GET /api/v1/users/3/folders/by_path`

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

at body $ (type), expected array, got object

body:

```diff
- [{"id":31,"name":"my files","full_name":"my files","position":null,"parent_folder_id":null,"context_type":"User","context_id":3,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T14:12:08Z","updated_at":"2026-11-27T14:12:08Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/31/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/31/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}]
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### folders-get-users-folders-duplicates

Arguments: `GET /api/v1/users/3/folders/1/duplicates`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

### folders-get-users-folders

Arguments: `GET /api/v1/users/3/folders`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/users/3/folders?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/users/3/folders?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/users/3/folders?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- [{"id":31,"name":"my files","full_name":"my files","position":null,"parent_folder_id":null,"context_type":"User","context_id":3,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T14:12:08Z","updated_at":"2026-11-27T14:12:08Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/31/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/31/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":false}]
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### folders-post-accounts-folders

Arguments: `POST /api/v1/accounts/6/folders`

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

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":", got "{\"⟨s⟩tatus\":\"unauthor…"

body:

```diff
- {"id":[..],"name":"New Folder [..]","full_name":"files/unfiled/New Folder [..]","position":[..],"parent_folder_id":14,"context_type":"Account","context_id":6,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T14:[..]:[..]","updated_at":"2026-11-27T14:[..]:[..]","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/[..]/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/[..]/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### folders-post-courses-folders

Arguments: `POST /api/v1/courses/1/folders`

at body, line 1, column 7, char 6: expected "{\"id\":⟨⟩", got "{\"id\":⟨2⟩2,\"name\":\"New Fo…"

body:

```diff
- {"id":[..],"name":"New Folder [..]","full_name":"course files/unfiled/New Folder [..]","position":[..],"parent_folder_id":3,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T14:[..]:[..]","updated_at":"2026-11-27T14:[..]:[..]","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/[..]/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/[..]/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"id":22,"name":"New Folder","full_name":"course files/Parity Folder/unfiled/New Folder","position":1,"parent_folder_id":21,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T14:23:48.110638Z","updated_at":"2026-11-27T14:23:48.110638Z","category":null,"locked":false,"all_url":"/api/v1/folders/22/all","folders_url":"/api/v1/folders/22/folders","files_url":"/api/v1/folders/22/files","files_count":0,"folders_count":0,"hidden":false,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
```

### folders-post-folders-copy-file

Arguments: `POST /api/v1/folders/1/copy_file`

at status (400→415)

status:

```diff
expected 400, got 415
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨2⟩pDb9x63FVKjy0kPV…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%[..]%3D[..]_[..]_[..].[..].[..]; path=/; httponly
+ _csrf_token=2pDb9x63FVKjy0kPV954M2BGU%2Btqh%2BvTPeMNWdU1cA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.message (missing key)

body:

```diff
- {"message":"source_file_id must be provided"}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### folders-post-folders-copy-folder

Arguments: `POST /api/v1/folders/1/copy_folder`

at status (400→415)

status:

```diff
expected 400, got 415
```

at body $.message (missing key)

body:

```diff
- {"message":"source_folder_id must be provided"}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### folders-post-folders-files

Arguments: `POST /api/v1/folders/1/files`

at status (500→404)

status:

```diff
expected 500, got 404
```

at body, line 1, column 24, char 23: expected "…s\":[{\"message\":\"⟨A⟩n error occurred…", got "…s\":[{\"message\":\"⟨T⟩he specified res…"

body:

```diff
- {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":[..]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### folders-post-folders-folders

Arguments: `POST /api/v1/folders/1/folders`

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

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":[..],"name":"New Folder [..]","full_name":"course files/New Folder [..]","position":[..],"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-27T14:[..]:[..]","updated_at":"2026-11-27T14:[..]:[..]","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/[..]/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/[..]/files","files_count":0,"folders_count":0,"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### folders-put-folders

Arguments: `PUT /api/v1/folders/1`

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

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":1,\"name\":\"cou…", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":1,"name":"course files","full_name":"course files","position":null,"parent_folder_id":1,"context_type":"Course","context_id":1,"unlock_at":null,"lock_at":null,"created_at":"2026-11-26T20:29:16Z","updated_at":"2026-11-27T13:00:59Z","locked":false,"folders_url":"http://127.0.0.1:[..]/api/v1/folders/1/folders","files_url":"http://127.0.0.1:[..]/api/v1/folders/1/files","files_count":0,"folders_count":[..],"hidden":null,"locked_for_user":false,"hidden_for_user":false,"for_submissions":false,"can_upload":true}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### group-categories-delete-group-categories

Arguments: `DELETE /api/v1/group_categories/1`

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized"}
+ {"id":1,"name":"Parity Category","role":null,"self_signup":null,"self_signup_end_at":null,"group_limit":null,"auto_leader":null,"context_type":"Course","course_id":1,"account_id":6,"non_collaborative":false,"protected":false,"allows_multiple_memberships":false,"is_member":false,"created_at":"2026-11-27T14:23:43.613643Z"}
```

### group-categories-get-accounts-group-categories

Arguments: `GET /api/v1/accounts/6/group_categories`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/group_categories?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/group_categories?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/group_categories?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### group-categories-get-courses-group-categories-export-tags

Arguments: `GET /api/v1/courses/1/group_categories/export_tags`

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

at header content-type, line 1, column 1, char 0: expected "⟨a⟩pplication/json;…", got "⟨t⟩ext/csv"

header content-type:

```diff
- application/json; charset=utf-8
+ text/csv
```

at body, line 1, column 1, char 0: expected "⟨{⟩\"status\":\"unauth…", got "⟨n⟩ame,canvas_user_…"

body:

```diff
- {"status":"unauthorized"}
+ name,canvas_user_id,login_id,tag_name,canvas_tag_id,tag_set_name,canvas_tag_set_id
+ Parity Student,3,,,,,
+ Test Student,8,,,,,
(trailing newline: expected false, got true)
```

### group-categories-get-courses-group-categories

Arguments: `GET /api/v1/courses/1/group_categories`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":1,"name":"Parity Category","role":"StudentEnrollment","self_signup":null,"self_signup_end_at":null,"group_limit":null,"auto_leader":null,"created_at":"2026-11-26T20:24:30Z","context_type":"Course","course_id":1,"progress":{"id":28,"context_id":1,"context_type":"GroupCategory","user_id":null,"tag":"assign_unassigned_members","completion":0.0,"workflow_state":"running","created_at":"2026-11-27T13:51:53Z","updated_at":"2026-11-27T13:51:53Z","message":null,"url":"http://127.0.0.1:[..]/api/v1/progress/28"},"sis_group_category_id":null,"sis_import_id":null,"protected":true,"allows_multiple_memberships":false,"is_member":false}]
+ []
```

### group-categories-get-group-categories-export

Arguments: `GET /api/v1/group_categories/1/export`

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

at header content-disposition (missing key)

header content-disposition:

```diff
- attachment; filename="Parity Category.csv"; filename*=UTF-8''Parity%20Category.csv
(missing)
```

at header content-transfer-encoding (missing key)

header content-transfer-encoding:

```diff
- binary
(missing)
```

at header content-type, line 1, column 1, char 0: expected "⟨t⟩ext/csv", got "⟨a⟩pplication/json;…"

header content-type:

```diff
- text/csv
+ application/json; charset=utf-8
```

at body, line 1, column 1, char 0: expected "⟨n⟩ame,canvas_user_…", got "⟨{⟩\"errors\":[{\"mess…"

body:

```diff
- name,canvas_user_id,user_id,login_id,sections,group_name
+ {"errors":[{"message":"The specified resource does not exist."}]}
(trailing newline: expected true, got false)
```

### group-categories-get-group-categories-groups

Arguments: `GET /api/v1/group_categories/1/groups`

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
- <http://127.0.0.1:[..]/api/v1/group_categories/1/groups?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/group_categories/1/groups?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/group_categories/1/groups?page=1&per_page=10>; rel="last"
(missing)
```

### group-categories-get-group-categories-users

Arguments: `GET /api/v1/group_categories/1/users`

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
- <http://127.0.0.1:[..]/api/v1/group_categories/1/users?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/group_categories/1/users?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/group_categories/1/users?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### group-categories-get-group-categories

Arguments: `GET /api/v1/group_categories/1`

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

at body $.id (missing key)

body:

```diff
- {"id":1,"name":"Parity Category","role":"StudentEnrollment","self_signup":null,"self_signup_end_at":null,"group_limit":null,"auto_leader":null,"created_at":"2026-11-26T20:24:30Z","context_type":"Course","course_id":1,"progress":{"id":28,"context_id":1,"context_type":"GroupCategory","user_id":null,"tag":"assign_unassigned_members","completion":0.0,"workflow_state":"running","created_at":"2026-11-27T13:51:53Z","updated_at":"2026-11-27T13:51:53Z","message":null,"url":"http://127.0.0.1:[..]/api/v1/progress/28"},"sis_group_category_id":null,"sis_import_id":null,"protected":true,"allows_multiple_memberships":false,"is_member":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
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

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"name":[{"attribute":"name","message":"Name is required","type":"Name is required"}]}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### group-categories-post-courses-group-categories-import-tags

Arguments: `POST /api/v1/courses/1/group_categories/import_tags`

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

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized"}
+ {"completion":0,"context_id":1,"context_type":"Course","created_at":"2026-11-27T14:23:48.311833+00:00","id":5,"message":null,"tag":"course_tag_import","updated_at":"2026-11-27T14:23:48.311849+00:00","url":"http://localhost:3000/api/v1/progress/5","user_id":1,"workflow_state":"running"}
```

### group-categories-post-courses-group-categories

Arguments: `POST /api/v1/courses/1/group_categories`

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"name":[{"attribute":"name","message":"Name is required","type":"Name is required"}]}}
+ {"errors":[{"message":"Name is required"}]}
```

### group-categories-post-group-categories-assign-unassigned-members

Arguments: `POST /api/v1/group_categories/1/assign_unassigned_members`

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

at body $.id (missing key)

body:

```diff
- {"id":28,"context_id":1,"context_type":"GroupCategory","user_id":null,"tag":"assign_unassigned_members","completion":0.0,"workflow_state":"running","created_at":"2026-11-27T13:51:53Z","updated_at":"2026-11-27T13:51:53Z","message":null,"url":"http://127.0.0.1:[..]/api/v1/progress/28"}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### group-categories-post-group-categories-import

Arguments: `POST /api/v1/group_categories/1/import`

at status (401→404)

status:

```diff
expected 401, got 404
```

at body $.status (missing key)

body:

```diff
- {"status":"unauthorized"}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### group-categories-put-group-categories

Arguments: `PUT /api/v1/group_categories/1`

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

at body $.id (missing key)

body:

```diff
- {"id":1,"name":"Parity Category","role":"StudentEnrollment","self_signup":null,"self_signup_end_at":null,"group_limit":null,"auto_leader":null,"created_at":"2026-11-26T20:24:30Z","context_type":"Course","course_id":1,"progress":{"id":28,"context_id":1,"context_type":"GroupCategory","user_id":null,"tag":"assign_unassigned_members","completion":0.0,"workflow_state":"running","created_at":"2026-11-27T13:51:53Z","updated_at":"2026-11-27T13:51:53Z","message":null,"url":"http://127.0.0.1:[..]/api/v1/progress/28"},"sis_group_category_id":null,"sis_import_id":null,"protected":true,"allows_multiple_memberships":false,"is_member":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### groups-delete-groups

Arguments: `DELETE /api/v1/groups/1`

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

### groups-get-accounts-groups

Arguments: `GET /api/v1/accounts/6/groups`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/groups?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/groups?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/groups?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### groups-get-courses-bulk-user-tags

Arguments: `GET /api/v1/courses/1/bulk_user_tags`

at body $ (type), expected object, got array

body:

```diff
- {}
+ []
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

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"entries":[],"unread_count":0}
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

at body $ (type), expected object, got array

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ []
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

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"id":1,"name":"Parity Group","description":null,"is_public":false,"join_level":"invitation_only","group_category_id":1,"max_membership":null,"created_at":"2026-11-27T14:23:43.613643Z","members_count":0,"storage_quota_mb":null,"context_type":null,"course_id":1,"context_name":null,"avatar_url":null,"role":null,"leader":null,"has_submission":false,"concluded":false,"non_collaborative":false}
```

### groups-get-users-self-groups

Arguments: `GET /api/v1/users/self/groups`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨x⟩YjUdVxpDD%2BhhSv…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]-[..]-[..].[..]; path=/; httponly
+ _csrf_token=xYjUdVxpDD%2BhhSv6XJs3Z0f6edeubnY0FLcQJZ3Igw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
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

### groups-post-group-categories-groups

Arguments: `POST /api/v1/group_categories/1/groups`

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

at body, line 1, column 3, char 2: expected "{\"⟨i⟩d\":", got "{\"⟨e⟩rrors\":[{\"messag…"

body:

```diff
- {"id":[..],"name":"[..]","description":null,"is_public":false,"join_level":"invitation_only","group_category_id":1,"max_membership":null,"created_at":"2026-11-27T14:[..]:[..]","members_count":0,"storage_quota_mb":50,"permissions":{"create_discussion_topic":true,"join":false,"create_announcement":true},"context_type":"Course","course_id":1,"context_name":"Parity Course","avatar_url":null,"role":"StudentEnrollment","leader":null,"users":[],"group_category":{"id":1,"name":"Parity Category","role":"StudentEnrollment","self_signup":null,"self_signup_end_at":null,"group_limit":null,"auto_leader":null,"created_at":"2026-11-26T20:24:30Z","context_type":"Course","course_id":1,"sis_group_category_id":null,"sis_import_id":null,"protected":true,"allows_multiple_memberships":false,"is_member":false},"sis_group_id":null,"sis_import_id":null,"has_submission":false,"concluded":false,"non_collaborative":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### groups-post-groups

Arguments: `POST /api/v1/groups`

at body, line 1, column 7, char 6: expected "{\"id\":⟨⟩", got "{\"id\":⟨5⟩,\"name\":\"8e29d8f…"

body:

```diff
- {"id":[..],"name":"[..]","description":null,"is_public":false,"join_level":"invitation_only","group_category_id":2,"max_membership":null,"created_at":"2026-11-27T14:[..]:[..]","members_count":1,"storage_quota_mb":50,"permissions":{"create_discussion_topic":true,"join":false,"create_announcement":true},"context_type":"Account","account_id":4,"context_name":"Default Account","avatar_url":null,"role":"communities","leader":null,"users":[{"id":1,"name":"Parity User","created_at":"2026-11-26T20:10:15Z","sortable_name":"User, Parity","short_name":"Parity User"}],"group_category":{"id":2,"name":"Communities","role":"communities","self_signup":null,"self_signup_end_at":null,"group_limit":null,"auto_leader":null,"created_at":"2026-11-26T20:48:31Z","context_type":"Account","account_id":4,"protected":true,"allows_multiple_memberships":true,"is_member":true},"has_submission":false,"concluded":false,"non_collaborative":false}
+ {"id":5,"name":"8e29d8fa-04c5-4841-b1dd-b3b4f2197294","description":null,"is_public":false,"join_level":"invitation_only","group_category_id":null,"max_membership":null,"created_at":"2026-11-27T14:23:48.525283Z","members_count":0,"storage_quota_mb":null,"context_type":"Account","course_id":null,"context_name":"Root Account","avatar_url":null,"role":null,"leader":null,"has_submission":false,"concluded":false,"non_collaborative":false}
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

at body $.errors (missing key)

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"id":1,"name":"Parity Group","description":null,"is_public":false,"join_level":"invitation_only","group_category_id":1,"max_membership":null,"created_at":"2026-11-27T14:23:43.613643Z","members_count":0,"storage_quota_mb":null,"context_type":null,"course_id":1,"context_name":null,"avatar_url":null,"role":null,"leader":null,"has_submission":false,"concluded":false,"non_collaborative":false}
```

### lti-registrations-delete-accounts-lti-registrations-bind

Arguments: `DELETE /api/v1/accounts/6/lti_registrations/4/bind`

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

at body $.errors[0].code (missing key)

body:

```diff
- {"errors":[{"code":"not_found","message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### lti-registrations-delete-accounts-lti-registrations

Arguments: `DELETE /api/v1/accounts/6/lti_registrations/4`

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

at body $.errors (type), expected string, got array

body:

```diff
- {"errors":"LTI registration not found"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
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

at body $.errors (type), expected string, got array

body:

```diff
- {"errors":"Not found"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### lti-registrations-get-accounts-lti-registrations-check-domain-duplicates

Arguments: `GET /api/v1/accounts/6/lti_registrations/check_domain_duplicates`

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

at body $.duplicates (missing key)

body:

```diff
- {"duplicates":[]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### lti-registrations-get-accounts-lti-registrations-history

Arguments: `GET /api/v1/accounts/6/lti_registrations/4/history`

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

at body $.errors[0].code (missing key)

body:

```diff
- {"errors":[{"code":"not_found","message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
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

at body $.errors (type), expected string, got array

body:

```diff
- {"errors":"Not found"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### lti-registrations-get-accounts-lti-registrations-overlay-history

Arguments: `GET /api/v1/accounts/6/lti_registrations/4/overlay_history`

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

at body $.errors[0].code (missing key)

body:

```diff
- {"errors":[{"code":"not_found","message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### lti-registrations-get-accounts-lti-registrations

Arguments: `GET /api/v1/accounts/6/lti_registrations`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/lti_registrations?page=1&per_page=15>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/lti_registrations?page=1&per_page=15>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/lti_registrations?page=1&per_page=15>; rel="last"
(missing)
```

at body $.total (missing key)

body:

```diff
- {"total":0,"data":[]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### lti-registrations-post-accounts-lti-registrations-bind

Arguments: `POST /api/v1/accounts/6/lti_registrations/4/bind`

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

at body $.errors[0] (type), expected string, got object

body:

```diff
- {"errors":["one of lti_configuration or url is required"]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### lti-registrations-post-accounts-lti-registrations-install-from-template

Arguments: `POST /api/v1/accounts/6/lti_registrations/4/install_from_template`

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

### lti-registrations-post-accounts-lti-registrations

Arguments: `POST /api/v1/accounts/6/lti_registrations`

at status (400→415)

status:

```diff
expected 400, got 415
```

at body $.errors[0].message, char 0: expected "⟨c⟩onfiguration is …", got "⟨E⟩xpected request …"

body:

```diff
- {"errors":[{"message":"configuration is missing"}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### lti-registrations-put-accounts-lti-registrations-reset

Arguments: `PUT /api/v1/accounts/6/lti_registrations/4/reset`

at body $.errors[0].code (missing key)

body:

```diff
- {"errors":[{"code":"not_found","message":"The specified resource does not exist."}]}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### lti-registrations-put-accounts-lti-registrations

Arguments: `PUT /api/v1/accounts/6/lti_registrations/4`

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

### outcome-groups-api-delete-accounts-outcome-groups-outcomes

Arguments: `DELETE /api/v1/accounts/6/outcome_groups/2/outcomes/2`

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

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### outcome-groups-api-delete-courses-outcome-groups-outcomes

Arguments: `DELETE /api/v1/courses/1/outcome_groups/2/outcomes/2`

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

at body $.context_type (missing key)

body:

```diff
- {"context_type":"Course","context_id":1,"quiz_lti":false,"url":"/api/v1/courses/1/outcome_groups/2/outcomes/2","outcome_group":{"id":2,"title":"Parity Course","vendor_guid":null,"url":"/api/v1/courses/1/outcome_groups/2","subgroups_url":"/api/v1/courses/1/outcome_groups/2/subgroups","outcomes_url":"/api/v1/courses/1/outcome_groups/2/outcomes","can_edit":true},"outcome":{"id":2,"context_type":"Course","context_id":1,"vendor_guid":null,"display_name":null,"title":"Parity outcome","url":"/api/v1/outcomes/2","can_edit":true,"has_updateable_rubrics":false},"assessed":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-delete-courses-outcome-groups

Arguments: `DELETE /api/v1/courses/1/outcome_groups/2`

at status (400→404)

status:

```diff
expected 400, got 404
```

at body, line 1, column 1, char 0: expected "⟨\"⟩error\"", got "⟨{⟩\"errors\":[{\"mess…"

body:

```diff
- "error"
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-get-accounts-outcome-groups-account-chain

Arguments: `GET /api/v1/accounts/6/outcome_groups/account_chain`

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

at header link (missing key)

header link:

```diff
- </api/v1/accounts/6/outcome_groups/account_chain?page=1&per_page=10>; rel="current",</api/v1/accounts/6/outcome_groups/account_chain?page=1&per_page=10>; rel="first",</api/v1/accounts/6/outcome_groups/account_chain?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
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

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### outcome-groups-api-get-accounts-outcome-groups

Arguments: `GET /api/v1/accounts/6/outcome_groups`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":1,"title":"Parity Account","vendor_guid":null,"url":"/api/v1/accounts/6/outcome_groups/1","subgroups_url":"/api/v1/accounts/6/outcome_groups/1/subgroups","outcomes_url":"/api/v1/accounts/6/outcome_groups/1/outcomes","can_edit":true,"import_url":"/api/v1/accounts/6/outcome_groups/1/import","context_id":6,"context_type":"Account","description":null}]
+ []
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

at body (lines added), line 1, length 0 expected, 93 got

body:

```diff
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
(trailing newline: expected true, got false)
```

### outcome-groups-api-get-courses-outcome-group-links

Arguments: `GET /api/v1/courses/1/outcome_group_links`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"context_type":"Course","context_id":1,"quiz_lti":false,"url":"/api/v1/courses/1/outcome_groups/2/outcomes/1","outcome_group":{"id":2,"title":"Parity Course","vendor_guid":null,"url":"/api/v1/courses/1/outcome_groups/2","subgroups_url":"/api/v1/courses/1/outcome_groups/2/subgroups","outcomes_url":"/api/v1/courses/1/outcome_groups/2/outcomes","can_edit":true},"outcome":{"id":1,"context_type":"Course","context_id":1,"vendor_guid":null,"display_name":null,"title":"Parity outcome","url":"/api/v1/outcomes/1","can_edit":true,"has_updateable_rubrics":false},"can_unlink":true,"assessed":false}]
+ []
```

### outcome-groups-api-get-courses-outcome-groups-337

Arguments: `GET /api/v1/courses/1/outcome_groups/2`

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

at body $.id (missing key)

body:

```diff
- {"id":2,"title":"Parity Course","vendor_guid":null,"url":"/api/v1/courses/1/outcome_groups/2","subgroups_url":"/api/v1/courses/1/outcome_groups/2/subgroups","outcomes_url":"/api/v1/courses/1/outcome_groups/2/outcomes","can_edit":true,"import_url":"/api/v1/courses/1/outcome_groups/2/import","context_id":1,"context_type":"Course","description":null}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-get-courses-outcome-groups-account-chain

Arguments: `GET /api/v1/courses/1/outcome_groups/account_chain`

at header link, line 1, column 2, char 1: expected "<⟨/⟩api/v1/courses/1…", got "<⟨h⟩ttp://127.0.0.1:…"

header link:

```diff
- </api/v1/courses/1/outcome_groups/account_chain?page=1&per_page=10>; rel="current",</api/v1/courses/1/outcome_groups/account_chain?page=1&per_page=10>; rel="first",</api/v1/courses/1/outcome_groups/account_chain?page=1&per_page=10>; rel="last"
+ <http://127.0.0.1:51468/api/v1/courses/1/outcome_groups/account_chain?page=1&per_page=10>; rel="current",<http://127.0.0.1:51468/api/v1/courses/1/outcome_groups/account_chain?page=1&per_page=10>; rel="first",<http://127.0.0.1:51468/api/v1/courses/1/outcome_groups/account_chain?page=1&per_page=10>; rel="last"
```

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":1,"title":"Parity Account","description":"Account level outcomes group.","dontImport":true,"url":"/api/v1/accounts/6/outcome_groups/1","subgroups_url":"/api/v1/accounts/6/outcome_groups/1/subgroups","outcomes_url":"/api/v1/accounts/6/outcome_groups/1/outcomes"}]
+ []
```

### outcome-groups-api-get-courses-outcome-groups-outcomes

Arguments: `GET /api/v1/courses/1/outcome_groups/2/outcomes`

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
- <http://127.0.0.1:[..]/api/v1/courses/1/outcome_groups/2/outcomes?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/courses/1/outcome_groups/2/outcomes?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/courses/1/outcome_groups/2/outcomes?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- [{"context_type":"Course","context_id":1,"quiz_lti":false,"url":"/api/v1/courses/1/outcome_groups/2/outcomes/1","outcome_group":{"id":2,"title":"Parity Course","vendor_guid":null,"url":"/api/v1/courses/1/outcome_groups/2","subgroups_url":"/api/v1/courses/1/outcome_groups/2/subgroups","outcomes_url":"/api/v1/courses/1/outcome_groups/2/outcomes","can_edit":true},"outcome":{"id":1,"context_type":"Course","context_id":1,"vendor_guid":null,"display_name":null,"title":"Parity outcome","url":"/api/v1/outcomes/1","can_edit":true,"has_updateable_rubrics":false},"can_unlink":true,"assessed":false}]
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-get-courses-outcome-groups-subgroups

Arguments: `GET /api/v1/courses/1/outcome_groups/2/subgroups`

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
- <http://127.0.0.1:[..]/api/v1/courses/1/outcome_groups/2/subgroups?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/courses/1/outcome_groups/2/subgroups?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/courses/1/outcome_groups/2/subgroups?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-get-courses-outcome-groups

Arguments: `GET /api/v1/courses/1/outcome_groups`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":2,"title":"Parity Course","vendor_guid":null,"url":"/api/v1/courses/1/outcome_groups/2","subgroups_url":"/api/v1/courses/1/outcome_groups/2/subgroups","outcomes_url":"/api/v1/courses/1/outcome_groups/2/outcomes","can_edit":true,"import_url":"/api/v1/courses/1/outcome_groups/2/import","context_id":1,"context_type":"Course","description":null}]
+ []
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
+ /api/v1/courses/1/outcome_groups/5
```

at body (lines added), line 1, length 0 expected, 506 got

body:

```diff
+ {"can_edit":true,"context_id":1,"context_type":"Course","course_id":1,"created_at":"2026-11-27T14:23:48.943908Z","description":null,"id":5,"import_url":"/api/v1/courses/1/outcome_groups/5/import","outcomes_url":"/api/v1/courses/1/outcome_groups/5/outcomes","parent_outcome_group_id":null,"subgroups_url":"/api/v1/courses/1/outcome_groups/5/subgroups","title":"Parity Course","updated_at":"2026-11-27T14:23:48.943908Z","url":"/api/v1/courses/1/outcome_groups/5","vendor_guid":null,"workflow_state":"active"}
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

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### outcome-groups-api-post-accounts-outcome-groups-outcomes

Arguments: `POST /api/v1/accounts/6/outcome_groups/2/outcomes`

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

### outcome-groups-api-post-accounts-outcome-groups-subgroups

Arguments: `POST /api/v1/accounts/6/outcome_groups/2/subgroups`

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

### outcome-groups-api-post-courses-outcome-groups-import

Arguments: `POST /api/v1/courses/1/outcome_groups/2/import`

at status (400→404)

status:

```diff
expected 400, got 404
```

at body, line 1, column 1, char 0: expected "⟨\"⟩error\"", got "⟨{⟩\"errors\":[{\"mess…"

body:

```diff
- "error"
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-post-courses-outcome-groups-outcomes

Arguments: `POST /api/v1/courses/1/outcome_groups/2/outcomes`

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

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"short_description":[{"attribute":"short_description","message":"can't be blank","type":"blank"}]}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### outcome-groups-api-post-courses-outcome-groups-subgroups

Arguments: `POST /api/v1/courses/1/outcome_groups/2/subgroups`

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

at body, line 1, column 1, char 0: expected "⟨\"⟩error\"", got "⟨{⟩\"status\":\"unauth…"

body:

```diff
- "error"
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### outcome-groups-api-post-global-outcome-groups-outcomes

Arguments: `POST /api/v1/global/outcome_groups/2/outcomes`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨3⟩IKmMKhz2wgthpi2q…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]_[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=3IKmMKhz2wgthpi2qL9P0M0VuL%2FwdIPBs%2FPP6KZQ8g%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### outcome-groups-api-put-accounts-outcome-groups-outcomes

Arguments: `PUT /api/v1/accounts/6/outcome_groups/2/outcomes/2`

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

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨u⟩ser not authoriz…"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### outcome-groups-api-put-courses-outcome-groups-outcomes

Arguments: `PUT /api/v1/courses/1/outcome_groups/2/outcomes/2`

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

at body $.context_type (missing key)

body:

```diff
- {"context_type":"Course","context_id":1,"quiz_lti":false,"url":"/api/v1/courses/1/outcome_groups/2/outcomes/2","outcome_group":{"id":2,"title":"Parity Course","vendor_guid":null,"url":"/api/v1/courses/1/outcome_groups/2","subgroups_url":"/api/v1/courses/1/outcome_groups/2/subgroups","outcomes_url":"/api/v1/courses/1/outcome_groups/2/outcomes","can_edit":true},"outcome":{"id":2,"context_type":"Course","context_id":1,"vendor_guid":null,"display_name":null,"title":"Parity outcome","url":"/api/v1/outcomes/2","can_edit":true,"has_updateable_rubrics":false},"can_unlink":true,"assessed":false}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-put-courses-outcome-groups

Arguments: `PUT /api/v1/courses/1/outcome_groups/2`

at status (400→404)

status:

```diff
expected 400, got 404
```

at body, line 1, column 1, char 0: expected "⟨\"⟩error\"", got "⟨{⟩\"errors\":[{\"mess…"

body:

```diff
- "error"
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### outcome-groups-api-put-global-outcome-groups-outcomes

Arguments: `PUT /api/v1/global/outcome_groups/2/outcomes/2`

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

at body $.message (missing key)

body:

```diff
- {"message":"Rubric not found"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### rubrics-api-get-accounts-rubrics-upload-rubrics

Arguments: `GET /api/v1/accounts/6/rubrics/upload/1/rubrics`

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

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### rubrics-api-get-accounts-rubrics-upload

Arguments: `GET /api/v1/accounts/6/rubrics/upload/1`

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
- {"id":1,"root_account_id":6,"workflow_state":"initializing","user_id":1,"attachment_id":null,"progress":0,"error_count":0,"error_data":[],"created_at":"2026-11-27T13:01:04Z","updated_at":"2026-11-27T13:01:04Z","account_id":6,"course_id":null,"user":{"id":1,"name":"Parity User","created_at":"2026-11-26T20:10:15Z","sortable_name":"User, Parity","short_name":"Parity User","sis_user_id":null,"integration_id":null,"login_id":"parity@example.com"}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
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

at body $.message (missing key)

body:

```diff
- {"message":"Rubric not found"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### rubrics-api-get-accounts-rubrics

Arguments: `GET /api/v1/accounts/6/rubrics`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/rubrics?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/rubrics?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/rubrics?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### rubrics-api-get-courses-rubrics-351

Arguments: `GET /api/v1/courses/1/rubrics/1`

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

at body $.id (missing key)

body:

```diff
- {"id":1,"title":"Parity Rubric","context_id":1,"context_type":"Course","points_possible":10.0,"reusable":false,"public":false,"read_only":false,"free_form_criterion_comments":null,"hide_score_total":null,"data":null,"rating_order":"descending","button_display":"numeric"}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### rubrics-api-get-courses-rubrics-upload

Arguments: `GET /api/v1/courses/1/rubrics/upload/1`

at body $.message, char 0: expected "⟨A⟩ctiveRecord::Rec…", got "⟨C⟩ouldn't find Rub…"

body:

```diff
- {"message":"ActiveRecord::RecordNotFound"}
+ {"message":"Couldn't find RubricImport with 'id'=1"}
```

### rubrics-api-get-courses-rubrics-used-locations

Arguments: `GET /api/v1/courses/1/rubrics/1/used_locations`

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
- </api/v1/courses/1/rubrics/1/used_locations?include_host=true&page=1&per_page=100>; rel="current",</api/v1/courses/1/rubrics/1/used_locations?include_host=true&page=1&per_page=100>; rel="first",</api/v1/courses/1/rubrics/1/used_locations?include_host=true&page=1&per_page=100>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### rubrics-api-get-courses-rubrics

Arguments: `GET /api/v1/courses/1/rubrics`

at body $ (length), length 1 expected, 0 got

body:

```diff
- [{"id":1,"title":"Parity Rubric","context_id":1,"context_type":"Course","points_possible":10.0,"reusable":false,"public":false,"read_only":false,"free_form_criterion_comments":null,"hide_score_total":null,"data":null,"rating_order":"descending","button_display":"numeric"}]
+ []
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

at body $.error (missing key)

body:

```diff
- {"error":"No rubric IDs provided"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
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

at body $.message (missing key)

body:

```diff
- {"message":"No file attached"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### rubrics-api-post-courses-rubrics-upload

Arguments: `POST /api/v1/courses/1/rubrics/upload`

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

at body $.message (missing key)

body:

```diff
- {"message":"No file attached"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### submissions-api-delete-courses-assignments-submissions-read

Arguments: `DELETE /api/v1/courses/1/assignments/1/submissions/3/read`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨q⟩3uc3guzdYk8%2B3W…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..].[..].[..]; path=/; httponly
+ _csrf_token=q3uc3guzdYk8%2B3WhtFCWGF79ZXeJ%2BgjfeZKaDOfgVA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### submissions-api-get-courses-assignments-gradeable-students-300

Arguments: `GET /api/v1/courses/1/assignments/gradeable_students`

at body $ (length), length 0 expected, 2 got

body:

```diff
- []
+ [{"assignment_ids":[],"avatar_image_url":null,"display_name":"Parity Student","html_url":"/courses/1/users/3","id":3,"pronouns":null},{"assignment_ids":[],"avatar_image_url":null,"display_name":"Test Student","html_url":"/courses/1/users/8","id":8,"pronouns":null}]
```

### submissions-api-get-sections-assignments-submissions-document-annotations-read

Arguments: `GET /api/v1/sections/3/assignments/1/submissions/3/document_annotations/read`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨8⟩BQYVLrD1idBDfcFg…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D[..]_[..]_[..].[..]_[..]_[..]_[..].[..]; path=/; httponly
+ _csrf_token=8BQYVLrD1idBDfcFg05YhFR%2Fs%2FH%2BbNB3cMLIEK8U8A%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### submissions-api-get-sections-assignments-submissions

Arguments: `GET /api/v1/sections/3/assignments/1/submissions`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨b⟩h9ppsC6RgI1d2nnl…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D[..]_[..]_[..]_[..].[..]; path=/; httponly
+ _csrf_token=bh9ppsC6RgI1d2nnlw5sWScBsRxKUSR%2B93ISHZbCKQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### submissions-api-post-courses-assignments-submissions-files

Arguments: `POST /api/v1/courses/1/assignments/1/submissions/3/files`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨H⟩t27vepetXuHg2H3C…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D[..]_[..]_[..].[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=Ht27vepetXuHg2H3CevFw6gbnQbahY3yfJjG3QBN%2FA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### submissions-api-post-courses-submissions-update-grades

Arguments: `POST /api/v1/courses/1/submissions/update_grades`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨g⟩S7iTHIX19AZ9YGvg…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D[..]_[..]_[..]-[..].[..]; path=/; httponly
+ _csrf_token=gS7iTHIX19AZ9YGvgAsczEH2CH1ndCi7e9h%2BRClZOQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### submissions-api-put-courses-assignments-anonymous-submissions

Arguments: `PUT /api/v1/courses/1/assignments/1/anonymous_submissions/3`

at status (404→415)

status:

```diff
expected 404, got 415
```

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨r⟩BfikOja%2BgVvq10…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]_[..].[..].[..]; path=/; httponly
+ _csrf_token=rBfikOja%2BgVvq10v0tWPprm9AAHLVIapMk6EMwsr7w%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.errors[0].message, char 0: expected "⟨T⟩he specified res…", got "⟨E⟩xpected request …"

body:

```diff
- {"errors":[{"message":"The specified resource does not exist."}]}
+ {"errors":[{"message":"Expected request with `Content-Type: application/json`"}]}
```

### submissions-api-put-courses-assignments-submissions-read-64

Arguments: `PUT /api/v1/courses/1/assignments/1/submissions/3/read/:item`

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

Arguments: `PUT /api/v1/courses/1/submissions/3/clear_unread`

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

### submissions-api-put-sections-assignments-anonymous-submissions

Arguments: `PUT /api/v1/sections/3/assignments/1/anonymous_submissions/3`

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

### submissions-api-put-sections-assignments-submissions-read

Arguments: `PUT /api/v1/sections/3/assignments/1/submissions/3/read`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨x⟩DoFnzc%2FL1YUxcC…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..].[..]; path=/; httponly
+ _csrf_token=xDoFnzc%2FL1YUxcCTrcAuhzyXgmHvQbKpYM0ovFPfvg%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### users-delete-users-sessions

Arguments: `DELETE /api/v1/users/3/sessions`

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

at body $.message (missing key)

body:

```diff
- {"message":"All sessions terminated"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-get-accounts-users

Arguments: `GET /api/v1/accounts/6/users`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/accounts/6/users?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/accounts/6/users?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/accounts/6/users?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- [{"id":3,"name":"Parity Student","created_at":"2026-11-27T05:29:37Z","sortable_name":"Student, Parity","short_name":"Parity Student","sis_user_id":null,"integration_id":null,"login_id":"student@example.com"},{"id":1,"name":"Parity User","created_at":"2026-11-26T20:10:15Z","sortable_name":"User, Parity","short_name":"Parity User","sis_user_id":null,"integration_id":null,"login_id":"parity@example.com"}]
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-get-dashboard-dashboard-cards

Arguments: `GET /api/v1/dashboard/dashboard_cards`

at body $[0].shortName, char 1: expected "P⟨a⟩rity Course", got "P⟨A⟩RITY"

body:

```diff
- [{"longName":"Parity Course - PARITY","shortName":"Parity Course","originalName":"Parity Course","courseCode":"PARITY","assetString":"course_1","href":"/courses/1","term":null,"subtitle":"enrolled as: Teacher","enrollmentState":"active","enrollmentType":"TeacherEnrollment","observee":null,"id":1,"isFavorited":false,"isK5Subject":false,"isHomeroom":false,"useClassicFont":false,"canManage":true,"canReadAnnouncements":true,"image":null,"color":null,"position":null,"published":true,"links":[{"css_class":"assignments","icon":"icon-assignment","hidden":null,"path":"/courses/1/assignments","label":"Assignments"},{"css_class":"discussions","icon":"icon-discussion","hidden":null,"path":"/courses/1/discussion_topics","label":"Discussions"}],"canChangeCoursePublishState":true,"defaultView":"modules","pagesUrl":"http://54.252.48.186/courses/1/pages","frontPageTitle":"Front Page"}]
+ [{"longName":"Parity Course - PARITY","shortName":"PARITY","originalName":"Parity Course","courseCode":"PARITY","assetString":"course_1","href":"/courses/1","term":null,"subtitle":"enrolled as Teacher","enrollmentState":"active","enrollmentType":"TeacherEnrollment","observee":null,"id":1,"isFavorited":false,"isK5Subject":false,"isHomeroom":false,"useClassicFont":false,"canManage":true,"canReadAnnouncements":true,"image":null,"color":null,"position":null,"published":true,"links":[{"css_class":"discussions","icon":null,"hidden":false,"path":"/courses/1/discussion_topics","label":"Discussions"},{"css_class":"assignments","icon":null,"hidden":false,"path":"/courses/1/assignments","label":"Assignments"},{"css_class":"announcements","icon":null,"hidden":false,"path":"/courses/1/announcements","label":"Announcements"},{"css_class":"files","icon":null,"hidden":false,"path":"/courses/1/files","label":"Files"}],"canChangeCoursePublishState":true,"defaultView":"modules","pagesUrl":"/courses/1/pages","frontPageTitle":null}]
```

### users-get-users-activity-stream

Arguments: `GET /api/v1/users/activity_stream`

at header link, line 1, column 19, char 18: expected "…ttp://127.0.0.1:⟨⟩", got "…ttp://127.0.0.1:⟨5⟩1468/api/v1/user…"

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/users/self/activity_stream?page=1&per_page=21>; rel="current",<http://127.0.0.1:[..]/api/v1/users/self/activity_stream?page=1&per_page=21>; rel="first",<http://127.0.0.1:[..]/api/v1/users/self/activity_stream?page=1&per_page=21>; rel="last"
+ <http://127.0.0.1:51468/api/v1/users/activity_stream?page=1&per_page=21>; rel="current",<http://127.0.0.1:51468/api/v1/users/activity_stream?page=1&per_page=21>; rel="first",<http://127.0.0.1:51468/api/v1/users/activity_stream?page=1&per_page=21>; rel="last"
```

### users-get-users-colors-434

Arguments: `GET /api/v1/users/3/colors/:asset_string`

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

### users-get-users-colors

Arguments: `GET /api/v1/users/3/colors`

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨9⟩sJZg7h5XQaXYQcvf…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..] [..]_[..]_[..]_[..].[..]-[..].[..]; path=/; httponly
+ _csrf_token=9sJZg7h5XQaXYQcvf%2B67ALPvrdFwjbv4IjJzXU6f%2FA%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.custom_colors (missing key)

body:

```diff
- {"custom_colors":{}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-get-users-dashboard-positions

Arguments: `GET /api/v1/users/3/dashboard_positions`

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

at body $.dashboard_positions (missing key)

body:

```diff
- {"dashboard_positions":{}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-get-users-graded-submissions

Arguments: `GET /api/v1/users/3/graded_submissions`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/users/3/graded_submissions?page=first&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/users/3/graded_submissions?page=first&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/users/3/graded_submissions?page=first&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-get-users-missing-submissions

Arguments: `GET /api/v1/users/3/missing_submissions`

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

at header link (missing key)

header link:

```diff
- <http://127.0.0.1:[..]/api/v1/users/3/missing_submissions?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/users/3/missing_submissions?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/users/3/missing_submissions?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- []
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-get-users-new-user-tutorial-statuses

Arguments: `GET /api/v1/users/3/new_user_tutorial_statuses`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at body $.message (missing key)

body:

```diff
- {"message":"This endpoint only works against the current user"}
+ {"status":"unauthenticated","errors":[{"message":"user authorization required"}]}
```

### users-get-users-self-upcoming-events

Arguments: `GET /api/v1/users/self/upcoming_events`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨l⟩sGfeqkIbE0MnLMwM…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]_[..].[..]; path=/; httponly
+ _csrf_token=lsGfeqkIbE0MnLMwMc%2FrUSobOjxSdThbQLdm1lXrNw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

### users-get-users-settings

Arguments: `GET /api/v1/users/3/settings`

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

at body $.manual_mark_as_read (missing key)

body:

```diff
- {"manual_mark_as_read":false,"collapse_global_nav":false,"collapse_course_nav":false,"hide_dashcard_color_overlays":false,"release_notes_badge_disabled":false,"comment_library_suggestions_enabled":false,"elementary_dashboard_disabled":false,"default_to_block_editor":false,"widget_dashboard_user_preference":false,"widget_dashboard_dark_mode":false}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-get-users

Arguments: `GET /api/v1/users/3`

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
- {"id":3,"name":"Parity Student","created_at":"2026-11-27T05:29:37Z","sortable_name":"Student, Parity","short_name":"Parity Student","last_name":"Student","first_name":"Parity","locale":null,"permissions":{"can_update_name":true,"can_update_avatar":false,"limit_parent_app_web_access":false}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨B⟩V6Vf6zrGm1XmW0l2…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]-[..]-[..].[..]; path=/; httponly
+ _csrf_token=BV6Vf6zrGm1XmW0l2d5Pc7DP%2FmEi32xPbvhd1665FQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"user":{"pseudonyms":[{"attribute":"pseudonyms","message":"is invalid","type":"invalid"}]},"pseudonym":{"unique_id":[{"attribute":"unique_id","message":"is invalid","type":"invalid"},{"attribute":"unique_id","message":"is too short (minimum is 1 character)","type":"too_short"}]},"observee":{},"pairing_code":{},"recaptcha":null}}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-post-users-files

Arguments: `POST /api/v1/users/3/files`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨D⟩q5hOmBznrgAoY0bo…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D[..]_[..]_[..]-[..]_[..].[..]; path=/; httponly
+ _csrf_token=Dq5hOmBznrgAoY0boHtI0%2Fx0WIWDHW%2FZYvr3iB%2FvBw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
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

### users-put-users-colors

Arguments: `PUT /api/v1/users/3/colors/:asset_string`

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

### users-put-users-dashboard-positions

Arguments: `PUT /api/v1/users/3/dashboard_positions`

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

### users-put-users-files-ui-version-preference

Arguments: `PUT /api/v1/users/3/files_ui_version_preference`

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

at body $.message (missing key)

body:

```diff
- {"message":"Invalid files_ui_version provided"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-put-users-new-user-tutorial-statuses

Arguments: `PUT /api/v1/users/3/new_user_tutorial_statuses/:page_name`

at header cache-control, line 1, column 4, char 3: expected "no-⟨c⟩ache", got "no-⟨s⟩tore"

header cache-control:

```diff
- no-cache
+ no-store
```

at body $.message (missing key)

body:

```diff
- {"message":"This endpoint only works against the current user"}
+ {"status":"unauthenticated","errors":[{"message":"user authorization required"}]}
```

### users-put-users-settings

Arguments: `PUT /api/v1/users/3/settings`

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

at body $.manual_mark_as_read (missing key)

body:

```diff
- {"manual_mark_as_read":false,"collapse_global_nav":false,"collapse_course_nav":false,"hide_dashcard_color_overlays":false,"release_notes_badge_disabled":false,"comment_library_suggestions_enabled":false,"elementary_dashboard_disabled":false,"default_to_block_editor":false,"widget_dashboard_user_preference":false,"widget_dashboard_dark_mode":false}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### users-put-users-text-editor-preference

Arguments: `PUT /api/v1/users/3/text_editor_preference`

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

at header content-type, line 1, column 17, char 16: expected "application/json⟨⟩", got "application/json⟨;⟩ charset=utf-8"

header content-type:

```diff
- application/json
+ application/json; charset=utf-8
```

at body (lines added), line 1, length 0 expected, 93 got

body:

```diff
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
(trailing newline: expected true, got false)
```

### users-put-users

Arguments: `PUT /api/v1/users/3`

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨u⟩OkoxK2Ns93EJqL95…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..]-[..]-[..]_[..]-[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=uOkoxK2Ns93EJqL95QBf2FHFVzhEp304AmjFBqn43A%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.id (missing key)

body:

```diff
- {"id":3,"name":"Parity Student","created_at":"2026-11-27T05:29:37Z","sortable_name":"Student, Parity","short_name":"Parity Student","locale":null,"time_zone":"Mountain Time (US \u0026 Canada)"}
+ {"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}
```

### wiki-pages-api-delete-courses-pages

Arguments: `DELETE /api/v1/courses/1/pages/front-page`

at status (400→404)

status:

```diff
expected 400, got 404
```

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"front_page":[{"attribute":"front_page","message":"The front page cannot be deleted","type":"The front page cannot be deleted"}]}}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-courses-front-page

Arguments: `GET /api/v1/courses/1/front_page`

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

at body $.url (missing key)

body:

```diff
- {"url":"front-page","title":"Front Page","created_at":"2026-11-27T12:16:09Z","editing_roles":"teachers","page_id":5,"last_edited_by":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null},"published":true,"hide_from_students":false,"front_page":true,"html_url":"http://127.0.0.1:[..]/courses/1/pages/front-page","todo_date":null,"publish_at":null,"updated_at":"2026-11-27T12:16:09Z","locked_for_user":false,"body":"Parity front page."}
+ {"message":"No front page has been set"}
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

### wiki-pages-api-get-courses-pages-343

Arguments: `GET /api/v1/courses/1/pages/front-page`

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

at body $.url (missing key)

body:

```diff
- {"url":"front-page","title":"Front Page","created_at":"2026-11-27T12:16:09Z","editing_roles":"teachers","page_id":5,"last_edited_by":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null},"published":true,"hide_from_students":false,"front_page":true,"html_url":"http://127.0.0.1:[..]/courses/1/pages/front-page","todo_date":null,"publish_at":null,"updated_at":"2026-11-27T12:16:09Z","locked_for_user":false,"body":"Parity front page."}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-courses-pages-revisions-345

Arguments: `GET /api/v1/courses/1/pages/front-page/revisions/1`

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

at body $.revision_id (missing key)

body:

```diff
- {"revision_id":1,"updated_at":"2026-11-27T12:16:09Z","latest":true,"url":"front-page","title":"Front Page","body":"Parity front page.","edited_by":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null}}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-courses-pages-revisions-latest

Arguments: `GET /api/v1/courses/1/pages/front-page/revisions/latest`

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

at body $.revision_id (missing key)

body:

```diff
- {"revision_id":1,"updated_at":"2026-11-27T12:16:09Z","latest":true,"url":"front-page","title":"Front Page","body":"Parity front page.","edited_by":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null}}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-get-courses-pages-revisions

Arguments: `GET /api/v1/courses/1/pages/front-page/revisions`

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
- <http://127.0.0.1:[..]/api/v1/courses/1/pages/front-page/revisions?page=1&per_page=10>; rel="current",<http://127.0.0.1:[..]/api/v1/courses/1/pages/front-page/revisions?page=1&per_page=10>; rel="first",<http://127.0.0.1:[..]/api/v1/courses/1/pages/front-page/revisions?page=1&per_page=10>; rel="last"
(missing)
```

at body $ (type), expected array, got object

body:

```diff
- [{"revision_id":1,"updated_at":"2026-11-27T12:16:09Z","latest":true,"edited_by":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null}}]
+ {"errors":[{"message":"The specified resource does not exist."}]}
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

### wiki-pages-api-post-courses-pages-accessibility-queue-scan

Arguments: `POST /api/v1/courses/1/pages/front-page/accessibility/queue_scan`

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

### wiki-pages-api-post-courses-pages-accessibility-scan

Arguments: `POST /api/v1/courses/1/pages/front-page/accessibility/scan`

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

### wiki-pages-api-post-courses-pages-ai-alt-text

Arguments: `POST /api/v1/courses/1/pages_ai/alt_text`

at status (403→400)

status:

```diff
expected 403, got 400
```

at body $.error (missing key)

body:

```diff
- {"error":"The feature is not available"}
+ {"errors":[{"message":"attachment_id is required"}]}
```

### wiki-pages-api-post-courses-pages-duplicate

Arguments: `POST /api/v1/courses/1/pages/front-page/duplicate`

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

### wiki-pages-api-post-courses-pages-revisions

Arguments: `POST /api/v1/courses/1/pages/front-page/revisions/1`

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

at body $.revision_id (missing key)

body:

```diff
- {"revision_id":1,"updated_at":"2026-11-27T12:16:09Z","latest":true,"url":"front-page","title":"Front Page","body":"Parity front page.","edited_by":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null}}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-post-courses-pages

Arguments: `POST /api/v1/courses/1/pages`

at status (400→422)

status:

```diff
expected 400, got 422
```

at body $.errors (type), expected object, got array

body:

```diff
- {"errors":{"title":[{"attribute":"title","message":"Title can't be blank","type":"Title can't be blank"}]}}
+ {"errors":[{"message":"title is required"}]}
```

### wiki-pages-api-put-courses-front-page

Arguments: `PUT /api/v1/courses/1/front_page`

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨6⟩mMS8cuD%2ByKDfWC…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%3D[..]_[..]_[..].[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=6mMS8cuD%2ByKDfWCEsWxJDnX2V8T888%2BjzOstR92I%2Fw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.url (missing key)

body:

```diff
- {"url":"front-page","title":"Front Page","created_at":"2026-11-27T12:16:09Z","editing_roles":"teachers","page_id":5,"last_edited_by":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null},"published":true,"hide_from_students":false,"front_page":true,"html_url":"http://127.0.0.1:[..]/courses/1/pages/front-page","todo_date":null,"publish_at":null,"updated_at":"2026-11-27T12:16:09Z","locked_for_user":false,"body":"Parity front page."}
+ {"errors":[{"message":"title is required"}]}
```

### wiki-pages-api-put-courses-pages

Arguments: `PUT /api/v1/courses/1/pages/front-page`

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

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨N⟩6alsFiePR5RB3s1W…"

header set-cookie:

```diff
- _csrf_token=[..]%3D%3D; path=/, log_session_id=[..]; path=/; httponly, _normandy_session=[..].[..]-[..].[..].[..]; path=/; httponly
+ _csrf_token=N6alsFiePR5RB3s1W7%2FzxNJ9L3UUEN5u82xZoPrepQ%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```

at body $.url (missing key)

body:

```diff
- {"url":"front-page","title":"Front Page","created_at":"2026-11-27T12:16:09Z","editing_roles":"teachers","page_id":5,"last_edited_by":{"id":1,"anonymous_id":"1","display_name":"Parity User","avatar_image_url":"http://127.0.0.1:[..]/images/messages/avatar-50.png","html_url":"http://127.0.0.1:[..]/courses/1/users/1","pronouns":null},"published":true,"hide_from_students":false,"front_page":true,"html_url":"http://127.0.0.1:[..]/courses/1/pages/front-page","todo_date":null,"publish_at":null,"updated_at":"2026-11-27T12:16:09Z","locked_for_user":false,"body":"Parity front page."}
+ {"errors":[{"message":"The specified resource does not exist."}]}
```

### wiki-pages-api-put-groups-pages

Arguments: `PUT /api/v1/groups/1/pages/front-page`

at header set-cookie, line 1, column 13, char 12: expected "_csrf_token=⟨⟩", got "_csrf_token=⟨d⟩Bgm60Asf%2FkFgoU…"

header set-cookie:

```diff
- _csrf_token=[..]%[..]%[..]%[..]%[..]%3D[..]_[..]_[..]-[..].[..]_[..].[..]; path=/; httponly
+ _csrf_token=dBgm60Asf%2FkFgoUUL3PR4LD1Ts0Xm4YnxpgVzVUyGw%3D%3D; path=/, log_session_id=6e704008d9998acd89bc6f83b1154193; path=/; httponly, _normandy_session=6e704008d9998acd89bc6f83b1154193.6e704008d9998acd89bc6f83b1154193; path=/; httponly
```
