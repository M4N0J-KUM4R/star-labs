# Authored-reference validation report (generated, do not hand-edit)

Command: `python3 tools/schema/canvas_reference.py --canvas-dir <canvas-src> \

  --out parity/canvas_schema_reference.json \

  --validate-authored tools/parity/canvas_authored_tables_reference.json`


## Verdict

`--validate-authored` reports **251 mismatches** (232 columns missing from the generated reference, 19 type disagreements) across 51 of the 34 authored tables. Per the task brief the reference is **not** adjusted to match: the generated reference is a faithful replay of the pinned `canvas-src/db/migrate` (instructure/canvas-lms@1c9f0bb), and every mismatch was verified to be absent from that source, not a parser drop.

Evidence for the provenance finding:

1. A programmatic check over all 95 migrate files found **0** of the authored-only columns (`add_column :table, :col` or a `t.<type> :col` inside the table's create_table block) — they simply do not exist in the pinned db/migrate (e.g. `accounts.enable_profiles`, `accounts.ip_whitelist`, `accounts.mfa_required` are pre-2026 Canvas columns that the 2026 init_canvas_db.rb squash no longer declares).
2. The type disagreements run both ways and include *newer* state the authored reference predates: `submissions.points_deducted` is `numeric(12,2)` here because migration `20260128083741_increase_points_deducted_precision.rb` runs after init (authored reference still says `float8`).
3. The 2026-11-25 workspace audit already records that the authored 34-table reference is the older, hand-maintained artifact (`tools/parity/authored_column_backlog.py` header: 'master 2026-04').

NOT-NULL semantics: the authored reference carries only `column -> PG type` strings and no nullability, so the comparison covers column existence and the Rails->PG type map; nullability of the generated reference is explicit (`not_null` boolean) and could not contradict the authored artifact.


## Per-table counts

| table | mismatches |
|---|---:|
| table accounts | 14 |
| table accounts.allowed_services | 1 |
| table assignment_overrides | 2 |
| table assignments | 33 |
| table assignments.allowed_extensions | 1 |
| table assignments.submission_types | 1 |
| table attachments | 8 |
| table attachments.filename | 1 |
| table attachments.size | 1 |
| table calendar_events | 4 |
| table calendar_events.location_name | 1 |
| table context_external_tools | 5 |
| table context_external_tools.url | 1 |
| table conversation_messages | 2 |
| table conversation_messages.root_account_ids | 1 |
| table conversation_participants | 3 |
| table conversation_participants.root_account_ids | 1 |
| table conversations | 5 |
| table conversations.root_account_ids | 1 |
| table course_sections | 4 |
| table courses | 21 |
| table courses.storage_quota | 1 |
| table discussion_entries | 7 |
| table discussion_topics | 10 |
| table enrollments | 13 |
| table files | 1 |
| table folders | 8 |
| table group_categories | 6 |
| table group_memberships | 6 |
| table groups | 7 |
| table groups.storage_quota | 1 |
| table learning_outcomes | 2 |
| table module_items | 1 |
| table modules | 1 |
| table outcome_groups | 1 |
| table progress | 1 |
| table pseudonyms | 9 |
| table quiz_questions | 4 |
| table quizzes | 6 |
| table rubric_associations | 4 |
| table rubrics | 1 |
| table rubrics.description | 1 |
| table submissions | 7 |
| table submissions.anonymous_id | 1 |
| table submissions.points_deducted | 1 |
| table submissions.url | 1 |
| table users | 23 |
| table users.locale | 1 |
| table users.lti_id | 1 |
| table wiki_pages | 13 |
| table wiki_pages.url | 1 |

## Column-level detail

- table accounts.allowed_services: authored text != generated varchar(255) (rails string, not_null=False)
- table accounts: column enable_course_catalog (boolean) missing from generated reference
- table accounts: column enable_profiles (boolean) missing from generated reference
- table accounts: column equella_enabled (boolean) missing from generated reference
- table accounts: column global_repository (boolean) missing from generated reference
- table accounts: column ip_whitelist (text) missing from generated reference
- table accounts: column ltr_launch_settings (text) missing from generated reference
- table accounts: column mfa_required (boolean) missing from generated reference
- table accounts: column push_subscription_enabled (boolean) missing from generated reference
- table accounts: column service_notification_id (bigint) missing from generated reference
- table accounts: column show_scheduler (boolean) missing from generated reference
- table accounts: column sis_default_grade_export (boolean) missing from generated reference
- table accounts: column sis_name (varchar(255)) missing from generated reference
- table accounts: column sync_enrollment_waits (boolean) missing from generated reference
- table accounts: column turnitin_shared_secret (varchar(255)) missing from generated reference
- table assignment_overrides: column current_version (integer) missing from generated reference
- table assignment_overrides: column group_id (bigint) missing from generated reference
- table assignments.allowed_extensions: authored text != generated varchar(255) (rails string, not_null=False)
- table assignments: column anonymize_students (boolean) missing from generated reference
- table assignments: column anonymous_id (varchar(255)) missing from generated reference
- table assignments: column applied_assignment_groups_id (bigint) missing from generated reference
- table assignments: column attempt_count (integer) missing from generated reference
- table assignments: column autograde_at (timestamp) missing from generated reference
- table assignments: column automatic_peer_review_count (integer) missing from generated reference
- table assignments: column automatic_peer_reviews_assigned (boolean) missing from generated reference
- table assignments: column course_id (bigint) missing from generated reference
- table assignments: column duplicate_notification_id (bigint) missing from generated reference
- table assignments: column eula_url (text) missing from generated reference
- table assignments: column excused_assignments_count (integer) missing from generated reference
- table assignments: column external_tool_id (bigint) missing from generated reference
- table assignments: column free_form_criterion_comments (boolean) missing from generated reference
- table assignments: column grading_period_id (bigint) missing from generated reference
- table assignments: column has_submitted_submissions (boolean) missing from generated reference
- table assignments: column ip_filter (text) missing from generated reference
- table assignments: column last_updated_by (bigint) missing from generated reference
- table assignments: column locked_for_user (boolean) missing from generated reference
- table assignments: column lti_advantage_line_item_id (bigint) missing from generated reference
- table assignments: column max_name_length (integer) missing from generated reference
- table assignments: column post_manually (boolean) missing from generated reference
- table assignments: column posted_at (timestamp) missing from generated reference
- table assignments: column presentation_tool_id (bigint) missing from generated reference
- table assignments: column published (boolean) missing from generated reference
- table assignments: column published_at (timestamp) missing from generated reference
- table assignments: column quiz_id (bigint) missing from generated reference
- table assignments: column remove_overridden_grades (boolean) missing from generated reference
- table assignments: column rubric_hidden (boolean) missing from generated reference
- table assignments: column saved_rubrics (boolean) missing from generated reference
- table assignments: column second_attempt_date (timestamp) missing from generated reference
- table assignments: column secure_params (text) missing from generated reference
- table assignments.submission_types: authored text != generated varchar(255) (rails string, not_null=False)
- table assignments: column submitted_count (integer) missing from generated reference
- table assignments: column visible_to_everybody (boolean) missing from generated reference
- table attachments: column attachment_id (bigint) missing from generated reference
- table attachments: column exclude_from_ffi (boolean) missing from generated reference
- table attachments.filename: authored varchar(255) != generated text (rails text, not_null=False)
- table attachments: column hidden (boolean) missing from generated reference
- table attachments: column hidden_unlocked (boolean) missing from generated reference
- table attachments: column last_lock_at (timestamp) missing from generated reference
- table attachments: column last_unlock_at (timestamp) missing from generated reference
- table attachments.size: authored integer != generated bigint (rails bigint, not_null=False)
- table attachments: column uploaded_data (text) missing from generated reference
- table attachments: column visibility (varchar(255)) missing from generated reference
- table calendar_events: column assignment_id (bigint) missing from generated reference
- table calendar_events: column duplicate_with_children_details (text) missing from generated reference
- table calendar_events: column imported_from_ics (boolean) missing from generated reference
- table calendar_events.location_name: authored varchar(255) != generated text (rails text, not_null=False)
- table calendar_events: column original_date (date) missing from generated reference
- table context_external_tools: column custom_fields (text) missing from generated reference
- table context_external_tools: column privacy_level (varchar(255)) missing from generated reference
- table context_external_tools: column text (text) missing from generated reference
- table context_external_tools.url: authored varchar(255) != generated varchar(4096) (rails string, not_null=False)
- table context_external_tools: column use_1_3 (boolean) missing from generated reference
- table context_external_tools: column vendor_code (varchar(255)) missing from generated reference
- table conversation_messages: column monologue (boolean) missing from generated reference
- table conversation_messages.root_account_ids: authored bigint != generated text (rails text, not_null=False)
- table conversation_messages: column subject (varchar(255)) missing from generated reference
- table conversation_participants: column last_authored_message_at (timestamp) missing from generated reference
- table conversation_participants: column last_message_author_id (bigint) missing from generated reference
- table conversation_participants: column last_message_body (varchar(255)) missing from generated reference
- table conversation_participants.root_account_ids: authored bigint != generated text (rails text, not_null=False)
- table conversations: column last_author (varchar(255)) missing from generated reference
- table conversations: column last_message (varchar(255)) missing from generated reference
- table conversations: column last_message_at (timestamp) missing from generated reference
- table conversations: column message_count (integer) missing from generated reference
- table conversations.root_account_ids: authored bigint != generated text (rails text, not_null=False)
- table conversations: column workflow_state (varchar(255)) missing from generated reference
- table course_sections: column enrollment_template_id (bigint) missing from generated reference
- table course_sections: column friendly_name (varchar(255)) missing from generated reference
- table course_sections: column position (integer) missing from generated reference
- table course_sections: column sis_commit_hash (varchar(255)) missing from generated reference
- table courses: column apply_assignment_group_weights (boolean) missing from generated reference
- table courses: column blueprint (boolean) missing from generated reference
- table courses: column calendar_event_id (bigint) missing from generated reference
- table courses: column concluded_at (timestamp) missing from generated reference
- table courses: column end_at (timestamp) missing from generated reference
- table courses: column final_grades_published_at (timestamp) missing from generated reference
- table courses: column hide_distribution_graphs (boolean) missing from generated reference
- table courses: column hide_final_grades (boolean) missing from generated reference
- table courses: column image_id (bigint) missing from generated reference
- table courses: column is_public_to_auth_users (boolean) missing from generated reference
- table courses: column licensing (varchar(255)) missing from generated reference
- table courses: column locked (boolean) missing from generated reference
- table courses: column migration_ids (text) missing from generated reference
- table courses: column organizational (boolean) missing from generated reference
- table courses: column public_syllabus (boolean) missing from generated reference
- table courses: column public_syllabus_to_auth (boolean) missing from generated reference
- table courses: column restrict_student_future_view (boolean) missing from generated reference
- table courses: column restrict_student_past_view (boolean) missing from generated reference
- table courses: column show_total_scores_as_points (boolean) missing from generated reference
- table courses.storage_quota: authored integer != generated bigint (rails bigint, not_null=False)
- table courses: column tab_positions (text) missing from generated reference
- table courses: column turnitin_assignment_id (varchar(255)) missing from generated reference
- table discussion_entries: column duplicate_entry_id (bigint) missing from generated reference
- table discussion_entries: column flavor_flag (varchar(255)) missing from generated reference
- table discussion_entries: column generation (integer) missing from generated reference
- table discussion_entries: column not_replied_for_deleted (boolean) missing from generated reference
- table discussion_entries: column published (boolean) missing from generated reference
- table discussion_entries: column total_children (integer) missing from generated reference
- table discussion_entries: column total_reply_count (integer) missing from generated reference
- table discussion_topics: column auto_locking (boolean) missing from generated reference
- table discussion_topics: column last_reply_at_previous (timestamp) missing from generated reference
- table discussion_topics: column last_reply_user_id (bigint) missing from generated reference
- table discussion_topics: column remove_previous_posts (boolean) missing from generated reference
- table discussion_topics: column rss_pseudo_guid (varchar(255)) missing from generated reference
- table discussion_topics: column sequential_replies (boolean) missing from generated reference
- table discussion_topics: column sticky (boolean) missing from generated reference
- table discussion_topics: column subscribed (boolean) missing from generated reference
- table discussion_topics: column user_name (varchar(255)) missing from generated reference
- table discussion_topics: column without_entries (boolean) missing from generated reference
- table enrollments: column assigned_quiz_lti_ids (text) missing from generated reference
- table enrollments: column locked (boolean) missing from generated reference
- table enrollments: column no_submission_feedback_check (boolean) missing from generated reference
- table enrollments: column no_submission_meta_check (boolean) missing from generated reference
- table enrollments: column notified_of_dashboard_change (boolean) missing from generated reference
- table enrollments: column observed_user_id (bigint) missing from generated reference
- table enrollments: column path_string (text) missing from generated reference
- table enrollments: column privilege_update_notice_sent (varchar(255)) missing from generated reference
- table enrollments: column section_sticky (boolean) missing from generated reference
- table enrollments: column sis_commit_hash (varchar(255)) missing from generated reference
- table enrollments: column sis_source_id (varchar(255)) missing from generated reference
- table enrollments: column time_zone (varchar(255)) missing from generated reference
- table enrollments: column totally_locked (boolean) missing from generated reference
- table files: missing from generated reference
- table folders: column hidden (boolean) missing from generated reference
- table folders: column hidden_unlocked (boolean) missing from generated reference
- table folders: column invisible (boolean) missing from generated reference
- table folders: column last_locked_at (timestamp) missing from generated reference
- table folders: column locked_by (bigint) missing from generated reference
- table folders: column storage_files_count (integer) missing from generated reference
- table folders: column submission_bytes_downloaded (integer) missing from generated reference
- table folders: column submission_bytes_quota (integer) missing from generated reference
- table group_categories: column account_id (bigint) missing from generated reference
- table group_categories: column delete_after (timestamp) missing from generated reference
- table group_categories: column enable_self_signup (boolean) missing from generated reference
- table group_categories: column join_limit (integer) missing from generated reference
- table group_categories: column split_group_count (integer) missing from generated reference
- table group_categories: column split_groups (boolean) missing from generated reference
- table group_memberships: column associated_user_id (bigint) missing from generated reference
- table group_memberships: column import_id (bigint) missing from generated reference
- table group_memberships: column sis_commit_hash (varchar(255)) missing from generated reference
- table group_memberships: column sis_pseudonym_id (bigint) missing from generated reference
- table group_memberships: column sis_source_id (varchar(255)) missing from generated reference
- table group_memberships: column stuck_sis_fields (text) missing from generated reference
- table groups: column attachments_count (integer) missing from generated reference
- table groups: column category (varchar(255)) missing from generated reference
- table groups: column invite_provided (boolean) missing from generated reference
- table groups: column sIS_import_batch_id (bigint) missing from generated reference
- table groups.storage_quota: authored integer != generated bigint (rails bigint, not_null=False)
- table groups: column tab_positions (text) missing from generated reference
- table groups: column time_zone (varchar(255)) missing from generated reference
- table groups: column wiki_page_counts (json) missing from generated reference
- table learning_outcomes: column ratings (text) missing from generated reference
- table learning_outcomes: column root_account_id (bigint) missing from generated reference
- table module_items: missing from generated reference
- table modules: missing from generated reference
- table outcome_groups: missing from generated reference
- table progress: missing from generated reference
- table pseudonyms: column certificate_b64 (text) missing from generated reference
- table pseudonyms: column external_user_guid (varchar(255)) missing from generated reference
- table pseudonyms: column last_sis_assignment_id (bigint) missing from generated reference
- table pseudonyms: column login_hint (text) missing from generated reference
- table pseudonyms: column persistent_session_id (varchar(255)) missing from generated reference
- table pseudonyms: column root_account_id (bigint) missing from generated reference
- table pseudonyms: column sis_source_id (varchar(255)) missing from generated reference
- table pseudonyms: column timed_out_at (timestamp) missing from generated reference
- table pseudonyms: column user_can_edit_login (boolean) missing from generated reference
- table quiz_questions: column answers (text) missing from generated reference
- table quiz_questions: column published_at (timestamp) missing from generated reference
- table quiz_questions: column regrade_option (varchar(255)) missing from generated reference
- table quiz_questions: column regraded (boolean) missing from generated reference
- table quizzes: column assignment_version (integer) missing from generated reference
- table quizzes: column last_edited_by_id (bigint) missing from generated reference
- table quizzes: column published (boolean) missing from generated reference
- table quizzes: column shuffle_questions (boolean) missing from generated reference
- table quizzes: column version_number (integer) missing from generated reference
- table quizzes: column visible (boolean) missing from generated reference
- table rubric_associations: column association_object_id (bigint) missing from generated reference
- table rubric_associations: column current_points (float8) missing from generated reference
- table rubric_associations: column current_score (float8) missing from generated reference
- table rubric_associations: column last_updated_at (timestamp) missing from generated reference
- table rubrics.description: authored varchar(255) != generated text (rails text, not_null=False)
- table rubrics: column hide_outcome_results (boolean) missing from generated reference
- table submissions.anonymous_id: authored varchar(255) != generated varchar(5) (rails string, not_null=False)
- table submissions: column deleted (boolean) missing from generated reference
- table submissions: column detached (boolean) missing from generated reference
- table submissions: column late (boolean) missing from generated reference
- table submissions.points_deducted: authored float8 != generated numeric(12,2) (rails decimal, not_null=False)
- table submissions: column scorekeeper_id (bigint) missing from generated reference
- table submissions: column second_attempt_time (timestamp) missing from generated reference
- table submissions: column second_meets_attempted (boolean) missing from generated reference
- table submissions.url: authored text != generated varchar(255) (rails string, not_null=False)
- table submissions: column wrong_count (integer) missing from generated reference
- table users: column bio (text) missing from generated reference
- table users: column birth_date (date) missing from generated reference
- table users: column creation_request_id (bigint) missing from generated reference
- table users: column email (varchar(255)) missing from generated reference
- table users: column email_channel_id (bigint) missing from generated reference
- table users: column enable_names_and_avatar (boolean) missing from generated reference
- table users: column featured_image_id (bigint) missing from generated reference
- table users: column full_name (varchar(255)) missing from generated reference
- table users: column hide_email_notifications (boolean) missing from generated reference
- table users: column last_name_first (boolean) missing from generated reference
- table users.locale: authored varchar(16) != generated varchar(255) (rails string, not_null=False)
- table users.lti_id: authored varchar(255) != generated text (rails text, not_null=False)
- table users: column manually_created (boolean) missing from generated reference
- table users: column migration_id (varchar(255)) missing from generated reference
- table users: column otp_secret_key (varchar(255)) missing from generated reference
- table users: column pseudonym_id (bigint) missing from generated reference
- table users: column public_write (boolean) missing from generated reference
- table users: column show_all_assignments (boolean) missing from generated reference
- table users: column sis_source_id (varchar(255)) missing from generated reference
- table users: column subscription (varchar(255)) missing from generated reference
- table users: column supervisor_user_id (bigint) missing from generated reference
- table users: column terms_of_use_accepted_at (timestamp) missing from generated reference
- table users: column title (varchar(255)) missing from generated reference
- table users: column user_can_manage_name (boolean) missing from generated reference
- table users: column work_email (varchar(255)) missing from generated reference
- table wiki_pages: column context_code (varchar(255)) missing from generated reference
- table wiki_pages: column deleted_at (timestamp) missing from generated reference
- table wiki_pages: column edited_at (timestamp) missing from generated reference
- table wiki_pages: column editor_id (bigint) missing from generated reference
- table wiki_pages: column front_page (boolean) missing from generated reference
- table wiki_pages: column hide_from_students (boolean) missing from generated reference
- table wiki_pages: column last_edited_by (bigint) missing from generated reference
- table wiki_pages: column position (integer) missing from generated reference
- table wiki_pages: column publish_after (timestamp) missing from generated reference
- table wiki_pages: column published (boolean) missing from generated reference
- table wiki_pages: column revision_number (integer) missing from generated reference
- table wiki_pages: column sticky (boolean) missing from generated reference
- table wiki_pages.url: authored varchar(255) != generated text (rails text, not_null=False)
- table wiki_pages: column usage_rights_id (bigint) missing from generated reference
