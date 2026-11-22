//! Batch D: pure GraphQL enum / scalar / sort / filter types — the
//! Canvas `*_type.rb` constants that need no database backing
//! (GraphQL parity gap completion).
//!
//! Each mirrors the GraphQL name in Canvas's inventory (Canvas's
//! graphql_name or the class name minus `Type`). Where a Canvas type is
//! an input + enum pair used by a connection's sort/filter arguments,
//! both are defined here. These types are leaf constants: exact value
//! sets come from Canvas's `enum` declarations in the corresponding
//! `app/graphql/types/**` file.

use async_graphql::{InputObject, SimpleObject};

// ---------------------------------------------------------------------------
// Scalars & trivial helpers
// ---------------------------------------------------------------------------

/// Canvas `DateTime` scalar. star-lms materializes it as a wrapper
/// object carrying the instant (documented deviation: Canvas exposes a
/// scalar; the chrono value rides in `iso`).
#[derive(Debug, Clone, SimpleObject)]
pub struct DateTime {
    pub iso: chrono::DateTime<chrono::Utc>,
}

/// Canvas `URL` scalar — a string carrying a URL (Canvas
/// `Types::UrlType`), materialized as a wrapper object (same
/// deviation class as DateTime).
#[derive(Debug, Clone, SimpleObject)]
pub struct URL {
    pub text: String,
}

/// Canvas `StringMap` scalar — a map of string keys to string values,
/// materialized as a wrapper object of key/value pairs.
#[derive(Debug, Clone, SimpleObject)]
pub struct StringMap {
    pub entries: Vec<StringMapEntry>,
}

/// One key/value row of a `StringMap`.
#[derive(Debug, Clone, SimpleObject)]
pub struct StringMapEntry {
    pub key: String,
    pub value: String,
}

/// Canvas `Noop` type — Canvas's placeholder type for fields pending
/// implementation (renders as an empty object).
#[derive(Debug, Clone, SimpleObject)]
pub struct Noop {
    /// Always true; Canvas's Noop carries no data.
    pub noop: bool,
}

/// Canvas `NodeType` interface discriminator enum. Canvas exposes the
/// interface; star-lms materializes it as an enum of the node kinds the
/// `node(id:)` root query resolves (documented shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum NodeType {
    Course,
    User,
    Assignment,
    Submission,
    DiscussionTopic,
}

// ---------------------------------------------------------------------------
// Ordering / direction enums + inputs
// ---------------------------------------------------------------------------

/// Canvas `OrderDirection` (common enum: ASC / DESC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum OrderDirection {
    Asc,
    Desc,
}

/// Canvas `CourseWorkSubmissionsOrderField`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum CourseWorkSubmissionsOrderField {
    PostedAt,
    GradedAt,
    SubmittedAt,
    Score,
}

/// Canvas `AssignmentTargetSortField`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AssignmentTargetSortField {
    Title,
    DueAt,
    Position,
}

/// Canvas `AssignmentTargetSortOrder` input.
#[derive(Debug, Clone, InputObject)]
pub struct AssignmentTargetSortOrder {
    pub field: AssignmentTargetSortField,
    pub direction: Option<OrderDirection>,
}

/// Canvas `AssignmentUsersSortField` family — `AccountUsersSortField`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AccountUsersSortField {
    Name,
    CreatedAt,
    LastLogin,
    SisId,
}

/// Canvas `AccountUsersSortInput`.
#[derive(Debug, Clone, InputObject)]
pub struct AccountUsersSortInput {
    pub field: Option<AccountUsersSortField>,
    pub direction: Option<OrderDirection>,
}

/// Canvas `AccountUsersFilter` input.
#[derive(Debug, Clone, InputObject)]
pub struct AccountUsersFilter {
    pub include_term_enrollments: Option<bool>,
    pub enrollment_type: Option<String>,
    pub search_term: Option<String>,
}

/// Canvas `CourseUsersSortFieldType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum CourseUsersSortFieldType {
    Name,
    Email,
    SisId,
    LastLogin,
}

/// Canvas `CourseUsersSortDirectionType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum CourseUsersSortDirectionType {
    Ascending,
    Descending,
}

/// Canvas `CourseUsersSortInputType`.
#[derive(Debug, Clone, InputObject)]
pub struct CourseUsersSortInputType {
    pub field: Option<CourseUsersSortFieldType>,
    pub direction: Option<CourseUsersSortDirectionType>,
}

/// Canvas `CourseUsersFilter`.
#[derive(Debug, Clone, InputObject)]
pub struct CourseUsersFilter {
    pub search_term: Option<String>,
    pub enrollment_type: Option<String>,
    pub include_inactive: Option<bool>,
}

/// Canvas `EnrollmentsSortFieldType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum EnrollmentsSortFieldType {
    UserName,
    SectionName,
    Role,
    CreatedAt,
}

/// Canvas `EnrollmentsSortDirectionType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum EnrollmentsSortDirectionType {
    Ascending,
    Descending,
}

/// Canvas `EnrollmentsSortInputType`.
#[derive(Debug, Clone, InputObject)]
pub struct EnrollmentsSortInputType {
    pub field: Option<EnrollmentsSortFieldType>,
    pub direction: Option<EnrollmentsSortDirectionType>,
}

/// Canvas `EnrollmentFilterInput`.
#[derive(Debug, Clone, InputObject)]
pub struct EnrollmentFilterInput {
    pub states: Option<Vec<EnrollmentWorkflowState>>,
    pub types: Option<Vec<EnrollmentType>>,
    pub roles: Option<Vec<EnrollmentRole>>,
}

/// Canvas `CourseSectionsFilter`.
#[derive(Debug, Clone, InputObject)]
pub struct CourseSectionsFilter {
    pub include_inactive: Option<bool>,
    pub search_term: Option<String>,
}

/// Canvas `InstitutionalTagUsersSortField`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum InstitutionalTagUsersSortField {
    Name,
    SisId,
    CreatedAt,
}

/// Canvas `InstitutionalTagUsersSortDirection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum InstitutionalTagUsersSortDirection {
    Asc,
    Desc,
}

/// Canvas `InstitutionalTagUsersSortInput`.
#[derive(Debug, Clone, InputObject)]
pub struct InstitutionalTagUsersSortInput {
    pub field: Option<InstitutionalTagUsersSortField>,
    pub direction: Option<InstitutionalTagUsersSortDirection>,
}

/// Canvas `UserGroupMembershipsFilterInput`.
#[derive(Debug, Clone, InputObject)]
pub struct UserGroupMembershipsFilterInput {
    pub include_inactive: Option<bool>,
}

/// Canvas `ModuleFilterInput`.
#[derive(Debug, Clone, InputObject)]
pub struct ModuleFilterInput {
    pub include_deleted: Option<bool>,
    pub published: Option<bool>,
}

/// Canvas `ModuleItemFilter`.
#[derive(Debug, Clone, InputObject)]
pub struct ModuleItemFilter {
    pub include_deleted: Option<bool>,
    pub published: Option<bool>,
}

/// Canvas `SubmissionFilterInput`.
#[derive(Debug, Clone, InputObject)]
pub struct SubmissionFilterInput {
    pub states: Option<Vec<SubmissionState>>,
    pub graded_states: Option<Vec<SubmissionGradingStatus>>,
    pub posting_statuses: Option<Vec<SubmissionPostingStatus>>,
}

/// Canvas `SubmissionCommentFilterInput`.
#[derive(Debug, Clone, InputObject)]
pub struct SubmissionCommentFilterInput {
    pub author_id: Option<async_graphql::ID>,
}

/// Canvas `SubmissionSearchFilterInput`.
#[derive(Debug, Clone, InputObject)]
pub struct SubmissionSearchFilterInput {
    pub text: Option<String>,
    pub states: Option<Vec<SubmissionState>>,
}

/// Canvas `SubmissionSearchOrderField`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SubmissionSearchOrderField {
    SubmittedAt,
    Score,
}

/// Canvas `SubmissionSearchOrder`.
#[derive(Debug, Clone, InputObject)]
pub struct SubmissionSearchOrder {
    pub field: Option<SubmissionSearchOrderField>,
    pub direction: Option<OrderDirection>,
}

/// Canvas `SubmissionOrderField`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SubmissionOrderField {
    SubmittedAt,
    GradedAt,
    PostedAt,
    Score,
}

/// Canvas `SubmissionOrderCriteria` input.
#[derive(Debug, Clone, InputObject)]
pub struct SubmissionOrderCriteria {
    pub field: Option<SubmissionOrderField>,
    pub direction: Option<OrderDirection>,
}

/// Canvas `SubmissionHistoryOrderField`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SubmissionHistoryOrderField {
    Attempt,
}

/// Canvas `SubmissionHistoryOrder`.
#[derive(Debug, Clone, InputObject)]
pub struct SubmissionHistoryOrder {
    pub field: Option<SubmissionHistoryOrderField>,
    pub direction: Option<OrderDirection>,
}

/// Canvas `SubmissionHistoryFilterInput`.
#[derive(Debug, Clone, InputObject)]
pub struct SubmissionHistoryFilterInput {
    pub attempts: Option<Vec<i32>>,
}

/// Canvas `AssignmentGroupState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AssignmentGroupState {
    Active,
    Deleted,
}

/// Canvas `AssignmentState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AssignmentState {
    Unpublished,
    Published,
    Deleted,
}

/// Canvas `SubmissionState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SubmissionState {
    Unsubmitted,
    Graded,
    PendingReview,
    Submitted,
}

/// Canvas `SubmissionGradingStatus` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SubmissionGradingStatus {
    Graded,
    NeedsGrading,
    NeedsReview,
}

/// Canvas `SubmissionPostingStatus` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SubmissionPostingStatus {
    Posted,
    Hidden,
}

/// Canvas `SubmissionStatusTagType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SubmissionStatusTagType {
    Missing,
    Late,
    Resubmitted,
    Excused,
    Extended,
}

/// Canvas `SubmissionCommentStatusType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SubmissionCommentStatusType {
    Read,
    Unread,
}

/// Canvas `SubmissionType` enum (Canvas Submission#submission_types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SubmissionType {
    OnlineTextEntry,
    OnlineUrl,
    OnlineUpload,
    OnlineQuiz,
    MediaRecording,
    StudentAnnotation,
    ExternalTool,
    NotGraded,
    None,
}

/// Canvas `DraftableSubmissionType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DraftableSubmissionType {
    OnlineTextEntry,
    OnlineUpload,
    MediaRecording,
}

/// Canvas `GradeState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum GradeState {
    Graded,
    Pending,
}

/// Canvas `EnrollmentType` enum (Canvas Enrollment::Type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum EnrollmentType {
    StudentEnrollment,
    TeacherEnrollment,
    TaEnrollment,
    DesignerEnrollment,
    ObserverEnrollment,
    StudentViewEnrollment,
}

/// Canvas `EnrollmentWorkflowState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum EnrollmentWorkflowState {
    Invited,
    CreationPending,
    Active,
    Deleted,
    Rejected,
    Completed,
    Inactive,
    Suspended,
}

/// Canvas `EnrollmentRole` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum EnrollmentRole {
    Student,
    Teacher,
    Ta,
    Designer,
    Observer,
    Custom,
}

/// Canvas `CourseWorkflowState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum CourseWorkflowState {
    Unpublished,
    Created,
    Claimed,
    Available,
    Completed,
    Deleted,
}

/// Canvas `CourseFilterableEnrollmentState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum CourseFilterableEnrollmentState {
    Active,
    InvitedOrPending,
    Completed,
    Deleted,
}

/// Canvas `CourseFilterableEnrollmentType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum CourseFilterableEnrollmentType {
    Student,
    Teacher,
    Ta,
    Observer,
    Designer,
}

/// Canvas `GradingType` enum (Canvas Assignment#grading_type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum GradingType {
    PassFail,
    Percent,
    LetterGrade,
    Points,
    GpaScale,
}

/// Canvas `GradingRole` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum GradingRole {
    Grader,
    FinalGrader,
    ModeratedGrader,
}

/// Canvas `GroupState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum GroupState {
    Available,
    Deleted,
}

/// Canvas `GroupMembershipState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum GroupMembershipState {
    Accepted,
    Invited,
    Requested,
    Rejected,
    Deleted,
}

/// Canvas `SelfSignupPolicy` enum (Canvas GroupCategory#self_signup).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum SelfSignupPolicy {
    Disabled,
    Restricted,
    Enabled,
}

/// Canvas `AutoLeaderPolicy` enum (Canvas GroupCategory#auto_leader).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AutoLeaderPolicy {
    Disabled,
    First,
    Random,
}

/// Canvas `AllowProvisionalGradingType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AllowProvisionalGradingType {
    Disabled,
    Enabled,
}

/// Canvas `ModuleCompletionRequirement` / `Requirement` enums (Canvas
/// ContentTag#completion_requirement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum Requirement {
    MustView,
    MustSubmit,
    MustContribute,
    MinScore,
    MustMarkDone,
}

/// Canvas `ModuleCompletionStatus` enum (Canvas
/// ContextModuleProgression#completion_status).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum ModuleCompletionStatus {
    Completed,
    InProgress,
    Started,
    Locked,
    Unlocked,
    Unstarted,
}

/// Canvas `ProgressState` enum (Canvas Progress#workflow_state).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum ProgressState {
    Queued,
    Running,
    Completed,
    Failed,
}

/// Canvas `ExternalToolState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum ExternalToolState {
    Active,
    Deleted,
}

/// Canvas `DiscussionFilterType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DiscussionFilterType {
    All,
    Unread,
    Unpublished,
    Graded,
    Assignment,
}

/// Canvas `DiscussionSortOrderType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DiscussionSortOrderType {
    Asc,
    Desc,
}

/// Canvas `DiscussionParticipantFilter` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DiscussionParticipantFilter {
    All,
    Unread,
    Replies,
}

/// Canvas `DiscussionTopicAnonymousStateType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DiscussionTopicAnonymousStateType {
    Off,
    Partial,
    Full,
}

/// Canvas `DiscussionTopicDiscussionType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DiscussionTopicDiscussionType {
    SideComment,
    Threaded,
}

/// Canvas `DiscussionEntryPinningType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DiscussionEntryPinningType {
    Pinned,
    PinnedByMe,
    Unpinned,
}

/// Canvas `AssessmentType` enum (Canvas RubricAssessment#assessment_type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AssessmentType {
    Grading,
    PeerReview,
    Provisonal,
    SelfAssessment,
}

/// Canvas `PeerReviewStatus` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum PeerReviewStatus {
    Assigned,
    Submitted,
    Graded,
    Removed,
}

/// Canvas `MediaTrackWorkflowState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum MediaTrackWorkflowState {
    Active,
    Deleted,
}

/// Canvas `MediaType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum MediaType {
    Video,
    Audio,
}

/// Canvas `InstitutionalTagWorkflowState` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum InstitutionalTagWorkflowState {
    Active,
    Archived,
    Deleted,
}

/// Canvas `NotificationPreferencesContextType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum NotificationPreferencesContextType {
    Account,
    Course,
}

/// Canvas `PreferredLanguageType` — Canvas stores ISO 639-1 codes;
/// materialized as a wrapper object (documented deviation).
#[derive(Debug, Clone, SimpleObject)]
pub struct PreferredLanguageType {
    pub code: String,
}

/// Canvas `OutcomeFriendlyDescriptionType` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum OutcomeFriendlyDescriptionType {
    Default,
    Custom,
}

/// Canvas `Sticker` enum (Canvas submission sticker values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum Sticker {
    ThumbsUp,
    ThumbsDown,
    RedFlag,
    Sunglasses,
    Surprised,
    Heart,
}

/// Canvas `EligibilityIssueLevel` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum EligibilityIssueLevel {
    Info,
    Warning,
    Error,
}

/// Canvas `ProficiencyRatingInput`.
#[derive(Debug, Clone, InputObject)]
pub struct ProficiencyRatingInput {
    pub description: Option<String>,
    pub points: f64,
    pub mastery: Option<bool>,
    pub color: Option<String>,
}
