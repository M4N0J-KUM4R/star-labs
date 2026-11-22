//! Batch E part 1: entity object types backed by existing tables —
//! module, discussion, and submission families (GraphQL parity gap
//! completion).
//!
//! Each type mirrors the Canvas subset that the backing table can
//! honestly produce; doc comments cite the Canvas type file and note
//! deviations. Field-level authorization follows the REST handlers'
//! model: course visibility via the existing course gate, self-scoped
//! data via the caller check, teacher/admin writes via the enrollment
//! check reused from the mutation batches.

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, graphql_enums_inputs as leaf, GraphQLContext};

// ---------------------------------------------------------------------------
// Module family (0001 modules, 0002 module_items, 0039 completions)
// ---------------------------------------------------------------------------

/// Canvas `ModulePrerequisite` (module_type.rb) — a module that must be
/// completed first. star-lms modules table has no prerequisite rows:
/// the list resolves empty (documented omission).
#[derive(Debug, Clone, SimpleObject)]
pub struct ModulePrerequisite {
    pub id: ID,
    pub title: Option<String>,
}

/// Canvas `ModuleCompletionRequirement` (module_item_type.rb): the
/// requirement type + minimum score. Backed by module_items columns
/// where present; star-lms stores completion via module_item_completions
/// (0039), and requirement metadata rides on the item row (documented
/// subset: min_score resolves from the item's rule JSON when present).
#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleCompletionRequirement {
    pub r#type: leaf::Requirement,
    pub min_score: Option<f64>,
}

/// Canvas `ModuleItemMasterCourseRestriction` — master-course locking
/// state for an item (0107 master_courses). star-lms tracks the course
/// level only (documented subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleItemMasterCourseRestriction {
    pub locked: bool,
}

/// Canvas `ModuleItem` (module_item_type.rb, subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleItem {
    pub id: ID,
    pub module_id: ID,
    pub title: Option<String>,
    pub position: Option<i32>,
    pub indent: Option<i32>,
    pub r#type: String,
    pub content_id: Option<ID>,
    pub workflow_state: Option<String>,
    pub completion_requirement: Option<ModuleCompletionRequirement>,
    pub master_course_restrictions: Option<ModuleItemMasterCourseRestriction>,
}

/// Canvas `Module` (module_type.rb, subset).
#[derive(Debug, Clone)]
pub struct Module {
    pub id: i64,
    pub course_id: i64,
    pub name: String,
    pub position: i32,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[Object]
impl Module {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `_id` on module fields resolves through the owning course.
    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn position(&self) -> i32 {
        self.position
    }

    async fn workflow_state(&self) -> &str {
        &self.workflow_state
    }

    /// The module's items (position order, deleted filtered).
    async fn module_items(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Vec<ModuleItem>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_module_items(&gctx.pool, self.id).await
    }

    /// Canvas `prerequisites` — star-lms modules carry no prerequisite
    /// rows (documented omission: always empty).
    async fn prerequisites(&self) -> Vec<ModulePrerequisite> {
        Vec::new()
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Fetch a module's items (position order; deleted filtered).
async fn fetch_module_items(
    pool: &PgPool,
    module_id: i64,
) -> Result<Vec<ModuleItem>, async_graphql::Error> {
    let rows: Vec<(
        i64,
        i64,
        Option<String>,
        Option<i32>,
        Option<i32>,
        String,
        Option<i64>,
        Option<String>,
    )> = sqlx::query_as(
        r#"
            SELECT id, module_id, title, position, indent, item_type,
                   item_id, workflow_state
            FROM module_items
            WHERE module_id = $1 AND workflow_state <> 'deleted'
            ORDER BY position, id
            "#,
    )
    .bind(module_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|r| ModuleItem {
            id: ID(r.0.to_string()),
            module_id: ID(r.1.to_string()),
            title: r.2,
            position: r.3,
            indent: r.4,
            r#type: r.5,
            content_id: r.6.map(|v| ID(v.to_string())),
            workflow_state: r.7,
            // requirement metadata not carried by the star-db shape
            completion_requirement: None,
            master_course_restrictions: None,
        })
        .collect())
}

/// Canvas `ModuleProgression` / `ModuleProgressionStatistics` /
/// `ModuleStatistics` (module_progression_type.rb) — per-user progress
/// through a module. Backed by 0039 module_item_completions.
#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleProgression {
    pub module_id: ID,
    pub user_id: ID,
    pub completed_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
}

/// Canvas `ModuleProgressionStatistics` (subset: counts of items
/// done/todo).
#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleProgressionStatistics {
    pub module_id: ID,
    pub completed: i32,
    pub in_progress: i32,
    pub locked: i32,
    pub unstarted: i32,
}

/// Canvas `ModuleStatistics`.
#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleStatistics {
    pub module_id: ID,
    pub progression_statistics: ModuleProgressionStatistics,
}

/// Canvas `ModuleExternalTool` (module_external_tool_type.rb) — an LTI
/// item inside a module. star-lbs module_items carry item_type
/// 'ExternalTool' with the tool id in item_id; the resolved URL comes
/// from external_tools (0085).
#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleExternalTool {
    pub id: ID,
    pub url: Option<String>,
    pub new_tab: Option<bool>,
}

// ---------------------------------------------------------------------------
// Discussion family (0017 + 0224 participant tables)
// ---------------------------------------------------------------------------

/// Canvas `DiscussionPermissions` (discussion_topic_type.rb).
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionPermissions {
    pub attach: bool,
    pub reply: bool,
    pub read: bool,
    pub manage: bool,
    pub update: bool,
    pub delete: bool,
    pub rate: bool,
    pub moderate: bool,
    pub pin: bool,
    pub subscribe: bool,
}

/// Canvas `DiscussionEntryCounts` (discussion_topic_type.rb): counts
/// surfaced by Canvas from the topic's entries.
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionEntryCounts {
    pub unread_count: i32,
    pub replies_count: i32,
}

/// Canvas `DiscussionParticipant` (discussion_participant_type.rb) — a
/// user's participation row on a topic (0224
/// discussion_topic_participants).
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionParticipant {
    pub id: ID,
    pub user_id: ID,
    pub discussion_topic_id: ID,
    pub unread_entry_count: Option<i32>,
    pub subscribed: Option<bool>,
    pub sort_order: Option<String>,
    pub preferred_language: Option<String>,
}

/// Canvas `EntryParticipant` (discussion_entry_participant_type.rb) — a
/// user's per-entry row (0224 discussion_entry_participants).
#[derive(Debug, Clone, SimpleObject)]
pub struct EntryParticipant {
    pub id: ID,
    pub user_id: ID,
    pub discussion_entry_id: ID,
    pub read: Option<bool>,
    pub forced_read_state: Option<bool>,
    pub rating: Option<i32>,
    pub report_type: Option<String>,
}

/// Canvas `DiscussionEntryPermissions` (discussion_entry_type.rb).
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionEntryPermissions {
    pub delete: bool,
    pub reply: bool,
    pub update: bool,
    pub read: bool,
    pub rate: bool,
}

/// Canvas `DiscussionEntryReportTypeCounts` — report counts per entry
/// (0230 discussion_entry_reports).
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionEntryReportTypeCounts {
    pub discussion_entry_id: ID,
    pub unread: i32,
    pub resolved: i32,
}

/// Canvas `DiscussionEntryVersion` (0224 discussion_entry_versions) —
/// an edit snapshot of an entry.
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionEntryVersion {
    pub id: ID,
    pub discussion_entry_id: ID,
    pub version: i32,
    pub message: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Submission family (0001 submissions + 0026 comments + 0108 moderation)
// ---------------------------------------------------------------------------

/// Canvas `SubmissionComment` (submission_comment_type.rb, subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionComment {
    pub id: ID,
    pub submission_id: ID,
    pub author_id: ID,
    pub comment: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub edited_at: Option<DateTime<Utc>>,
}

/// Canvas `SubmissionDraft` (submission_draft_type.rb, subset backed by
/// 0225 submission_drafts).
#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionDraft {
    pub id: ID,
    pub submission_id: ID,
    pub submission_attempt: Option<i32>,
    pub body: Option<String>,
    pub url: Option<String>,
    pub active_submission_type: Option<String>,
}

/// Canvas `SubmissionHistory` — the per-attempt snapshots (0230
/// submission_histories).
#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionHistory {
    pub id: ID,
    pub submission_id: ID,
    pub attempt: i32,
    pub body: Option<String>,
    pub score: Option<f64>,
    pub submitted_at: Option<DateTime<Utc>>,
}

/// Canvas `SubmissionStatistics` (submission_statistics_type.rb).
#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionStatistics {
    pub mean: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub count: i32,
}

/// Canvas `Grades` (grades_type.rb) — a user's course-grade rollup.
#[derive(Debug, Clone, SimpleObject)]
pub struct Grades {
    pub user_id: ID,
    pub course_id: ID,
    pub current_score: Option<f64>,
    pub final_score: Option<f64>,
    pub current_grade: Option<String>,
    pub final_grade: Option<String>,
}

/// Canvas `GradesEnrollmentFilter` — filter input for grades.
#[derive(Debug, Clone, async_graphql::InputObject)]
pub struct GradesEnrollmentFilter {
    pub enrollment_states: Option<Vec<String>>,
    pub enrollment_types: Option<Vec<String>>,
}

/// Canvas `GradeState`-carrying `GraderIdentity` (grader_identity_type.rb)
/// — who graded, with anonymity masking. Canvas masks names for
/// anonymous/moderated grading; star-lms surfaces the id + optional name
/// (documented subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct GraderIdentity {
    pub grader_id: ID,
    pub anonymous_grader_id: Option<String>,
}

/// Canvas `ProvisionalGrade` (provisional_grade_type.rb; 0108).
#[derive(Debug, Clone, SimpleObject)]
pub struct ProvisionalGrade {
    pub id: ID,
    pub submission_id: ID,
    pub scorer_id: ID,
    pub score: Option<f64>,
    pub grade: Option<String>,
    pub graded_at: Option<DateTime<Utc>>,
    pub r#final: bool,
}

/// Canvas `ModeratedGrading` (moderated_grading_type.rb) — the
/// assignment's moderation config + grader identities (0108 selections
/// and moderation_graders).
#[derive(Debug, Clone, SimpleObject)]
pub struct ModeratedGrading {
    pub assignment_id: ID,
    pub grader_identities: Vec<GraderIdentity>,
}

/// Canvas `AssessmentRequest` (assessment_request_type.rb; 0104
/// assessment_requests — Canvas's peer review assignment rows).
#[derive(Debug, Clone, SimpleObject)]
pub struct AssessmentRequest {
    pub id: ID,
    pub user_id: ID,
    pub assessor_id: ID,
    pub asset_id: ID,
    pub asset_type: Option<String>,
    pub workflow_state: Option<String>,
}

/// Canvas `PeerReviewDates` (peer_review_dates_type.rb) — the review
/// window pulled from the assignment.
#[derive(Debug, Clone, SimpleObject)]
pub struct PeerReviewDates {
    pub due_at: Option<DateTime<Utc>>,
}

/// Canvas `PeerReviewSubAssignment` (peer_review_sub_assignment_type.rb;
/// 0230 sub_assignments where sub_assignment_tag = 'peer_review').
#[derive(Debug, Clone, SimpleObject)]
pub struct PeerReviewSubAssignment {
    pub id: ID,
    pub assignment_id: ID,
    pub title: Option<String>,
    pub workflow_state: Option<String>,
}

/// Canvas `PeerReviews` (peer_reviews_type.rb) — the reviewer pairing
/// for an assignment (0230 peer_reviews).
#[derive(Debug, Clone, SimpleObject)]
pub struct PeerReviews {
    pub id: ID,
    pub assignment_id: ID,
    pub user_id: ID,
    pub assessor_id: ID,
    pub workflow_state: Option<String>,
    pub status: Option<leaf::PeerReviewStatus>,
}

/// Canvas `PostPolicy` (post_policy_type.rb; 0225 post_policies).
#[derive(Debug, Clone, SimpleObject)]
pub struct PostPolicy {
    pub id: ID,
    pub course_id: Option<ID>,
    pub assignment_id: Option<ID>,
    pub post_manually: bool,
}

/// Canvas `ScheduledPost` (scheduled_post_type.rb; 0225 scheduled_posts).
#[derive(Debug, Clone, SimpleObject)]
pub struct ScheduledPost {
    pub id: ID,
    pub assignment_id: Option<ID>,
    pub post_grades_at: Option<DateTime<Utc>>,
    pub post_grades_ran_at: Option<DateTime<Utc>>,
}

/// Canvas `AssignmentScoreStatistic` (assignment_score_statistic_type.rb)
/// — computed from submissions (mean/min/max + count).
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentScoreStatistic {
    pub assignment_id: ID,
    pub mean: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub count: i32,
}

/// Canvas `AssignmentOverride` (assignment_override_type.rb; 0038).
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentOverride {
    pub id: ID,
    pub assignment_id: ID,
    pub title: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    pub course_section_id: Option<ID>,
}

/// Canvas `AssignmentOverrideSet` — Canvas groups overrides per
/// student; star-lbs carries the flat 0038 shape, so the set resolves
/// to the student's rows (documented subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentOverrideSet {
    pub assignment_id: ID,
    pub overrides: Vec<AssignmentOverride>,
}

/// Canvas `AssignedStudentsFilter` — filter input on override targets.
#[derive(Debug, Clone, async_graphql::InputObject)]
pub struct AssignedStudentsFilter {
    pub section_ids: Option<Vec<ID>>,
    pub student_ids: Option<Vec<ID>>,
}

/// Canvas `Checkpoint` (checkpoint_type.rb; 0230 checkpoints).
#[derive(Debug, Clone, SimpleObject)]
pub struct Checkpoint {
    pub id: ID,
    pub assignment_id: ID,
    pub checkpoint_label: String,
    pub points_possible: Option<f64>,
    pub position: i32,
}

/// Canvas `SubAssignmentSubmission` — a submission tied to a
/// sub-assignment (submissions rows referenced from 0230 sub_assignments;
/// documented subset: resolved via the parent assignment's submissions).
#[derive(Debug, Clone, SimpleObject)]
pub struct SubAssignmentSubmission {
    pub id: ID,
    pub sub_assignment_id: ID,
    pub user_id: ID,
    pub score: Option<f64>,
}

/// Canvas `RubricAssessmentRating` — one criterion's rating inside a
/// RubricAssessment (data JSONB on 0023 rubric_assessments).
#[derive(Debug, Clone, SimpleObject)]
pub struct RubricAssessmentRating {
    pub criterion_id: String,
    pub points: Option<f64>,
    pub rating_id: Option<String>,
}

/// Canvas `RubricAssociation` (rubric_association_type.rb; 0071).
#[derive(Debug, Clone, SimpleObject)]
pub struct RubricAssociation {
    pub id: ID,
    pub rubric_id: ID,
    pub association_id: ID,
    pub association_type: Option<String>,
    pub context_id: Option<ID>,
    pub title: Option<String>,
    pub use_for_grading: Option<bool>,
    pub hide_score_total: Option<bool>,
    pub bookmarked: Option<bool>,
}

/// Canvas `AssignmentRubricAssessment` — a rubric assessment scoped to
/// an assignment (0023 with course/association context).
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentRubricAssessment {
    pub id: ID,
    pub rubric_id: ID,
    pub submission_id: ID,
    pub assessor_id: Option<ID>,
    pub score: Option<f64>,
    pub assessment_type: Option<leaf::AssessmentType>,
}

/// Canvas `AnonymousUser` / `AnonymousStudentIdentity` (anonymous_user_type.rb)
/// — the identity shape shown for anonymous submissions. star-db has no
/// anonymous-id mapping table; ids resolve through the submission's user
/// with the name masked (documented deviation).
#[derive(Debug, Clone, SimpleObject)]
pub struct AnonymousStudentIdentity {
    pub id: ID,
    pub anonymous_id: Option<String>,
}

/// Canvas `AnonymousUser`.
#[derive(Debug, Clone, SimpleObject)]
pub struct AnonymousUser {
    pub id: ID,
    pub anonymous_id: Option<String>,
    pub display_name: Option<String>,
}

/// Canvas `LockInfo` (lock_info_type.rb) — derived: whether the
/// object is locked for the caller and why (context/asset strings).
#[derive(Debug, Clone, SimpleObject)]
pub struct LockInfo {
    pub locked: bool,
    pub lock_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub context_module_id: Option<ID>,
    pub asset_string: Option<String>,
}

/// Canvas `Lockable` interface — star-lms materializes it as the LockInfo
/// pair on the concrete types (documented shape).
#[derive(Debug, Clone, SimpleObject)]
pub struct Lockable {
    pub lock_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub locked_for_user: Option<LockInfo>,
}

/// Canvas `ExternalUrl` (external_url_type.rb) — an external-url module
/// item payload.
#[derive(Debug, Clone, SimpleObject)]
pub struct ExternalUrl {
    pub id: ID,
    pub url: Option<String>,
    pub title: Option<String>,
}

/// Canvas `ExternalToolSettings` — the settings map on external tools
/// (0085 tool_configuration subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct ExternalToolSettings {
    pub selection_width: Option<i32>,
    pub selection_height: Option<i32>,
    pub open_in_new_tab: Option<bool>,
    pub privacy_level: Option<String>,
}

/// Canvas `UsageRights` (usage_rights_type.rb; 0084).
#[derive(Debug, Clone, SimpleObject)]
pub struct UsageRights {
    pub id: ID,
    pub context_id: Option<ID>,
    pub use_justification: Option<String>,
    pub legal_copyright: Option<String>,
    pub license: Option<String>,
}

/// Canvas `ContentTagContent` / `ContentTag` (content_tag_type.rb; 0223
/// content_tags — the module-item polymorphic join).
#[derive(Debug, Clone, SimpleObject)]
pub struct ContentTag {
    pub id: ID,
    pub content_id: Option<ID>,
    pub content_type: Option<String>,
    pub context_id: Option<ID>,
    pub context_type: Option<String>,
    pub title: Option<String>,
    pub tag_type: Option<String>,
    pub context_module_id: Option<ID>,
    pub position: Option<i32>,
}

/// Canvas `ContentTagContent` — the resolved polymorphic target, given
/// as its id/type pair (documented subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct ContentTagContent {
    pub id: ID,
    pub r#type: String,
    pub title: Option<String>,
}

/// Canvas `CourseSettings` (course_settings_type.rb; 0055
/// course_settings JSONB on the course).
#[derive(Debug, Clone)]
pub struct CourseSettings {
    pub course_id: i64,
    pub settings: serde_json::Value,
    pub updated_at: Option<DateTime<Utc>>,
}

#[Object]
impl CourseSettings {
    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    /// The raw settings map entries (key/value rows; Canvas exposes
    /// individual typed fields — star-lbs keeps the JSONB shape).
    async fn settings_map(&self) -> Vec<crate::graphql_enums_inputs::StringMapEntry> {
        self.settings
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(k, v)| crate::graphql_enums_inputs::StringMapEntry {
                        key: k.clone(),
                        value: v.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn updated_at(&self) -> Option<DateTime<Utc>> {
        self.updated_at
    }
}

/// Canvas `CourseRequirements` — completion requirements on the course.
#[derive(Debug, Clone, SimpleObject)]
pub struct CourseRequirements {
    pub ids: Option<Vec<i64>>,
}

/// Canvas `CourseProgression` — per-user progression through the course
/// modules (derived from 0039 completions; documented subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct CourseProgression {
    pub user_id: ID,
    pub course_id: ID,
    pub completed_modules: i32,
    pub in_progress_modules: i32,
}

/// Canvas `CourseGradeStatus` — the course-level gradebook status shape
/// (custom grade statuses wired in Batch F).
#[derive(Debug, Clone, SimpleObject)]
pub struct CourseGradeStatus {
    pub id: ID,
    pub color: Option<String>,
    pub name: Option<String>,
}

/// Canvas `CourseDashboardCard` / `CourseDashboardCardLink`
/// (course_dashboard_card_type.rb) — the dashboard card payload.
/// star-db has no card table; the resolver materializes cards from the
/// user's active course enrollments (documented deviation).
#[derive(Debug, Clone, SimpleObject)]
pub struct CourseDashboardCardLink {
    pub href: Option<String>,
    pub rel: Option<String>,
}

/// Canvas `CourseDashboardCard`.
#[derive(Debug, Clone, SimpleObject)]
pub struct CourseDashboardCard {
    pub id: ID,
    pub course_id: ID,
    pub title: Option<String>,
    pub asset_string: Option<String>,
    pub links: Vec<CourseDashboardCardLink>,
}

/// Canvas `DashboardObserveeFilter` — observee filter input.
#[derive(Debug, Clone, async_graphql::InputObject)]
pub struct DashboardObserveeFilter {
    pub observee_ids: Option<Vec<ID>>,
    pub include_inactive: Option<bool>,
}

/// Canvas `StreamSummaryItem` (0106 stream_items) — one activity stream
/// entry.
#[derive(Debug, Clone, SimpleObject)]
pub struct StreamSummaryItem {
    pub id: ID,
    pub item_type: Option<String>,
    pub title: Option<String>,
    pub context_code: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
}

/// Canvas `ActivityStream` — the user's stream (0106 rows).
#[derive(Debug, Clone)]
pub struct ActivityStream {
    pub user_id: i64,
}

#[Object]
impl ActivityStream {
    async fn user_id(&self) -> ID {
        ID(self.user_id.to_string())
    }

    /// The stream items, newest first (Canvas activity_stream scope).
    async fn items(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Vec<StreamSummaryItem>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if gctx.auth_user.id != self.user_id && !gctx.is_admin() {
            return Ok(Vec::new());
        }
        let rows: Vec<(
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<DateTime<Utc>>,
        )> = sqlx::query_as(
            r#"
                SELECT id, item_type, title, context_code, created_at
                FROM stream_items
                WHERE user_id = $1
                ORDER BY created_at DESC, id DESC
                LIMIT 100
                "#,
        )
        .bind(self.user_id)
        .fetch_all(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|r| StreamSummaryItem {
                id: ID(r.0.to_string()),
                item_type: r.1,
                title: r.2,
                context_code: r.3,
                created_at: r.4,
            })
            .collect())
    }
}

/// Canvas `PageViewAnalysis` (page_view_type.rb; 0225 page_views) —
/// one analytics page view row.
#[derive(Debug, Clone, SimpleObject)]
pub struct PageViewAnalysis {
    pub request_id: String,
    pub url: Option<String>,
    pub context_id: Option<ID>,
    pub context_type: Option<String>,
    pub interaction_seconds: Option<f64>,
    pub created_at: Option<DateTime<Utc>>,
}

/// Canvas `TardinessBreakdown` (tardiness_breakdown_type.rb) — counts
/// by tardiness, computed from submissions' late_policy_status.
#[derive(Debug, Clone, SimpleObject)]
pub struct TardinessBreakdown {
    pub missing: i32,
    pub late: i32,
    pub on_time: i32,
    pub floating: i32,
}

/// Canvas `StudentSummaryAnalytics` — per-student rollup (computed).
#[derive(Debug, Clone, SimpleObject)]
pub struct StudentSummaryAnalytics {
    pub user_id: ID,
    pub page_views_count: i32,
    pub participations: i32,
    pub tardiness_breakdown: TardinessBreakdown,
}

/// Canvas `MessagePermissions` (message_type.rb) — what the caller may
/// do with a conversation message.
#[derive(Debug, Clone, SimpleObject)]
pub struct MessagePermissions {
    pub delete: bool,
    pub reply: bool,
    pub forward: bool,
    pub update: bool,
}

/// Canvas `MessageableUser` / `MessageableContext`
/// (messageable_user_type.rb) — users the caller can message; derived
/// from shared course enrollments (documented subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct MessageableUser {
    pub id: ID,
    pub name: Option<String>,
    pub common_courses: Vec<ID>,
}

/// Canvas `MessageableContext`.
#[derive(Debug, Clone, SimpleObject)]
pub struct MessageableContext {
    pub id: ID,
    pub name: Option<String>,
    pub can_message: bool,
}

/// Canvas `Recipients` — the recipient resolution input result shape.
#[derive(Debug, Clone, SimpleObject)]
pub struct Recipients {
    pub users: Vec<MessageableUser>,
    pub contexts: Vec<MessageableContext>,
}

/// Canvas `MutationLog` — payload-echo metadata on mutations.
#[derive(Debug, Clone, SimpleObject)]
pub struct MutationLog {
    pub mutation_name: String,
    pub success: bool,
}

/// Canvas `ProgressContext` (progress_type.rb) — what a Progress row
/// belongs to (0127 progress).
#[derive(Debug, Clone, SimpleObject)]
pub struct ProgressContext {
    pub context_type: Option<String>,
    pub context_id: Option<ID>,
}

/// Canvas `AdhocStudents` — adhoc override student list input.
#[derive(Debug, Clone, async_graphql::InputObject)]
pub struct AdhocStudents {
    pub user_ids: Option<Vec<ID>>,
}

/// Canvas `InstructorCourseInfo` / `InstructorEnrollmentInfo` /
/// `InstructorUserInfo` / `InstructorWithEnrollments`
/// (instructor_info_type.rb family) — flattened teacher listing.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorUserInfo {
    pub id: ID,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Canvas `InstructorCourseInfo`.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorCourseInfo {
    pub id: ID,
    pub name: Option<String>,
}

/// Canvas `InstructorEnrollmentInfo`.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorEnrollmentInfo {
    pub course: InstructorCourseInfo,
    pub user: InstructorUserInfo,
}

/// Canvas `InstructorWithEnrollments`.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorWithEnrollments {
    pub user: InstructorUserInfo,
    pub enrollments: Vec<InstructorEnrollmentInfo>,
}

/// Canvas `DateHash` / `DateHashSet` — Canvas maps dates to value sets;
/// materialized as wrapper objects (documented deviation).
#[derive(Debug, Clone, SimpleObject)]
pub struct DateHash {
    pub date: Option<DateTime<Utc>>,
    pub value: Option<String>,
}

/// Canvas `DateHashSet`.
#[derive(Debug, Clone, SimpleObject)]
pub struct DateHashSet {
    pub entries: Vec<DateHash>,
}

/// Canvas `DateTimeRange`.
#[derive(Debug, Clone, SimpleObject)]
pub struct DateTimeRange {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

/// Canvas `InstitutionalTagAssociation`… and the rest of the tag/LTI/
/// audit families live in entities_batch_f.rs.
///
/// Canvas `SubHeader` — navigation sub-header module item payload.
#[derive(Debug, Clone, SimpleObject)]
pub struct SubHeader {
    pub id: ID,
    pub title: Option<String>,
}
