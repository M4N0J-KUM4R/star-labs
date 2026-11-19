# Canvas to Rust parity scoreboard

Measured, deterministic output. Canvas source is not committed; refresh the manifest locally with `tools/parity/parity.py manifest`.

## Summary

| Area | Implemented | Total | Coverage |
|---|---:|---:|---:|
| REST API routes | 481 | 1403 | 34.3% |
| REST routes referenced from `ui/` | 258 | 503 | 51.3% |
| GraphQL files | 1 | 390 | 0.3% |
| GraphQL object types | 14 | 170 | 8.2% |
| GraphQL mutations | 0 | 110 | 0.0% |
| UI feature directories | 0 | 232 | 0.0% |
| JS/JSX/TS/TSX files | 0 | 5459 | 0.0% |
| ERB views | 0 | 475 | 0.0% |
| js_env files | 0 | 124 | 0.0% |
| Schema tables (vs 688 Canvas models) | 119 | 688 | n/a |

Implemented REST route count: **481 / 1403** (34.3%).

Star registrations: 634 route/method pairs from 541 `.route(...)` calls (0 unparsed); 157 have no Canvas match and are listed as extras below.

Canvas source sizes: 390 GraphQL files (170 object types, 110 mutations), 688 models, 232 UI feature dirs, 5459 JS/TSX files, 475 ERB views, 124 js_env files (490 call sites).

## Controllers with coverage (72)

| Controller | Implemented | Total | Coverage |
|---|---:|---:|---:|
| `discussion_topics_api` | 58 | 58 | 100.0% |
| `users` | 25 | 39 | 64.1% |
| `wiki_pages_api` | 20 | 29 | 69.0% |
| `submissions_api` | 16 | 44 | 36.4% |
| `accounts` | 14 | 22 | 63.6% |
| `courses` | 14 | 38 | 36.8% |
| `folders` | 14 | 34 | 41.2% |
| `master_courses/master_templates` | 13 | 13 | 100.0% |
| `outcome_groups_api` | 13 | 40 | 32.5% |
| `enrollments_api` | 12 | 14 | 85.7% |
| `files` | 12 | 26 | 46.2% |
| `calendar_events_api` | 10 | 15 | 66.7% |
| `group_categories` | 10 | 16 | 62.5% |
| `conversations` | 9 | 16 | 56.2% |
| `groups` | 9 | 17 | 52.9% |
| `context_module_items_api` | 8 | 12 | 66.7% |
| `external_tools` | 8 | 25 | 32.0% |
| `learning_object_dates` | 8 | 15 | 53.3% |
| `role_overrides` | 8 | 8 | 100.0% |
| `authentication_providers` | 7 | 8 | 87.5% |
| `quizzes/quiz_submissions_api` | 7 | 7 | 100.0% |
| `quizzes/quizzes_api` | 7 | 7 | 100.0% |
| `appointment_groups` | 6 | 8 | 75.0% |
| `blackout_dates` | 6 | 13 | 46.2% |
| `content_migrations` | 6 | 25 | 24.0% |
| `notification_preferences` | 6 | 10 | 60.0% |
| `quizzes/quiz_groups` | 6 | 6 | 100.0% |
| `release_notes` | 6 | 8 | 75.0% |
| `rubrics_api` | 6 | 15 | 40.0% |
| `sections` | 6 | 9 | 66.7% |
| `account_notifications` | 5 | 8 | 62.5% |
| `assignment_overrides` | 5 | 10 | 50.0% |
| `assignments_api` | 5 | 12 | 41.7% |
| `context_modules_api` | 5 | 8 | 62.5% |
| `grade_change_audit_api` | 5 | 12 | 41.7% |
| `group_memberships` | 5 | 11 | 45.5% |
| `planner_notes` | 5 | 5 | 100.0% |
| `quizzes/quiz_questions` | 5 | 5 | 100.0% |
| `usage_rights` | 5 | 9 | 55.6% |
| `admins` | 4 | 4 | 100.0% |
| `assignment_groups_api` | 4 | 5 | 80.0% |
| `communication_channels` | 4 | 12 | 33.3% |
| `discussion_entries` | 4 | 4 | 100.0% |
| `discussion_topics` | 4 | 10 | 40.0% |
| `favorites` | 4 | 8 | 50.0% |
| `grading_standards_api` | 4 | 10 | 40.0% |
| `jobs_v2` | 4 | 12 | 33.3% |
| `lti/registrations` | 4 | 20 | 20.0% |
| `peer_reviews_api` | 4 | 9 | 44.4% |
| `quizzes/quiz_reports` | 4 | 4 | 100.0% |
| `content_exports_api` | 3 | 14 | 21.4% |
| `feature_flags` | 3 | 17 | 17.6% |
| `media_objects` | 3 | 10 | 30.0% |
| `migration_issues` | 3 | 16 | 18.8% |
| `rubrics` | 3 | 3 | 100.0% |
| `submission_comments_api` | 3 | 4 | 75.0% |
| `terms` | 3 | 3 | 100.0% |
| `developer_keys` | 2 | 5 | 40.0% |
| `jobs` | 2 | 3 | 66.7% |
| `lti/account_external_tools` | 2 | 4 | 50.0% |
| `lti/deployments` | 2 | 5 | 40.0% |
| `sub_accounts` | 2 | 2 | 100.0% |
| `terms_api` | 2 | 2 | 100.0% |
| `assignment_groups` | 1 | 1 | 100.0% |
| `assignments` | 1 | 1 | 100.0% |
| `grading_periods` | 1 | 8 | 12.5% |
| `lti/account_lookup` | 1 | 1 | 100.0% |
| `media_tracks` | 1 | 6 | 16.7% |
| `profile` | 1 | 2 | 50.0% |
| `quizzes/quiz_statistics` | 1 | 1 | 100.0% |
| `submissions` | 1 | 2 | 50.0% |
| `tabs` | 1 | 5 | 20.0% |

## GraphQL operation coverage

Names are matched exactly: star-graphql type names (mapped through the documented renames below) against Canvas `graphql_name`s from `app/graphql/types/*_type.rb`, and mutation field names against the operations of `app/graphql/mutations/**` (classes whose superclass chain reaches `Mutations::BaseMutation`).

### Implemented types (14 of 170 Canvas names)

`Account`, `Assignment`, `Course`, `CoursePermissions`, `Discussion`, `Enrollment`, `Query`, `Quiz`, `QuizItem`, `QuizSubmission`, `Section`, `Submission`, `Term`, `User`

Renames mapped to Canvas names: `CourseSection` -> `Section`, `DiscussionTopic` -> `Discussion`, `EnrollmentTerm` -> `Term`, `QuizQuestion` -> `QuizItem`.

### Missing types (232)

`AIGradeCriterionResult`, `AIGradeRating`, `AIGradeResult`, `AccountNotification`, `AccountUsersFilter`, `AccountUsersSortField`, `AccountUsersSortInput`, `ActivityStream`, `AdhocStudents`, `AllocationRule`, `AllocationRulesFilterInput`, `AllowProvisionalGradingType`, `AnonymousStudentIdentity`, `AnonymousUser`, `AssessmentRequest`, `AssessmentType`, `AssignedStudentsFilter`, `AssignmentAllocationRules`, `AssignmentGroup`, `AssignmentGroupRules`, `AssignmentGroupState`, `AssignmentOverride`, `AssignmentOverrideSet`, `AssignmentRubricAssessment`, `AssignmentScoreStatistic`, `AssignmentState`, `AssignmentTargetSortField`, `AssignmentTargetSortOrder`, `AuditEvent`, `AuditEventExternalTool`, `AuditEventQuiz`, `AuditEventRole`, `AuditEventType`, `AuditEventUser`, `AuditLogs`, `AutoGradeEligibility`, `AutoGradeIssue`, `AutoLeaderPolicy`, `Checkpoint`, `CommentBankItem`, `CommunicationChannel`, `ContentTag`, `ContentTagContent`, `Conversation`, `ConversationMessage`, `ConversationParticipant`, `CourseDashboardCard`, `CourseDashboardCardLink`, `CourseFilterableEnrollmentState`, `CourseFilterableEnrollmentType`, `CourseGradeStatus`, `CourseOutcomeAlignmentStats`, `CourseProgression`, `CourseRequirements`, `CourseSectionsFilter`, `CourseSettings`, `CourseUsersFilter`, `CourseUsersSortDirectionType`, `CourseUsersSortFieldType`, `CourseUsersSortInputType`, `CourseWorkSubmissionsOrderField`, `CourseWorkflowState`, `CustomGradeStatus`, `DashboardObserveeFilter`, `DateHash`, `DateHashSet`, `DateTime`, `DateTimeRange`, `DiscussionEntry`, `DiscussionEntryCounts`, `DiscussionEntryPermissions`, `DiscussionEntryPinningType`, `DiscussionEntryReportTypeCounts`, `DiscussionEntryVersion`, `DiscussionFilterType`, `DiscussionParticipant`, `DiscussionParticipantFilter`, `DiscussionPermissions`, `DiscussionSortOrderType`, `DiscussionTopicAnonymousStateType`, `DiscussionTopicDiscussionType`, `DraftableSubmissionType`, `EligibilityIssue`, `EligibilityIssueLevel`, `EnrollmentFilterInput`, `EnrollmentRole`, `EnrollmentType`, `EnrollmentWorkflowState`, `EnrollmentsSortDirectionType`, `EnrollmentsSortFieldType`, `EnrollmentsSortInputType`, `EntryParticipant`, `ExternalTool`, `ExternalToolPlacement`, `ExternalToolPlacements`, `ExternalToolSettings`, `ExternalToolState`, `ExternalUrl`, `File`, `Folder`, `GradeState`, `GraderIdentity`, `Grades`, `GradesEnrollmentFilter`, `GradingPeriod`, `GradingPeriodGroup`, `GradingRole`, `GradingStandard`, `GradingStandardItem`, `GradingType`, `Group`, `GroupMembership`, `GroupMembershipState`, `GroupSet`, `GroupState`, `InboxSettings`, `InstitutionalTag`, `InstitutionalTagAssociation`, `InstitutionalTagCategory`, `InstitutionalTagUsersSortDirection`, `InstitutionalTagUsersSortField`, `InstitutionalTagUsersSortInput`, `InstitutionalTagWorkflowState`, `InstructorCourseInfo`, `InstructorEnrollmentInfo`, `InstructorUserInfo`, `InstructorWithEnrollments`, `InternalSetting`, `LearningOutcome`, `LearningOutcomeGroup`, `LockInfo`, `Lockable`, `LtiAsset`, `LtiAssetProcessor`, `LtiAssetProcessorIframe`, `LtiAssetProcessorWindowSettings`, `LtiAssetReport`, `MediaObject`, `MediaSource`, `MediaTrack`, `MediaTrackWorkflowState`, `MediaType`, `MessagePermissions`, `MessageableContext`, `MessageableUser`, `ModeratedGrading`, `Module`, `ModuleCompletionRequirement`, `ModuleCompletionStatus`, `ModuleExternalTool`, `ModuleFilterInput`, `ModuleItem`, `ModuleItemFilter`, `ModuleItemMasterCourseRestriction`, `ModulePrerequisite`, `ModuleProgression`, `ModuleProgressionStatistics`, `ModuleStatistics`, `Mutation`, `MutationLog`, `NodeType`, `Noop`, `Notification`, `NotificationPolicy`, `NotificationPreferences`, `NotificationPreferencesContextType`, `OrderDirection`, `OutcomeAlignment`, `OutcomeCalculationMethod`, `OutcomeFriendlyDescriptionType`, `OutcomeProficiency`, `Page`, `PageViewAnalysis`, `PeerReviewDates`, `PeerReviewStatus`, `PeerReviewSubAssignment`, `PeerReviews`, `PostPolicy`, `PreferredLanguageType`, `ProficiencyRating`, `ProficiencyRatingInput`, `Progress`, `ProgressContext`, `ProgressState`, `ProvisionalGrade`, `Recipients`, `Requirement`, `Rubric`, `RubricAssessment`, `RubricAssessmentRating`, `RubricAssociation`, `RubricCriterion`, `RubricRating`, `ScheduledPost`, `SelfSignupPolicy`, `SpeedGraderSettings`, `StandardGradeStatus`, `Sticker`, `StreamSummaryItem`, `StringMap`, `StudentSummaryAnalytics`, `SubAssignmentSubmission`, `SubHeader`, `SubmissionComment`, `SubmissionCommentFilterInput`, `SubmissionCommentStatusType`, `SubmissionDraft`, `SubmissionFilterInput`, `SubmissionGradingStatus`, `SubmissionHistory`, `SubmissionHistoryFilterInput`, `SubmissionHistoryOrder`, `SubmissionHistoryOrderField`, `SubmissionOrderCriteria`, `SubmissionOrderField`, `SubmissionPostingStatus`, `SubmissionSearchFilterInput`, `SubmissionSearchOrder`, `SubmissionSearchOrderField`, `SubmissionState`, `SubmissionStatistics`, `SubmissionStatusTagType`, `SubmissionType`, `TardinessBreakdown`, `TurnitinContext`, `TurnitinData`, `URL`, `UsageRights`, `UserGroupMembershipsFilterInput`, `ValidationError`, `VericiteContext`, `VericiteData`

### Implemented mutations (0 of 110 Canvas operations)

(none)

### Missing mutations (100)

`acceptEnrollmentInvitation`, `addConversationMessage`, `applyInstitutionalTag`, `autoGradeSubmission`, `createAllocationRule`, `createAssignment`, `createCommentBankItem`, `createConversation`, `createDiscussionEntry`, `createDiscussionTopic`, `createGroupInSet`, `createGroupSet`, `createInstitutionalTag`, `createInstitutionalTagCategory`, `createInternalSetting`, `createLearningOutcome`, `createLearningOutcomeGroup`, `createModule`, `createOutcomeCalculationMethod`, `createOutcomeProficiency`, `createSubmission`, `createSubmissionComment`, `createSubmissionDraft`, `createUserInboxLabel`, `deleteAllocationRule`, `deleteCommentBankItem`, `deleteConversationMessages`, `deleteConversations`, `deleteCustomGradeStatus`, `deleteDiscussionEntry`, `deleteDiscussionTopic`, `deleteInternalSetting`, `deleteOutcomeCalculationMethod`, `deleteOutcomeLinks`, `deleteOutcomeProficiency`, `deleteSubmissionComment`, `deleteSubmissionDraft`, `deleteUserInboxLabel`, `dismissAccountNotification`, `hideAssignmentGrades`, `hideAssignmentGradesForSections`, `importOutcomes`, `markSubmissionCommentsRead`, `moveOutcomeLinks`, `postAssignmentGrades`, `postAssignmentGradesForSections`, `postDraftSubmissionComment`, `rejectEnrollmentInvitation`, `removeInstitutionalTag`, `reorderModuleItems`, `restoreDeletedDiscussionEntry`, `saveRubricAssessment`, `selectProvisionalGrade`, `setAssignmentPostPolicy`, `setCoursePostPolicy`, `setFriendlyDescription`, `setModuleItemCompletion`, `setOverrideScore`, `setOverrideStatus`, `setRubricSelfAssessment`, `submitAutoGradeFeedback`, `subscribeToDiscussionTopic`, `updateAllocationRule`, `updateAssignment`, `updateCommentBankItem`, `updateConversationParticipants`, `updateDiscussionEntriesReadState`, `updateDiscussionEntry`, `updateDiscussionEntryParticipant`, `updateDiscussionReadState`, `updateDiscussionThreadReadState`, `updateDiscussionTopic`, `updateDiscussionTopicParticipant`, `updateGradebookGroupFilter`, `updateInstitutionalTag`, `updateInstitutionalTagArchivedState`, `updateInstitutionalTagCategory`, `updateInstitutionalTagCategoryArchivedState`, `updateInternalSetting`, `updateLearnerDashboardTabSelection`, `updateLearningOutcome`, `updateLearningOutcomeGroup`, `updateMyInboxSettings`, `updateNotificationPreferences`, `updateOutcomeCalculationMethod`, `updateOutcomeProficiency`, `updateRubricArchivedState`, `updateRubricAssessmentReadState`, `updateSpeedGraderSettings`, `updateSplitScreenViewDeeplyNestedAlert`, `updateSubmissionGrade`, `updateSubmissionGradeStatus`, `updateSubmissionSticker`, `updateSubmissionStudentEnteredScore`, `updateSubmissionsReadState`, `updateUserDiscussionsSplitscreenView`, `updateWidgetDashboardConfig`, `updateWidgetDashboardLayout`, `upsertCustomGradeStatus`, `upsertStandardGradeStatus`

Star names with no Canvas counterpart (not counted): `AssignmentConnection`, `AssignmentEdge`, `DiscussionTopicConnection`, `DiscussionTopicEdge`, `EnrollmentConnection`, `EnrollmentEdge`, `PageInfo`, `QuizConnection`, `QuizEdge`, `QuizQuestionConnection`, `QuizQuestionEdge`, `QuizSubmissionConnection`, `QuizSubmissionEdge`, `SubmissionConnection`, `SubmissionEdge`, `UserConnection`, `UserEdge`.

## Star routes with no Canvas match (157)

| Verb | Path | Crate |
|---|---|---|
| GET | `/` | `star-graphql` |
| POST | `/` | `star-graphql` |
| DELETE | `/account_notifications/{id}` | `star-accounts` |
| PUT | `/accounts/{accountId}/features/{feature}` | `star-features` |
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
| GET | `/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}` | `star-submission-comments` |
| GET | `/courses/{courseId}/calendar_events` | `star-calendar` |
| POST | `/courses/{courseId}/calendar_events` | `star-calendar` |
| GET | `/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}` | `star-discussions` |
| PUT | `/courses/{courseId}/features/{feature}` | `star-features` |
| GET | `/courses/{courseId}/final_grade` | `star-grading-standards` |
| GET | `/courses/{courseId}/folders/root` | `star-files` |
| GET | `/courses/{courseId}/gradebook` | `star-assignments` |
| POST | `/courses/{courseId}/groups` | `star-groups` |
| DELETE | `/courses/{courseId}/modules/items/{itemId}` | `star-modules` |
| GET | `/courses/{courseId}/modules/items/{itemId}` | `star-modules` |
| PUT | `/courses/{courseId}/modules/items/{itemId}` | `star-modules` |
| POST | `/courses/{courseId}/outcome_groups` | `star-outcomes` |
| GET | `/courses/{courseId}/outcomes` | `star-outcomes` |
| POST | `/courses/{courseId}/outcomes` | `star-outcomes` |
| GET | `/courses/{courseId}/role_overrides` | `star-permissions` |
| PUT | `/courses/{courseId}/role_overrides` | `star-permissions` |
| GET | `/courses/{courseId}/rubrics/{rubricId}/assessments` | `star-rubrics` |
| POST | `/courses/{courseId}/rubrics/{rubricId}/assessments` | `star-rubrics` |
| POST | `/courses/{courseId}/rubrics/{rubricId}/associations` | `star-rubrics` |
| GET | `/dashboard` | `star-dashboard` |
| GET | `/developer_keys` | `star-auth` |
| POST | `/developer_keys` | `star-auth` |
| GET | `/developer_keys/{id}` | `star-auth` |
| GET | `/enrollments` | `star-enrollments` |
| POST | `/enrollments` | `star-enrollments` |
| DELETE | `/enrollments/{id}` | `star-enrollments` |
| GET | `/files/quota` | `star-files` |
| GET | `/folders` | `star-files` |
| POST | `/folders` | `star-files` |
| DELETE | `/grading_periods/{id}` | `star-grading-standards` |
| GET | `/grading_periods/{id}` | `star-grading-standards` |
| PUT | `/grading_periods/{id}` | `star-grading-standards` |
| DELETE | `/grading_schemes/{id}` | `star-grading-standards` |
| GET | `/grading_schemes/{id}` | `star-grading-standards` |
| PUT | `/grading_schemes/{id}` | `star-grading-standards` |
| DELETE | `/grading_standards/{id}` | `star-grading-standards` |
| GET | `/grading_standards/{id}` | `star-grading-standards` |
| GET | `/groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}` | `star-discussions` |
| POST | `/groups/{groupId}/pages/{urlOrId}` | `star-pages` |
| POST | `/groups/{groupId}/users` | `star-groups` |
| GET | `/health` | `star-http` |
| POST | `/jobs/{id}/cancel` | `star-jobs` |
| GET | `/jobs2` | `star-jobs` |
| POST | `/login/oauth2/token` | `star-auth` |
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
| DELETE | `/rubric_assessments/{id}` | `star-rubrics` |
| GET | `/rubric_assessments/{id}` | `star-rubrics` |
| PUT | `/rubric_assessments/{id}` | `star-rubrics` |
| DELETE | `/rubric_associations/{id}` | `star-rubrics` |
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
| GET | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` |
| PUT | `/submissions/{submissionId}/comments/{commentId}` | `star-submission-comments` |
| PUT | `/submissions/{id}/grade` | `star-assignments` |
| DELETE | `/terms/{id}` | `star-courses` |
| GET | `/terms/{id}` | `star-courses` |
| PUT | `/terms/{id}` | `star-courses` |
| GET | `/tokens` | `star-auth` |
| POST | `/tokens` | `star-auth` |
| DELETE | `/tokens/{id}` | `star-auth` |
| GET | `/tokens/{id}` | `star-auth` |
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
