//! Batch F: the remaining entity families — institutional tags, LTI
//! assets/processors, audit events, grade statuses, AI grading,
//! allocation rules, inbox/speedgrader settings, media sources,
//! outcome proficiency/calculation, notifications, and the
//! Turnitin/Vericite contexts (GraphQL parity gap completion).
//!
//! Every type cites its Canvas type file and notes deviations; backing
//! tables are 0223/0224/0225 canvas-schema tables plus 0230.

use async_graphql::SimpleObject;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::graphql_enums_inputs as leaf;

// ---------------------------------------------------------------------------
// Institutional tags (0224 institutional_tags / _categories / _associations)
// ---------------------------------------------------------------------------

// NOTE: the `InstitutionalTag` and `InstitutionalTagCategory` objects live
// in `root_types.rs` — they are the only ones in the schema (they also
// answer `Query.institutionalTag`, `Query.institutionalTagCategory` and the
// `Node` interface), and they carry the Canvas field names.

/// Canvas `InstitutionalTagAssociation`
/// (institutional_tag_association_type.rb; 0224).
#[derive(Debug, Clone, SimpleObject)]
pub struct InstitutionalTagAssociation {
    pub id: ID_,
    pub institutional_tag_id: ID_,
    pub user_id: Option<ID_>,
    pub course_id: Option<ID_>,
    pub workflow_state: Option<String>,
}

/// Local alias so the field docs stay short; async_graphql::ID re-exported.
type ID_ = async_graphql::ID;

// ---------------------------------------------------------------------------
// LTI assets / processors / reports (0224)
// ---------------------------------------------------------------------------

/// Canvas `LtiAssetProcessorWindowSettings`
/// (lti_asset_processor_window_settings_type.rb) — the iframe window
/// sizing from the 0224 JSONB.
#[derive(Debug, Clone, SimpleObject)]
pub struct LtiAssetProcessorWindowSettings {
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/// Canvas `LtiAssetProcessorIframe` (lti_asset_processor_iframe_type.rb).
#[derive(Debug, Clone, SimpleObject)]
pub struct LtiAssetProcessorIframe {
    pub url: Option<String>,
    pub window_settings: Option<LtiAssetProcessorWindowSettings>,
}

/// Canvas `LtiAssetProcessor` (lti_asset_processor_type.rb; 0224
/// lti_asset_processors — `iframe`/`window` ride in the JSONB columns).
#[derive(Debug, Clone, SimpleObject)]
pub struct LtiAssetProcessor {
    pub id: ID_,
    pub url: Option<String>,
    pub title: Option<String>,
    pub text: Option<String>,
    pub icon: Option<String>,
    pub report: Option<Value>,
    pub assignment_id: Option<ID_>,
    pub context_external_tool_id: Option<ID_>,
    pub workflow_state: Option<String>,
}

/// Canvas `LtiAsset` (lti_asset_type.rb; 0224 lti_assets).
#[derive(Debug, Clone, SimpleObject)]
pub struct LtiAsset {
    pub id: ID_,
    pub workflow_state: Option<String>,
}

/// Canvas `LtiAssetReport` (lti_asset_report_type.rb; 0224
/// lti_asset_reports).
#[derive(Debug, Clone, SimpleObject)]
pub struct LtiAssetReport {
    pub id: ID_,
    pub report_type: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    pub title: Option<String>,
    pub comment: Option<String>,
    pub indication_color: Option<String>,
    pub indication_alt: Option<String>,
    pub processing_progress: Option<String>,
    pub error_code: Option<String>,
    pub lti_asset_id: Option<ID_>,
    pub lti_asset_processor_id: Option<ID_>,
    pub workflow_state: Option<String>,
}

// ---------------------------------------------------------------------------
// Audit events (0230 audit_events)
// ---------------------------------------------------------------------------

/// Canvas `AuditEventType` — the discriminator payload embedded in an
/// AuditEvent's JSONB (Canvas subclasses AuditEvent per asset).
#[derive(Debug, Clone, SimpleObject)]
pub struct AuditEventType {
    pub event_type: Option<String>,
    pub payload: Option<Value>,
}

/// Canvas `AuditEventUser` — the user actor shape inside audit payloads.
#[derive(Debug, Clone, SimpleObject)]
pub struct AuditEventUser {
    pub id: Option<ID_>,
    pub name: Option<String>,
}

/// Canvas `AuditEventRole` — a role change payload.
#[derive(Debug, Clone, SimpleObject)]
pub struct AuditEventRole {
    pub id: Option<ID_>,
    pub label: Option<String>,
}

/// Canvas `AuditEventQuiz` — a quiz audit payload.
#[derive(Debug, Clone, SimpleObject)]
pub struct AuditEventQuiz {
    pub id: Option<ID_>,
    pub title: Option<String>,
}

/// Canvas `AuditEventExternalTool` — an external-tool audit payload.
#[derive(Debug, Clone, SimpleObject)]
pub struct AuditEventExternalTool {
    pub id: Option<ID_>,
    pub name: Option<String>,
}

/// Canvas `AuditEvent` (audit_event_type.rb; 0230 audit_events).
#[derive(Debug, Clone, SimpleObject)]
pub struct AuditEvent {
    pub id: ID_,
    pub context_type: Option<String>,
    pub context_id: Option<ID_>,
    pub uuid: Option<String>,
    pub event_type: Option<String>,
    pub payload: Option<Value>,
    pub created_at: Option<DateTime<Utc>>,
}

// NOTE: the `AuditLogs` object lives in `root_types.rs` — it is the only one
// in the schema (it answers `Query.auditLogs` and exposes Canvas'
// `mutationLogs` connection).

// ---------------------------------------------------------------------------
// AI grading (0225 auto_grade_results)
// ---------------------------------------------------------------------------

/// Canvas `AIGradeRating` — one rubric rating chosen by the AI grader
/// (from the auto_grade_results JSONB).
#[derive(Debug, Clone, SimpleObject)]
pub struct AIGradeRating {
    pub rating_id: Option<String>,
    pub points: Option<f64>,
}

/// Canvas `AIGradeCriterionResult` — one criterion's result.
#[derive(Debug, Clone, SimpleObject)]
pub struct AIGradeCriterionResult {
    pub criterion_id: Option<String>,
    pub score: Option<f64>,
    pub rating: Option<AIGradeRating>,
}

/// Canvas `AIGradeResult` (ai_grade_result_type.rb; 0225
/// auto_grade_results).
#[derive(Debug, Clone, SimpleObject)]
pub struct AIGradeResult {
    pub id: ID_,
    pub submission_id: Option<ID_>,
    pub score: Option<f64>,
    pub criterion_results: Vec<AIGradeCriterionResult>,
}

/// Canvas `AutoGradeEligibility` — whether a submission can be auto
/// graded and why not (derived).
#[derive(Debug, Clone, SimpleObject)]
pub struct AutoGradeEligibility {
    pub eligible: bool,
    pub reasons: Vec<String>,
}

/// Canvas `AutoGradeIssue` — one issue from an auto-grade run
/// (auto_grade_results JSONB).
#[derive(Debug, Clone, SimpleObject)]
pub struct AutoGradeIssue {
    pub code: Option<String>,
    pub message: Option<String>,
}

/// Canvas `EligibilityIssue` — the structured sibling of eligibility
/// reasons.
#[derive(Debug, Clone, SimpleObject)]
pub struct EligibilityIssue {
    pub level: Option<leaf::EligibilityIssueLevel>,
    pub code: Option<String>,
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Allocation rules (0224 allocation_rules)
// ---------------------------------------------------------------------------

/// Canvas `AllocationRule` (allocation_rule_type.rb; 0224
/// allocation_rules).
#[derive(Debug, Clone, SimpleObject)]
pub struct AllocationRule {
    pub id: ID_,
    pub assignment_id: Option<ID_>,
    pub workflow_state: Option<String>,
}

/// Canvas `AllocationRulesFilterInput` — filter for the rules list.
#[derive(Debug, Clone, async_graphql::InputObject)]
pub struct AllocationRulesFilterInput {
    pub assignment_ids: Option<Vec<async_graphql::ID>>,
    pub include_deleted: Option<bool>,
}

/// Canvas `AssignmentAllocationRules` — the assignment's rule bundle.
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentAllocationRules {
    pub assignment_id: ID_,
    pub rules: Vec<AllocationRule>,
}

// ---------------------------------------------------------------------------
// Grade statuses (0223 custom_grade_statuses / 0225 standard_grade_statuses)
// ---------------------------------------------------------------------------

/// Canvas `CustomGradeStatus` (custom_grade_status_type.rb; 0223).
#[derive(Debug, Clone, SimpleObject)]
pub struct CustomGradeStatus {
    pub id: ID_,
    pub color: Option<String>,
    pub name: Option<String>,
    pub workflow_state: Option<String>,
    pub applies_to_submissions: Option<bool>,
    pub applies_to_final_grade: Option<bool>,
}

/// Canvas `StandardGradeStatus` (standard_grade_status_type.rb; 0225).
#[derive(Debug, Clone, SimpleObject)]
pub struct StandardGradeStatus {
    pub id: ID_,
    pub color: Option<String>,
    pub status_name: Option<String>,
    pub hidden: Option<bool>,
}

// ---------------------------------------------------------------------------
// Inbox / speedgrader settings (0224 inbox_settings, 0230 speed_grader_settings)
// ---------------------------------------------------------------------------

// NOTE: the `InboxSettings` object lives in `root_types.rs` — it is the only
// one in the schema (it answers `Query.myInboxSettings`).

/// Canvas `SpeedGraderSettings` (speed_grader_settings_type.rb; 0230
/// speed_grader_settings JSONB).
#[derive(Debug, Clone, SimpleObject)]
pub struct SpeedGraderSettings {
    pub id: ID_,
    pub user_id: ID_,
    pub course_id: Option<ID_>,
    pub settings: Option<Value>,
}

// ---------------------------------------------------------------------------
// Media source (0098 media_objects)
// ---------------------------------------------------------------------------

/// Canvas `MediaSource` (media_source_type.rb) — the playable media
/// URL set; star-db stores the object row without CDN URLs, so the
/// source resolves to the object's identifiers (documented subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct MediaSource {
    pub media_id: String,
    pub media_type: Option<leaf::MediaType>,
}

// ---------------------------------------------------------------------------
// Outcomes (0225 proficiency / calculation / alignment)
// ---------------------------------------------------------------------------

// NOTE: the `ProficiencyRating`, `OutcomeProficiency` and
// `OutcomeCalculationMethod` objects live in `root_types.rs` — they are the
// only ones in the schema (they answer `Query.outcomeProficiency` and
// `Query.outcomeCalculationMethod` and the `Node` interface).

/// Canvas `OutcomeAlignment` (outcome_alignment_type.rb; 0044
/// outcome_alignments).
#[derive(Debug, Clone, SimpleObject)]
pub struct OutcomeAlignment {
    pub id: ID_,
    pub outcome_id: ID_,
    pub content_type: Option<String>,
    pub content_id: Option<ID_>,
    pub workflow_state: Option<String>,
}

/// Canvas `CourseOutcomeAlignmentStats` — alignment rollup per course
/// (derived from 0044 outcome_alignments).
#[derive(Debug, Clone, SimpleObject)]
pub struct CourseOutcomeAlignmentStats {
    pub course_id: ID_,
    pub total_alignments: i32,
    pub aligned_outcomes: i32,
}

// ---------------------------------------------------------------------------
// Notifications (0045) — the type Canvas exposes for the preference map
// ---------------------------------------------------------------------------

/// Canvas `NotificationPreferences` (notification_preferences_type.rb)
/// — the per-channel preference set (0045 notification_preferences; the
/// GraphQL type wraps the same rows the User field surfaces).
#[derive(Debug, Clone, SimpleObject)]
pub struct NotificationPreferences {
    pub channel_id: ID_,
    pub preference_count: i32,
}

// ---------------------------------------------------------------------------
// Plagiarism contexts (derived; Canvas turnitin_data_type.rb etc.)
// ---------------------------------------------------------------------------

/// Canvas `TurnitinData` — Turnitin similarity payload on a submission
/// (submissions JSONB when present; nulls otherwise).
#[derive(Debug, Clone, SimpleObject)]
pub struct TurnitinData {
    pub status: Option<String>,
    pub similarity_score: Option<f64>,
    pub webct_score: Option<f64>,
    pub publication_match_score: Option<f64>,
}

/// Canvas `TurnitinContext` — the launch context for a Turnitin report
/// (derived; star-db has no turnitin_columns — documented subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct TurnitinContext {
    pub submission_id: ID_,
    pub asset_string: Option<String>,
}

/// Canvas `VericiteData` — the VeriCite sibling of TurnitinData.
#[derive(Debug, Clone, SimpleObject)]
pub struct VericiteData {
    pub status: Option<String>,
    pub similarity_score: Option<f64>,
}

/// Canvas `VericiteContext`.
#[derive(Debug, Clone, SimpleObject)]
pub struct VericiteContext {
    pub submission_id: ID_,
    pub asset_string: Option<String>,
}

// ---------------------------------------------------------------------------
// Comment bank (0223 comment_bank_items)
// ---------------------------------------------------------------------------

/// Canvas `CommentBankItem` (comment_bank_item_type.rb; 0223
/// comment_bank_items).
#[derive(Debug, Clone, SimpleObject)]
pub struct CommentBankItem {
    pub id: ID_,
    pub course_id: Option<ID_>,
    pub user_id: Option<ID_>,
    pub assignment_id: Option<ID_>,
    pub comment: Option<String>,
    pub workflow_state: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal settings (0230 internal_settings)
// ---------------------------------------------------------------------------

// NOTE: the `InternalSetting` object lives in `root_types.rs` — it is the
// only one in the schema (it answers `Query.internalSetting` and the `Node`
// interface).
