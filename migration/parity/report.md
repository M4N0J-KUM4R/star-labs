# Canvas to Rust parity scoreboard

Measured, deterministic output. Canvas source is not committed; refresh the manifest locally with `tools/parity/parity.py manifest`.

## Summary

| Area | Implemented | Total | Coverage |
|---|---:|---:|---:|
| REST API routes | 1403 | 1403 | 100.0% |
| REST routes referenced from `ui/` | 503 | 503 | 100.0% |
| GraphQL files | 1 | 390 | 0.3% |
| GraphQL object types | 246 | 246 | 100.0% |
| GraphQL mutations | 100 | 100 | 100.0% |
| UI feature directories | 0 | 232 | 0.0% |
| JS/JSX/TS/TSX files | 0 | 5459 | 0.0% |
| ERB views | 0 | 475 | 0.0% |
| js_env files | 0 | 124 | 0.0% |
| Schema tables (vs 688 Canvas models) | 397 | 688 | n/a |

Implemented REST route count: **1403 / 1403** (100.0%).

Star registrations: 1620 route/method pairs from 1328 `.route(...)` calls (0 unparsed); 208 have no Canvas match and are listed as extras below.

Canvas source sizes: 390 GraphQL files (246 object types, 100 mutations), 688 models, 232 UI feature dirs, 5459 JS/TSX files, 475 ERB views, 124 js_env files (490 call sites).

## Controllers with coverage (222)

| Controller | Implemented | Total | Coverage |
|---|---:|---:|---:|
| `discussion_topics_api` | 58 | 58 | 100.0% |
| `submissions_api` | 44 | 44 | 100.0% |
| `outcome_groups_api` | 40 | 40 | 100.0% |
| `users` | 39 | 39 | 100.0% |
| `courses` | 38 | 38 | 100.0% |
| `folders` | 34 | 34 | 100.0% |
| `wiki_pages_api` | 29 | 29 | 100.0% |
| `files` | 26 | 26 | 100.0% |
| `content_migrations` | 25 | 25 | 100.0% |
| `external_tools` | 25 | 25 | 100.0% |
| `accounts` | 22 | 22 | 100.0% |
| `lti/registrations` | 20 | 20 | 100.0% |
| `feature_flags` | 17 | 17 | 100.0% |
| `groups` | 17 | 17 | 100.0% |
| `conversations` | 16 | 16 | 100.0% |
| `group_categories` | 16 | 16 | 100.0% |
| `migration_issues` | 16 | 16 | 100.0% |
| `calendar_events_api` | 15 | 15 | 100.0% |
| `learning_object_dates` | 15 | 15 | 100.0% |
| `rubrics_api` | 15 | 15 | 100.0% |
| `content_exports_api` | 14 | 14 | 100.0% |
| `enrollments_api` | 14 | 14 | 100.0% |
| `blackout_dates` | 13 | 13 | 100.0% |
| `master_courses/master_templates` | 13 | 13 | 100.0% |
| `assignments_api` | 12 | 12 | 100.0% |
| `communication_channels` | 12 | 12 | 100.0% |
| `context_module_items_api` | 12 | 12 | 100.0% |
| `grade_change_audit_api` | 12 | 12 | 100.0% |
| `jobs_v2` | 12 | 12 | 100.0% |
| `lti/ims/tool_setting` | 12 | 12 | 100.0% |
| `group_memberships` | 11 | 11 | 100.0% |
| `lti/ims/dynamic_registration` | 11 | 11 | 100.0% |
| `assignment_overrides` | 10 | 10 | 100.0% |
| `discussion_topics` | 10 | 10 | 100.0% |
| `grading_standards_api` | 10 | 10 | 100.0% |
| `media_objects` | 10 | 10 | 100.0% |
| `notification_preferences` | 10 | 10 | 100.0% |
| `ai_experiences` | 9 | 9 | 100.0% |
| `peer_reviews_api` | 9 | 9 | 100.0% |
| `polling/poll_sessions` | 9 | 9 | 100.0% |
| `sections` | 9 | 9 | 100.0% |
| `usage_rights` | 9 | 9 | 100.0% |
| `account_notifications` | 8 | 8 | 100.0% |
| `ai_conversations` | 8 | 8 | 100.0% |
| `appointment_groups` | 8 | 8 | 100.0% |
| `authentication_providers` | 8 | 8 | 100.0% |
| `content_shares` | 8 | 8 | 100.0% |
| `context_modules_api` | 8 | 8 | 100.0% |
| `course_paces` | 8 | 8 | 100.0% |
| `csp_settings` | 8 | 8 | 100.0% |
| `favorites` | 8 | 8 | 100.0% |
| `grading_periods` | 8 | 8 | 100.0% |
| `lti/tool_proxy` | 8 | 8 | 100.0% |
| `release_notes` | 8 | 8 | 100.0% |
| `role_overrides` | 8 | 8 | 100.0% |
| `collaborations` | 7 | 7 | 100.0% |
| `conferences` | 7 | 7 | 100.0% |
| `eportfolios_api` | 7 | 7 | 100.0% |
| `page_views` | 7 | 7 | 100.0% |
| `quizzes/quiz_submissions_api` | 7 | 7 | 100.0% |
| `quizzes/quizzes_api` | 7 | 7 | 100.0% |
| `sis_imports_api` | 7 | 7 | 100.0% |
| `support_helpers/turnitin` | 7 | 7 | 100.0% |
| `user_observees` | 7 | 7 | 100.0% |
| `account_calendars_api` | 6 | 6 | 100.0% |
| `account_reports` | 6 | 6 | 100.0% |
| `block_editor_templates_api` | 6 | 6 | 100.0% |
| `external_feeds` | 6 | 6 | 100.0% |
| `learn_platform` | 6 | 6 | 100.0% |
| `lti/context_controls` | 6 | 6 | 100.0% |
| `lti/resource_links` | 6 | 6 | 100.0% |
| `lti_api` | 6 | 6 | 100.0% |
| `media_tracks` | 6 | 6 | 100.0% |
| `outcome_imports_api` | 6 | 6 | 100.0% |
| `outcome_results` | 6 | 6 | 100.0% |
| `pseudonyms` | 6 | 6 | 100.0% |
| `quizzes/quiz_groups` | 6 | 6 | 100.0% |
| `quizzes/quiz_submission_questions` | 6 | 6 | 100.0% |
| `assignment_groups_api` | 5 | 5 | 100.0% |
| `bookmarks/bookmarks` | 5 | 5 | 100.0% |
| `conditional_release/rules` | 5 | 5 | 100.0% |
| `course_nicknames` | 5 | 5 | 100.0% |
| `custom_gradebook_columns_api` | 5 | 5 | 100.0% |
| `developer_keys` | 5 | 5 | 100.0% |
| `gradebook_filters_api` | 5 | 5 | 100.0% |
| `lti/data_services` | 5 | 5 | 100.0% |
| `lti/deployments` | 5 | 5 | 100.0% |
| `lti/ims/line_items` | 5 | 5 | 100.0% |
| `lti/ims/tool_proxy` | 5 | 5 | 100.0% |
| `lti/originality_reports_api` | 5 | 5 | 100.0% |
| `lti/subscriptions_api` | 5 | 5 | 100.0% |
| `lti/tool_configurations_api` | 5 | 5 | 100.0% |
| `observer_alert_thresholds_api` | 5 | 5 | 100.0% |
| `planner_notes` | 5 | 5 | 100.0% |
| `planner_overrides` | 5 | 5 | 100.0% |
| `polling/poll_choices` | 5 | 5 | 100.0% |
| `polling/polls` | 5 | 5 | 100.0% |
| `quizzes/quiz_questions` | 5 | 5 | 100.0% |
| `tabs` | 5 | 5 | 100.0% |
| `temporary_enrollment_pairings_api` | 5 | 5 | 100.0% |
| `tokens` | 5 | 5 | 100.0% |
| `admins` | 4 | 4 | 100.0% |
| `career_experience` | 4 | 4 | 100.0% |
| `course_pacing/section_paces_api` | 4 | 4 | 100.0% |
| `course_pacing/student_enrollment_paces_api` | 4 | 4 | 100.0% |
| `discussion_entries` | 4 | 4 | 100.0% |
| `gradebook_history_api` | 4 | 4 | 100.0% |
| `grading_period_sets` | 4 | 4 | 100.0% |
| `lti/account_external_tools` | 4 | 4 | 100.0% |
| `lti/lti_apps` | 4 | 4 | 100.0% |
| `microsoft_sync/groups` | 4 | 4 | 100.0% |
| `outcome_proficiency_api` | 4 | 4 | 100.0% |
| `outcomes_api` | 4 | 4 | 100.0% |
| `provisional_grades` | 4 | 4 | 100.0% |
| `quizzes/quiz_reports` | 4 | 4 | 100.0% |
| `search` | 4 | 4 | 100.0% |
| `submission_comments_api` | 4 | 4 | 100.0% |
| `announcements_api` | 3 | 3 | 100.0% |
| `assessment_question_banks` | 3 | 3 | 100.0% |
| `authentication_audit_api` | 3 | 3 | 100.0% |
| `brand_configs_api` | 3 | 3 | 100.0% |
| `course_reports` | 3 | 3 | 100.0% |
| `custom_data` | 3 | 3 | 100.0% |
| `custom_gradebook_column_data_api` | 3 | 3 | 100.0% |
| `discovery_pages_api` | 3 | 3 | 100.0% |
| `epub_exports` | 3 | 3 | 100.0% |
| `jobs` | 3 | 3 | 100.0% |
| `late_policy` | 3 | 3 | 100.0% |
| `lti/asset_processor_tii_migrations_api` | 3 | 3 | 100.0% |
| `lti/ims/asset_processor_eula` | 3 | 3 | 100.0% |
| `lti/ims/authentication` | 3 | 3 | 100.0% |
| `lti/submissions_api` | 3 | 3 | 100.0% |
| `module_assignment_overrides` | 3 | 3 | 100.0% |
| `observer_alerts_api` | 3 | 3 | 100.0% |
| `outcomes_academic_benchmark_import_api` | 3 | 3 | 100.0% |
| `rubric_assessments` | 3 | 3 | 100.0% |
| `rubric_associations` | 3 | 3 | 100.0% |
| `rubrics` | 3 | 3 | 100.0% |
| `services_api` | 3 | 3 | 100.0% |
| `shared_brand_configs` | 3 | 3 | 100.0% |
| `smart_search` | 3 | 3 | 100.0% |
| `support_helpers/asset_processor` | 3 | 3 | 100.0% |
| `terms` | 3 | 3 | 100.0% |
| `app_center` | 2 | 2 | 100.0% |
| `canvadoc_sessions` | 2 | 2 | 100.0% |
| `canvas_career/user_experiences` | 2 | 2 | 100.0% |
| `conditional_release/stats` | 2 | 2 | 100.0% |
| `content_imports` | 2 | 2 | 100.0% |
| `course_audit_api` | 2 | 2 | 100.0% |
| `course_tag_conversion` | 2 | 2 | 100.0% |
| `discussion_topic_users` | 2 | 2 | 100.0% |
| `gradebooks` | 2 | 2 | 100.0% |
| `jwts` | 2 | 2 | 100.0% |
| `learning_mastery_gradebook_settings_api` | 2 | 2 | 100.0% |
| `live_assessments/assessments` | 2 | 2 | 100.0% |
| `live_assessments/results` | 2 | 2 | 100.0% |
| `lti/asset_processor` | 2 | 2 | 100.0% |
| `lti/feature_flags` | 2 | 2 | 100.0% |
| `lti/ims/asset_processor` | 2 | 2 | 100.0% |
| `lti/ims/authorization` | 2 | 2 | 100.0% |
| `lti/ims/names_and_roles` | 2 | 2 | 100.0% |
| `lti/ims/notice_handlers` | 2 | 2 | 100.0% |
| `lti/ims/results` | 2 | 2 | 100.0% |
| `lti/ims/tool_consumer_profile` | 2 | 2 | 100.0% |
| `lti/membership_service` | 2 | 2 | 100.0% |
| `lti/token` | 2 | 2 | 100.0% |
| `lti/users_api` | 2 | 2 | 100.0% |
| `microfrontends_release_tag_override` | 2 | 2 | 100.0% |
| `moderation_set` | 2 | 2 | 100.0% |
| `permissions_help` | 2 | 2 | 100.0% |
| `planner` | 2 | 2 | 100.0% |
| `plugins` | 2 | 2 | 100.0% |
| `polling/poll_submissions` | 2 | 2 | 100.0% |
| `profile` | 2 | 2 | 100.0% |
| `progress` | 2 | 2 | 100.0% |
| `quizzes/outstanding_quiz_submissions` | 2 | 2 | 100.0% |
| `quizzes/quiz_assignment_overrides` | 2 | 2 | 100.0% |
| `quizzes/quiz_submission_events_api` | 2 | 2 | 100.0% |
| `quizzes/quiz_submission_users` | 2 | 2 | 100.0% |
| `security` | 2 | 2 | 100.0% |
| `sis_api` | 2 | 2 | 100.0% |
| `sis_import_errors_api` | 2 | 2 | 100.0% |
| `sub_accounts` | 2 | 2 | 100.0% |
| `submissions` | 2 | 2 | 100.0% |
| `support_helpers/plagiarism_platform` | 2 | 2 | 100.0% |
| `syllabus_api` | 2 | 2 | 100.0% |
| `terms_api` | 2 | 2 | 100.0% |
| `web_zip_exports` | 2 | 2 | 100.0% |
| `what_if_grades_api` | 2 | 2 | 100.0% |
| `accessibility_course_scans` | 1 | 1 | 100.0% |
| `accessibility_course_statistics` | 1 | 1 | 100.0% |
| `anonymous_provisional_grades` | 1 | 1 | 100.0% |
| `assignment_extensions` | 1 | 1 | 100.0% |
| `assignment_groups` | 1 | 1 | 100.0% |
| `assignments` | 1 | 1 | 100.0% |
| `comm_messages_api` | 1 | 1 | 100.0% |
| `course_pacing/bulk_student_enrollment_paces_api` | 1 | 1 | 100.0% |
| `course_pacing/pace_contexts_api` | 1 | 1 | 100.0% |
| `developer_key_account_bindings` | 1 | 1 | 100.0% |
| `disable_post_to_sis_api` | 1 | 1 | 100.0% |
| `errors` | 1 | 1 | 100.0% |
| `gradebook_settings` | 1 | 1 | 100.0% |
| `history` | 1 | 1 | 100.0% |
| `immersive_reader` | 1 | 1 | 100.0% |
| `inst_access_tokens` | 1 | 1 | 100.0% |
| `lmgb_user_details` | 1 | 1 | 100.0% |
| `login` | 1 | 1 | 100.0% |
| `lti/account_lookup` | 1 | 1 | 100.0% |
| `lti/ims/progress` | 1 | 1 | 100.0% |
| `lti/ims/scores` | 1 | 1 | 100.0% |
| `lti/plagiarism_assignments_api` | 1 | 1 | 100.0% |
| `lti/public_jwk` | 1 | 1 | 100.0% |
| `observer_pairing_codes_api` | 1 | 1 | 100.0% |
| `quizzes/course_quiz_extensions` | 1 | 1 | 100.0% |
| `quizzes/quiz_extensions` | 1 | 1 | 100.0% |
| `quizzes/quiz_ip_filters` | 1 | 1 | 100.0% |
| `quizzes/quiz_statistics` | 1 | 1 | 100.0% |
| `quizzes/quiz_submission_files` | 1 | 1 | 100.0% |
| `quizzes_next/quizzes_api` | 1 | 1 | 100.0% |
| `scopes_api` | 1 | 1 | 100.0% |
| `study_assist` | 1 | 1 | 100.0% |
| `support_helpers/submission_lifecycle_manage` | 1 | 1 | 100.0% |

## GraphQL operation coverage

Names are matched exactly: star-graphql type names (mapped through the documented renames below) against Canvas `graphql_name`s from `app/graphql/types/*_type.rb`, and mutation field names against the operations of `app/graphql/mutations/**` (classes whose superclass chain reaches `Mutations::BaseMutation`).

### Implemented types (246 of 246 Canvas names)

`AIGradeCriterionResult`, `AIGradeRating`, `AIGradeResult`, `Account`, `AccountNotification`, `AccountUsersFilter`, `AccountUsersSortField`, `AccountUsersSortInput`, `ActivityStream`, `AdhocStudents`, `AllocationRule`, `AllocationRulesFilterInput`, `AllowProvisionalGradingType`, `AnonymousStudentIdentity`, `AnonymousUser`, `AssessmentRequest`, `AssessmentType`, `AssignedStudentsFilter`, `Assignment`, `AssignmentAllocationRules`, `AssignmentGroup`, `AssignmentGroupRules`, `AssignmentGroupState`, `AssignmentOverride`, `AssignmentOverrideSet`, `AssignmentRubricAssessment`, `AssignmentScoreStatistic`, `AssignmentState`, `AssignmentTargetSortField`, `AssignmentTargetSortOrder`, `AuditEvent`, `AuditEventExternalTool`, `AuditEventQuiz`, `AuditEventRole`, `AuditEventType`, `AuditEventUser`, `AuditLogs`, `AutoGradeEligibility`, `AutoGradeIssue`, `AutoLeaderPolicy`, `Checkpoint`, `CommentBankItem`, `CommunicationChannel`, `ContentTag`, `ContentTagContent`, `Conversation`, `ConversationMessage`, `ConversationParticipant`, `Course`, `CourseDashboardCard`, `CourseDashboardCardLink`, `CourseFilterableEnrollmentState`, `CourseFilterableEnrollmentType`, `CourseGradeStatus`, `CourseOutcomeAlignmentStats`, `CoursePermissions`, `CourseProgression`, `CourseRequirements`, `CourseSectionsFilter`, `CourseSettings`, `CourseUsersFilter`, `CourseUsersSortDirectionType`, `CourseUsersSortFieldType`, `CourseUsersSortInputType`, `CourseWorkSubmissionsOrderField`, `CourseWorkflowState`, `CustomGradeStatus`, `DashboardObserveeFilter`, `DateHash`, `DateHashSet`, `DateTime`, `DateTimeRange`, `Discussion`, `DiscussionEntry`, `DiscussionEntryCounts`, `DiscussionEntryPermissions`, `DiscussionEntryPinningType`, `DiscussionEntryReportTypeCounts`, `DiscussionEntryVersion`, `DiscussionFilterType`, `DiscussionParticipant`, `DiscussionParticipantFilter`, `DiscussionPermissions`, `DiscussionSortOrderType`, `DiscussionTopicAnonymousStateType`, `DiscussionTopicDiscussionType`, `DraftableSubmissionType`, `EligibilityIssue`, `EligibilityIssueLevel`, `Enrollment`, `EnrollmentFilterInput`, `EnrollmentRole`, `EnrollmentType`, `EnrollmentWorkflowState`, `EnrollmentsSortDirectionType`, `EnrollmentsSortFieldType`, `EnrollmentsSortInputType`, `EntryParticipant`, `ExternalTool`, `ExternalToolPlacement`, `ExternalToolPlacements`, `ExternalToolSettings`, `ExternalToolState`, `ExternalUrl`, `File`, `Folder`, `GradeState`, `GraderIdentity`, `Grades`, `GradesEnrollmentFilter`, `GradingPeriod`, `GradingPeriodGroup`, `GradingRole`, `GradingStandard`, `GradingStandardItem`, `GradingType`, `Group`, `GroupMembership`, `GroupMembershipState`, `GroupSet`, `GroupState`, `InboxSettings`, `InstitutionalTag`, `InstitutionalTagAssociation`, `InstitutionalTagCategory`, `InstitutionalTagUsersSortDirection`, `InstitutionalTagUsersSortField`, `InstitutionalTagUsersSortInput`, `InstitutionalTagWorkflowState`, `InstructorCourseInfo`, `InstructorEnrollmentInfo`, `InstructorUserInfo`, `InstructorWithEnrollments`, `InternalSetting`, `LearningOutcome`, `LearningOutcomeGroup`, `LockInfo`, `Lockable`, `LtiAsset`, `LtiAssetProcessor`, `LtiAssetProcessorIframe`, `LtiAssetProcessorWindowSettings`, `LtiAssetReport`, `MediaObject`, `MediaSource`, `MediaTrack`, `MediaTrackWorkflowState`, `MediaType`, `MessagePermissions`, `MessageableContext`, `MessageableUser`, `ModeratedGrading`, `Module`, `ModuleCompletionRequirement`, `ModuleCompletionStatus`, `ModuleExternalTool`, `ModuleFilterInput`, `ModuleItem`, `ModuleItemFilter`, `ModuleItemMasterCourseRestriction`, `ModulePrerequisite`, `ModuleProgression`, `ModuleProgressionStatistics`, `ModuleStatistics`, `Mutation`, `MutationLog`, `NodeType`, `Noop`, `Notification`, `NotificationPolicy`, `NotificationPreferences`, `NotificationPreferencesContextType`, `OrderDirection`, `OutcomeAlignment`, `OutcomeCalculationMethod`, `OutcomeFriendlyDescriptionType`, `OutcomeProficiency`, `Page`, `PageViewAnalysis`, `PeerReviewDates`, `PeerReviewStatus`, `PeerReviewSubAssignment`, `PeerReviews`, `PostPolicy`, `PreferredLanguageType`, `ProficiencyRating`, `ProficiencyRatingInput`, `Progress`, `ProgressContext`, `ProgressState`, `ProvisionalGrade`, `Query`, `Quiz`, `QuizItem`, `QuizSubmission`, `Recipients`, `Requirement`, `Rubric`, `RubricAssessment`, `RubricAssessmentRating`, `RubricAssociation`, `RubricCriterion`, `RubricRating`, `ScheduledPost`, `Section`, `SelfSignupPolicy`, `SpeedGraderSettings`, `StandardGradeStatus`, `Sticker`, `StreamSummaryItem`, `StringMap`, `StudentSummaryAnalytics`, `SubAssignmentSubmission`, `SubHeader`, `Submission`, `SubmissionComment`, `SubmissionCommentFilterInput`, `SubmissionCommentStatusType`, `SubmissionDraft`, `SubmissionFilterInput`, `SubmissionGradingStatus`, `SubmissionHistory`, `SubmissionHistoryFilterInput`, `SubmissionHistoryOrder`, `SubmissionHistoryOrderField`, `SubmissionOrderCriteria`, `SubmissionOrderField`, `SubmissionPostingStatus`, `SubmissionSearchFilterInput`, `SubmissionSearchOrder`, `SubmissionSearchOrderField`, `SubmissionState`, `SubmissionStatistics`, `SubmissionStatusTagType`, `SubmissionType`, `TardinessBreakdown`, `Term`, `TurnitinContext`, `TurnitinData`, `URL`, `UsageRights`, `User`, `UserGroupMembershipsFilterInput`, `ValidationError`, `VericiteContext`, `VericiteData`

Renames mapped to Canvas names: `CourseSection` -> `Section`, `DiscussionTopic` -> `Discussion`, `EnrollmentTerm` -> `Term`, `QuizQuestion` -> `QuizItem`.

### Missing types (0)

(none)

### Implemented mutations (100 of 100 Canvas operations)

`acceptEnrollmentInvitation`, `addConversationMessage`, `applyInstitutionalTag`, `autoGradeSubmission`, `createAllocationRule`, `createAssignment`, `createCommentBankItem`, `createConversation`, `createDiscussionEntry`, `createDiscussionTopic`, `createGroupInSet`, `createGroupSet`, `createInstitutionalTag`, `createInstitutionalTagCategory`, `createInternalSetting`, `createLearningOutcome`, `createLearningOutcomeGroup`, `createModule`, `createOutcomeCalculationMethod`, `createOutcomeProficiency`, `createSubmission`, `createSubmissionComment`, `createSubmissionDraft`, `createUserInboxLabel`, `deleteAllocationRule`, `deleteCommentBankItem`, `deleteConversationMessages`, `deleteConversations`, `deleteCustomGradeStatus`, `deleteDiscussionEntry`, `deleteDiscussionTopic`, `deleteInternalSetting`, `deleteOutcomeCalculationMethod`, `deleteOutcomeLinks`, `deleteOutcomeProficiency`, `deleteSubmissionComment`, `deleteSubmissionDraft`, `deleteUserInboxLabel`, `dismissAccountNotification`, `hideAssignmentGrades`, `hideAssignmentGradesForSections`, `importOutcomes`, `markSubmissionCommentsRead`, `moveOutcomeLinks`, `postAssignmentGrades`, `postAssignmentGradesForSections`, `postDraftSubmissionComment`, `rejectEnrollmentInvitation`, `removeInstitutionalTag`, `reorderModuleItems`, `restoreDeletedDiscussionEntry`, `saveRubricAssessment`, `selectProvisionalGrade`, `setAssignmentPostPolicy`, `setCoursePostPolicy`, `setFriendlyDescription`, `setModuleItemCompletion`, `setOverrideScore`, `setOverrideStatus`, `setRubricSelfAssessment`, `submitAutoGradeFeedback`, `subscribeToDiscussionTopic`, `updateAllocationRule`, `updateAssignment`, `updateCommentBankItem`, `updateConversationParticipants`, `updateDiscussionEntriesReadState`, `updateDiscussionEntry`, `updateDiscussionEntryParticipant`, `updateDiscussionReadState`, `updateDiscussionThreadReadState`, `updateDiscussionTopic`, `updateDiscussionTopicParticipant`, `updateGradebookGroupFilter`, `updateInstitutionalTag`, `updateInstitutionalTagArchivedState`, `updateInstitutionalTagCategory`, `updateInstitutionalTagCategoryArchivedState`, `updateInternalSetting`, `updateLearnerDashboardTabSelection`, `updateLearningOutcome`, `updateLearningOutcomeGroup`, `updateMyInboxSettings`, `updateNotificationPreferences`, `updateOutcomeCalculationMethod`, `updateOutcomeProficiency`, `updateRubricArchivedState`, `updateRubricAssessmentReadState`, `updateSpeedGraderSettings`, `updateSplitScreenViewDeeplyNestedAlert`, `updateSubmissionGrade`, `updateSubmissionGradeStatus`, `updateSubmissionSticker`, `updateSubmissionStudentEnteredScore`, `updateSubmissionsReadState`, `updateUserDiscussionsSplitscreenView`, `updateWidgetDashboardConfig`, `updateWidgetDashboardLayout`, `upsertCustomGradeStatus`, `upsertStandardGradeStatus`

### Missing mutations (0)

(none)

Star names with no Canvas counterpart (not counted): `AcceptEnrollmentInvitationInput`, `AddConversationMessageInput`, `AllocationRulePayload`, `ApplyInstitutionalTagInput`, `AssignmentConnection`, `AssignmentEdge`, `AssignmentGroupConnection`, `AssignmentGroupEdge`, `AutoGradeSubmissionInput`, `AutoGradeSubmissionPayload`, `BatchBConversation`, `BatchBConversationMessage`, `BatchBDiscussionEntry`, `BatchBDiscussionTopic`, `BatchBError`, `BatchBGroup`, `BatchBGroupSet`, `BatchBModule`, `BatchBSubmission`, `BatchBSubmissionComment`, `BatchCError`, `BatchCLearningOutcome`, `BatchCLearningOutcomeGroup`, `BatchCNotificationPolicy`, `CommentBankItemPayload`, `ConversationMessageConnection`, `ConversationMessageEdge`, `ConversationParticipantConnection`, `ConversationParticipantEdge`, `ConversationParticipantPayload`, `ConversationPayload`, `CreateAllocationRuleInput`, `CreateAssignmentPayload`, `CreateCommentBankItemInput`, `CreateConversationInput`, `CreateDiscussionEntryInput`, `CreateDiscussionTopicInput`, `CreateGroupInSetInput`, `CreateGroupSetInput`, `CreateInstitutionalTagCategoryInput`, `CreateInstitutionalTagInput`, `CreateInternalSettingInput`, `CreateLearningOutcomeGroupInput`, `CreateLearningOutcomeInput`, `CreateModuleInput`, `CreateOutcomeCalculationMethodInput`, `CreateOutcomeProficiencyInput`, `CreateSubmissionCommentInput`, `CreateSubmissionDraftInput`, `CreateSubmissionInput`, `CreateUserInboxLabelInput`, `CustomGradeStatusPayload`, `DeleteAllocationRuleInput`, `DeleteCommentBankItemInput`, `DeleteConversationInput`, `DeleteConversationMessagesInput`, `DeleteConversationsInput`, `DeleteCustomGradeStatusInput`, `DeleteDiscussionEntryInput`, `DeleteDiscussionTopicInput`, `DeleteInternalSettingInput`, `DeleteLearningOutcomeGroupInput`, `DeleteLearningOutcomeInput`, `DeleteOutcomeCalculationMethodInput`, `DeleteOutcomeLinksInput`, `DeleteOutcomeProficiencyInput`, `DeleteSubmissionCommentInput`, `DeleteSubmissionDraftInput`, `DeleteUserInboxLabelInput`, `DiscussionEntryConnection`, `DiscussionEntryEdge`, `DiscussionEntryPayload`, `DiscussionTopicConnection`, `DiscussionTopicEdge`, `DiscussionTopicPayload`, `DismissAccountNotificationInput`, `DraftSubmissionCommentPayload`, `EnrollmentConnection`, `EnrollmentEdge`, `EnrollmentInvitationPayload`, `ExternalToolConnection`, `ExternalToolEdge`, `FolderConnection`, `FolderEdge`, `FriendlyDescriptionPayload`, `G1Error`, `G1SuccessPayload`, `G2Error`, `G2SuccessPayload`, `G3Error`, `G3SuccessPayload`, `GradebookFilterPayload`, `GroupConnection`, `GroupEdge`, `GroupMembershipConnection`, `GroupMembershipEdge`, `GroupPayload`, `GroupSetConnection`, `GroupSetEdge`, `GroupSetPayload`, `HideAssignmentGradesForSectionsInput`, `HideAssignmentGradesInput`, `ImportOutcomesInput`, `ImportOutcomesPayload`, `InboxSettingsPayload`, `InstitutionalTagAssociationPayload`, `InstitutionalTagCategoryPayload`, `InstitutionalTagPayload`, `InternalSettingPayload`, `LearnerDashboardTabSelectionPayload`, `LearningOutcomeConnection`, `LearningOutcomeEdge`, `LearningOutcomeGroupPayload`, `LearningOutcomePayload`, `MarkSubmissionCommentsReadInput`, `ModuleConnection`, `ModuleEdge`, `ModuleItemCompletionPayload`, `ModuleItemConnection`, `ModuleItemEdge`, `ModulePayload`, `MoveOutcomeLinksInput`, `NotificationPreferencePayload`, `OutcomeCalculationMethodPayload`, `OutcomeProficiencyPayload`, `OverrideScorePayload`, `OverrideStatusPayload`, `PageConnection`, `PageEdge`, `PageInfo`, `PostAssignmentGradesForSectionsInput`, `PostAssignmentGradesInput`, `PostAssignmentGradesPayload`, `PostDraftSubmissionCommentInput`, `QuizConnection`, `QuizEdge`, `QuizQuestionConnection`, `QuizQuestionEdge`, `QuizSubmissionConnection`, `QuizSubmissionEdge`, `RejectEnrollmentInvitationInput`, `RemoveInstitutionalTagInput`, `ReorderModuleItemsInput`, `RestoreDeletedDiscussionEntryInput`, `RubricAssessmentPayload`, `RubricConnection`, `RubricEdge`, `RubricSelfAssessmentPayload`, `SaveRubricAssessmentInput`, `SelectProvisionalGradeInput`, `SelectProvisionalGradePayload`, `SetAssignmentPostPolicyInput`, `SetCoursePostPolicyInput`, `SetFriendlyDescriptionInput`, `SetModuleItemCompletionInput`, `SetOverrideScoreInput`, `SetOverrideStatusInput`, `SetRubricSelfAssessmentInput`, `SpeedGraderSettingsPayload`, `SplitscreenPreferencePayload`, `StandardGradeStatusPayload`, `StringMapEntry`, `StudentEnteredScorePayload`, `SubmissionCommentPayload`, `SubmissionConnection`, `SubmissionDraftPayload`, `SubmissionEdge`, `SubmissionGradeStatusPayload`, `SubmissionPayload`, `SubmissionStickerPayload`, `SubmitAutoGradeFeedbackInput`, `SubscribeToDiscussionTopicInput`, `SuccessPayload`, `UpdateAllocationRuleInput`, `UpdateAssignmentPayload`, `UpdateCommentBankItemInput`, `UpdateConversationParticipantsInput`, `UpdateDiscussionEntriesReadStateInput`, `UpdateDiscussionEntryInput`, `UpdateDiscussionEntryParticipantInput`, `UpdateDiscussionEntryPayload`, `UpdateDiscussionReadStateInput`, `UpdateDiscussionThreadReadStateInput`, `UpdateDiscussionTopicInput`, `UpdateDiscussionTopicParticipantInput`, `UpdateDiscussionTopicPayload`, `UpdateGradebookGroupFilterInput`, `UpdateInstitutionalTagArchivedStateInput`, `UpdateInstitutionalTagCategoryArchivedStateInput`, `UpdateInstitutionalTagCategoryInput`, `UpdateInstitutionalTagInput`, `UpdateInternalSettingInput`, `UpdateLearnerDashboardTabSelectionInput`, `UpdateLearningOutcomeGroupInput`, `UpdateLearningOutcomeInput`, `UpdateMyInboxSettingsInput`, `UpdateNotificationPreferencesInput`, `UpdateOutcomeCalculationMethodInput`, `UpdateOutcomeProficiencyInput`, `UpdateRubricArchivedStateInput`, `UpdateRubricAssessmentReadStateInput`, `UpdateSpeedGraderSettingsInput`, `UpdateSplitScreenViewDeeplyNestedAlertInput`, `UpdateSubmissionGradeInput`, `UpdateSubmissionGradeStatusInput`, `UpdateSubmissionStickerInput`, `UpdateSubmissionStudentEnteredScoreInput`, `UpdateSubmissionsReadStateInput`, `UpdateUserDiscussionsSplitscreenViewInput`, `UpdateWidgetDashboardConfigInput`, `UpdateWidgetDashboardLayoutInput`, `UpdatedDiscussionEntry`, `UpdatedDiscussionTopic`, `UpsertCustomGradeStatusInput`, `UpsertStandardGradeStatusInput`, `UserConnection`, `UserEdge`, `UserInboxLabelPayload`, `WidgetDashboardPayload`, `deleteConversation`, `deleteLearningOutcome`, `deleteLearningOutcomeGroup`.

## Star routes with no Canvas API match (208)

Classified, because the fix differs by class: `api_verb_mismatch` is a one-line verb change, `web_*` means Canvas answers that path as a **web** (HTML/JS) route rather than an API route — a port, not a REST parity item — and `absent` means Canvas has nothing like it.

| Class | Count | Action |
|---|---:|---|
| `api_verb_mismatch` | 18 | change the registered verb to Canvas' |
| `web_exact` | 43 | port of a Canvas web route: keep, label as web/UI |
| `web_verb_mismatch` | 16 | web port with the wrong verb |
| `web_nested` | 26 | web route flattened to top level: re-nest or label |
| `api_nested` | 19 | API route missing its parent segment: re-nest |
| `absent` | 86 | delete, or document as an intentional custom endpoint |

| Verb | Path | Crate | Class | Canvas closest |
|---|---|---|---|---|
| GET | `/api/lti/accounts/{accountId}/tool_consumer_profile/{toolConsumerProfileId}` | `star-lti` | absent | - |
| GET | `/api/lti/courses/{courseId}/tool_consumer_profile/{toolConsumerProfileId}` | `star-lti` | absent | - |
| GET | `/api/lti/{context}/{contextId}/tool_consumer_profile` | `star-lti` | absent | - |
| GET | `/api/sis/{context}/{contextId}/assignments` | `star-auth` | absent | - |
| DELETE | `/assignment_overrides/{id}` | `star-assignments` | absent | - |
| GET | `/assignment_overrides/{id}` | `star-assignments` | absent | - |
| PUT | `/assignment_overrides/{id}` | `star-assignments` | absent | - |
| GET | `/assignments/{id}/submissions/me` | `star-assignments` | absent | - |
| POST | `/auth/login` | `star-auth` | absent | - |
| POST | `/auth/logout` | `star-auth` | absent | - |
| GET | `/auth/me` | `star-auth` | absent | - |
| POST | `/auth/register` | `star-auth` | absent | - |
| GET | `/conversations/compose` | `star-ui` | absent | - |
| POST | `/conversations/mark_all_read` | `star-ui` | absent | - |
| GET | `/conversations/{id}/participants` | `star-conversations` | absent | - |
| PUT | `/conversations/{id}/read` | `star-conversations` | absent | - |
| GET | `/courses/{courseId}/announcements/unread_count` | `star-announcements` | absent | - |
| DELETE | `/courses/{courseId}/announcements/{id}/read` | `star-announcements` | absent | - |
| PUT | `/courses/{courseId}/announcements/{id}/read` | `star-announcements` | absent | - |
| DELETE | `/courses/{courseId}/assignment_annotations/{id}/read` | `star-assignments` | absent | - |
| PUT | `/courses/{courseId}/assignment_annotations/{id}/read` | `star-assignments` | absent | - |
| GET | `/courses/{courseId}/assignment_groups/{id}/rules` | `star-assignment-groups` | absent | - |
| PUT | `/courses/{courseId}/assignment_groups/{id}/rules` | `star-assignment-groups` | absent | - |
| GET | `/courses/{id}/assignments/new` | `star-ui` | absent | - |
| POST | `/courses/{id}/assignments/new` | `star-ui` | absent | - |
| GET | `/courses/{id}/assignments/{aid}/edit` | `star-ui` | absent | - |
| POST | `/courses/{id}/assignments/{aid}/edit` | `star-ui` | absent | - |
| GET | `/courses/{courseId}/assignments/{id}/submissions/self` | `star-assignments` | absent | - |
| GET | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments` | `star-submission-comments` | absent | - |
| POST | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments` | `star-submission-comments` | absent | - |
| POST | `/courses/{id}/assignments/{aid}/submit` | `star-ui` | absent | - |
| GET | `/courses/{id}/discussions` | `star-ui` | absent | - |
| POST | `/courses/{id}/discussions` | `star-ui` | absent | - |
| GET | `/courses/{id}/discussions/new` | `star-ui` | absent | - |
| GET | `/courses/{id}/discussions/{topic_id}` | `star-ui` | absent | - |
| POST | `/courses/{id}/discussions/{topic_id}/entries` | `star-ui` | absent | - |
| GET | `/courses/{courseId}/final_grade` | `star-grading-standards` | absent | - |
| GET | `/courses/{courseId}/folders/root` | `star-files` | absent | - |
| GET | `/courses/{courseId}/gradebook` | `star-assignments` | absent | - |
| GET | `/courses/{id}/gradebook` | `star-ui` | absent | - |
| GET | `/courses/{id}/gradebook.csv` | `star-ui` | absent | - |
| GET | `/courses/{id}/gradebook/speedgrader` | `star-ui` | absent | - |
| POST | `/courses/{id}/gradebook/speedgrader/grade` | `star-ui` | absent | - |
| GET | `/courses/{courseId}/mastery_paths/stats/{stat}` | `star-modules` | absent | - |
| POST | `/courses/{id}/quizzes/{qid}/submissions/{sid}/submit` | `star-ui` | absent | - |
| GET | `/courses/{courseId}/role_overrides` | `star-permissions` | absent | - |
| PUT | `/courses/{courseId}/role_overrides` | `star-permissions` | absent | - |
| DELETE | `/courses/{courseId}/rubric_assessments/{id}/read` | `star-assignments` | absent | - |
| PUT | `/courses/{courseId}/rubric_assessments/{id}/read` | `star-assignments` | absent | - |
| PUT | `/courses/{courseId}/submissions/{submissionId}/read/{item}` | `star-assignments` | absent | - |
| GET | `/courses/{id}/users/add` | `star-ui` | absent | - |
| POST | `/courses/{id}/users/add` | `star-ui` | absent | - |
| POST | `/courses/{id}/users/{user_id}/conclude` | `star-ui` | absent | - |
| POST | `/courses/{id}/users/{user_id}/reactivate` | `star-ui` | absent | - |
| GET | `/health` | `star-http` | absent | - |
| POST | `/jobs/{id}/cancel` | `star-jobs` | absent | - |
| GET | `/jobs2` | `star-jobs` | absent | - |
| GET | `/notifications/unread_count` | `star-notifications` | absent | - |
| DELETE | `/notifications/{id}` | `star-notifications` | absent | - |
| PUT | `/notifications/{id}/read` | `star-notifications` | absent | - |
| POST | `/quizzes/{id}/start` | `star-quizzes` | absent | - |
| GET | `/quizzes/{id}/submissions/me` | `star-quizzes` | absent | - |
| GET | `/reset_password/{token}` | `star-ui` | absent | - |
| POST | `/reset_password/{token}` | `star-ui` | absent | - |
| DELETE | `/sections/{sectionId}/assignment_annotations/{id}/read` | `star-assignments` | absent | - |
| PUT | `/sections/{sectionId}/assignment_annotations/{id}/read` | `star-assignments` | absent | - |
| DELETE | `/sections/{sectionId}/rubric_assessments/{id}/read` | `star-assignments` | absent | - |
| PUT | `/sections/{sectionId}/rubric_assessments/{id}/read` | `star-assignments` | absent | - |
| GET | `/static/{hash}/{file}` | `star-ui` | absent | - |
| PUT | `/submissions/{submissionId}/comments/read` | `star-submission-comments` | absent | - |
| GET | `/submissions/{submissionId}/comments/unread_count` | `star-submission-comments` | absent | - |
| PUT | `/submissions/{id}/grade` | `star-assignments` | absent | - |
| GET | `/users/self` | `star-users` | absent | - |
| GET | `/users/self/communication_channels` | `star-notifications` | absent | - |
| POST | `/users/self/communication_channels` | `star-notifications` | absent | - |
| DELETE | `/users/self/communication_channels/{id}` | `star-notifications` | absent | - |
| GET | `/users/self/notification_preferences` | `star-notifications` | absent | - |
| PUT | `/users/self/notification_preferences` | `star-notifications` | absent | - |
| GET | `/users/self/notifications` | `star-notifications` | absent | - |
| GET | `/users/self/profile` | `star-users` | absent | - |
| PUT | `/users/self/profile` | `star-users` | absent | - |
| GET | `/users/{userId}/folders/media` | `star-files` | absent | - |
| GET | `/users/{user_id}/notification_preference_categories` | `star-notifications` | absent | - |
| PUT | `/users/{user_id}/notification_preference_categories/{category}` | `star-notifications` | absent | - |
| GET | `/users/{user_id}/notification_preferences` | `star-notifications` | absent | - |
| PUT | `/users/{user_id}/notification_preferences` | `star-notifications` | absent | - |
| GET | `/content_migrations/migrators` | `star-content` | api_nested | `accounts/{}/content_migrations/migrators` |
| GET | `/content_migrations/{id}` | `star-content` | api_nested | `accounts/{}/content_migrations/{}` |
| GET | `/enrollments` | `star-enrollments` | api_nested | `sections/{}/enrollments` |
| POST | `/enrollments` | `star-enrollments` | api_nested | `sections/{}/enrollments` |
| GET | `/files/quota` | `star-files` | api_nested | `courses/{}/files/quota` |
| DELETE | `/grading_periods/{id}` | `star-grading-standards` | api_nested | `accounts/{}/grading_periods/{}` |
| GET | `/grading_periods/{id}` | `star-grading-standards` | api_nested | `accounts/{}/grading_periods/{}` |
| PUT | `/grading_periods/{id}` | `star-grading-standards` | api_nested | `accounts/{}/grading_periods/{}` |
| DELETE | `/lti_registrations/{id}` | `star-lti` | api_nested | `accounts/{}/lti_registrations/{}` |
| GET | `/lti_registrations/{id}` | `star-lti` | api_nested | `accounts/{}/lti_registrations/{}` |
| GET | `/lti_registrations/{id}/deployments` | `star-lti` | api_nested | `accounts/{}/lti_registrations/{}/deployments` |
| POST | `/lti_registrations/{id}/deployments` | `star-lti` | api_nested | `accounts/{}/lti_registrations/{}/deployments` |
| DELETE | `/modules/{id}/items/{itemId}` | `star-modules` | api_nested | `courses/{}/modules/{}/items/{}` |
| GET | `/outcome_groups/{id}/outcomes` | `star-outcomes` | api_nested | `accounts/{}/outcome_groups/{}/outcomes` |
| POST | `/outcome_groups/{id}/outcomes` | `star-outcomes` | api_nested | `accounts/{}/outcome_groups/{}/outcomes` |
| GET | `/rubric_assessments/{id}` | `star-rubrics` | api_nested | `courses/{}/rubric_associations/{}/rubric_assessments/{}` |
| DELETE | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` | api_nested | `courses/{}/assignments/{}/submissions/{}/comments/{}` |
| GET | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` | api_nested | `courses/{}/assignments/{}/submissions/{}/comments/{}` |
| PUT | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` | api_nested | `courses/{}/assignments/{}/submissions/{}/comments/{}` |
| POST | `/accounts/{accountId}/grading_periods` | `star-grading-standards` | api_verb_mismatch | `GET` |
| PUT | `/accounts/{id}/settings` | `star-accounts` | api_verb_mismatch | `GET` |
| POST | `/accounts/{id}/users/{userId}` | `star-accounts` | api_verb_mismatch | `DELETE` |
| POST | `/courses` | `star-courses` | api_verb_mismatch | `GET` |
| GET | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}` | `star-submission-comments` | api_verb_mismatch | `DELETE`, `PUT` |
| GET | `/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}` | `star-discussions` | api_verb_mismatch | `DELETE`, `PUT` |
| POST | `/courses/{courseId}/groups` | `star-groups` | api_verb_mismatch | `GET` |
| POST | `/courses/{courseId}/outcome_groups` | `star-outcomes` | api_verb_mismatch | `GET` |
| POST | `/courses/{id}/settings` | `star-ui` | api_verb_mismatch | `GET`, `PUT` |
| GET | `/developer_keys/{id}` | `star-auth` | api_verb_mismatch | `DELETE`, `PUT` |
| GET | `/groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}` | `star-discussions` | api_verb_mismatch | `DELETE`, `PUT` |
| DELETE | `/groups/{groupId}/files/{id}` | `star-files` | api_verb_mismatch | `GET` |
| POST | `/groups/{groupId}/pages/{urlOrId}` | `star-pages` | api_verb_mismatch | `DELETE`, `GET`, `PUT` |
| POST | `/groups/{groupId}/users` | `star-groups` | api_verb_mismatch | `DELETE`, `GET` |
| GET | `/users/self/communication_channels/{communication_channel_id}/notification_preferences` | `star-notifications` | api_verb_mismatch | `PUT` |
| GET | `/users/self/communication_channels/{communication_channel_id}/notification_preferences/{notification}` | `star-notifications` | api_verb_mismatch | `PUT` |
| PUT | `/users/{user_id}/communication_channels/{type}/{address}/notification_preferences` | `star-notifications` | api_verb_mismatch | `GET` |
| PUT | `/users/{user_id}/communication_channels/{type}/{address}/notification_preferences/{notification}` | `star-notifications` | api_verb_mismatch | `GET` |
| GET | `/` | `star-graphql` | web_exact | `WEB GET` |
| GET | `/` | `star-ui` | web_exact | `WEB GET` |
| DELETE | `/account_notifications/{id}` | `star-accounts` | web_exact | `WEB DELETE` |
| POST | `/accounts/{account_id}/deep_linking_response` | `star-lti` | web_exact | `WEB POST` |
| GET | `/accounts/{accountId}/grading_schemes` | `star-grading-standards` | web_exact | `WEB GET` |
| POST | `/accounts/{accountId}/grading_schemes` | `star-grading-standards` | web_exact | `WEB POST` |
| GET | `/accounts/{accountId}/role_overrides` | `star-permissions` | web_exact | `WEB GET` |
| DELETE | `/announcements/{id}` | `star-announcements` | web_exact | `WEB DELETE` |
| GET | `/announcements/{id}` | `star-announcements` | web_exact | `WEB GET` |
| PUT | `/announcements/{id}` | `star-announcements` | web_exact | `WEB PUT` |
| GET | `/assignments/{id}` | `star-assignments` | web_exact | `WEB GET` |
| GET | `/courses/{courseId}/announcements` | `star-announcements` | web_exact | `WEB GET` |
| POST | `/courses/{courseId}/announcements` | `star-announcements` | web_exact | `WEB POST` |
| GET | `/courses/{courseId}/calendar_events` | `star-calendar` | web_exact | `WEB GET` |
| POST | `/courses/{courseId}/calendar_events` | `star-calendar` | web_exact | `WEB POST` |
| POST | `/courses/{id}/deep_linking_response` | `star-lti` | web_exact | `WEB POST` |
| GET | `/courses/{course_id}/external_tools/{id}/eula_launch` | `star-lti` | web_exact | `WEB GET` |
| GET | `/courses/{id}/grades` | `star-ui` | web_exact | `WEB GET` |
| DELETE | `/courses/{courseId}/modules/items/{itemId}` | `star-modules` | web_exact | `WEB DELETE` |
| GET | `/courses/{courseId}/modules/items/{itemId}` | `star-modules` | web_exact | `WEB GET` |
| PUT | `/courses/{courseId}/modules/items/{itemId}` | `star-modules` | web_exact | `WEB PUT` |
| GET | `/courses/{courseId}/outcomes` | `star-outcomes` | web_exact | `WEB GET` |
| POST | `/courses/{courseId}/outcomes` | `star-outcomes` | web_exact | `WEB POST` |
| POST | `/courses/{id}/quizzes/new` | `star-ui` | web_exact | `WEB POST` |
| GET | `/dashboard` | `star-dashboard` | web_exact | `WEB GET` |
| GET | `/dashboard` | `star-ui` | web_exact | `WEB GET` |
| GET | `/deep_linking_cancel` | `star-lti` | web_exact | `WEB GET` |
| GET | `/folders` | `star-files` | web_exact | `WEB GET` |
| POST | `/folders` | `star-files` | web_exact | `WEB POST` |
| POST | `/groups/{group_id}/deep_linking_response` | `star-lti` | web_exact | `WEB POST` |
| GET | `/groups/{group_id}/external_tools/{id}` | `star-lti` | web_exact | `WEB GET` |
| GET | `/login` | `star-ui` | web_exact | `WEB GET` |
| GET | `/login/oauth2/auth` | `star-auth` | web_exact | `WEB GET` |
| GET | `/login/oauth2/jwks` | `star-auth` | web_exact | `WEB GET` |
| DELETE | `/login/oauth2/token` | `star-auth` | web_exact | `WEB DELETE` |
| POST | `/login/oauth2/token` | `star-auth` | web_exact | `WEB POST` |
| GET | `/pages/{id}` | `star-pages` | web_exact | `WEB GET` |
| GET | `/register` | `star-ui` | web_exact | `WEB GET` |
| DELETE | `/rubrics/{id}` | `star-rubrics` | web_exact | `WEB DELETE` |
| GET | `/rubrics/{id}` | `star-rubrics` | web_exact | `WEB GET` |
| PUT | `/rubrics/{id}` | `star-rubrics` | web_exact | `WEB PUT` |
| GET | `/submissions/{submissionId}/comments` | `star-submission-comments` | web_exact | `WEB GET` |
| GET | `/users` | `star-users` | web_exact | `WEB GET` |
| GET | `/assignments/{id}/submissions` | `star-assignments` | web_nested | `courses/{}/assignments/{}/submissions` |
| POST | `/assignments/{id}/submissions` | `star-assignments` | web_nested | `courses/{}/assignments/{}/submissions` |
| GET | `/content_exports/{id}` | `star-content` | web_nested | `courses/{}/content_exports/{}` |
| GET | `/courses/{id}/{tab}` | `star-ui` | web_nested | `career/courses/{}/{}` |
| GET | `/developer_keys` | `star-auth` | web_nested | `accounts/{}/developer_keys` |
| POST | `/developer_keys` | `star-auth` | web_nested | `accounts/{}/developer_keys` |
| DELETE | `/grading_schemes/{id}` | `star-grading-standards` | web_nested | `accounts/{}/grading_schemes/{}` |
| GET | `/grading_schemes/{id}` | `star-grading-standards` | web_nested | `accounts/{}/grading_schemes/{}` |
| PUT | `/grading_schemes/{id}` | `star-grading-standards` | web_nested | `accounts/{}/grading_schemes/{}` |
| DELETE | `/grading_standards/{id}` | `star-grading-standards` | web_nested | `accounts/{}/grading_standards/{}` |
| GET | `/grading_standards/{id}` | `star-grading-standards` | web_nested | `accounts/{}/grading_standards/{}` |
| DELETE | `/modules/{id}` | `star-modules` | web_nested | `courses/{}/modules/{}` |
| GET | `/modules/{id}` | `star-modules` | web_nested | `courses/{}/modules/{}` |
| PUT | `/modules/{id}` | `star-modules` | web_nested | `courses/{}/modules/{}` |
| POST | `/modules/{id}/items` | `star-modules` | web_nested | `courses/{}/modules/{}/items` |
| DELETE | `/outcome_groups/{id}` | `star-outcomes` | web_nested | `accounts/{}/outcome_groups/{}` |
| GET | `/outcome_groups/{id}` | `star-outcomes` | web_nested | `accounts/{}/outcome_groups/{}` |
| PUT | `/outcome_groups/{id}` | `star-outcomes` | web_nested | `accounts/{}/outcome_groups/{}` |
| GET | `/outcomes/{outcomeId}/results` | `star-outcomes` | web_nested | `accounts/{}/outcomes/{}/results` |
| POST | `/outcomes/{outcomeId}/results` | `star-outcomes` | web_nested | `accounts/{}/outcomes/{}/results` |
| DELETE | `/quizzes/{id}` | `star-quizzes` | web_nested | `courses/{}/quizzes/{}` |
| GET | `/quizzes/{id}` | `star-quizzes` | web_nested | `courses/{}/quizzes/{}` |
| PUT | `/quizzes/{id}` | `star-quizzes` | web_nested | `courses/{}/quizzes/{}` |
| DELETE | `/terms/{id}` | `star-courses` | web_nested | `accounts/{}/terms/{}` |
| GET | `/terms/{id}` | `star-courses` | web_nested | `accounts/{}/terms/{}` |
| PUT | `/terms/{id}` | `star-courses` | web_nested | `accounts/{}/terms/{}` |
| POST | `/` | `star-graphql` | web_verb_mismatch | `GET` |
| PUT | `/accounts/{accountId}/role_overrides` | `star-permissions` | web_verb_mismatch | `GET`, `POST` |
| DELETE | `/assignments/{id}` | `star-assignments` | web_verb_mismatch | `GET` |
| PUT | `/assignments/{id}` | `star-assignments` | web_verb_mismatch | `GET` |
| GET | `/courses/{id}/quizzes/new` | `star-ui` | web_verb_mismatch | `POST` |
| GET | `/courses/{id}/quizzes/{qid}/take` | `star-ui` | web_verb_mismatch | `MATCH` |
| POST | `/courses/{id}/quizzes/{qid}/take` | `star-ui` | web_verb_mismatch | `MATCH` |
| DELETE | `/enrollments/{id}` | `star-enrollments` | web_verb_mismatch | `GET` |
| GET | `/forgot_password` | `star-ui` | web_verb_mismatch | `MATCH` |
| POST | `/forgot_password` | `star-ui` | web_verb_mismatch | `MATCH` |
| POST | `/login` | `star-ui` | web_verb_mismatch | `GET` |
| POST | `/logout` | `star-ui` | web_verb_mismatch | `DELETE`, `GET` |
| DELETE | `/pages/{id}` | `star-pages` | web_verb_mismatch | `GET` |
| PUT | `/pages/{id}` | `star-pages` | web_verb_mismatch | `GET` |
| POST | `/register` | `star-ui` | web_verb_mismatch | `GET` |
| POST | `/submissions/{submissionId}/comments` | `star-submission-comments` | web_verb_mismatch | `GET` |

## Parser scope and limitations

- Canvas routes are verb declarations inside `ApiRouteSet.draw` blocks, plus Rails `resources` declarations (API defaults: index/create/show/update/destroy, honoring `only:`, `except:`, `path:`, `controller:`, and nesting) and `%w[...].each` context loops, which are expanded for every value.
- Conditional context prefixes assigned from `var == "literal" ? var : "..."` are evaluated per loop binding, and `if`/`unless var == "literal"` guards filter bindings; this expands the `outcome_groups_api` routes to `courses/...`, `accounts/...`, and `global/...`. Other runtime interpolations are left as `{}` and cannot match concrete Rust paths.
- UI references are `/api/v1/...` string literals in `ui/`; runtime-built paths and hard-coded ids that cannot be normalized are missed.
- GraphQL type names come from `graphql_name` declarations (or the class name minus a trailing `Type`) in `app/graphql/types/*_type.rb`, and mutation operations from classes whose superclass chain reaches `Mutations::BaseMutation` in `app/graphql/mutations/**`; the denominators remain the file counts (170 type files, 110 mutation files). star-graphql names are extracted from `#[Object]` impls and async-graphql derives, with documented renames (CourseSection->Section, EnrollmentTerm->Term, DiscussionTopic->Discussion, QuizQuestion->QuizItem) mapped to their Canvas graphql_names before matching. Connection and edge helpers (e.g. `PageInfo`, `*Edge`, `*Connection`) only count when Canvas defines a type of the same name.
- Rust paths are matched by `(verb, normalized path)`; axum's `nest("/api/v1", ...)` mount is assumed for all crate routers.
- Extras include deliberate flat-path deviations (for example `/assignments/{id}` where Canvas uses `/courses/{course_id}/assignments/{id}`) and auth/dashboard/health routes Canvas does not expose.
- The schema row counts `CREATE TABLE` statements in `star-db` migrations (36 authored tables); sqlx adds `_sqlx_migrations`, so a live database has 37 tables.
