# Canvas to Rust parity scoreboard

Measured, deterministic output. Canvas source is not committed; refresh the manifest locally with `tools/parity/parity.py manifest`.

## Summary

| Area | Implemented | Total | Coverage |
|---|---:|---:|---:|
| REST API routes | 1002 | 1403 | 71.4% |
| REST routes referenced from `ui/` | 361 | 503 | 71.8% |
| GraphQL files | 1 | 390 | 0.3% |
| GraphQL object types | 28 | 170 | 16.5% |
| GraphQL mutations | 2 | 110 | 1.8% |
| UI feature directories | 0 | 232 | 0.0% |
| JS/JSX/TS/TSX files | 0 | 5459 | 0.0% |
| ERB views | 0 | 475 | 0.0% |
| js_env files | 0 | 124 | 0.0% |
| Schema tables (vs 688 Canvas models) | 370 | 688 | n/a |

Implemented REST route count: **1002 / 1403** (71.4%).

Star registrations: 1212 route/method pairs from 1003 `.route(...)` calls (0 unparsed); 208 have no Canvas match and are listed as extras below.

Canvas source sizes: 390 GraphQL files (170 object types, 110 mutations), 688 models, 232 UI feature dirs, 5459 JS/TSX files, 475 ERB views, 124 js_env files (490 call sites).

## Controllers with coverage (115)

| Controller | Implemented | Total | Coverage |
|---|---:|---:|---:|
| `discussion_topics_api` | 58 | 58 | 100.0% |
| `submissions_api` | 44 | 44 | 100.0% |
| `users` | 39 | 39 | 100.0% |
| `outcome_groups_api` | 38 | 40 | 95.0% |
| `courses` | 36 | 38 | 94.7% |
| `folders` | 27 | 34 | 79.4% |
| `content_migrations` | 25 | 25 | 100.0% |
| `external_tools` | 25 | 25 | 100.0% |
| `wiki_pages_api` | 25 | 29 | 86.2% |
| `files` | 23 | 26 | 88.5% |
| `accounts` | 22 | 22 | 100.0% |
| `lti/registrations` | 20 | 20 | 100.0% |
| `feature_flags` | 17 | 17 | 100.0% |
| `groups` | 17 | 17 | 100.0% |
| `conversations` | 16 | 16 | 100.0% |
| `group_categories` | 16 | 16 | 100.0% |
| `migration_issues` | 16 | 16 | 100.0% |
| `rubrics_api` | 15 | 15 | 100.0% |
| `content_exports_api` | 14 | 14 | 100.0% |
| `enrollments_api` | 14 | 14 | 100.0% |
| `blackout_dates` | 13 | 13 | 100.0% |
| `master_courses/master_templates` | 13 | 13 | 100.0% |
| `assignments_api` | 12 | 12 | 100.0% |
| `communication_channels` | 12 | 12 | 100.0% |
| `grade_change_audit_api` | 12 | 12 | 100.0% |
| `lti/ims/tool_setting` | 12 | 12 | 100.0% |
| `group_memberships` | 11 | 11 | 100.0% |
| `lti/ims/dynamic_registration` | 11 | 11 | 100.0% |
| `assignment_overrides` | 10 | 10 | 100.0% |
| `calendar_events_api` | 10 | 15 | 66.7% |
| `discussion_topics` | 10 | 10 | 100.0% |
| `media_objects` | 10 | 10 | 100.0% |
| `peer_reviews_api` | 9 | 9 | 100.0% |
| `polling/poll_sessions` | 9 | 9 | 100.0% |
| `account_notifications` | 8 | 8 | 100.0% |
| `content_shares` | 8 | 8 | 100.0% |
| `context_module_items_api` | 8 | 12 | 66.7% |
| `csp_settings` | 8 | 8 | 100.0% |
| `grading_periods` | 8 | 8 | 100.0% |
| `learning_object_dates` | 8 | 15 | 53.3% |
| `lti/tool_proxy` | 8 | 8 | 100.0% |
| `role_overrides` | 8 | 8 | 100.0% |
| `authentication_providers` | 7 | 8 | 87.5% |
| `jobs_v2` | 7 | 12 | 58.3% |
| `notification_preferences` | 7 | 10 | 70.0% |
| `quizzes/quiz_submissions_api` | 7 | 7 | 100.0% |
| `quizzes/quizzes_api` | 7 | 7 | 100.0% |
| `support_helpers/turnitin` | 7 | 7 | 100.0% |
| `user_observees` | 7 | 7 | 100.0% |
| `appointment_groups` | 6 | 8 | 75.0% |
| `block_editor_templates_api` | 6 | 6 | 100.0% |
| `learn_platform` | 6 | 6 | 100.0% |
| `lti/context_controls` | 6 | 6 | 100.0% |
| `lti/resource_links` | 6 | 6 | 100.0% |
| `media_tracks` | 6 | 6 | 100.0% |
| `quizzes/quiz_groups` | 6 | 6 | 100.0% |
| `release_notes` | 6 | 8 | 75.0% |
| `sections` | 6 | 9 | 66.7% |
| `context_modules_api` | 5 | 8 | 62.5% |
| `lti/data_services` | 5 | 5 | 100.0% |
| `lti/deployments` | 5 | 5 | 100.0% |
| `lti/ims/line_items` | 5 | 5 | 100.0% |
| `lti/ims/tool_proxy` | 5 | 5 | 100.0% |
| `planner_notes` | 5 | 5 | 100.0% |
| `planner_overrides` | 5 | 5 | 100.0% |
| `polling/poll_choices` | 5 | 5 | 100.0% |
| `polling/polls` | 5 | 5 | 100.0% |
| `quizzes/quiz_questions` | 5 | 5 | 100.0% |
| `tabs` | 5 | 5 | 100.0% |
| `tokens` | 5 | 5 | 100.0% |
| `usage_rights` | 5 | 9 | 55.6% |
| `admins` | 4 | 4 | 100.0% |
| `assignment_groups_api` | 4 | 5 | 80.0% |
| `career_experience` | 4 | 4 | 100.0% |
| `discussion_entries` | 4 | 4 | 100.0% |
| `favorites` | 4 | 8 | 50.0% |
| `gradebook_history_api` | 4 | 4 | 100.0% |
| `grading_standards_api` | 4 | 10 | 40.0% |
| `lti/account_external_tools` | 4 | 4 | 100.0% |
| `lti/lti_apps` | 4 | 4 | 100.0% |
| `provisional_grades` | 4 | 4 | 100.0% |
| `quizzes/quiz_reports` | 4 | 4 | 100.0% |
| `submission_comments_api` | 4 | 4 | 100.0% |
| `epub_exports` | 3 | 3 | 100.0% |
| `jobs` | 3 | 3 | 100.0% |
| `lti/ims/authentication` | 3 | 3 | 100.0% |
| `rubric_assessments` | 3 | 3 | 100.0% |
| `rubric_associations` | 3 | 3 | 100.0% |
| `rubrics` | 3 | 3 | 100.0% |
| `shared_brand_configs` | 3 | 3 | 100.0% |
| `terms` | 3 | 3 | 100.0% |
| `canvas_career/user_experiences` | 2 | 2 | 100.0% |
| `content_imports` | 2 | 2 | 100.0% |
| `developer_keys` | 2 | 5 | 40.0% |
| `discussion_topic_users` | 2 | 2 | 100.0% |
| `lti/ims/authorization` | 2 | 2 | 100.0% |
| `lti/ims/names_and_roles` | 2 | 2 | 100.0% |
| `lti/ims/results` | 2 | 2 | 100.0% |
| `moderation_set` | 2 | 2 | 100.0% |
| `polling/poll_submissions` | 2 | 2 | 100.0% |
| `sub_accounts` | 2 | 2 | 100.0% |
| `submissions` | 2 | 2 | 100.0% |
| `terms_api` | 2 | 2 | 100.0% |
| `web_zip_exports` | 2 | 2 | 100.0% |
| `anonymous_provisional_grades` | 1 | 1 | 100.0% |
| `assignment_extensions` | 1 | 1 | 100.0% |
| `assignment_groups` | 1 | 1 | 100.0% |
| `assignments` | 1 | 1 | 100.0% |
| `errors` | 1 | 1 | 100.0% |
| `inst_access_tokens` | 1 | 1 | 100.0% |
| `lti/account_lookup` | 1 | 1 | 100.0% |
| `lti/ims/scores` | 1 | 1 | 100.0% |
| `profile` | 1 | 2 | 50.0% |
| `quizzes/quiz_statistics` | 1 | 1 | 100.0% |
| `search` | 1 | 4 | 25.0% |

## GraphQL operation coverage

Names are matched exactly: star-graphql type names (mapped through the documented renames below) against Canvas `graphql_name`s from `app/graphql/types/*_type.rb`, and mutation field names against the operations of `app/graphql/mutations/**` (classes whose superclass chain reaches `Mutations::BaseMutation`).

### Implemented types (28 of 170 Canvas names)

`Account`, `Assignment`, `Conversation`, `ConversationMessage`, `ConversationParticipant`, `Course`, `CoursePermissions`, `Discussion`, `Enrollment`, `File`, `Folder`, `Group`, `GroupMembership`, `GroupSet`, `Module`, `ModuleItem`, `Mutation`, `Page`, `Progress`, `Query`, `Quiz`, `QuizItem`, `QuizSubmission`, `Section`, `Submission`, `Term`, `User`, `ValidationError`

Renames mapped to Canvas names: `CourseSection` -> `Section`, `DiscussionTopic` -> `Discussion`, `EnrollmentTerm` -> `Term`, `QuizQuestion` -> `QuizItem`.

### Missing types (218)

`AIGradeCriterionResult`, `AIGradeRating`, `AIGradeResult`, `AccountNotification`, `AccountUsersFilter`, `AccountUsersSortField`, `AccountUsersSortInput`, `ActivityStream`, `AdhocStudents`, `AllocationRule`, `AllocationRulesFilterInput`, `AllowProvisionalGradingType`, `AnonymousStudentIdentity`, `AnonymousUser`, `AssessmentRequest`, `AssessmentType`, `AssignedStudentsFilter`, `AssignmentAllocationRules`, `AssignmentGroup`, `AssignmentGroupRules`, `AssignmentGroupState`, `AssignmentOverride`, `AssignmentOverrideSet`, `AssignmentRubricAssessment`, `AssignmentScoreStatistic`, `AssignmentState`, `AssignmentTargetSortField`, `AssignmentTargetSortOrder`, `AuditEvent`, `AuditEventExternalTool`, `AuditEventQuiz`, `AuditEventRole`, `AuditEventType`, `AuditEventUser`, `AuditLogs`, `AutoGradeEligibility`, `AutoGradeIssue`, `AutoLeaderPolicy`, `Checkpoint`, `CommentBankItem`, `CommunicationChannel`, `ContentTag`, `ContentTagContent`, `CourseDashboardCard`, `CourseDashboardCardLink`, `CourseFilterableEnrollmentState`, `CourseFilterableEnrollmentType`, `CourseGradeStatus`, `CourseOutcomeAlignmentStats`, `CourseProgression`, `CourseRequirements`, `CourseSectionsFilter`, `CourseSettings`, `CourseUsersFilter`, `CourseUsersSortDirectionType`, `CourseUsersSortFieldType`, `CourseUsersSortInputType`, `CourseWorkSubmissionsOrderField`, `CourseWorkflowState`, `CustomGradeStatus`, `DashboardObserveeFilter`, `DateHash`, `DateHashSet`, `DateTime`, `DateTimeRange`, `DiscussionEntry`, `DiscussionEntryCounts`, `DiscussionEntryPermissions`, `DiscussionEntryPinningType`, `DiscussionEntryReportTypeCounts`, `DiscussionEntryVersion`, `DiscussionFilterType`, `DiscussionParticipant`, `DiscussionParticipantFilter`, `DiscussionPermissions`, `DiscussionSortOrderType`, `DiscussionTopicAnonymousStateType`, `DiscussionTopicDiscussionType`, `DraftableSubmissionType`, `EligibilityIssue`, `EligibilityIssueLevel`, `EnrollmentFilterInput`, `EnrollmentRole`, `EnrollmentType`, `EnrollmentWorkflowState`, `EnrollmentsSortDirectionType`, `EnrollmentsSortFieldType`, `EnrollmentsSortInputType`, `EntryParticipant`, `ExternalTool`, `ExternalToolPlacement`, `ExternalToolPlacements`, `ExternalToolSettings`, `ExternalToolState`, `ExternalUrl`, `GradeState`, `GraderIdentity`, `Grades`, `GradesEnrollmentFilter`, `GradingPeriod`, `GradingPeriodGroup`, `GradingRole`, `GradingStandard`, `GradingStandardItem`, `GradingType`, `GroupMembershipState`, `GroupState`, `InboxSettings`, `InstitutionalTag`, `InstitutionalTagAssociation`, `InstitutionalTagCategory`, `InstitutionalTagUsersSortDirection`, `InstitutionalTagUsersSortField`, `InstitutionalTagUsersSortInput`, `InstitutionalTagWorkflowState`, `InstructorCourseInfo`, `InstructorEnrollmentInfo`, `InstructorUserInfo`, `InstructorWithEnrollments`, `InternalSetting`, `LearningOutcome`, `LearningOutcomeGroup`, `LockInfo`, `Lockable`, `LtiAsset`, `LtiAssetProcessor`, `LtiAssetProcessorIframe`, `LtiAssetProcessorWindowSettings`, `LtiAssetReport`, `MediaObject`, `MediaSource`, `MediaTrack`, `MediaTrackWorkflowState`, `MediaType`, `MessagePermissions`, `MessageableContext`, `MessageableUser`, `ModeratedGrading`, `ModuleCompletionRequirement`, `ModuleCompletionStatus`, `ModuleExternalTool`, `ModuleFilterInput`, `ModuleItemFilter`, `ModuleItemMasterCourseRestriction`, `ModulePrerequisite`, `ModuleProgression`, `ModuleProgressionStatistics`, `ModuleStatistics`, `MutationLog`, `NodeType`, `Noop`, `Notification`, `NotificationPolicy`, `NotificationPreferences`, `NotificationPreferencesContextType`, `OrderDirection`, `OutcomeAlignment`, `OutcomeCalculationMethod`, `OutcomeFriendlyDescriptionType`, `OutcomeProficiency`, `PageViewAnalysis`, `PeerReviewDates`, `PeerReviewStatus`, `PeerReviewSubAssignment`, `PeerReviews`, `PostPolicy`, `PreferredLanguageType`, `ProficiencyRating`, `ProficiencyRatingInput`, `ProgressContext`, `ProgressState`, `ProvisionalGrade`, `Recipients`, `Requirement`, `Rubric`, `RubricAssessment`, `RubricAssessmentRating`, `RubricAssociation`, `RubricCriterion`, `RubricRating`, `ScheduledPost`, `SelfSignupPolicy`, `SpeedGraderSettings`, `StandardGradeStatus`, `Sticker`, `StreamSummaryItem`, `StringMap`, `StudentSummaryAnalytics`, `SubAssignmentSubmission`, `SubHeader`, `SubmissionComment`, `SubmissionCommentFilterInput`, `SubmissionCommentStatusType`, `SubmissionDraft`, `SubmissionFilterInput`, `SubmissionGradingStatus`, `SubmissionHistory`, `SubmissionHistoryFilterInput`, `SubmissionHistoryOrder`, `SubmissionHistoryOrderField`, `SubmissionOrderCriteria`, `SubmissionOrderField`, `SubmissionPostingStatus`, `SubmissionSearchFilterInput`, `SubmissionSearchOrder`, `SubmissionSearchOrderField`, `SubmissionState`, `SubmissionStatistics`, `SubmissionStatusTagType`, `SubmissionType`, `TardinessBreakdown`, `TurnitinContext`, `TurnitinData`, `URL`, `UsageRights`, `UserGroupMembershipsFilterInput`, `VericiteContext`, `VericiteData`

### Implemented mutations (2 of 110 Canvas operations)

`createAssignment`, `updateAssignment`

### Missing mutations (98)

`acceptEnrollmentInvitation`, `addConversationMessage`, `applyInstitutionalTag`, `autoGradeSubmission`, `createAllocationRule`, `createCommentBankItem`, `createConversation`, `createDiscussionEntry`, `createDiscussionTopic`, `createGroupInSet`, `createGroupSet`, `createInstitutionalTag`, `createInstitutionalTagCategory`, `createInternalSetting`, `createLearningOutcome`, `createLearningOutcomeGroup`, `createModule`, `createOutcomeCalculationMethod`, `createOutcomeProficiency`, `createSubmission`, `createSubmissionComment`, `createSubmissionDraft`, `createUserInboxLabel`, `deleteAllocationRule`, `deleteCommentBankItem`, `deleteConversationMessages`, `deleteConversations`, `deleteCustomGradeStatus`, `deleteDiscussionEntry`, `deleteDiscussionTopic`, `deleteInternalSetting`, `deleteOutcomeCalculationMethod`, `deleteOutcomeLinks`, `deleteOutcomeProficiency`, `deleteSubmissionComment`, `deleteSubmissionDraft`, `deleteUserInboxLabel`, `dismissAccountNotification`, `hideAssignmentGrades`, `hideAssignmentGradesForSections`, `importOutcomes`, `markSubmissionCommentsRead`, `moveOutcomeLinks`, `postAssignmentGrades`, `postAssignmentGradesForSections`, `postDraftSubmissionComment`, `rejectEnrollmentInvitation`, `removeInstitutionalTag`, `reorderModuleItems`, `restoreDeletedDiscussionEntry`, `saveRubricAssessment`, `selectProvisionalGrade`, `setAssignmentPostPolicy`, `setCoursePostPolicy`, `setFriendlyDescription`, `setModuleItemCompletion`, `setOverrideScore`, `setOverrideStatus`, `setRubricSelfAssessment`, `submitAutoGradeFeedback`, `subscribeToDiscussionTopic`, `updateAllocationRule`, `updateCommentBankItem`, `updateConversationParticipants`, `updateDiscussionEntriesReadState`, `updateDiscussionEntry`, `updateDiscussionEntryParticipant`, `updateDiscussionReadState`, `updateDiscussionThreadReadState`, `updateDiscussionTopic`, `updateDiscussionTopicParticipant`, `updateGradebookGroupFilter`, `updateInstitutionalTag`, `updateInstitutionalTagArchivedState`, `updateInstitutionalTagCategory`, `updateInstitutionalTagCategoryArchivedState`, `updateInternalSetting`, `updateLearnerDashboardTabSelection`, `updateLearningOutcome`, `updateLearningOutcomeGroup`, `updateMyInboxSettings`, `updateNotificationPreferences`, `updateOutcomeCalculationMethod`, `updateOutcomeProficiency`, `updateRubricArchivedState`, `updateRubricAssessmentReadState`, `updateSpeedGraderSettings`, `updateSplitScreenViewDeeplyNestedAlert`, `updateSubmissionGrade`, `updateSubmissionGradeStatus`, `updateSubmissionSticker`, `updateSubmissionStudentEnteredScore`, `updateSubmissionsReadState`, `updateUserDiscussionsSplitscreenView`, `updateWidgetDashboardConfig`, `updateWidgetDashboardLayout`, `upsertCustomGradeStatus`, `upsertStandardGradeStatus`

Star names with no Canvas counterpart (not counted): `AssignmentConnection`, `AssignmentEdge`, `ConversationMessageConnection`, `ConversationMessageEdge`, `ConversationParticipantConnection`, `ConversationParticipantEdge`, `CreateAssignmentPayload`, `DiscussionTopicConnection`, `DiscussionTopicEdge`, `EnrollmentConnection`, `EnrollmentEdge`, `FolderConnection`, `FolderEdge`, `GroupConnection`, `GroupEdge`, `GroupMembershipConnection`, `GroupMembershipEdge`, `GroupSetConnection`, `GroupSetEdge`, `ModuleConnection`, `ModuleEdge`, `ModuleItemConnection`, `ModuleItemEdge`, `PageConnection`, `PageEdge`, `PageInfo`, `QuizConnection`, `QuizEdge`, `QuizQuestionConnection`, `QuizQuestionEdge`, `QuizSubmissionConnection`, `QuizSubmissionEdge`, `SubmissionConnection`, `SubmissionEdge`, `UpdateAssignmentPayload`, `UserConnection`, `UserEdge`.

## Star routes with no Canvas match (208)

| Verb | Path | Crate |
|---|---|---|
| GET | `/` | `star-graphql` |
| GET | `/` | `star-ui` |
| POST | `/` | `star-graphql` |
| DELETE | `/account_notifications/{id}` | `star-accounts` |
| POST | `/accounts/{account_id}/deep_linking_response` | `star-lti` |
| POST | `/accounts/{accountId}/grading_periods` | `star-grading-standards` |
| GET | `/accounts/{accountId}/grading_schemes` | `star-grading-standards` |
| POST | `/accounts/{accountId}/grading_schemes` | `star-grading-standards` |
| GET | `/accounts/{accountId}/role_overrides` | `star-permissions` |
| PUT | `/accounts/{accountId}/role_overrides` | `star-permissions` |
| PUT | `/accounts/{id}/settings` | `star-accounts` |
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
| GET | `/content_migrations/migrators` | `star-content` |
| GET | `/content_migrations/{id}` | `star-content` |
| GET | `/conversations/compose` | `star-ui` |
| POST | `/conversations/mark_all_read` | `star-ui` |
| GET | `/conversations/{id}/participants` | `star-conversations` |
| PUT | `/conversations/{id}/read` | `star-conversations` |
| POST | `/courses` | `star-courses` |
| GET | `/courses/{courseId}/announcements` | `star-announcements` |
| POST | `/courses/{courseId}/announcements` | `star-announcements` |
| GET | `/courses/{courseId}/announcements/unread_count` | `star-announcements` |
| DELETE | `/courses/{courseId}/announcements/{id}/read` | `star-announcements` |
| PUT | `/courses/{courseId}/announcements/{id}/read` | `star-announcements` |
| DELETE | `/courses/{courseId}/assignment_annotations/{id}/read` | `star-assignments` |
| PUT | `/courses/{courseId}/assignment_annotations/{id}/read` | `star-assignments` |
| GET | `/courses/{courseId}/assignment_groups/{id}/rules` | `star-assignment-groups` |
| PUT | `/courses/{courseId}/assignment_groups/{id}/rules` | `star-assignment-groups` |
| GET | `/courses/{id}/assignments/new` | `star-ui` |
| POST | `/courses/{id}/assignments/new` | `star-ui` |
| GET | `/courses/{id}/assignments/{aid}/edit` | `star-ui` |
| POST | `/courses/{id}/assignments/{aid}/edit` | `star-ui` |
| GET | `/courses/{courseId}/assignments/{id}/submissions/self` | `star-assignments` |
| GET | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments` | `star-submission-comments` |
| POST | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments` | `star-submission-comments` |
| GET | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}` | `star-submission-comments` |
| POST | `/courses/{id}/assignments/{aid}/submit` | `star-ui` |
| GET | `/courses/{courseId}/calendar_events` | `star-calendar` |
| POST | `/courses/{courseId}/calendar_events` | `star-calendar` |
| POST | `/courses/{id}/deep_linking_response` | `star-lti` |
| GET | `/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}` | `star-discussions` |
| GET | `/courses/{id}/discussions` | `star-ui` |
| POST | `/courses/{id}/discussions` | `star-ui` |
| GET | `/courses/{id}/discussions/new` | `star-ui` |
| GET | `/courses/{id}/discussions/{topic_id}` | `star-ui` |
| POST | `/courses/{id}/discussions/{topic_id}/entries` | `star-ui` |
| GET | `/courses/{course_id}/external_tools/{id}/eula_launch` | `star-lti` |
| GET | `/courses/{courseId}/final_grade` | `star-grading-standards` |
| GET | `/courses/{courseId}/folders/by_path/{*full_path}` | `star-files` |
| GET | `/courses/{courseId}/folders/root` | `star-files` |
| GET | `/courses/{courseId}/gradebook` | `star-assignments` |
| GET | `/courses/{id}/gradebook` | `star-ui` |
| GET | `/courses/{id}/gradebook.csv` | `star-ui` |
| GET | `/courses/{id}/gradebook/speedgrader` | `star-ui` |
| POST | `/courses/{id}/gradebook/speedgrader/grade` | `star-ui` |
| GET | `/courses/{id}/grades` | `star-ui` |
| POST | `/courses/{courseId}/groups` | `star-groups` |
| DELETE | `/courses/{courseId}/modules/items/{itemId}` | `star-modules` |
| GET | `/courses/{courseId}/modules/items/{itemId}` | `star-modules` |
| PUT | `/courses/{courseId}/modules/items/{itemId}` | `star-modules` |
| POST | `/courses/{courseId}/outcome_groups` | `star-outcomes` |
| GET | `/courses/{courseId}/outcomes` | `star-outcomes` |
| POST | `/courses/{courseId}/outcomes` | `star-outcomes` |
| GET | `/courses/{id}/quizzes/new` | `star-ui` |
| POST | `/courses/{id}/quizzes/new` | `star-ui` |
| POST | `/courses/{id}/quizzes/{qid}/submissions/{sid}/submit` | `star-ui` |
| GET | `/courses/{id}/quizzes/{qid}/take` | `star-ui` |
| POST | `/courses/{id}/quizzes/{qid}/take` | `star-ui` |
| GET | `/courses/{courseId}/role_overrides` | `star-permissions` |
| PUT | `/courses/{courseId}/role_overrides` | `star-permissions` |
| DELETE | `/courses/{courseId}/rubric_assessments/{id}/read` | `star-assignments` |
| PUT | `/courses/{courseId}/rubric_assessments/{id}/read` | `star-assignments` |
| POST | `/courses/{id}/settings` | `star-ui` |
| PUT | `/courses/{courseId}/submissions/{submissionId}/read/{item}` | `star-assignments` |
| GET | `/courses/{id}/users/add` | `star-ui` |
| POST | `/courses/{id}/users/add` | `star-ui` |
| POST | `/courses/{id}/users/{user_id}/conclude` | `star-ui` |
| POST | `/courses/{id}/users/{user_id}/reactivate` | `star-ui` |
| GET | `/courses/{id}/{tab}` | `star-ui` |
| GET | `/dashboard` | `star-dashboard` |
| GET | `/dashboard` | `star-ui` |
| GET | `/deep_linking_cancel` | `star-lti` |
| GET | `/developer_keys` | `star-auth` |
| POST | `/developer_keys` | `star-auth` |
| GET | `/developer_keys/{id}` | `star-auth` |
| GET | `/enrollments` | `star-enrollments` |
| POST | `/enrollments` | `star-enrollments` |
| DELETE | `/enrollments/{id}` | `star-enrollments` |
| GET | `/files/quota` | `star-files` |
| GET | `/folders` | `star-files` |
| POST | `/folders` | `star-files` |
| GET | `/forgot_password` | `star-ui` |
| POST | `/forgot_password` | `star-ui` |
| DELETE | `/grading_periods/{id}` | `star-grading-standards` |
| GET | `/grading_periods/{id}` | `star-grading-standards` |
| PUT | `/grading_periods/{id}` | `star-grading-standards` |
| DELETE | `/grading_schemes/{id}` | `star-grading-standards` |
| GET | `/grading_schemes/{id}` | `star-grading-standards` |
| PUT | `/grading_schemes/{id}` | `star-grading-standards` |
| DELETE | `/grading_standards/{id}` | `star-grading-standards` |
| GET | `/grading_standards/{id}` | `star-grading-standards` |
| POST | `/groups/{group_id}/deep_linking_response` | `star-lti` |
| GET | `/groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}` | `star-discussions` |
| GET | `/groups/{group_id}/external_tools/{id}` | `star-lti` |
| DELETE | `/groups/{groupId}/files/{id}` | `star-files` |
| GET | `/groups/{groupId}/folders/by_path/{*full_path}` | `star-files` |
| POST | `/groups/{groupId}/pages/{urlOrId}` | `star-pages` |
| POST | `/groups/{groupId}/users` | `star-groups` |
| GET | `/health` | `star-http` |
| POST | `/jobs/{id}/cancel` | `star-jobs` |
| GET | `/jobs2` | `star-jobs` |
| GET | `/jobs2/{id}/{group}` | `star-jobs` |
| GET | `/jobs2/{id}/{group}/search` | `star-jobs` |
| GET | `/login` | `star-ui` |
| POST | `/login` | `star-ui` |
| GET | `/login/oauth2/auth` | `star-auth` |
| GET | `/login/oauth2/jwks` | `star-auth` |
| DELETE | `/login/oauth2/token` | `star-auth` |
| POST | `/login/oauth2/token` | `star-auth` |
| POST | `/logout` | `star-ui` |
| DELETE | `/lti_registrations/{id}` | `star-lti` |
| GET | `/lti_registrations/{id}` | `star-lti` |
| GET | `/lti_registrations/{id}/deployments` | `star-lti` |
| POST | `/lti_registrations/{id}/deployments` | `star-lti` |
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
| GET | `/register` | `star-ui` |
| POST | `/register` | `star-ui` |
| GET | `/reset_password/{token}` | `star-ui` |
| POST | `/reset_password/{token}` | `star-ui` |
| GET | `/rubric_assessments/{id}` | `star-rubrics` |
| DELETE | `/rubrics/{id}` | `star-rubrics` |
| GET | `/rubrics/{id}` | `star-rubrics` |
| PUT | `/rubrics/{id}` | `star-rubrics` |
| DELETE | `/sections/{sectionId}/assignment_annotations/{id}/read` | `star-assignments` |
| PUT | `/sections/{sectionId}/assignment_annotations/{id}/read` | `star-assignments` |
| DELETE | `/sections/{sectionId}/rubric_assessments/{id}/read` | `star-assignments` |
| PUT | `/sections/{sectionId}/rubric_assessments/{id}/read` | `star-assignments` |
| GET | `/static/{hash}/{file}` | `star-ui` |
| GET | `/submissions/{submissionId}/comments` | `star-submission-comments` |
| POST | `/submissions/{submissionId}/comments` | `star-submission-comments` |
| PUT | `/submissions/{submissionId}/comments/read` | `star-submission-comments` |
| GET | `/submissions/{submissionId}/comments/unread_count` | `star-submission-comments` |
| DELETE | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` |
| GET | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` |
| PUT | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` |
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
| GET | `/users/self/communication_channels/{communication_channel_id}/notification_preferences/{notification}` | `star-notifications` |
| GET | `/users/self/notification_preferences` | `star-notifications` |
| PUT | `/users/self/notification_preferences` | `star-notifications` |
| GET | `/users/self/notifications` | `star-notifications` |
| GET | `/users/self/profile` | `star-users` |
| PUT | `/users/self/profile` | `star-users` |
| PUT | `/users/{user_id}/communication_channels/{type}/{address}/notification_preferences` | `star-notifications` |
| PUT | `/users/{user_id}/communication_channels/{type}/{address}/notification_preferences/{notification}` | `star-notifications` |
| GET | `/users/{userId}/folders/by_path/{*full_path}` | `star-files` |
| GET | `/users/{userId}/folders/media` | `star-files` |
| GET | `/users/{user_id}/notification_preference_categories` | `star-notifications` |
| PUT | `/users/{user_id}/notification_preference_categories/{category}` | `star-notifications` |
| GET | `/users/{user_id}/notification_preferences` | `star-notifications` |
| PUT | `/users/{user_id}/notification_preferences` | `star-notifications` |

## Parser scope and limitations

- Canvas routes are verb declarations inside `ApiRouteSet.draw` blocks, plus Rails `resources` declarations (API defaults: index/create/show/update/destroy, honoring `only:`, `except:`, `path:`, `controller:`, and nesting) and `%w[...].each` context loops, which are expanded for every value.
- Conditional context prefixes assigned from `var == "literal" ? var : "..."` are evaluated per loop binding, and `if`/`unless var == "literal"` guards filter bindings; this expands the `outcome_groups_api` routes to `courses/...`, `accounts/...`, and `global/...`. Other runtime interpolations are left as `{}` and cannot match concrete Rust paths.
- UI references are `/api/v1/...` string literals in `ui/`; runtime-built paths and hard-coded ids that cannot be normalized are missed.
- GraphQL type names come from `graphql_name` declarations (or the class name minus a trailing `Type`) in `app/graphql/types/*_type.rb`, and mutation operations from classes whose superclass chain reaches `Mutations::BaseMutation` in `app/graphql/mutations/**`; the denominators remain the file counts (170 type files, 110 mutation files). star-graphql names are extracted from `#[Object]` impls and async-graphql derives, with documented renames (CourseSection->Section, EnrollmentTerm->Term, DiscussionTopic->Discussion, QuizQuestion->QuizItem) mapped to their Canvas graphql_names before matching. Connection and edge helpers (e.g. `PageInfo`, `*Edge`, `*Connection`) only count when Canvas defines a type of the same name.
- Rust paths are matched by `(verb, normalized path)`; axum's `nest("/api/v1", ...)` mount is assumed for all crate routers.
- Extras include deliberate flat-path deviations (for example `/assignments/{id}` where Canvas uses `/courses/{course_id}/assignments/{id}`) and auth/dashboard/health routes Canvas does not expose.
- The schema row counts `CREATE TABLE` statements in `star-db` migrations (36 authored tables); sqlx adds `_sqlx_migrations`, so a live database has 37 tables.
