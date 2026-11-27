//! Canvas GraphQL mutations — the second batch.
//!
//! Canvas' root mutations live in `canvas-src/app/graphql/mutations/`.
//! Each mutation takes exactly one `input` argument (Canvas'
//! `HasSingleInputArgument`) and answers with a payload carrying
//! `errors: [ValidationError!]`. Authorization failures and missing records
//! are top-level execution errors with Canvas' exact strings, exactly like
//! the assignment mutations in `mutations.rs`.
//!
//! The mutation names, input field names, input field types and payload
//! shapes here are Canvas' own — taken from
//! `canvas-src/schema.graphql` and the Ruby mutation classes. Nothing is
//! renamed.

use async_graphql::{Context, InputObject, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{
    db_err,
    mutations::ValidationError,
    node::parse_relay_or_legacy_id,
    root_types::{
        AccountNotification, AssignmentGroup, InternalSetting, LearningOutcome,
        LearningOutcomeGroup, SubmissionComment,
    },
    GraphQLContext,
};

/// Container for the Canvas root mutations implemented in this slice.
#[derive(Default)]
pub struct RootMutations;

#[Object]
impl RootMutations {
    // -----------------------------------------------------------------
    // Internal settings
    // -----------------------------------------------------------------

    /// Canvas `createInternalSetting`
    /// (`app/graphql/mutations/create_internal_setting.rb`).
    async fn create_internal_setting(
        &self,
        ctx: &Context<'_>,
        input: CreateInternalSettingInput,
    ) -> Result<CreateInternalSettingPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        create_internal_setting(&gctx.pool, gctx.is_admin(), input).await
    }

    /// Canvas `updateInternalSetting`
    /// (`app/graphql/mutations/update_internal_setting.rb`).
    async fn update_internal_setting(
        &self,
        ctx: &Context<'_>,
        input: UpdateInternalSettingInput,
    ) -> Result<UpdateInternalSettingPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        update_internal_setting(&gctx.pool, gctx.is_admin(), input).await
    }

    /// Canvas `deleteInternalSetting`
    /// (`app/graphql/mutations/delete_internal_setting.rb`).
    async fn delete_internal_setting(
        &self,
        ctx: &Context<'_>,
        input: DeleteInternalSettingInput,
    ) -> Result<DeleteInternalSettingPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        delete_internal_setting(&gctx.pool, gctx.is_admin(), input).await
    }

    // -----------------------------------------------------------------
    // Inbox settings
    // -----------------------------------------------------------------

    /// Canvas `updateMyInboxSettings`
    /// (`app/graphql/mutations/update_my_inbox_settings.rb`).
    async fn update_my_inbox_settings(
        &self,
        ctx: &Context<'_>,
        input: UpdateMyInboxSettingsInput,
    ) -> Result<UpdateMyInboxSettingsPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        update_my_inbox_settings(&gctx.pool, gctx.auth_user.id, input).await
    }

    // -----------------------------------------------------------------
    // Account notifications
    // -----------------------------------------------------------------

    /// Canvas `dismissAccountNotification`
    /// (`app/graphql/mutations/dismiss_account_notification.rb`).
    async fn dismiss_account_notification(
        &self,
        ctx: &Context<'_>,
        input: DismissAccountNotificationInput,
    ) -> Result<DismissAccountNotificationPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        dismiss_account_notification(&gctx.pool, gctx.auth_user.id, input).await
    }

    // -----------------------------------------------------------------
    // Enrollment invitations
    // -----------------------------------------------------------------

    /// Canvas `acceptEnrollmentInvitation`
    /// (`app/graphql/mutations/accept_enrollment_invitation.rb`).
    async fn accept_enrollment_invitation(
        &self,
        ctx: &Context<'_>,
        input: AcceptEnrollmentInvitationInput,
    ) -> Result<AcceptEnrollmentInvitationPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        accept_enrollment_invitation(&gctx.pool, gctx.auth_user.id, input).await
    }

    /// Canvas `rejectEnrollmentInvitation`
    /// (`app/graphql/mutations/reject_enrollment_invitation.rb`).
    async fn reject_enrollment_invitation(
        &self,
        ctx: &Context<'_>,
        input: RejectEnrollmentInvitationInput,
    ) -> Result<RejectEnrollmentInvitationPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        reject_enrollment_invitation(&gctx.pool, gctx.auth_user.id, input).await
    }

    // -----------------------------------------------------------------
    // Discussion topics
    // -----------------------------------------------------------------

    /// Canvas `createDiscussionTopic`
    /// (`app/graphql/mutations/create_discussion_topic.rb`).
    async fn create_discussion_topic(
        &self,
        ctx: &Context<'_>,
        input: CreateDiscussionTopicInput,
    ) -> Result<CreateDiscussionTopicPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        create_discussion_topic(&gctx.pool, gctx.auth_user.id, gctx.is_admin(), input).await
    }

    /// Canvas `updateDiscussionTopic`
    /// (`app/graphql/mutations/update_discussion_topic.rb`).
    async fn update_discussion_topic(
        &self,
        ctx: &Context<'_>,
        input: UpdateDiscussionTopicInput,
    ) -> Result<UpdateDiscussionTopicPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        update_discussion_topic(&gctx.pool, gctx.auth_user.id, gctx.is_admin(), input).await
    }

    /// Canvas `deleteDiscussionTopic`
    /// (`app/graphql/mutations/delete_discussion_topic.rb`).
    async fn delete_discussion_topic(
        &self,
        ctx: &Context<'_>,
        input: DeleteDiscussionTopicInput,
    ) -> Result<DeleteDiscussionTopicPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        delete_discussion_topic(&gctx.pool, gctx.auth_user.id, gctx.is_admin(), input).await
    }

    /// Canvas `subscribeToDiscussionTopic`
    /// (`app/graphql/mutations/subscribe_to_discussion_topic.rb`).
    async fn subscribe_to_discussion_topic(
        &self,
        ctx: &Context<'_>,
        input: SubscribeToDiscussionTopicInput,
    ) -> Result<SubscribeToDiscussionTopicPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        subscribe_to_discussion_topic(&gctx.pool, gctx.auth_user.id, input).await
    }

    // -----------------------------------------------------------------
    // Learning outcomes
    // -----------------------------------------------------------------

    /// Canvas `createLearningOutcome`
    /// (`app/graphql/mutations/create_learning_outcome.rb`).
    async fn create_learning_outcome(
        &self,
        ctx: &Context<'_>,
        input: CreateLearningOutcomeInput,
    ) -> Result<CreateLearningOutcomePayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        create_learning_outcome(&gctx.pool, gctx.is_admin(), input).await
    }

    /// Canvas `updateLearningOutcome`
    /// (`app/graphql/mutations/update_learning_outcome.rb`).
    async fn update_learning_outcome(
        &self,
        ctx: &Context<'_>,
        input: UpdateLearningOutcomeInput,
    ) -> Result<UpdateLearningOutcomePayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        update_learning_outcome(&gctx.pool, gctx.is_admin(), input).await
    }

    /// Canvas `createLearningOutcomeGroup`
    /// (`app/graphql/mutations/create_learning_outcome_group.rb`).
    async fn create_learning_outcome_group(
        &self,
        ctx: &Context<'_>,
        input: CreateLearningOutcomeGroupInput,
    ) -> Result<CreateLearningOutcomeGroupPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        create_learning_outcome_group(&gctx.pool, gctx.is_admin(), input).await
    }

    /// Canvas `updateLearningOutcomeGroup`
    /// (`app/graphql/mutations/update_learning_outcome_group.rb`).
    async fn update_learning_outcome_group(
        &self,
        ctx: &Context<'_>,
        input: UpdateLearningOutcomeGroupInput,
    ) -> Result<UpdateLearningOutcomeGroupPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        update_learning_outcome_group(&gctx.pool, gctx.is_admin(), input).await
    }

    // -----------------------------------------------------------------
    // Submission comments
    // -----------------------------------------------------------------

    /// Canvas `createSubmissionComment`
    /// (`app/graphql/mutations/create_submission_comment.rb`).
    async fn create_submission_comment(
        &self,
        ctx: &Context<'_>,
        input: CreateSubmissionCommentInput,
    ) -> Result<CreateSubmissionCommentPayload, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        create_submission_comment(&gctx.pool, gctx.auth_user.id, gctx.is_admin(), input).await
    }
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Canvas `DiscussionTopicContextType`
/// (`app/graphql/types/discussion_topic_context_type_type.rb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DiscussionTopicContextType {
    Course,
    Group,
}

/// Canvas `DiscussionTopicDiscussionType`
/// (`app/graphql/types/discussion_topic_discussion_type_type.rb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DiscussionTopicDiscussionType {
    #[graphql(name = "flat")]
    Flat,
    #[graphql(name = "not_threaded")]
    NotThreaded,
    #[graphql(name = "side_comment")]
    SideComment,
    #[graphql(name = "threaded")]
    Threaded,
}

impl DiscussionTopicDiscussionType {
    fn as_str(self) -> &'static str {
        match self {
            DiscussionTopicDiscussionType::Flat => "flat",
            DiscussionTopicDiscussionType::NotThreaded => "not_threaded",
            DiscussionTopicDiscussionType::SideComment => "side_comment",
            DiscussionTopicDiscussionType::Threaded => "threaded",
        }
    }
}

/// Canvas `DiscussionTopicAnonymousStateType`
/// (`app/graphql/types/discussion_topic_anonymous_state_type.rb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum DiscussionTopicAnonymousStateType {
    #[graphql(name = "full_anonymity")]
    FullAnonymity,
    #[graphql(name = "off")]
    Off,
    #[graphql(name = "partial_anonymity")]
    PartialAnonymity,
}

impl DiscussionTopicAnonymousStateType {
    fn as_str(self) -> &'static str {
        match self {
            DiscussionTopicAnonymousStateType::FullAnonymity => "full_anonymity",
            DiscussionTopicAnonymousStateType::Off => "off",
            DiscussionTopicAnonymousStateType::PartialAnonymity => "partial_anonymity",
        }
    }
}

/// Canvas `ProficiencyRatingInput`
/// (`app/graphql/types/proficiency_rating_type.rb`).
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "ProficiencyRatingInput")]
pub struct ProficiencyRatingInput {
    pub description: String,
    pub points: f64,
}

// ---------------------------------------------------------------------------
// Input types
// ---------------------------------------------------------------------------

/// Canvas `CreateInternalSettingInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "CreateInternalSettingInput")]
pub struct CreateInternalSettingInput {
    pub name: String,
    pub value: String,
}

/// Canvas `UpdateInternalSettingInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "UpdateInternalSettingInput")]
pub struct UpdateInternalSettingInput {
    pub internal_setting_id: ID,
    pub value: String,
}

/// Canvas `DeleteInternalSettingInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "DeleteInternalSettingInput")]
pub struct DeleteInternalSettingInput {
    pub internal_setting_id: ID,
}

/// Canvas `UpdateMyInboxSettingsInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "UpdateMyInboxSettingsInput")]
pub struct UpdateMyInboxSettingsInput {
    pub out_of_office_first_date: Option<String>,
    pub out_of_office_last_date: Option<String>,
    pub out_of_office_message: Option<String>,
    pub out_of_office_subject: Option<String>,
    pub signature: Option<String>,
    pub use_out_of_office: bool,
    pub use_signature: bool,
}

/// Canvas `DismissAccountNotificationInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "DismissAccountNotificationInput")]
pub struct DismissAccountNotificationInput {
    pub notification_id: ID,
}

/// Canvas `AcceptEnrollmentInvitationInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "AcceptEnrollmentInvitationInput")]
pub struct AcceptEnrollmentInvitationInput {
    pub enrollment_uuid: String,
}

/// Canvas `RejectEnrollmentInvitationInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "RejectEnrollmentInvitationInput")]
pub struct RejectEnrollmentInvitationInput {
    pub enrollment_uuid: String,
}

/// Canvas `CreateDiscussionTopicInput`.
///
/// The Canvas input carries ~20 fields; the ones star-db has no column for
/// (`assignment`, `checkpoints`, `podcastEnabled`, `sortOrder`, ...) are
/// accepted for client compatibility and ignored on write, exactly as the
/// REST discussion handlers ignore them.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "CreateDiscussionTopicInput")]
pub struct CreateDiscussionTopicInput {
    pub context_id: ID,
    pub context_type: DiscussionTopicContextType,
    pub title: Option<String>,
    pub message: Option<String>,
    pub published: Option<bool>,
    pub discussion_type: Option<DiscussionTopicDiscussionType>,
    pub anonymous_state: Option<DiscussionTopicAnonymousStateType>,
    pub is_anonymous_author: Option<bool>,
    pub is_announcement: Option<bool>,
    pub locked: Option<bool>,
    pub todo_date: Option<DateTime<Utc>>,
    pub delayed_post_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    /// Accepted-for-shape only: star-db's `discussion_topics` table has no
    /// group category link.
    #[allow(dead_code)]
    pub group_category_id: Option<ID>,
    /// Accepted-for-shape only: no attachment column.
    #[allow(dead_code)]
    pub file_id: Option<ID>,
    /// Accepted-for-shape only: no section-visibility column.
    #[allow(dead_code)]
    pub specific_sections: Option<String>,
    /// Accepted-for-shape only: no `allow_rating` column.
    #[allow(dead_code)]
    pub allow_rating: Option<bool>,
    /// Accepted-for-shape only: no `expanded` column.
    #[allow(dead_code)]
    pub expanded: Option<bool>,
    /// Accepted-for-shape only: no `expanded_locked` column.
    #[allow(dead_code)]
    pub expanded_locked: Option<bool>,
    /// Accepted-for-shape only: no `only_visible_to_overrides` column.
    #[allow(dead_code)]
    pub only_visible_to_overrides: Option<bool>,
    /// Accepted-for-shape only: no `require_initial_post` column.
    #[allow(dead_code)]
    pub require_initial_post: Option<bool>,
    /// Accepted-for-shape only: no `sort_order` column.
    #[allow(dead_code)]
    pub sort_order: Option<String>,
    /// Accepted-for-shape only: no `sort_order_locked` column.
    #[allow(dead_code)]
    pub sort_order_locked: Option<bool>,
    /// Accepted-for-shape only: no `podcast_enabled` column.
    #[allow(dead_code)]
    pub podcast_enabled: Option<bool>,
    /// Accepted-for-shape only: no `podcast_has_student_posts` column.
    #[allow(dead_code)]
    pub podcast_has_student_posts: Option<bool>,
    /// Accepted-for-shape only: no `only_graders_can_rate` column.
    #[allow(dead_code)]
    pub only_graders_can_rate: Option<bool>,
}

/// Canvas `UpdateDiscussionTopicInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "UpdateDiscussionTopicInput")]
pub struct UpdateDiscussionTopicInput {
    pub discussion_topic_id: ID,
    pub title: Option<String>,
    pub message: Option<String>,
    pub published: Option<bool>,
    pub discussion_type: Option<DiscussionTopicDiscussionType>,
    pub anonymous_state: Option<DiscussionTopicAnonymousStateType>,
    pub locked: Option<bool>,
    pub todo_date: Option<DateTime<Utc>>,
    pub delayed_post_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    pub notify_users: Option<bool>,
    /// Accepted-for-shape only: no attachment column.
    #[allow(dead_code)]
    pub remove_attachment: Option<bool>,
    /// Accepted-for-shape only: no `assignment` link.
    #[allow(dead_code)]
    pub assignment: Option<String>,
    /// Accepted-for-shape only: no `checkpoints` column.
    #[allow(dead_code)]
    pub checkpoints: Option<Vec<String>>,
    /// Accepted-for-shape only: no `set_checkpoints` column.
    #[allow(dead_code)]
    pub set_checkpoints: Option<bool>,
    /// Accepted-for-shape only: no group category link.
    #[allow(dead_code)]
    pub group_category_id: Option<ID>,
    /// Accepted-for-shape only: no attachment column.
    #[allow(dead_code)]
    pub file_id: Option<ID>,
    /// Accepted-for-shape only: no section-visibility column.
    #[allow(dead_code)]
    pub specific_sections: Option<String>,
    /// Accepted-for-shape only: no `allow_rating` column.
    #[allow(dead_code)]
    pub allow_rating: Option<bool>,
    /// Accepted-for-shape only: no `expanded` column.
    #[allow(dead_code)]
    pub expanded: Option<bool>,
    /// Accepted-for-shape only: no `expanded_locked` column.
    #[allow(dead_code)]
    pub expanded_locked: Option<bool>,
    /// Accepted-for-shape only: no `only_visible_to_overrides` column.
    #[allow(dead_code)]
    pub only_visible_to_overrides: Option<bool>,
    /// Accepted-for-shape only: no `require_initial_post` column.
    #[allow(dead_code)]
    pub require_initial_post: Option<bool>,
    /// Accepted-for-shape only: no `sort_order` column.
    #[allow(dead_code)]
    pub sort_order: Option<String>,
    /// Accepted-for-shape only: no `sort_order_locked` column.
    #[allow(dead_code)]
    pub sort_order_locked: Option<bool>,
    /// Accepted-for-shape only: no `podcast_enabled` column.
    #[allow(dead_code)]
    pub podcast_enabled: Option<bool>,
    /// Accepted-for-shape only: no `podcast_has_student_posts` column.
    #[allow(dead_code)]
    pub podcast_has_student_posts: Option<bool>,
    /// Accepted-for-shape only: no `only_graders_can_rate` column.
    #[allow(dead_code)]
    pub only_graders_can_rate: Option<bool>,
}

/// Canvas `DeleteDiscussionTopicInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "DeleteDiscussionTopicInput")]
pub struct DeleteDiscussionTopicInput {
    pub id: ID,
}

/// Canvas `SubscribeToDiscussionTopicInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "SubscribeToDiscussionTopicInput")]
pub struct SubscribeToDiscussionTopicInput {
    pub discussion_topic_id: ID,
    pub subscribed: bool,
}

/// Canvas `CreateLearningOutcomeInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "CreateLearningOutcomeInput")]
pub struct CreateLearningOutcomeInput {
    pub group_id: ID,
    pub title: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub calculation_int: Option<i32>,
    pub calculation_method: Option<String>,
    pub mastery_points: Option<f64>,
    pub vendor_guid: Option<String>,
    /// Accepted-for-shape only: star-db stores the rating scale on the
    /// outcome's proficiency ratings, which this mutation does not write.
    #[allow(dead_code)]
    pub ratings: Option<Vec<ProficiencyRatingInput>>,
}

/// Canvas `UpdateLearningOutcomeInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "UpdateLearningOutcomeInput")]
pub struct UpdateLearningOutcomeInput {
    pub id: ID,
    pub title: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub calculation_int: Option<i32>,
    pub calculation_method: Option<String>,
    pub mastery_points: Option<f64>,
    pub vendor_guid: Option<String>,
    /// Accepted-for-shape only: see `CreateLearningOutcomeInput.ratings`.
    #[allow(dead_code)]
    pub ratings: Option<Vec<ProficiencyRatingInput>>,
}

/// Canvas `CreateLearningOutcomeGroupInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "CreateLearningOutcomeGroupInput")]
pub struct CreateLearningOutcomeGroupInput {
    pub id: ID,
    pub title: String,
    pub description: Option<String>,
    pub vendor_guid: Option<String>,
}

/// Canvas `UpdateLearningOutcomeGroupInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "UpdateLearningOutcomeGroupInput")]
pub struct UpdateLearningOutcomeGroupInput {
    pub id: ID,
    pub title: Option<String>,
    pub description: Option<String>,
    pub vendor_guid: Option<String>,
    /// Accepted-for-shape only: star-db's `learning_outcome_groups` table
    /// has no parent link yet.
    #[allow(dead_code)]
    pub parent_outcome_group_id: Option<ID>,
}

/// Canvas `CreateSubmissionCommentInput`.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "CreateSubmissionCommentInput")]
pub struct CreateSubmissionCommentInput {
    pub submission_id: ID,
    pub comment: String,
    pub attempt: Option<i32>,
    #[graphql(default = false)]
    pub draft_comment: bool,
    #[graphql(default = false)]
    pub group_comment: bool,
    /// Accepted-for-shape only: star-db's `submission_comments` table has no
    /// attachment link.
    #[allow(dead_code)]
    pub file_ids: Option<Vec<ID>>,
    /// Accepted-for-shape only: no media comment column.
    #[allow(dead_code)]
    pub media_object_id: Option<ID>,
    /// Accepted-for-shape only: no media comment column.
    #[allow(dead_code)]
    pub media_object_type: Option<String>,
    /// Accepted-for-shape only: no assessment request column.
    #[allow(dead_code)]
    pub reviewer_submission_id: Option<ID>,
}

// ---------------------------------------------------------------------------
// Payload types
// ---------------------------------------------------------------------------

/// Canvas `CreateInternalSettingPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct CreateInternalSettingPayload {
    pub internal_setting: Option<InternalSetting>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `UpdateInternalSettingPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct UpdateInternalSettingPayload {
    pub internal_setting: Option<InternalSetting>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `DeleteInternalSettingPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct DeleteInternalSettingPayload {
    pub internal_setting_id: Option<ID>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `UpdateMyInboxSettingsPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct UpdateMyInboxSettingsPayload {
    pub my_inbox_settings: Option<crate::root_types::InboxSettings>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `DismissAccountNotificationPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct DismissAccountNotificationPayload {
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `AcceptEnrollmentInvitationPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct AcceptEnrollmentInvitationPayload {
    pub enrollment: Option<crate::Enrollment>,
    pub success: bool,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `RejectEnrollmentInvitationPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct RejectEnrollmentInvitationPayload {
    pub enrollment: Option<crate::Enrollment>,
    pub success: bool,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `CreateDiscussionTopicPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct CreateDiscussionTopicPayload {
    pub discussion_topic: Option<crate::discussion_topic::DiscussionTopic>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `UpdateDiscussionTopicPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct UpdateDiscussionTopicPayload {
    pub discussion_topic: Option<crate::discussion_topic::DiscussionTopic>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `DeleteDiscussionTopicPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct DeleteDiscussionTopicPayload {
    pub discussion_topic_id: Option<ID>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `SubscribeToDiscussionTopicPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct SubscribeToDiscussionTopicPayload {
    pub discussion_topic: Option<crate::discussion_topic::DiscussionTopic>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `CreateLearningOutcomePayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct CreateLearningOutcomePayload {
    pub learning_outcome: Option<LearningOutcome>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `UpdateLearningOutcomePayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct UpdateLearningOutcomePayload {
    pub learning_outcome: Option<LearningOutcome>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `CreateLearningOutcomeGroupPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct CreateLearningOutcomeGroupPayload {
    pub learning_outcome_group: Option<LearningOutcomeGroup>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `UpdateLearningOutcomeGroupPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct UpdateLearningOutcomeGroupPayload {
    pub learning_outcome_group: Option<LearningOutcomeGroup>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `CreateSubmissionCommentPayload`.
#[derive(Debug, Clone, SimpleObject)]
pub struct CreateSubmissionCommentPayload {
    pub submission_comment: Option<SubmissionComment>,
    pub errors: Option<Vec<ValidationError>>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn nonempty(errors: Vec<ValidationError>) -> Option<Vec<ValidationError>> {
    if errors.is_empty() {
        None
    } else {
        Some(errors)
    }
}

/// Parse a Canvas id argument that names the expected type, turning a parse
/// failure into Canvas' "expected an id for <Type>" error.
fn parse_typed_id(raw: &str, expected: &str) -> Result<i64, async_graphql::Error> {
    parse_relay_or_legacy_id(raw, expected)
}

// ---------------------------------------------------------------------------
// Internal settings
// ---------------------------------------------------------------------------

/// `createInternalSetting(input: CreateInternalSettingInput!)`.
pub async fn create_internal_setting(
    pool: &PgPool,
    is_admin: bool,
    input: CreateInternalSettingInput,
) -> Result<CreateInternalSettingPayload, async_graphql::Error> {
    if !is_admin {
        return Err(async_graphql::Error::new("insufficient permission"));
    }

    // Canvas' `Setting.set(name, value)` upserts by name.
    let row: Option<crate::root_data::InternalSettingRow> = sqlx::query_as(
        r#"
        INSERT INTO settings (name, value, created_at, updated_at)
        SELECT $1, $2, now(), now()
        WHERE NOT EXISTS (SELECT 1 FROM settings WHERE name = $1)
        RETURNING id, name, value, secret, created_at, updated_at
        "#,
    )
    .bind(&input.name)
    .bind(&input.value)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let row = match row {
        Some(row) => row,
        None => sqlx::query_as(
            r#"
            UPDATE settings SET value = $2, updated_at = now()
            WHERE name = $1
            RETURNING id, name, value, secret, created_at, updated_at
            "#,
        )
        .bind(&input.name)
        .bind(&input.value)
        .fetch_one(pool)
        .await
        .map_err(db_err)?,
    };

    Ok(CreateInternalSettingPayload {
        internal_setting: Some(crate::root_data::internal_setting_from_row(row)),
        errors: None,
    })
}

/// `updateInternalSetting(input: UpdateInternalSettingInput!)`.
pub async fn update_internal_setting(
    pool: &PgPool,
    is_admin: bool,
    input: UpdateInternalSettingInput,
) -> Result<UpdateInternalSettingPayload, async_graphql::Error> {
    let setting_id = parse_typed_id(&input.internal_setting_id.0, "InternalSetting")?;

    let existing: Option<(String, bool)> =
        sqlx::query_as("SELECT name, secret FROM settings WHERE id = $1")
            .bind(setting_id)
            .fetch_optional(pool)
            .await
            .map_err(db_err)?;

    let Some((_name, secret)) = existing else {
        return Err(async_graphql::Error::new("not found"));
    };
    if !is_admin || secret {
        return Err(async_graphql::Error::new("insufficient permission"));
    }

    let row: crate::root_data::InternalSettingRow = sqlx::query_as(
        r#"
        UPDATE settings SET value = $2, updated_at = now()
        WHERE id = $1
        RETURNING id, name, value, secret, created_at, updated_at
        "#,
    )
    .bind(setting_id)
    .bind(&input.value)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(UpdateInternalSettingPayload {
        internal_setting: Some(crate::root_data::internal_setting_from_row(row)),
        errors: None,
    })
}

/// `deleteInternalSetting(input: DeleteInternalSettingInput!)`.
pub async fn delete_internal_setting(
    pool: &PgPool,
    is_admin: bool,
    input: DeleteInternalSettingInput,
) -> Result<DeleteInternalSettingPayload, async_graphql::Error> {
    let setting_id = parse_typed_id(&input.internal_setting_id.0, "InternalSetting")?;

    let existing: Option<(String, bool)> =
        sqlx::query_as("SELECT name, secret FROM settings WHERE id = $1")
            .bind(setting_id)
            .fetch_optional(pool)
            .await
            .map_err(db_err)?;

    let Some((_name, secret)) = existing else {
        return Err(async_graphql::Error::new("not found"));
    };
    if !is_admin || secret {
        return Err(async_graphql::Error::new("insufficient permission"));
    }

    sqlx::query("DELETE FROM settings WHERE id = $1")
        .bind(setting_id)
        .execute(pool)
        .await
        .map_err(db_err)?;

    Ok(DeleteInternalSettingPayload {
        internal_setting_id: Some(ID(setting_id.to_string())),
        errors: None,
    })
}

// ---------------------------------------------------------------------------
// Inbox settings
// ---------------------------------------------------------------------------

/// `updateMyInboxSettings(input: UpdateMyInboxSettingsInput!)`.
pub async fn update_my_inbox_settings(
    pool: &PgPool,
    user_id: i64,
    input: UpdateMyInboxSettingsInput,
) -> Result<UpdateMyInboxSettingsPayload, async_graphql::Error> {
    let first_date = parse_inbox_date(input.out_of_office_first_date.as_deref());
    let last_date = parse_inbox_date(input.out_of_office_last_date.as_deref());

    let row: crate::root_data::InboxSettingsRow = sqlx::query_as(
        r#"
        INSERT INTO inbox_settings (
            user_id, use_signature, signature, use_out_of_office,
            out_of_office_first_date, out_of_office_last_date,
            out_of_office_subject, out_of_office_message,
            root_account_id, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8,
                COALESCE((SELECT root_account_id FROM accounts ORDER BY id LIMIT 1), 1),
                now(), now())
        ON CONFLICT (user_id) DO UPDATE SET
            use_signature = EXCLUDED.use_signature,
            signature = EXCLUDED.signature,
            use_out_of_office = EXCLUDED.use_out_of_office,
            out_of_office_first_date = EXCLUDED.out_of_office_first_date,
            out_of_office_last_date = EXCLUDED.out_of_office_last_date,
            out_of_office_subject = EXCLUDED.out_of_office_subject,
            out_of_office_message = EXCLUDED.out_of_office_message,
            updated_at = now()
        RETURNING id, user_id, use_signature, signature, use_out_of_office,
                  out_of_office_first_date, out_of_office_last_date,
                  out_of_office_subject, out_of_office_message,
                  created_at, updated_at
        "#,
    )
    .bind(user_id.to_string())
    .bind(input.use_signature)
    .bind(&input.signature)
    .bind(input.use_out_of_office)
    .bind(first_date)
    .bind(last_date)
    .bind(&input.out_of_office_subject)
    .bind(&input.out_of_office_message)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(UpdateMyInboxSettingsPayload {
        my_inbox_settings: crate::root_data::inbox_settings_from_row(row),
        errors: None,
    })
}

/// Canvas passes the out-of-office dates as strings; Canvas' inbox service
/// stores them as dates.
fn parse_inbox_date(raw: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = raw?;
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return Some(parsed.with_timezone(&Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(
            date.and_hms_opt(0, 0, 0)?,
            Utc,
        ));
    }
    None
}

// ---------------------------------------------------------------------------
// Account notifications
// ---------------------------------------------------------------------------

/// `dismissAccountNotification(input: DismissAccountNotificationInput!)`.
///
/// Canvas stores the dismissed notification in the user's
/// `closed_notifications` preference.
pub async fn dismiss_account_notification(
    pool: &PgPool,
    user_id: i64,
    input: DismissAccountNotificationInput,
) -> Result<DismissAccountNotificationPayload, async_graphql::Error> {
    let notification_id = parse_typed_id(&input.notification_id.0, "AccountNotification")?;

    let owned: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM account_notifications WHERE id = $1 AND user_id = $2")
            .bind(notification_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await
            .map_err(db_err)?;

    if owned.is_none() {
        return Err(async_graphql::Error::new("Notification not found"));
    }

    sqlx::query(
        r#"
        UPDATE users
        SET preferences = jsonb_set(
                preferences,
                '{closed_notifications}',
                COALESCE(preferences->'closed_notifications', '[]'::jsonb)
                    || to_jsonb($2::bigint),
                true
            ),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(user_id)
    .bind(notification_id)
    .execute(pool)
    .await
    .map_err(db_err)?;

    Ok(DismissAccountNotificationPayload { errors: None })
}

// ---------------------------------------------------------------------------
// Enrollment invitations
// ---------------------------------------------------------------------------

/// `acceptEnrollmentInvitation(input: AcceptEnrollmentInvitationInput!)`.
pub async fn accept_enrollment_invitation(
    pool: &PgPool,
    user_id: i64,
    input: AcceptEnrollmentInvitationInput,
) -> Result<AcceptEnrollmentInvitationPayload, async_graphql::Error> {
    let enrollment = find_invitation(pool, user_id, &input.enrollment_uuid).await?;
    let enrollment_id = enrollment.id;

    sqlx::query(
        "UPDATE enrollments SET workflow_state = 'active', updated_at = now() WHERE id = $1",
    )
    .bind(enrollment_id)
    .execute(pool)
    .await
    .map_err(db_err)?;

    let row = fetch_enrollment_row(pool, enrollment_id).await?;
    Ok(AcceptEnrollmentInvitationPayload {
        enrollment: row.map(crate::Enrollment::from),
        success: true,
        errors: None,
    })
}

/// `rejectEnrollmentInvitation(input: RejectEnrollmentInvitationInput!)`.
pub async fn reject_enrollment_invitation(
    pool: &PgPool,
    user_id: i64,
    input: RejectEnrollmentInvitationInput,
) -> Result<RejectEnrollmentInvitationPayload, async_graphql::Error> {
    let enrollment = find_invitation(pool, user_id, &input.enrollment_uuid).await?;
    let enrollment_id = enrollment.id;

    sqlx::query(
        "UPDATE enrollments SET workflow_state = 'rejected', updated_at = now() WHERE id = $1",
    )
    .bind(enrollment_id)
    .execute(pool)
    .await
    .map_err(db_err)?;

    let row = fetch_enrollment_row(pool, enrollment_id).await?;
    Ok(RejectEnrollmentInvitationPayload {
        enrollment: row.map(crate::Enrollment::from),
        success: true,
        errors: None,
    })
}

/// The enrollment behind an invitation uuid, with Canvas' checks.
async fn find_invitation(
    pool: &PgPool,
    user_id: i64,
    enrollment_uuid: &str,
) -> Result<InvitationRow, async_graphql::Error> {
    let row: Option<InvitationRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, workflow_state
        FROM enrollments
        WHERE uuid = $1
        "#,
    )
    .bind(enrollment_uuid)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let Some(row) = row else {
        return Err(async_graphql::Error::new("Enrollment invitation not found"));
    };
    if row.user_id != user_id {
        return Err(async_graphql::Error::new("Unauthorized"));
    }
    if row.workflow_state != "invited" {
        return Err(async_graphql::Error::new(
            "Enrollment is not in invited state",
        ));
    }
    Ok(row)
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct InvitationRow {
    id: i64,
    user_id: i64,
    workflow_state: String,
}

async fn fetch_enrollment_row(
    pool: &PgPool,
    id: i64,
) -> Result<Option<crate::EnrollmentRow>, async_graphql::Error> {
    let row: Option<crate::EnrollmentRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, course_id, role, workflow_state, created_at, updated_at
        FROM enrollments
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Discussion topics
// ---------------------------------------------------------------------------

/// `createDiscussionTopic(input: CreateDiscussionTopicInput!)`.
pub async fn create_discussion_topic(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    input: CreateDiscussionTopicInput,
) -> Result<CreateDiscussionTopicPayload, async_graphql::Error> {
    let context_id = parse_typed_id(&input.context_id.0, "Context")?;

    // Canvas: `find_context` — an unknown context is a validation error.
    let course_id: Option<(i64,)> = match input.context_type {
        DiscussionTopicContextType::Course => {
            sqlx::query_as("SELECT id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
                .bind(context_id)
                .fetch_optional(pool)
                .await
                .map_err(db_err)?
        }
        DiscussionTopicContextType::Group => {
            sqlx::query_as("SELECT id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'")
                .bind(context_id)
                .fetch_optional(pool)
                .await
                .map_err(db_err)?
        }
    };
    let Some((course_id,)) = course_id else {
        return Ok(CreateDiscussionTopicPayload {
            discussion_topic: None,
            errors: nonempty(vec![ValidationError {
                attribute: Some("contextId".to_string()),
                message: "Invalid context".to_string(),
            }]),
        });
    };

    // Canvas: `verify_authorized_action!(discussion_topic, :create)` — a
    // teacher of the context course, or an admin.
    if !is_admin && !crate::user_teaches_course(pool, user_id, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }

    // Canvas: anonymous state is only kept for the two anonymity modes.
    let anonymous_state = match input.anonymous_state {
        Some(state)
            if state != DiscussionTopicAnonymousStateType::Off
                && state != DiscussionTopicAnonymousStateType::PartialAnonymity
                && state != DiscussionTopicAnonymousStateType::FullAnonymity =>
        {
            None
        }
        other => other.map(|state| state.as_str().to_string()),
    };

    let is_announcement = input.is_announcement.unwrap_or(false);
    let workflow_state = if input.published.unwrap_or(false) || is_announcement {
        "active"
    } else {
        "unpublished"
    };
    let discussion_type = input
        .discussion_type
        .map(|t| t.as_str().to_string())
        .unwrap_or_else(|| "threaded".to_string());

    let row: crate::discussion_topic::TopicRow = sqlx::query_as(
        r#"
        INSERT INTO discussion_topics (
            course_id, title, message, workflow_state, discussion_type,
            locked, user_id, todo_date, delayed_post_at, lock_at, anonymous_state
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, course_id, title, message, workflow_state, discussion_type,
                  locked, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(input.title.as_deref().unwrap_or(""))
    .bind(&input.message)
    .bind(workflow_state)
    .bind(&discussion_type)
    .bind(input.locked.unwrap_or(false))
    .bind(user_id)
    .bind(input.todo_date)
    .bind(input.delayed_post_at)
    .bind(input.lock_at)
    .bind(&anonymous_state)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(CreateDiscussionTopicPayload {
        discussion_topic: Some(row.into()),
        errors: None,
    })
}

/// `updateDiscussionTopic(input: UpdateDiscussionTopicInput!)`.
pub async fn update_discussion_topic(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    input: UpdateDiscussionTopicInput,
) -> Result<UpdateDiscussionTopicPayload, async_graphql::Error> {
    let topic_id = parse_typed_id(&input.discussion_topic_id.0, "DiscussionTopic")?;

    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let Some((course_id,)) = existing else {
        return Err(async_graphql::Error::new("not found"));
    };
    if !is_admin && !crate::user_teaches_course(pool, user_id, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }

    // Canvas only touches the fields the input carries.
    sqlx::query(
        r#"
        UPDATE discussion_topics
        SET title = COALESCE($2, title),
            message = COALESCE($3, message),
            workflow_state = COALESCE($4, workflow_state),
            discussion_type = COALESCE($5, discussion_type),
            locked = COALESCE($6, locked),
            todo_date = COALESCE($7, todo_date),
            delayed_post_at = COALESCE($8, delayed_post_at),
            lock_at = COALESCE($9, lock_at),
            anonymous_state = COALESCE($10, anonymous_state),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(topic_id)
    .bind(input.title.as_deref())
    .bind(input.message.as_deref())
    .bind(
        input
            .published
            .map(|p| if p { "active" } else { "unpublished" }),
    )
    .bind(input.discussion_type.map(|t| t.as_str()))
    .bind(input.locked)
    .bind(input.todo_date)
    .bind(input.delayed_post_at)
    .bind(input.lock_at)
    .bind(input.anonymous_state.map(|s| s.as_str()))
    .execute(pool)
    .await
    .map_err(db_err)?;

    let row = crate::discussion_topic::fetch_discussion_topic(pool, topic_id)
        .await?
        .ok_or_else(|| async_graphql::Error::new("not found"))?;

    Ok(UpdateDiscussionTopicPayload {
        discussion_topic: Some(row),
        errors: None,
    })
}

/// `deleteDiscussionTopic(input: DeleteDiscussionTopicInput!)`.
///
/// Canvas deletes the row; star-lms soft-deletes it, keeping the id in the
/// payload so the client can drop it from its list.
pub async fn delete_discussion_topic(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    input: DeleteDiscussionTopicInput,
) -> Result<DeleteDiscussionTopicPayload, async_graphql::Error> {
    let topic_id = parse_typed_id(&input.id.0, "DiscussionTopic")?;

    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let Some((course_id,)) = existing else {
        return Err(async_graphql::Error::new("Unable to find Discussion Topic"));
    };
    if !is_admin && !crate::user_teaches_course(pool, user_id, course_id).await? {
        return Err(async_graphql::Error::new("Insufficient permissions"));
    }

    sqlx::query(
        "UPDATE discussion_topics SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(topic_id)
    .execute(pool)
    .await
    .map_err(db_err)?;

    Ok(DeleteDiscussionTopicPayload {
        discussion_topic_id: Some(ID(topic_id.to_string())),
        errors: None,
    })
}

/// `subscribeToDiscussionTopic(input: SubscribeToDiscussionTopicInput!)`.
pub async fn subscribe_to_discussion_topic(
    pool: &PgPool,
    user_id: i64,
    input: SubscribeToDiscussionTopicInput,
) -> Result<SubscribeToDiscussionTopicPayload, async_graphql::Error> {
    let topic_id = parse_typed_id(&input.discussion_topic_id.0, "DiscussionTopic")?;

    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let Some((course_id,)) = existing else {
        return Err(async_graphql::Error::new("not found"));
    };
    if !crate::user_enrolled_in_course(pool, user_id, course_id).await?
        && !crate::user_teaches_course(pool, user_id, course_id).await?
    {
        return Err(async_graphql::Error::new("unauthorized"));
    }

    sqlx::query(
        r#"
        INSERT INTO discussion_topic_participants (
            discussion_topic_id, user_id, unread_entry_count, workflow_state,
            subscribed, root_account_id, created_at, updated_at
        )
        VALUES ($1, $2, 0, 'active', $3,
                COALESCE((SELECT root_account_id FROM accounts ORDER BY id LIMIT 1), 1),
                now(), now())
        ON CONFLICT (discussion_topic_id, user_id) DO UPDATE
            SET subscribed = EXCLUDED.subscribed, updated_at = now()
        "#,
    )
    .bind(topic_id)
    .bind(user_id)
    .bind(input.subscribed)
    .execute(pool)
    .await
    .map_err(db_err)?;

    let row = crate::discussion_topic::fetch_discussion_topic(pool, topic_id)
        .await?
        .ok_or_else(|| async_graphql::Error::new("not found"))?;

    Ok(SubscribeToDiscussionTopicPayload {
        discussion_topic: Some(row),
        errors: None,
    })
}

// ---------------------------------------------------------------------------
// Learning outcomes
// ---------------------------------------------------------------------------

/// `createLearningOutcome(input: CreateLearningOutcomeInput!)`.
pub async fn create_learning_outcome(
    pool: &PgPool,
    is_admin: bool,
    input: CreateLearningOutcomeInput,
) -> Result<CreateLearningOutcomePayload, async_graphql::Error> {
    let group_id = parse_typed_id(&input.group_id.0, "LearningOutcomeGroup")?;

    let group: Option<(i64, Option<i64>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, context_id, context_type
        FROM learning_outcome_groups
        WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let Some((_id, context_id, context_type)) = group else {
        return Err(async_graphql::Error::new("group not found"));
    };
    if !is_admin {
        return Err(async_graphql::Error::new("insufficient permission"));
    }

    let row: crate::root_data::LearningOutcomeRow = sqlx::query_as(
        r#"
        INSERT INTO learning_outcomes (
            title, description, display_name, context_id, context_type,
            calculation_int, calculation_method, mastery_points, vendor_guid,
            workflow_state, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'active', now(), now())
        RETURNING id, title, description, display_name, context_id, context_type,
                  calculation_int, calculation_method, mastery_points,
                  points_possible, assessed, vendor_guid, workflow_state,
                  created_at, updated_at
        "#,
    )
    .bind(&input.title)
    .bind(&input.description)
    .bind(&input.display_name)
    .bind(context_id)
    .bind(&context_type)
    .bind(input.calculation_int)
    .bind(&input.calculation_method)
    .bind(input.mastery_points)
    .bind(&input.vendor_guid)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    // Canvas links the outcome into the group (`outcome_group.add_outcome`).
    sqlx::query(
        r#"
        INSERT INTO learning_outcome_links (outcome_id, outcome_group_id, workflow_state)
        VALUES ($1, $2, 'active')
        ON CONFLICT (outcome_id, outcome_group_id) DO NOTHING
        "#,
    )
    .bind(row.id)
    .bind(group_id)
    .execute(pool)
    .await
    .map_err(db_err)?;

    Ok(CreateLearningOutcomePayload {
        learning_outcome: Some(crate::root_data::learning_outcome_from_row(row)),
        errors: None,
    })
}

/// `updateLearningOutcome(input: UpdateLearningOutcomeInput!)`.
pub async fn update_learning_outcome(
    pool: &PgPool,
    is_admin: bool,
    input: UpdateLearningOutcomeInput,
) -> Result<UpdateLearningOutcomePayload, async_graphql::Error> {
    let outcome_id = parse_typed_id(&input.id.0, "LearningOutcome")?;

    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM learning_outcomes WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(outcome_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    if existing.is_none() {
        return Err(async_graphql::Error::new(format!(
            "unable to find LearningOutcome for id {outcome_id}"
        )));
    }
    if !is_admin {
        return Err(async_graphql::Error::new("insufficient permissions"));
    }

    let row: crate::root_data::LearningOutcomeRow = sqlx::query_as(
        r#"
        UPDATE learning_outcomes
        SET title = $2,
            description = COALESCE($3, description),
            display_name = COALESCE($4, display_name),
            calculation_int = COALESCE($5, calculation_int),
            calculation_method = COALESCE($6, calculation_method),
            mastery_points = COALESCE($7, mastery_points),
            vendor_guid = COALESCE($8, vendor_guid),
            updated_at = now()
        WHERE id = $1
        RETURNING id, title, description, display_name, context_id, context_type,
                  calculation_int, calculation_method, mastery_points,
                  points_possible, assessed, vendor_guid, workflow_state,
                  created_at, updated_at
        "#,
    )
    .bind(outcome_id)
    .bind(&input.title)
    .bind(&input.description)
    .bind(&input.display_name)
    .bind(input.calculation_int)
    .bind(&input.calculation_method)
    .bind(input.mastery_points)
    .bind(&input.vendor_guid)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(UpdateLearningOutcomePayload {
        learning_outcome: Some(crate::root_data::learning_outcome_from_row(row)),
        errors: None,
    })
}

/// `createLearningOutcomeGroup(input: CreateLearningOutcomeGroupInput!)`.
///
/// Canvas creates a **child** of the group named by `id`.
pub async fn create_learning_outcome_group(
    pool: &PgPool,
    is_admin: bool,
    input: CreateLearningOutcomeGroupInput,
) -> Result<CreateLearningOutcomeGroupPayload, async_graphql::Error> {
    let parent_id = parse_typed_id(&input.id.0, "LearningOutcomeGroup")?;

    let parent: Option<(i64, Option<i64>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, context_id, context_type
        FROM learning_outcome_groups
        WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(parent_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let Some((_id, context_id, context_type)) = parent else {
        return Err(async_graphql::Error::new("Group not found"));
    };
    if !is_admin {
        return Err(async_graphql::Error::new("Insufficient permissions"));
    }

    let row: crate::root_data::LearningOutcomeGroupRow = sqlx::query_as(
        r#"
        INSERT INTO learning_outcome_groups (
            title, description, context_id, context_type, vendor_guid,
            workflow_state, created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, 'active', now(), now())
        RETURNING id, title, description, context_id, context_type, vendor_guid,
                  workflow_state, created_at, updated_at
        "#,
    )
    .bind(&input.title)
    .bind(&input.description)
    .bind(context_id)
    .bind(&context_type)
    .bind(&input.vendor_guid)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(CreateLearningOutcomeGroupPayload {
        learning_outcome_group: Some(crate::root_data::learning_outcome_group_from_row(row)),
        errors: None,
    })
}

/// `updateLearningOutcomeGroup(input: UpdateLearningOutcomeGroupInput!)`.
pub async fn update_learning_outcome_group(
    pool: &PgPool,
    is_admin: bool,
    input: UpdateLearningOutcomeGroupInput,
) -> Result<UpdateLearningOutcomeGroupPayload, async_graphql::Error> {
    let group_id = parse_typed_id(&input.id.0, "LearningOutcomeGroup")?;

    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM learning_outcome_groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    if existing.is_none() {
        return Err(async_graphql::Error::new("Group not found"));
    }
    if !is_admin {
        return Err(async_graphql::Error::new("Insufficient permissions"));
    }

    let row: crate::root_data::LearningOutcomeGroupRow = sqlx::query_as(
        r#"
        UPDATE learning_outcome_groups
        SET title = COALESCE($2, title),
            description = COALESCE($3, description),
            vendor_guid = COALESCE($4, vendor_guid),
            updated_at = now()
        WHERE id = $1
        RETURNING id, title, description, context_id, context_type, vendor_guid,
                  workflow_state, created_at, updated_at
        "#,
    )
    .bind(group_id)
    .bind(input.title.as_deref())
    .bind(input.description.as_deref())
    .bind(input.vendor_guid.as_deref())
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(UpdateLearningOutcomeGroupPayload {
        learning_outcome_group: Some(crate::root_data::learning_outcome_group_from_row(row)),
        errors: None,
    })
}

// ---------------------------------------------------------------------------
// Submission comments
// ---------------------------------------------------------------------------

/// `createSubmissionComment(input: CreateSubmissionCommentInput!)`.
pub async fn create_submission_comment(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    input: CreateSubmissionCommentInput,
) -> Result<CreateSubmissionCommentPayload, async_graphql::Error> {
    let submission_id = parse_typed_id(&input.submission_id.0, "Submission")?;

    let submission: Option<(i64, i64)> =
        sqlx::query_as("SELECT id, user_id FROM submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_optional(pool)
            .await
            .map_err(db_err)?;

    let Some((_id, submission_user_id)) = submission else {
        return Err(async_graphql::Error::new("not found"));
    };

    // Canvas: `verify_authorized_action!(submission, :comment)` — the
    // submission's author, a teacher of its course, or an admin.
    let author = submission_user_id == user_id;
    if !author && !is_admin {
        let assignment_id: i64 =
            sqlx::query_scalar("SELECT assignment_id FROM submissions WHERE id = $1")
                .bind(submission_id)
                .fetch_one(pool)
                .await
                .map_err(db_err)?;
        if !crate::user_teaches_course_by_assignment(pool, user_id, assignment_id).await? {
            return Err(async_graphql::Error::new("not found"));
        }
    }

    let row: crate::root_data::SubmissionCommentRow = sqlx::query_as(
        r#"
        INSERT INTO submission_comments (submission_id, author_id, comment, workflow_state, created_at, updated_at)
        VALUES ($1, $2, $3, 'active', now(), now())
        RETURNING id, submission_id, author_id, comment, created_at, updated_at
        "#,
    )
    .bind(submission_id)
    .bind(user_id)
    .bind(&input.comment)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(CreateSubmissionCommentPayload {
        submission_comment: Some(crate::root_data::submission_comment_from_row(row)),
        errors: None,
    })
}

// Keep the unused-import checker honest about the types this module names in
// its public signatures.
const _: Option<AccountNotification> = None;
const _: Option<AssignmentGroup> = None;
