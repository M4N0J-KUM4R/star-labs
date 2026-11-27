-- Migration 0231: Canvas column parity for the authored cohort.
--
-- PR #200's canvas-schema files (0223-0225) added every Canvas
-- table star-db did not define; tables from migrations
-- 0001-0222 were skipped because they already existed, so their
-- Canvas columns were never added. This closes that backlog:
-- every reference column missing from an authored table, added
-- nullable with no default (additive-only; no backfill; the
-- additive pattern is exactly `ALTER TABLE ... ADD COLUMN IF
-- NOT EXISTS` used throughout 0030-0229).
--
-- Reference: tools/parity/canvas_authored_tables_reference.json
-- (Canvas LMS db/migrate replay, master 2026-04 — the same
-- reference the 0223-0225 files were generated from).

-- accounts: 20 columns
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS allow_sis_import boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS allowed_services text;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS enable_course_catalog boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS enable_profiles boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS equella_enabled boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS global_repository boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS ip_whitelist text;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS ltr_launch_settings text;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS mfa_required boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS push_subscription_enabled boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS service_notification_id bigint;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS show_scheduler boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS sis_default_grade_export boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS sis_name varchar(255);
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS sync_enrollment_waits boolean;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS turnitin_account_id varchar(255);
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS turnitin_pledge text;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS turnitin_shared_secret varchar(255);

-- assignment_groups: 7 columns
ALTER TABLE assignment_groups ADD COLUMN IF NOT EXISTS context_id bigint;
ALTER TABLE assignment_groups ADD COLUMN IF NOT EXISTS context_type varchar(255);
ALTER TABLE assignment_groups ADD COLUMN IF NOT EXISTS default_assignment_name varchar(255);
ALTER TABLE assignment_groups ADD COLUMN IF NOT EXISTS integration_data text;
ALTER TABLE assignment_groups ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE assignment_groups ADD COLUMN IF NOT EXISTS rules text;
ALTER TABLE assignment_groups ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);

-- assignment_overrides: 5 columns
ALTER TABLE assignment_overrides ADD COLUMN IF NOT EXISTS assignment_version integer;
ALTER TABLE assignment_overrides ADD COLUMN IF NOT EXISTS current_version integer;
ALTER TABLE assignment_overrides ADD COLUMN IF NOT EXISTS quiz_id bigint;
ALTER TABLE assignment_overrides ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE assignment_overrides ADD COLUMN IF NOT EXISTS set_type varchar(255);

-- assignments: 50 columns
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS allowed_attempts integer;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS allowed_extensions text;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS anonymize_students boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS applied_assignment_groups_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS attempt_count integer;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS autograde_at timestamp;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS automatic_peer_review_count integer;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS automatic_peer_reviews_assigned boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS cloned_item_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS context_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS context_type varchar(255);
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS duplicate_notification_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS eula_url text;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS excused_assignments_count integer;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS external_tool_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS free_form_criterion_comments boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grading_period_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grading_standard_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grading_type varchar(255);
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS group_category_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS has_submitted_submissions boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS intra_group_peer_reviews boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS ip_filter text;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS last_updated_by bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS lock_at timestamp;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS locked_for_user boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS lti_advantage_line_item_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS max_name_length integer;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS only_visible_to_overrides boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS peer_reviews_assigned boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS peer_reviews_due_at timestamp;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS position integer;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS post_manually boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS posted_at timestamp;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS presentation_tool_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS published boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS published_at timestamp;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS quiz_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS remove_overridden_grades boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS rubric_hidden boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS saved_rubrics boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS second_attempt_date timestamp;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS secure_params text;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS submission_types text;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS submitted_count integer;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS turnitin_enabled boolean;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS turnitin_settings text;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS unlock_at timestamp;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS visible_to_everybody boolean;

-- attachments: 8 columns
ALTER TABLE attachments ADD COLUMN IF NOT EXISTS attachment_id bigint;
ALTER TABLE attachments ADD COLUMN IF NOT EXISTS exclude_from_ffi boolean;
ALTER TABLE attachments ADD COLUMN IF NOT EXISTS hidden boolean;
ALTER TABLE attachments ADD COLUMN IF NOT EXISTS hidden_unlocked boolean;
ALTER TABLE attachments ADD COLUMN IF NOT EXISTS last_lock_at timestamp;
ALTER TABLE attachments ADD COLUMN IF NOT EXISTS last_unlock_at timestamp;
ALTER TABLE attachments ADD COLUMN IF NOT EXISTS uploaded_data text;
ALTER TABLE attachments ADD COLUMN IF NOT EXISTS visibility varchar(255);

-- calendar_events: 11 columns
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS all_day boolean;
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS all_day_date date;
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS assignment_id bigint;
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS blackout_date boolean;
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS cloned_item_id bigint;
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS context_code varchar(255);
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS duplicate_with_children_details text;
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS effective_context_code varchar(255);
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS important_dates boolean;
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS imported_from_ics boolean;
ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS original_date date;

-- context_external_tools: 5 columns
ALTER TABLE context_external_tools ADD COLUMN IF NOT EXISTS custom_fields text;
ALTER TABLE context_external_tools ADD COLUMN IF NOT EXISTS privacy_level varchar(255);
ALTER TABLE context_external_tools ADD COLUMN IF NOT EXISTS text text;
ALTER TABLE context_external_tools ADD COLUMN IF NOT EXISTS use_1_3 boolean;
ALTER TABLE context_external_tools ADD COLUMN IF NOT EXISTS vendor_code varchar(255);

-- conversation_messages: 10 columns
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS forwarded_message_ids text;
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS generated boolean;
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS has_attachments boolean;
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS has_media_objects boolean;
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS media_comment_id varchar(255);
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS media_comment_type varchar(255);
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS monologue boolean;
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS root_account_ids bigint;
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS subject varchar(255);
ALTER TABLE conversation_messages ADD COLUMN IF NOT EXISTS updated_at timestamp;

-- conversation_participants: 8 columns
ALTER TABLE conversation_participants ADD COLUMN IF NOT EXISTS label varchar(255);
ALTER TABLE conversation_participants ADD COLUMN IF NOT EXISTS last_authored_message_at timestamp;
ALTER TABLE conversation_participants ADD COLUMN IF NOT EXISTS last_message_author_id bigint;
ALTER TABLE conversation_participants ADD COLUMN IF NOT EXISTS last_message_body varchar(255);
ALTER TABLE conversation_participants ADD COLUMN IF NOT EXISTS message_count integer;
ALTER TABLE conversation_participants ADD COLUMN IF NOT EXISTS root_account_ids bigint;
ALTER TABLE conversation_participants ADD COLUMN IF NOT EXISTS subscribed boolean;
ALTER TABLE conversation_participants ADD COLUMN IF NOT EXISTS updated_at timestamp;

-- conversations: 7 columns
ALTER TABLE conversations ADD COLUMN IF NOT EXISTS has_attachments boolean;
ALTER TABLE conversations ADD COLUMN IF NOT EXISTS has_media_objects boolean;
ALTER TABLE conversations ADD COLUMN IF NOT EXISTS last_author varchar(255);
ALTER TABLE conversations ADD COLUMN IF NOT EXISTS last_message varchar(255);
ALTER TABLE conversations ADD COLUMN IF NOT EXISTS last_message_at timestamp;
ALTER TABLE conversations ADD COLUMN IF NOT EXISTS message_count integer;
ALTER TABLE conversations ADD COLUMN IF NOT EXISTS root_account_ids bigint;

-- course_sections: 10 columns
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS enrollment_template_id bigint;
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS friendly_name varchar(255);
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS nonxlist_course_id bigint;
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS position integer;
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS restrict_enrollments_to_section_dates boolean;
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS sis_batch_id bigint;
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS sis_commit_hash varchar(255);
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);
ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS stuck_sis_fields text;

-- courses: 33 columns
ALTER TABLE courses ADD COLUMN IF NOT EXISTS apply_assignment_group_weights boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS blueprint boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS calendar_event_id bigint;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS concluded_at timestamp;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS default_view varchar(255);
ALTER TABLE courses ADD COLUMN IF NOT EXISTS final_grades_published_at timestamp;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS grading_standard_id bigint;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS hide_distribution_graphs boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS hide_final_grades boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS homeroom_course boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS image_id bigint;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS indexed boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS is_public boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS is_public_to_auth_users boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS licensing varchar(255);
ALTER TABLE courses ADD COLUMN IF NOT EXISTS locked boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS migration_ids text;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS open_enrollment boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS organizational boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS public_description text;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS public_syllabus boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS public_syllabus_to_auth boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS restrict_enrollments_to_course_dates boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS restrict_student_future_view boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS restrict_student_past_view boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS self_enrollment boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS show_total_scores_as_points boolean;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);
ALTER TABLE courses ADD COLUMN IF NOT EXISTS tab_positions text;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS template_course_id bigint;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS time_zone varchar(255);
ALTER TABLE courses ADD COLUMN IF NOT EXISTS turnitin_assignment_id varchar(255);

-- discussion_entries: 15 columns
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS attachment_id bigint;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS depth integer;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS discussion_topic_id bigint;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS duplicate_entry_id bigint;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS editor_id bigint;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS flavor_flag varchar(255);
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS generation integer;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS legacy boolean;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS not_replied_for_deleted boolean;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS published boolean;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS rating_count integer;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS rating_sum integer;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS root_entry_id bigint;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS total_children integer;
ALTER TABLE discussion_entries ADD COLUMN IF NOT EXISTS total_reply_count integer;

-- discussion_topics: 23 columns
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS assignment_id bigint;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS attachment_id bigint;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS auto_locking boolean;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS context_id bigint;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS context_type varchar(255);
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS delayed_post_at timestamp;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS last_reply_at timestamp;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS last_reply_at_previous timestamp;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS last_reply_user_id bigint;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS lock_at timestamp;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS podcast_enabled boolean;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS podcast_has_student_posts boolean;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS remove_previous_posts boolean;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS require_initial_post boolean;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS rss_pseudo_guid varchar(255);
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS sequential_replies boolean;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS sort_by_rating boolean;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS sticky boolean;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS subscribed boolean;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS todo_date timestamp;
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS user_name varchar(255);
ALTER TABLE discussion_topics ADD COLUMN IF NOT EXISTS without_entries boolean;

-- enrollments: 25 columns
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS assigned_quiz_lti_ids text;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS associated_user_id bigint;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS completed_at timestamp;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS end_at timestamp;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS last_activity_at timestamp;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS last_attended_at timestamp;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS locked boolean;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS no_submission_feedback_check boolean;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS no_submission_meta_check boolean;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS notified_of_dashboard_change boolean;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS observed_user_id bigint;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS path_string text;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS privilege_update_notice_sent varchar(255);
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS role_id bigint;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS section_sticky boolean;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS self_enrolled boolean;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS sis_batch_id bigint;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS sis_commit_hash varchar(255);
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS sis_pseudonym_id bigint;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS stuck_sis_fields text;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS time_zone varchar(255);
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS totally_locked boolean;
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS type varchar(255);

-- files: 19 columns
ALTER TABLE files ADD COLUMN IF NOT EXISTS attachment_id bigint;
ALTER TABLE files ADD COLUMN IF NOT EXISTS context_id bigint;
ALTER TABLE files ADD COLUMN IF NOT EXISTS context_type varchar(255);
ALTER TABLE files ADD COLUMN IF NOT EXISTS deleted_at timestamp;
ALTER TABLE files ADD COLUMN IF NOT EXISTS exclude_from_ffi boolean;
ALTER TABLE files ADD COLUMN IF NOT EXISTS file_state varchar(255);
ALTER TABLE files ADD COLUMN IF NOT EXISTS hidden_unlocked boolean;
ALTER TABLE files ADD COLUMN IF NOT EXISTS last_lock_at timestamp;
ALTER TABLE files ADD COLUMN IF NOT EXISTS last_unlock_at timestamp;
ALTER TABLE files ADD COLUMN IF NOT EXISTS lock_at_previous timestamp;
ALTER TABLE files ADD COLUMN IF NOT EXISTS md5 varchar(255);
ALTER TABLE files ADD COLUMN IF NOT EXISTS migration_id varchar(255);
ALTER TABLE files ADD COLUMN IF NOT EXISTS need_notify boolean;
ALTER TABLE files ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE files ADD COLUMN IF NOT EXISTS root_attachment_id bigint;
ALTER TABLE files ADD COLUMN IF NOT EXISTS size integer;
ALTER TABLE files ADD COLUMN IF NOT EXISTS uploaded_data text;
ALTER TABLE files ADD COLUMN IF NOT EXISTS viewed_at timestamp;
ALTER TABLE files ADD COLUMN IF NOT EXISTS visibility varchar(255);

-- folders: 10 columns
ALTER TABLE folders ADD COLUMN IF NOT EXISTS cloned_item_id bigint;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS hidden boolean;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS hidden_unlocked boolean;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS invisible boolean;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS last_locked_at timestamp;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS locked_by bigint;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS storage_files_count integer;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS submission_bytes_downloaded integer;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS submission_bytes_quota integer;

-- group_categories: 8 columns
ALTER TABLE group_categories ADD COLUMN IF NOT EXISTS context_id bigint;
ALTER TABLE group_categories ADD COLUMN IF NOT EXISTS context_type varchar(255);
ALTER TABLE group_categories ADD COLUMN IF NOT EXISTS delete_after timestamp;
ALTER TABLE group_categories ADD COLUMN IF NOT EXISTS enable_self_signup boolean;
ALTER TABLE group_categories ADD COLUMN IF NOT EXISTS join_limit integer;
ALTER TABLE group_categories ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE group_categories ADD COLUMN IF NOT EXISTS split_group_count integer;
ALTER TABLE group_categories ADD COLUMN IF NOT EXISTS split_groups boolean;

-- group_memberships: 8 columns
ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS associated_user_id bigint;
ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS import_id bigint;
ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS sis_batch_id bigint;
ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS sis_commit_hash varchar(255);
ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS sis_pseudonym_id bigint;
ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);
ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS stuck_sis_fields text;

-- groups: 15 columns
ALTER TABLE groups ADD COLUMN IF NOT EXISTS attachments_count integer;
ALTER TABLE groups ADD COLUMN IF NOT EXISTS category varchar(255);
ALTER TABLE groups ADD COLUMN IF NOT EXISTS context_id bigint;
ALTER TABLE groups ADD COLUMN IF NOT EXISTS context_type varchar(255);
ALTER TABLE groups ADD COLUMN IF NOT EXISTS invite_provided boolean;
ALTER TABLE groups ADD COLUMN IF NOT EXISTS max_membership integer;
ALTER TABLE groups ADD COLUMN IF NOT EXISTS migration_id varchar(255);
ALTER TABLE groups ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE groups ADD COLUMN IF NOT EXISTS sIS_import_batch_id bigint;
ALTER TABLE groups ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);
ALTER TABLE groups ADD COLUMN IF NOT EXISTS storage_quota integer;
ALTER TABLE groups ADD COLUMN IF NOT EXISTS tab_positions text;
ALTER TABLE groups ADD COLUMN IF NOT EXISTS time_zone varchar(255);
ALTER TABLE groups ADD COLUMN IF NOT EXISTS uuid varchar(255);
ALTER TABLE groups ADD COLUMN IF NOT EXISTS wiki_page_counts json;

-- learning_outcomes: 9 columns
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS calculation_int integer;
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS calculation_method varchar(255);
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS context_id bigint;
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS context_type varchar(255);
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS migration_id varchar(255);
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS ratings text;
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS short_description varchar(255);
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS vendor_guid varchar(255);

-- module_items: 11 columns
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS completion_requirement text;
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS content_id bigint;
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS content_type varchar(255);
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS deleted_at timestamp;
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS last_duplicate_module_item_id bigint;
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS master_course_restriction_lock boolean;
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS migration_id varchar(255);
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS new_tab boolean;
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS thumbnail_url varchar(255);
ALTER TABLE module_items ADD COLUMN IF NOT EXISTS unknown_if_url boolean;

-- modules: 13 columns
ALTER TABLE modules ADD COLUMN IF NOT EXISTS completion_requirements text;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS context_id bigint;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS context_type varchar(255);
ALTER TABLE modules ADD COLUMN IF NOT EXISTS deleted_at timestamp;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS entropy_start_position integer;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS last_duplicate_module_id bigint;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS prerequisites text;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS publish_after timestamp;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS publish_final_grade boolean;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS publish_progression_target integer;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS published_at timestamp;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE modules ADD COLUMN IF NOT EXISTS unlock_requirement varchar(255);

-- outcome_groups: 4 columns
ALTER TABLE outcome_groups ADD COLUMN IF NOT EXISTS depth integer;
ALTER TABLE outcome_groups ADD COLUMN IF NOT EXISTS migration_id varchar(255);
ALTER TABLE outcome_groups ADD COLUMN IF NOT EXISTS outcomes_count integer;
ALTER TABLE outcome_groups ADD COLUMN IF NOT EXISTS root_account_id bigint;

-- progress: 2 columns
ALTER TABLE progress ADD COLUMN IF NOT EXISTS completed_at timestamp;
ALTER TABLE progress ADD COLUMN IF NOT EXISTS results json;

-- pseudonyms: 9 columns
ALTER TABLE pseudonyms ADD COLUMN IF NOT EXISTS certificate_b64 text;
ALTER TABLE pseudonyms ADD COLUMN IF NOT EXISTS external_user_guid varchar(255);
ALTER TABLE pseudonyms ADD COLUMN IF NOT EXISTS last_sis_assignment_id bigint;
ALTER TABLE pseudonyms ADD COLUMN IF NOT EXISTS login_hint text;
ALTER TABLE pseudonyms ADD COLUMN IF NOT EXISTS persistent_session_id varchar(255);
ALTER TABLE pseudonyms ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE pseudonyms ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);
ALTER TABLE pseudonyms ADD COLUMN IF NOT EXISTS timed_out_at timestamp;
ALTER TABLE pseudonyms ADD COLUMN IF NOT EXISTS user_can_edit_login boolean;

-- quiz_questions: 6 columns
ALTER TABLE quiz_questions ADD COLUMN IF NOT EXISTS assessment_question_id bigint;
ALTER TABLE quiz_questions ADD COLUMN IF NOT EXISTS published_at timestamp;
ALTER TABLE quiz_questions ADD COLUMN IF NOT EXISTS question_data text;
ALTER TABLE quiz_questions ADD COLUMN IF NOT EXISTS regrade_option varchar(255);
ALTER TABLE quiz_questions ADD COLUMN IF NOT EXISTS regraded boolean;
ALTER TABLE quiz_questions ADD COLUMN IF NOT EXISTS root_account_id bigint;

-- quizzes: 21 columns
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS assignment_id bigint;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS assignment_version integer;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS could_be_locked boolean;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS deleted_at timestamp;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS due_at timestamp;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS hide_correct_answers_at timestamp;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS hide_results varchar(255);
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS ip_filter varchar(255);
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS last_assignment_id bigint;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS last_edited_by_id bigint;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS lock_at timestamp;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS one_question_at_a_time boolean;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS published boolean;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS root_account_id bigint;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS scoring_policy varchar(255);
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS show_correct_answers boolean;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS show_correct_answers_at timestamp;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS shuffle_answers boolean;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS unlock_at timestamp;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS version_number integer;
ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS visible boolean;

-- rubric_associations: 5 columns
ALTER TABLE rubric_associations ADD COLUMN IF NOT EXISTS association_object_id bigint;
ALTER TABLE rubric_associations ADD COLUMN IF NOT EXISTS current_points float8;
ALTER TABLE rubric_associations ADD COLUMN IF NOT EXISTS current_score float8;
ALTER TABLE rubric_associations ADD COLUMN IF NOT EXISTS last_updated_at timestamp;
ALTER TABLE rubric_associations ADD COLUMN IF NOT EXISTS root_account_id bigint;

-- rubrics: 9 columns
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS context_id bigint;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS context_type varchar(255);
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS description varchar(255);
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS free_form_criterion_comments boolean;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS hide_outcome_results boolean;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS hide_score_total boolean;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS migration_id varchar(255);
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS public boolean;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS reusable boolean;

-- submissions: 18 columns
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS cached_due_date timestamp;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS deleted boolean;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS detached boolean;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS grade varchar(255);
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS grade_matches_current_submission boolean;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS graded_anonymously boolean;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS group_id bigint;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS late boolean;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS media_comment_id varchar(255);
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS media_comment_type varchar(255);
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS posted_at timestamp;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS processed boolean;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS redo_request boolean;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS scorekeeper_id bigint;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS second_attempt_time timestamp;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS second_meets_attempted boolean;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS url text;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS wrong_count integer;

-- users: 30 columns
ALTER TABLE users ADD COLUMN IF NOT EXISTS bio text;
ALTER TABLE users ADD COLUMN IF NOT EXISTS birth_date date;
ALTER TABLE users ADD COLUMN IF NOT EXISTS creation_request_id bigint;
ALTER TABLE users ADD COLUMN IF NOT EXISTS deleted_at timestamp;
ALTER TABLE users ADD COLUMN IF NOT EXISTS email_channel_id bigint;
ALTER TABLE users ADD COLUMN IF NOT EXISTS enable_names_and_avatar boolean;
ALTER TABLE users ADD COLUMN IF NOT EXISTS featured_image_id bigint;
ALTER TABLE users ADD COLUMN IF NOT EXISTS full_name varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS hide_email_notifications boolean;
ALTER TABLE users ADD COLUMN IF NOT EXISTS last_name_first boolean;
ALTER TABLE users ADD COLUMN IF NOT EXISTS locale varchar(16);
ALTER TABLE users ADD COLUMN IF NOT EXISTS lti_id varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS manually_created boolean;
ALTER TABLE users ADD COLUMN IF NOT EXISTS migration_id varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS otp_secret_key varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS otp_secret_key_enc varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS pseudonym_id bigint;
ALTER TABLE users ADD COLUMN IF NOT EXISTS public_write boolean;
ALTER TABLE users ADD COLUMN IF NOT EXISTS root_account_ids bigint;
ALTER TABLE users ADD COLUMN IF NOT EXISTS short_name varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS show_all_assignments boolean;
ALTER TABLE users ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS stuck_sis_fields text;
ALTER TABLE users ADD COLUMN IF NOT EXISTS subscription varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS supervisor_user_id bigint;
ALTER TABLE users ADD COLUMN IF NOT EXISTS terms_of_use_accepted_at timestamp;
ALTER TABLE users ADD COLUMN IF NOT EXISTS time_zone varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS title varchar(255);
ALTER TABLE users ADD COLUMN IF NOT EXISTS user_can_manage_name boolean;
ALTER TABLE users ADD COLUMN IF NOT EXISTS work_email varchar(255);

-- wiki_pages: 13 columns
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS context_code varchar(255);
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS deleted_at timestamp;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS edited_at timestamp;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS editor_id bigint;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS front_page boolean;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS hide_from_students boolean;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS last_edited_by bigint;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS position integer;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS publish_after timestamp;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS published boolean;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS revision_number integer;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS sticky boolean;
ALTER TABLE wiki_pages ADD COLUMN IF NOT EXISTS usage_rights_id bigint;

-- End of migration 0231.
