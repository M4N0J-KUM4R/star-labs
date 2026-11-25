# Schema mapping: the 101 star-only tables

Branch `p5/infra-schema-map` (base 70792ef) · 2026-11-25 · docs-only change.

Inputs: `orchestration/parity-audit-2026-11-25.md` §6 (the 101 star-only names), `canvas-src/`
(Canvas LMS Ruby source, pinned upstream 1c9f0bb), and `migration/crates/` at 70792ef.
Star-db ground truth re-verified here: 170 migration files, 396 unique `CREATE TABLE` names; canvas
`db/migrate` = 95 files, 298 unique `create_table` names + the 7 `aua_logs_0..6` partitions created by
a Ruby loop (`db/migrate/20101210192618_init_canvas_db.rb:31`). All 101 names below are confirmed
absent from canvas `create_table` and present in star-db.

## 0. The 102nd name — `would` (parser artifact, excluded)

The audit counted a 102nd name, `would`. It is not a table: the audit's `create_table` pattern matched
prose inside SQL comments, e.g. `migration/crates/star-db/migrations/0128_collaborations.sql:17`
("-- 0128-numbered CREATE TABLE would run BEFORE the snapshot's unguarded …") and
`0229_conferences_tails.sql:8`. It is excluded from this mapping.

## 1. Method

1. **Ownership** — all 399 `.rs` files under `migration/crates/` were scanned for a table name adjacent to a SQL
   verb (`FROM`/`JOIN`/`INSERT INTO`/`UPDATE`/`DELETE FROM`/`CREATE|ALTER|DROP TABLE`/`TRUNCATE`). Matches in
   `src/` count as handler usage; `tests/` matches are counted separately (context only). This shows which
   handlers *reach* a table, not that results match Canvas (the limit the audit states in §6).
2. **Canvas twin verification** — every candidate was checked against `canvas-src`: `app/models/*.rb`
   (`self.table_name`, namespacing, STI), the 298-name `create_table` set, and full-tree greps for the name.
   No twin was assumed from the name alone; each row cites the canvas file/line that decides it.
3. **Classes**:
   - **canvas twin: table** — the star table represents a canvas table (same or different name); star handlers may or may not use it.
   - **obsolete duplicate** — a star-only table with zero handler references whose entity is already carried in star-db by a table handlers actually use (usually the canvas-named twin).
   - **star-specific (reason)** — no canvas counterpart found in canvas-src.

## 2. Summary

| class | count |
|---|---|
| canvas twin | 74 (incl. the 7 `aua_logs_*` partitions) |
| obsolete duplicate | 6 |
| star-specific | 21 |
| **total** | **101** |

Handler usage across the 101 (src-reference counts): `read` 10, `read+write` 62, `write` 12.
17 tables have zero handler (src) references: `account_user_dismissals`, `assignment_annotations`, `aua_logs_0`, `aua_logs_1`, `aua_logs_2`, `aua_logs_3`, `aua_logs_4`, `aua_logs_5`, `aua_logs_6`, `audit_events`, `checkpoints`, `discussion_entry_reports`, `document_annotation_reads`, `master_courses`, `peer_reviews`, `sub_assignments`, `submission_histories`.

## 3. Per-table classification

| star table | canvas twin | owner crate(s) (src) | read/write | evidence (usage · creating star migration · canvas evidence) |
|---|---|---|---|---|
| `account_terms_of_service` | canvas twin: terms_of_services | star-accounts | read | src 2 +12 test · mig: 0089_account_admin_deep.sql · canvas app/models/terms_of_service.rb:21 TermsOfService < ApplicationRecord (table terms_of_services, created in canvas db/migrate) |
| `account_user_dismissals` | star-specific | — | — | none · mig: 0230_graphql_parity_completion.sql · no canvas counterpart (no account-notification dismissal table/model in canvas-src); unused stub from 0230_graphql_parity_completion.sql |
| `announcement_reads` | canvas twin: discussion_topic_participants | star-announcements | read+write | src 2 · mig: 0052_announcement_reads.sql · announcements are discussion_topics (app/models/announcement.rb:21, STI); topic read state = participants.workflow_state (app/models/discussion_topic.rb:632-642) |
| `announcements` | canvas twin: discussion_topics | star-announcements, star-calendar, star-content, star-dashboard, star-search | read+write | src 20 +3 test · mig: 0001_core_schema.sql · canvas app/models/announcement.rb:21 `class Announcement < DiscussionTopic` (STI) |
| `assignment_annotations` | canvas twin: canvadocs_annotation_contexts | — | — | none · mig: 0119_assignment_annotations.sql · canvas app/models/canvadocs_annotation_context.rb:20-23 (belongs_to attachment/submission/root_account); star-db holds canvadocs_annotation_contexts too (0223) — both currently unreferenced |
| `assignment_group_rules` | canvas twin: assignment_groups | star-assignment-groups, star-graphql | read+write | src 3 +1 test · mig: 0049_assignment_group_rules.sql · canvas keeps rules as a column on assignment_groups (app/models/assignment_group.rb:33,61); star decomposes into rows |
| `aua_logs_0` | canvas twin: aua_logs | — | — | none · mig: 0223_canvas_schema_tables_a.sql · canvas partition: app/models/asset_user_access_log.rb:54 self.table_name = "aua_logs_0"; created by Ruby loop (db/migrate/20101210192618_init_canvas_db.rb:31); canvas truncates/cycles partitions (asset_user_access_log.rb:222). Star created all 7 (0223), no base aua_logs table |
| `aua_logs_1` | canvas twin: aua_logs | — | — | none · mig: 0223_canvas_schema_tables_a.sql · canvas partition: app/models/asset_user_access_log.rb:58 self.table_name = "aua_logs_1"; created by Ruby loop (db/migrate/20101210192618_init_canvas_db.rb:31); canvas truncates/cycles partitions (asset_user_access_log.rb:222). Star created all 7 (0223), no base aua_logs table |
| `aua_logs_2` | canvas twin: aua_logs | — | — | none · mig: 0223_canvas_schema_tables_a.sql · canvas partition: app/models/asset_user_access_log.rb:62 self.table_name = "aua_logs_2"; created by Ruby loop (db/migrate/20101210192618_init_canvas_db.rb:31); canvas truncates/cycles partitions (asset_user_access_log.rb:222). Star created all 7 (0223), no base aua_logs table |
| `aua_logs_3` | canvas twin: aua_logs | — | — | none · mig: 0223_canvas_schema_tables_a.sql · canvas partition: app/models/asset_user_access_log.rb:66 self.table_name = "aua_logs_3"; created by Ruby loop (db/migrate/20101210192618_init_canvas_db.rb:31); canvas truncates/cycles partitions (asset_user_access_log.rb:222). Star created all 7 (0223), no base aua_logs table |
| `aua_logs_4` | canvas twin: aua_logs | — | — | none · mig: 0223_canvas_schema_tables_a.sql · canvas partition: app/models/asset_user_access_log.rb:70 self.table_name = "aua_logs_4"; created by Ruby loop (db/migrate/20101210192618_init_canvas_db.rb:31); canvas truncates/cycles partitions (asset_user_access_log.rb:222). Star created all 7 (0223), no base aua_logs table |
| `aua_logs_5` | canvas twin: aua_logs | — | — | none · mig: 0223_canvas_schema_tables_a.sql · canvas partition: app/models/asset_user_access_log.rb:74 self.table_name = "aua_logs_5"; created by Ruby loop (db/migrate/20101210192618_init_canvas_db.rb:31); canvas truncates/cycles partitions (asset_user_access_log.rb:222). Star created all 7 (0223), no base aua_logs table |
| `aua_logs_6` | canvas twin: aua_logs | — | — | none · mig: 0223_canvas_schema_tables_a.sql · canvas partition: app/models/asset_user_access_log.rb:78 self.table_name = "aua_logs_6"; created by Ruby loop (db/migrate/20101210192618_init_canvas_db.rb:31); canvas truncates/cycles partitions (asset_user_access_log.rb:222). Star created all 7 (0223), no base aua_logs table |
| `audit_events` | obsolete duplicate | — | — | none · mig: 0230_graphql_parity_completion.sql · canvas "audit events" = AnonymousOrModerationEvent (app/models/audit_event_service.rb:20-26); star-db anonymous_or_moderation_events (0108) is used by star-assignments; star audit_events (0230) has zero references |
| `authentication_events` | canvas twin: auditor_authentication_records | star-audit | read | src 1 · mig: 0246_authentication_events.sql · canvas app/models/auditors/active_record/authentication_record.rb sets self.table_name = "auditor_authentication_records" |
| `calendar_user_preferences` | canvas twin: user_preference_values | star-calendar | read+write | src 3 · mig: 0095_calendar_events_deep.sql · canvas get_preference(:enabled_account_calendars) (app/models/user.rb:4078-4081) |
| `checkpoints` | obsolete duplicate | — | — | none · mig: 0230_graphql_parity_completion.sql · canvas Checkpoint is a wrapper class (lib/checkpoint.rb:21); checkpoint data persists as SubAssignment rows in assignments (app/models/abstract_assignment.rb:23-24; db/migrate 20101210192618_init_canvas_db.rb:874 has_sub_assignments); star checkpoints (0230) has zero references |
| `context_settings` | star-specific | star-lti | read+write | src 3 · mig: 0011_context_settings.sql · no canvas table by this name (canvas gradebooks_controller.rb:1333-1339 context_settings is a local var merged into the gradebook_settings user preference); star uses it for per-context JSONB settings incl. top-nav favorites (crates/star-lti/src/external_tools.rs:1624-1634) |
| `conversation_message_removals` | canvas twin: conversation_message_participants | star-conversations | read+write | src 10 +2 test · mig: 0076_conversation_batches.sql · canvas marks deletion via workflow_state='deleted' on the participant row (app/models/conversation_message_participant.rb:34-44) |
| `course_events` | canvas twin: auditor_course_records | star-auth | read | src 1 · mig: 0250_course_events.sql · canvas app/models/auditors/active_record/course_record.rb sets self.table_name = "auditor_course_records" |
| `course_nicknames` | canvas twin: user_preference_values | star-users | read+write | src 7 +2 test · mig: 0235_course_nicknames.sql · canvas stores nicknames as preference key (app/models/user.rb:2040-2043); no CourseNickname model/table in canvas-src |
| `course_settings` | canvas twin: courses | star-courses, star-modules | read+write | src 3 +6 test · mig: 0055_course_settings.sql · canvas course settings are columns on courses; star row holds "Canvas course_settings_json keys (effective values)" (star-db 0055 comment); handlers read courses.settings + course_settings together (crates/star-modules/src/course_pacing.rs:468) |
| `course_tabs` | canvas twin: courses | star-collaborations, star-courses | read+write | src 5 +4 test · mig: 0055_course_settings.sql · canvas serializes tab_configuration on courses (app/models/course.rb:83,1707) |
| `dashboard_positions` | canvas twin: user_preference_values | star-dashboard, star-users | read+write | src 5 +1 test · mig: 0073_todo_views.sql · canvas get_preference(:dashboard_positions) (app/models/user.rb:2012-2013) |
| `discussion_entry_ratings` | canvas twin: discussion_entry_participants | star-discussions | read+write | src 3 +2 test · mig: 0041_discussion_reads.sql · canvas rating lives on entry participants (app/models/discussion_entry_participant.rb:52-53) |
| `discussion_entry_reads` | canvas twin: discussion_entry_participants | star-discussions | read+write | src 6 +4 test · mig: 0041_discussion_reads.sql · canvas workflow_state 'read' + forced_read_state (app/models/discussion_entry_participant.rb:43,47-48) |
| `discussion_entry_reports` | obsolete duplicate | — | — | none · mig: 0230_graphql_parity_completion.sql · canvas entry reports = report_type on discussion_entry_participants (app/models/discussion_entry.rb:594-595,659); star-db discussion_entry_participants is used by star-graphql; star discussion_entry_reports (0230) has zero references |
| `discussion_subscriptions` | canvas twin: discussion_topic_participants | star-discussions | read+write | src 3 +6 test · mig: 0069_discussion_subscriptions.sql · canvas subscribed column (db/migrate/20101210192618_init_canvas_db.rb:2272) |
| `discussion_topic_reads` | canvas twin: discussion_topic_participants | star-discussions | read+write | src 8 +6 test · mig: 0041_discussion_reads.sql · canvas read_state reads participants.workflow_state (app/models/discussion_topic.rb:632-642); StreamItem mirror at :661 |
| `document_annotation_reads` | star-specific | — | — | none · mig: 0122_rubric_annotation_read_states.sql · no canvas annotation read-state table (canvas annotation contexts = canvadocs_annotation_contexts); star-db has no document_annotations table either; unused stub (0122) |
| `external_tool_favorites` | canvas twin: accounts (settings) | star-lti | read+write | src 3 +1 test · mig: 0085_external_tools.sql · canvas RCE favorites = inheritable account setting rce_favorite_tool_ids (app/models/account.rb:410,2863-2864; app/models/context_external_tool.rb:522); favorites table is Course/Group only (app/models/favorite.rb:21,24) |
| `external_tools` | canvas twin: context_external_tools | star-content, star-features, star-graphql, star-lti | read+write | src 21 +10 test · mig: 0085_external_tools.sql · canvas app/models/context_external_tool.rb (table context_external_tools in canvas db/migrate) |
| `files` | canvas twin: attachments | star-content, star-courses, star-files, star-graphql, star-lti, star-media | read+write | src 48 +11 test · mig: 0005_files.sql · canvas app/models/attachment.rb (table attachments) |
| `grade_change_events` | canvas twin: auditor_grade_change_records | star-audit, star-graphql | read+write | src 9 +6 test · mig: 0092_grade_change_audit.sql · canvas app/models/auditors/active_record/grade_change_record.rb sets self.table_name = "auditor_grade_change_records" |
| `gradebook_user_filters` | canvas twin: gradebook_filters | star-graphql | write | src 1 · mig: 0230_graphql_parity_completion.sql · canvas app/models/gradebook_filter.rb (table gradebook_filters in canvas db/migrate); note both names are used by handlers — see §4 |
| `grading_scheme_entries` | canvas twin: grading_standards | star-grading-standards, star-graphql, star-ui | read+write | src 12 +4 test · mig: 0027_grading_standards.sql · canvas serializes the scheme into grading_standards.data (app/models/grading_standard.rb:31,57) |
| `group_files` | canvas twin: attachments | star-files, star-groups, star-media | read+write | src 10 +4 test · mig: 0064_group_invitations.sql, 0253_media_objects.sql · canvas app/models/attachment.rb (table attachments) |
| `group_invitations` | canvas twin: group_memberships | star-groups | write | src 1 +1 test · mig: 0064_group_invitations.sql · canvas invites = GroupMembership workflow_state 'invited' (app/models/group_membership.rb:126-131); star adds email invitations |
| `internal_settings` | canvas twin: settings | star-graphql | write | src 3 · mig: 0230_graphql_parity_completion.sql · canvas Setting key-value store (app/models/setting.rb:21, table settings) |
| `job_clusters` | star-specific | star-jobs | read | src 2 +3 test · mig: 0057_jobs_v2.sql · star jobs v2 infrastructure (0057_jobs_v2.sql, star-jobs); canvas delayed_jobs has no cluster concept |
| `jobs` | canvas twin: delayed_jobs | star-account-reports, star-jobs, star-outcomes, star-sis | read+write | src 41 +25 test · mig: 0033_jobs.sql · canvas writes delayed_jobs (table created in canvas db/migrate); star jobs v2 (0033/0057) replaces it |
| `learner_dashboard_tab_selections` | star-specific | star-graphql | write | src 1 · mig: 0230_graphql_parity_completion.sql · star-ui dashboard tab selection (0230, crates/star-graphql/src/mutations_batch_g3.rs:1009); no canvas counterpart |
| `learning_object_date_overrides` | star-specific | star-learning-dates | read+write | src 10 +3 test · mig: 0090_learning_object_dates.sql · canvas computes overrides from assignment_overrides (canvas learning_object_dates_controller.rb:23); star persists (0090_learning_object_dates.sql) |
| `learning_object_dates` | star-specific | star-learning-dates | read+write | src 10 +3 test · mig: 0090_learning_object_dates.sql · canvas computes these dates from assignments/quizzes/overrides (canvas learning_object_dates_controller.rb:23); star persists denormalized rows (0090_learning_object_dates.sql) |
| `learning_outcome_links` | canvas twin: content_tags | star-content, star-graphql | read+write | src 10 +2 test · mig: 0022_outcomes.sql · canvas link rows are ContentTags (app/models/learning_outcome_group.rb:43); no canvas table named learning_outcome_links |
| `lti_data_service_subscriptions` | star-specific | star-lti | read+write | src 10 +1 test · mig: 0212_lti_advantage_services.sql · LTI Advantage subscriptions; no canvas table/model found in canvas-src |
| `lti_deployments` | star-specific | star-lti | read+write | src 12 +4 test · mig: 0078_lti_registrations.sql, 0232_lti_registrations_page_views.sql · canvas LTI 1.3 has no deployment table (canvas registration model: app/models/lti/ims/registration.rb → lti_ims_registrations); star registry (0078/0232) |
| `lti_nonces` | star-specific | star-lti | read+write | src 2 +1 test · mig: 0238_lti_nonces.sql · LTI 1.3 replay protection; no canvas counterpart found in canvas-src |
| `master_course_subscriptions` | canvas twin: master_courses_child_subscriptions | star-courses | read+write | src 10 +4 test · mig: 0107_master_courses.sql · canvas course.rb:295 class_name "MasterCourses::ChildSubscription" (table master_courses_child_subscriptions) |
| `master_courses` | obsolete duplicate | — | — | none · mig: 0107_master_courses.sql · canvas master-course templates live in master_courses_master_templates (app/models/course.rb:1261); star represents them via master_templates (used by star-courses); star master_courses (0107) has zero references |
| `master_migrations` | canvas twin: master_courses_master_migrations | star-courses | read+write | src 10 +1 test · mig: 0107_master_courses.sql · canvas app/models/master_courses/master_migration.rb (table master_courses_master_migrations) |
| `master_templates` | canvas twin: master_courses_master_templates | star-courses | read | src 4 +2 test · mig: 0107_master_courses.sql · canvas course.rb:1261; app/models/master_courses/master_template.rb |
| `module_item_completions` | canvas twin: context_module_progressions | star-graphql, star-modules | write | src 4 +8 test · mig: 0039_module_completions.sql · canvas app/models/context_module_progression.rb (table context_module_progressions) |
| `module_items` | canvas twin: content_tags | star-graphql, star-modules | read+write | src 27 +22 test · mig: 0002_content_extras.sql · canvas module items are ContentTags (app/models/context_module.rb:36) |
| `modules` | canvas twin: context_modules | star-content, star-graphql, star-modules | read+write | src 30 +17 test · mig: 0001_core_schema.sql · canvas app/models/context_module.rb (table context_modules) |
| `notification_preferences` | canvas twin: notification_policies | star-graphql, star-notifications, star-users | read+write | src 10 +7 test · mig: 0045_communication_channels.sql · canvas serves notification_preferences from NotificationPolicy rows (app/controllers/notification_preferences_controller.rb:69-70) |
| `observer_enrollments` | canvas twin: user_observers | star-enrollments, star-quizzes, star-users | read+write | src 14 +2 test · mig: 0141_user_observees.sql · canvas app/models/user_observation_link.rb:26-29 (table user_observers, observer+student); ObserverEnrollment is STI on enrollments (app/models/observer_enrollment.rb:21) |
| `outcome_ab_migrations` | star-specific | star-outcomes | read+write | src 2 +1 test · mig: 0254_port_a_gaps.sql · star A/B outcome-migration bookkeeping (0254, crates/star-outcomes/src/outcome_gaps.rs:154); no canvas counterpart |
| `outcome_alignments` | canvas twin: content_tags | star-outcomes | read | src 2 +2 test · mig: 0044_outcome_groups.sql · canvas alignments are ContentTags (app/models/learning_outcome.rb:54) |
| `outcome_groups` | canvas twin: learning_outcome_groups | star-outcomes | read+write | src 51 +10 test · mig: 0044_outcome_groups.sql · canvas app/models/learning_outcome_group.rb (table learning_outcome_groups) |
| `outcome_links` | canvas twin: content_tags | star-outcomes | read+write | src 17 +3 test · mig: 0044_outcome_groups.sql · canvas child_outcome_links are ContentTags (app/models/learning_outcome_group.rb:43) |
| `page_revisions` | canvas twin: versions | star-pages | read+write | src 5 +3 test · mig: 0040_page_revisions.sql · canvas versions pages via simply_versioned → versions table (app/models/wiki_page.rb:306; app/models/version.rb) |
| `page_view_queries` | star-specific | star-search | read+write | src 2 +2 test · mig: 0232_lti_registrations_page_views.sql · star async page-view query jobs (0232, crates/star-search/src/page_views.rs:536); canvas serves page views directly from page_views |
| `pages` | canvas twin: wiki_pages | star-calendar, star-content, star-graphql, star-modules, star-pages, star-search | read+write | src 29 +22 test · mig: 0001_core_schema.sql · canvas app/models/wiki_page.rb (table wiki_pages) |
| `password_reset_tokens` | star-specific | star-ui | read+write | src 3 +5 test · mig: 0222_password_reset_tokens.sql · star forgot-password tokens (0222, star-ui); canvas reset flow uses pseudonym/communication-channel tokens; no canvas table found in canvas-src |
| `peer_reviews` | obsolete duplicate | — | — | none · mig: 0230_graphql_parity_completion.sql · canvas peer-review requests = assessment_requests (star-db 0104, used by star-assignments); newer canvas also models PeerReviewSubAssignment STI on assignments (app/models/peer_review_sub_assignment.rb); star peer_reviews (0230) has zero references |
| `planner_items` | canvas twin: planner_notes | star-courses | read | src 1 · mig: 0252_misc_gaps.sql · canvas app/models/planner_note.rb; star columns (user_id, course_id, title, details, todo_date, workflow_state) mirror PlannerNote |
| `plugins` | canvas twin: plugin_settings | star-courses | read+write | src 2 · mig: 0252_misc_gaps.sql · canvas app/models/plugin_setting.rb:21 PluginSetting < ApplicationRecord (table plugin_settings) |
| `poll_choices` | canvas twin: polling_poll_choices | star-polling | read+write | src 7 +1 test · mig: 0100_polling.sql · canvas app/models/polling/poll_choice.rb (table_name polling_poll_choices) |
| `poll_sessions` | canvas twin: polling_poll_sessions | star-polling | read+write | src 9 · mig: 0100_polling.sql · canvas app/models/polling/poll_session.rb |
| `poll_submissions` | canvas twin: polling_poll_submissions | star-polling | read+write | src 4 +1 test · mig: 0100_polling.sql · canvas app/models/polling/poll_submission.rb |
| `polls` | canvas twin: polling_polls | star-polling | read+write | src 9 +2 test · mig: 0100_polling.sql · canvas app/models/polling/poll.rb (table polling_polls) |
| `progress` | canvas twin: progresses | star-content, star-gradebook, star-graphql, star-groups, star-jobs, star-modules, star-sis | read+write | src 11 +9 test · mig: 0127_progress.sql · canvas app/models/progress.rb (conventional table progresses, created in canvas db/migrate) |
| `provisional_grades` | canvas twin: moderated_grading_provisional_grades | star-assignments, star-graphql | read+write | src 13 +20 test · mig: 0108_moderated_grading.sql · canvas app/models/moderated_grading/provisional_grade.rb |
| `question_banks` | canvas twin: assessment_question_banks | star-content, star-quizzes | read | src 2 · mig: 0037_quiz_questions.sql · canvas app/models/assessment_question_bank.rb |
| `quiz_extensions` | canvas twin: quiz_submissions | star-quizzes | write | src 1 · mig: 0251_quiz_family.sql · canvas Quizzes::QuizExtension is a wrapper that saves extra_attempts/extra_time on quiz_submissions (app/models/quizzes/quiz_extension.rb:20-42); no canvas table |
| `quiz_ip_filters` | canvas twin: quizzes (ip_filter) | star-quizzes | read | src 1 · mig: 0251_quiz_family.sql · canvas stores the IP filter as a string column on quizzes (app/models/quizzes/quiz.rb:122,157,265); star normalizes to rows |
| `quiz_question_groups` | canvas twin: quiz_groups | star-quizzes | read+write | src 9 +1 test · mig: 0037_quiz_questions.sql · canvas app/models/quizzes/quiz_group.rb (table_name quiz_groups) |
| `release_notes` | star-specific | star-accounts, star-dashboard | read+write | src 9 +5 test · mig: 0097_favorites_release_notes.sql · canvas stores release notes in DynamoDB (app/models/release_note.rb:43,163 ddb_table_name), not a SQL table; star persists publish/unpublish in Postgres (0097) |
| `rubric_assessment_reads` | star-specific | star-graphql | write | src 1 · mig: 0122_rubric_annotation_read_states.sql · no canvas read-state storage for rubric assessments; star graphql mutation writes it (crates/star-graphql/src/mutations_batch_g2.rs:1350) |
| `rubric_ratings` | canvas twin: rubric_assessments | star-graphql, star-rubrics | read+write | src 5 +5 test · mig: 0023_rubrics.sql · canvas serializes ratings into rubric_assessments.data (app/models/rubric_assessment.rb:38) |
| `score_overrides` | canvas twin: scores | star-gradebook | write | src 1 · mig: 0249_score_overrides.sql · canvas override_score lives on scores (app/models/enrollment.rb:1221-1228) |
| `sis_import_errors` | canvas twin: sis_batch_errors | star-sis | read | src 2 +2 test · mig: 0242_sis_import_errors.sql · canvas app/models/sis_batch_error.rb (table sis_batch_errors) |
| `smart_search_indexes` | star-specific | star-search | read+write | src 2 +3 test · mig: 0131_smart_search.sql · star indexing progress (0131, crates/star-search/src/lib.rs:986); canvas smart search stores pgvector embeddings in per-model *_embeddings tables (canvas lib/smart_search.rb:130-134) — the 3 embedding tables star lacks |
| `speed_grader_settings` | canvas twin: user_preference_values | star-graphql | write | src 1 · mig: 0230_graphql_parity_completion.sql · canvas keeps SpeedGrader settings as a user preference (app/models/user.rb:1954) |
| `sub_assignments` | obsolete duplicate | — | — | none · mig: 0230_graphql_parity_completion.sql · canvas SubAssignment is STI on assignments (app/models/sub_assignment.rb:21; app/models/abstract_assignment.rb:23-24); star sub_assignments (0230) has zero references |
| `submission_comment_attachments` | canvas twin: attachments | star-submission-comments | read+write | src 2 +3 test · mig: 0077_comment_attachments.sql · canvas wires comment attachments through attachments (app/models/submission_comment.rb:416-421, cached_attachments :85) |
| `submission_comment_reads` | star-specific | star-graphql | write | src 1 · mig: 0054_submission_comment_reads.sql · no canvas per-comment read table found in canvas-src; star graphql mutation writes it (crates/star-graphql/src/mutations_batch_g2.rs:927) |
| `submission_files` | canvas twin: attachments | star-assignments | read+write | src 2 · mig: 0060_submissions_api.sql · canvas submission files are attachments (app/models/submission.rb:1111-1112 attachment_ids) |
| `submission_histories` | canvas twin: versions | — | — | none · mig: 0230_graphql_parity_completion.sql · canvas versions submissions via simply_versioned → versions (app/models/submission.rb:563); star versions table (0225) is also unreferenced |
| `submission_reads` | canvas twin: content_participations | star-assignments | read+write | src 8 +9 test · mig: 0060_submissions_api.sql · canvas tracks per-user submission read state via content_participations (app/models/content_participation.rb:21,27 polymorphic [:submission]) |
| `terms` | canvas twin: enrollment_terms | star-courses, star-graphql, star-modules | read+write | src 8 +1 test · mig: 0034_terms_sections.sql · canvas app/models/enrollment_term.rb (table enrollment_terms) |
| `todo_ignores` | star-specific | star-users | read+write | src 6 · mig: 0073_todo_views.sql · star todo-list ignore rows (0073, crates/star-users/src/lib.rs:1732); canvas has no such persistence (planner dismissals live in planner_overrides) |
| `tool_settings` | canvas twin: lti_tool_settings | star-lti | read+write | src 7 +6 test · mig: 0221_tool_settings.sql · canvas app/models/lti/tool_setting.rb:22 ToolSetting < ApplicationRecord (namespaced → lti_tool_settings) |
| `user_colors` | star-specific | star-users | read+write | src 5 · mig: 0082_user_account_mgmt.sql · star per-user context colors (0082, crates/star-users/src/lib.rs:801); no canvas counterpart found in canvas-src |
| `user_files` | canvas twin: attachments | star-files, star-media, star-users | read+write | src 6 +5 test · mig: 0056_user_settings.sql, 0253_media_objects.sql · canvas app/models/attachment.rb (table attachments) |
| `user_inbox_labels` | canvas twin: user_preference_values | star-graphql | write | src 2 · mig: 0230_graphql_parity_completion.sql · canvas inbox labels = user preference (app/models/user.rb:4085); conversation participant label is 'starred' only (app/models/conversation_participant.rb:201) |
| `user_merges` | star-specific | star-users | read+write | src 3 +2 test · mig: 0123_user_preferences_and_merges.sql · canvas merge is an operation over existing rows (canvas lib/user_merge.rb) with no persisted merge table in canvas-src; star records merge history (0123) |
| `user_preferences` | canvas twin: user_preference_values | star-accounts, star-assignments, star-gradebook, star-graphql, star-ui, star-users | read+write | src 15 +2 test · mig: 0123_user_preferences_and_merges.sql, 0248_gradebook_settings_what_if.sql · canvas app/models/user_preference_value.rb:26 UserPreferenceValue < ApplicationRecord (canvas also keeps users.preferences) |
| `user_settings` | canvas twin: user_preference_values | star-users | read+write | src 6 +1 test · mig: 0056_user_settings.sql · canvas stores these as user preferences (app/models/user.rb:2179-2184) |
| `widget_dashboards` | star-specific | star-graphql | write | src 2 · mig: 0230_graphql_parity_completion.sql · star-ui widget layout (0230, crates/star-graphql/src/mutations_batch_g3.rs:1054); no canvas counterpart |

## 4. Double representations — both names present in star-db

Star-db imported the canvas-named twins (mostly `0223/0224/0225_canvas_schema_tables_*.sql`). Counts are
**src** (handler) references across `migration/crates/*/src`. "Handlers use" is decided by those counts, per
the audit's §6 caveat that this shows reach, not behavioral parity.

| entity | canvas-named table(s) in star-db — src refs | star-named table(s) — src refs | handlers use |
|---|---|---|---|
| attachments | `attachments` 9 (star-account-reports, star-learning-dates, star-media, star-outcomes, star-sis) | `files` 48 (star-content, star-courses, star-files, star-graphql, star-lti, star-media); `user_files` 6 (star-files, star-media, star-users); `group_files` 10 (star-files, star-groups, star-media); `submission_files` 2 (star-assignments); `submission_comment_attachments` 2 (star-submission-comments) | BOTH sides used |
| wiki pages + revisions | `wiki_pages` 0; `versions` 0 | `pages` 29 (star-calendar, star-content, star-graphql, star-modules, star-pages, star-search); `page_revisions` 5 (star-pages) | star-named side |
| modules | `context_modules` 6 (star-assignments, star-learning-dates, star-lti) | `modules` 30 (star-content, star-graphql, star-modules) | BOTH sides used |
| module items | `content_tags` 5 (star-lti, star-outcomes) | `module_items` 27 (star-graphql, star-modules) | BOTH sides used |
| module progression | `context_module_progressions` 0 | `module_item_completions` 4 (star-graphql, star-modules) | star-named side |
| terms | `enrollment_terms` 12 (star-account-reports, star-courses, star-sis) | `terms` 8 (star-courses, star-graphql, star-modules) | BOTH sides used |
| external tools | `context_external_tools` 4 (star-lti) | `external_tools` 21 (star-content, star-features, star-graphql, star-lti) | BOTH sides used |
| announcements | `discussion_topics` 64 (star-calendar, star-content, star-discussions, star-graphql, star-search) | `announcements` 20 (star-announcements, star-calendar, star-content, star-dashboard, star-search) | BOTH sides used |
| topic reads/subscriptions | `discussion_topic_participants` 3 (star-graphql) | `discussion_topic_reads` 8 (star-discussions); `announcement_reads` 2 (star-announcements); `discussion_subscriptions` 3 (star-discussions) | BOTH sides used |
| entry reads/ratings/reports | `discussion_entry_participants` 2 (star-graphql) | `discussion_entry_reads` 6 (star-discussions); `discussion_entry_ratings` 3 (star-discussions); `discussion_entry_reports` 0 | BOTH sides used |
| message removals | `conversation_message_participants` 0 | `conversation_message_removals` 10 (star-conversations) | star-named side |
| jobs | `delayed_jobs` 0 | `jobs` 41 (star-account-reports, star-jobs, star-outcomes, star-sis) | star-named side |
| planner | `planner_notes` 7 (star-calendar); `planner_overrides` 5 (star-calendar) | `planner_items` 1 (star-courses) | BOTH sides used |
| outcome groups | `learning_outcome_groups` 13 (star-content, star-graphql) | `outcome_groups` 51 (star-outcomes) | BOTH sides used |
| outcome links/alignments | `content_tags` 5 (star-lti, star-outcomes) | `outcome_links` 17 (star-outcomes); `learning_outcome_links` 10 (star-content, star-graphql); `outcome_alignments` 2 (star-outcomes) | BOTH sides used |
| sis errors | `sis_batch_errors` 0 | `sis_import_errors` 2 (star-sis) | star-named side |
| grading schemes | `grading_standards` 20 (star-grading-standards, star-graphql, star-ui) | `grading_scheme_entries` 12 (star-grading-standards, star-graphql, star-ui) | BOTH sides used |
| rubric ratings | `rubric_assessments` 14 (star-assignments, star-graphql, star-rubrics) | `rubric_ratings` 5 (star-graphql, star-rubrics) | BOTH sides used |
| question banks | `assessment_question_banks` 2 (star-quizzes) | `question_banks` 2 (star-content, star-quizzes) | BOTH sides used |
| quiz groups | `quiz_groups` 0 | `quiz_question_groups` 9 (star-quizzes) | star-named side |
| polling | `polling_polls` 0; `polling_poll_choices` 0; `polling_poll_sessions` 0; `polling_poll_submissions` 0 | `polls` 9 (star-polling); `poll_choices` 7 (star-polling); `poll_sessions` 9 (star-polling); `poll_submissions` 4 (star-polling) | star-named side |
| provisional grades | `moderated_grading_provisional_grades` 0 | `provisional_grades` 13 (star-assignments, star-graphql) | star-named side |
| gradebook filters | `gradebook_filters` 5 (star-courses) | `gradebook_user_filters` 1 (star-graphql) | BOTH sides used |
| master courses | `master_courses_master_templates` 0; `master_courses_master_migrations` 0; `master_courses_child_subscriptions` 0 | `master_templates` 4 (star-courses); `master_migrations` 10 (star-courses); `master_course_subscriptions` 10 (star-courses); `master_courses` 0 | star-named side |
| user preferences | `user_preference_values` 0 | `user_preferences` 15 (star-accounts, star-assignments, star-gradebook, star-graphql, star-ui, star-users); `user_settings` 6 (star-users); `dashboard_positions` 5 (star-dashboard, star-users); `course_nicknames` 7 (star-users); `speed_grader_settings` 1 (star-graphql); `calendar_user_preferences` 3 (star-calendar); `user_inbox_labels` 2 (star-graphql) | star-named side |
| settings | `settings` 0 | `internal_settings` 3 (star-graphql) | star-named side |
| plugin settings | `plugin_settings` 1 (star-conferences) | `plugins` 2 (star-courses) | BOTH sides used |
| terms of service | `terms_of_services` 0 | `account_terms_of_service` 2 (star-accounts) | star-named side |
| notification prefs | `notification_policies` 0 | `notification_preferences` 10 (star-graphql, star-notifications, star-users) | star-named side |
| lti tool settings | `lti_tool_settings` 0 | `tool_settings` 7 (star-lti) | star-named side |
| auditors | `auditor_authentication_records` 0; `auditor_grade_change_records` 0; `auditor_course_records` 0 | `authentication_events` 1 (star-audit); `grade_change_events` 9 (star-audit, star-graphql); `course_events` 1 (star-auth) | star-named side |
| progress | `progresses` 7 (star-auth, star-courses, star-lti) | `progress` 11 (star-content, star-gradebook, star-graphql, star-groups, star-jobs, star-modules, star-sis) | BOTH sides used |
| observers | `user_observers` 0 | `observer_enrollments` 14 (star-enrollments, star-quizzes, star-users) | star-named side |
| score overrides | `scores` 0 | `score_overrides` 1 (star-gradebook) | star-named side |
| submission reads | `content_participations` 0 | `submission_reads` 8 (star-assignments) | star-named side |
| peer reviews | `assessment_requests` 11 (star-assignments) | `peer_reviews` 0 | canvas-named side |
| moderation events | `anonymous_or_moderation_events` 2 (star-assignments) | `audit_events` 0 | canvas-named side |
| sub-assignments | `assignments` 147 (star-announcements, star-assignment-groups, star-assignments, star-audit, star-auth, star-calendar, star-content, star-courses, star-dashboard, star-gradebook, star-graphql, star-learning-dates, star-lti, star-modules, star-outcomes, star-rubrics, star-search, star-submission-comments, star-ui, star-users) | `sub_assignments` 0; `checkpoints` 0 | canvas-named side |

Entities where **both** sides carry handler traffic (a one-sided MIG-16 mapping would strand data):

- attachments
- modules
- module items
- terms
- external tools
- announcements
- topic reads/subscriptions
- entry reads/ratings/reports
- planner
- outcome groups
- outcome links/alignments
- grading schemes
- rubric ratings
- question banks
- gradebook filters
- plugin settings
- progress

## 5. What this means for MIG-16 (data mapping)

- **Default direction: canvas → star-named tables.** For most twins the canvas-schema import tables
  (`0223/0224/0225`) are unreferenced scaffolding (0 src uses: `wiki_pages`, `versions`,
  `context_module_progressions`, `delayed_jobs`, `sis_batch_errors`, `quiz_groups`, all four `polling_polls*`
  tables, `moderated_grading_provisional_grades`, the three `master_courses_*` tables,
  `user_preference_values`, `settings`, `terms_of_services`, `notification_policies`, `lti_tool_settings`,
  the three `auditor_*_records`, `user_observers`, `scores`, `content_participations`,
  `conversation_message_participants`). Whether to drop them or keep them as import staging is a
  coordinator decision — this doc only records that no handler reads them.
- **Both-sides-used entities need per-entity reconciliation** (list above): e.g. `progresses` (7 src) vs
  `progress` (11 src), `enrollment_terms` (12) vs `terms` (8), `context_modules` (6) vs `modules` (30),
  `attachments` (9) vs the `files`/`user_files`/`group_files` family (64), `discussion_topics` (64) vs
  `announcements` (20), `grading_standards` (20) feeding the decomposed `grading_scheme_entries` (12),
  `rubric_assessments` (14) feeding `rubric_ratings` (5), `gradebook_filters` (5) vs
  `gradebook_user_filters` (1).
- **Reverse-direction cases**: `assessment_requests` (11 src), `anonymous_or_moderation_events` (2 src),
  `discussion_entry_participants` (2 src) are the used representations; their star-named shadows
  (`peer_reviews`, `audit_events`, `discussion_entry_reports`) are dead — classified *obsolete duplicate*.
- **Canvas-side storage that is not a Postgres table**: release notes (DynamoDB), RCE/top-nav tool favorites
  (account settings), quiz IP filters and quiz extensions (`quizzes`/`quiz_submissions` columns + wrapper
  classes), course nicknames / dashboard positions / inbox labels / calendar prefs / user settings
  (`user_preference_values` keys), learning-object dates (computed). MIG-16 must map from those sources,
  not from tables that do not exist.
- **aua_logs**: star-db has the 7 canvas partitions but no base `aua_logs` table and no handler references;
  canvas itself truncates/cycles the partitions (`asset_user_access_log.rb:222`).

## 6. Limits of this mapping

- Name-level parity only; column/type parity was not re-audited (see parity-audit §6).
- Ownership is grep-classified (SQL-keyword adjacency over `.rs` files); SQL comments inside query strings can
  in principle add false hits — each owner crate above was sanity-checked against its sample lines.
- No local database and no cargo runs (CI-only per repo rules): nothing here was validated by executing
  handlers; canvas line references were read from the pinned source, not run.
- canvas-src is pinned (1c9f0bb) with 95 migration files; a few canvas tables known from upstream history
  (e.g. `peer_reviews`) have no `create_table` in this set and no AR model in this tree — such names are
  classified on the tree's own evidence, with the search noted in the row.
