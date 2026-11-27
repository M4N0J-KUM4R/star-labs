//! Object types for the Canvas GraphQL query root.
//!
//! These are the types the 28 root fields of
//! `app/graphql/types/query_type.rb` return, plus the connection and
//! auxiliary types they need. Field names and nullability follow Canvas'
//! `schema.graphql` exactly; where star-db has no backing column the field
//! is documented as a deviation and resolves to the Canvas nullability
//! (null) rather than being omitted, so a client query still validates.
//!
//! Canvas references:
//! - `app/graphql/types/account_notification_type.rb`
//! - `app/graphql/types/assignment_group_type.rb`
//! - `app/graphql/types/institutional_tag_type.rb`
//! - `app/graphql/types/institutional_tag_category_type.rb`
//! - `app/graphql/types/internal_setting_type.rb`
//! - `app/graphql/types/learning_outcome_type.rb`
//! - `app/graphql/types/learning_outcome_group_type.rb`
//! - `app/graphql/types/inbox_settings_type.rb`
//! - `app/graphql/types/outcome_calculation_method_type.rb`
//! - `app/graphql/types/outcome_proficiency_type.rb`
//! - `app/graphql/types/proficiency_rating_type.rb`
//! - `app/graphql/types/rubric_type.rb`
//! - `app/graphql/types/audit_logs_type.rb`
//! - `app/graphql/types/instructor_connection_type.rb`

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};

use crate::PageInfo;

// ---------------------------------------------------------------------------
// AccountNotification
// ---------------------------------------------------------------------------

/// Canvas GraphQL `AccountNotification`
/// (`app/graphql/types/account_notification_type.rb`).
///
/// Canvas shape: `_id: ID!`, `accountId: ID!`, `accountName: String`,
/// `createdAt: DateTime`, `endAt: DateTime!`, `icon: String`, `id: ID!`,
/// `message: String!`, `notificationType: String`, `siteAdmin: Boolean!`,
/// `startAt: DateTime!`, `subject: String!`, `updatedAt: DateTime`.
#[derive(Debug, Clone)]
pub struct AccountNotification {
    pub(crate) id: i64,
    pub(crate) account_id: i64,
    pub(crate) account_name: Option<String>,
    pub(crate) message: String,
    pub(crate) subject: String,
    pub(crate) icon: Option<String>,
    pub(crate) notification_type: Option<String>,
    pub(crate) site_admin: bool,
    pub(crate) start_at: DateTime<Utc>,
    pub(crate) end_at: DateTime<Utc>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl AccountNotification {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `AccountNotification._id` — the legacy canvas id.
    #[graphql(name = "_id")]
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn account_id(&self) -> ID {
        ID(self.account_id.to_string())
    }

    /// Canvas `AccountNotification.accountName` — the name of the account the
    /// notification belongs to.
    async fn account_name(&self) -> Option<&str> {
        self.account_name.as_deref()
    }

    async fn message(&self) -> &str {
        &self.message
    }

    async fn subject(&self) -> &str {
        &self.subject
    }

    async fn icon(&self) -> Option<&str> {
        self.icon.as_deref()
    }

    /// Canvas `AccountNotification.notificationType` — the workflow state of
    /// the notification row (`active` / `deleted`).
    async fn notification_type(&self) -> Option<&str> {
        self.notification_type.as_deref()
    }

    /// Canvas `AccountNotification.siteAdmin` — whether the notification is
    /// scoped to site admins. The port has no per-notification flag, so this
    /// is false unless the row's account is the root account.
    async fn site_admin(&self) -> bool {
        self.site_admin
    }

    async fn start_at(&self) -> DateTime<Utc> {
        self.start_at
    }

    async fn end_at(&self) -> DateTime<Utc> {
        self.end_at
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

// ---------------------------------------------------------------------------
// AssignmentGroup
// ---------------------------------------------------------------------------

/// Canvas GraphQL `AssignmentGroup`
/// (`app/graphql/types/assignment_group_type.rb`).
///
/// Canvas shape: `_id: ID!`, `createdAt: DateTime`, `groupWeight: Float`,
/// `id: ID!`, `name: String`, `position: Int`, `sisId: String`,
/// `state: AssignmentGroupState!`, `updatedAt: DateTime`,
/// `assignmentsConnection: AssignmentConnection!`.
#[derive(Debug, Clone)]
pub struct AssignmentGroup {
    pub(crate) id: i64,
    pub(crate) course_id: i64,
    pub(crate) name: String,
    pub(crate) position: i32,
    pub(crate) group_weight: f64,
    pub(crate) workflow_state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl AssignmentGroup {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn position(&self) -> i32 {
        self.position
    }

    /// Canvas `AssignmentGroup.groupWeight`.
    async fn group_weight(&self) -> f64 {
        self.group_weight
    }

    /// Canvas `AssignmentGroup.state` — the `AssignmentGroupState` enum
    /// (`available` / `deleted`).
    async fn state(&self) -> AssignmentGroupState {
        match self.workflow_state.as_str() {
            "deleted" => AssignmentGroupState::Deleted,
            _ => AssignmentGroupState::Available,
        }
    }

    /// Canvas `AssignmentGroup.sisId` — star-db's `assignment_groups` table
    /// has no SIS column (deviation: always null).
    async fn sis_id(&self) -> Option<&str> {
        None
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Canvas `AssignmentGroup.assignmentsConnection` — the assignments that
    /// belong to this group.
    async fn assignments_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<crate::assignment::AssignmentConnection>, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        Ok(Some(
            crate::root_data::fetch_group_assignments(
                &gctx.pool,
                self.course_id,
                self.id,
                first,
                after.as_deref(),
            )
            .await?,
        ))
    }
}

/// Canvas `AssignmentGroupState` enum
/// (`app/graphql/types/assignment_group_state_type.rb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AssignmentGroupState {
    #[graphql(name = "available")]
    Available,
    #[graphql(name = "deleted")]
    Deleted,
}

// ---------------------------------------------------------------------------
// InstitutionalTag / InstitutionalTagCategory
// ---------------------------------------------------------------------------

/// Canvas GraphQL `InstitutionalTag`
/// (`app/graphql/types/institutional_tag_type.rb`).
///
/// Canvas shape: `_id: ID!`, `associationsCount: Int!`,
/// `category: InstitutionalTagCategory`, `createdAt: DateTime`,
/// `description: String!`, `id: ID!`, `name: String!`,
/// `updatedAt: DateTime`, `workflowState: String!`,
/// `usersConnection: UserConnection`.
#[derive(Debug, Clone)]
pub struct InstitutionalTag {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) category_id: i64,
    pub(crate) workflow_state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl InstitutionalTag {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn description(&self) -> &str {
        &self.description
    }

    /// Canvas `InstitutionalTag.workflowState` (`active` / `deleted`).
    async fn workflow_state(&self) -> &str {
        &self.workflow_state
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Canvas `InstitutionalTag.associationsCount` — the number of
    /// institutional tag associations pointing at this tag.
    async fn associations_count(&self, ctx: &Context<'_>) -> Result<i32, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        crate::root_data::count_institutional_tag_associations(&gctx.pool, self.id).await
    }

    /// Canvas `InstitutionalTag.category` — the category this tag sits in.
    async fn category(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<InstitutionalTagCategory>, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        crate::root_data::fetch_institutional_tag_category(&gctx.pool, self.category_id).await
    }
}

/// Canvas GraphQL `InstitutionalTagCategory`
/// (`app/graphql/types/institutional_tag_category_type.rb`).
///
/// Canvas shape: `_id: ID!`, `associationsCount: Int!`,
/// `createdAt: DateTime`, `description: String`, `id: ID!`,
/// `name: String!`, `updatedAt: DateTime`, `workflowState: String!`,
/// `tagsConnection: InstitutionalTagConnection`.
#[derive(Debug, Clone)]
pub struct InstitutionalTagCategory {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) workflow_state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl InstitutionalTagCategory {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Canvas `InstitutionalTagCategory.workflowState` (`active` / `deleted`).
    async fn workflow_state(&self) -> &str {
        &self.workflow_state
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Canvas `InstitutionalTagCategory.associationsCount`.
    async fn associations_count(&self, ctx: &Context<'_>) -> Result<i32, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        crate::root_data::count_institutional_tag_category_associations(&gctx.pool, self.id).await
    }

    /// Canvas `InstitutionalTagCategory.tagsConnection` — the tags in this
    /// category.
    async fn tags_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<InstitutionalTagConnection>, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        Ok(Some(
            crate::root_data::fetch_institutional_tag_connection(
                &gctx.pool,
                Some(self.id),
                first,
                after.as_deref(),
            )
            .await?,
        ))
    }
}

/// Relay edge for an institutional tag connection.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstitutionalTagEdge {
    pub cursor: String,
    pub node: InstitutionalTag,
}

/// Canvas `InstitutionalTagConnection` — edges + page info.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstitutionalTagConnection {
    pub edges: Vec<InstitutionalTagEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// InternalSetting
// ---------------------------------------------------------------------------

/// Canvas GraphQL `InternalSetting`
/// (`app/graphql/types/internal_setting_type.rb`).
///
/// Canvas shape: `_id: ID!`, `createdAt: DateTime`, `id: ID!`,
/// `name: String!`, `secret: Boolean!`, `updatedAt: DateTime`,
/// `value: String`.
#[derive(Debug, Clone)]
pub struct InternalSetting {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) value: Option<String>,
    pub(crate) secret: bool,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl InternalSetting {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }

    /// Canvas `InternalSetting.secret` — whether the value is hidden.
    async fn secret(&self) -> bool {
        self.secret
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

// ---------------------------------------------------------------------------
// LearningOutcome
// ---------------------------------------------------------------------------

/// Canvas GraphQL `LearningOutcome`
/// (`app/graphql/types/learning_outcome_type.rb`).
///
/// Canvas shape: `_id: ID!`, `assessed: Boolean!`, `calculationInt: Int`,
/// `calculationMethod: String`, `contextId: ID`, `contextType: String`,
/// `createdAt: DateTime`, `description: String`, `displayName: String`,
/// `id: ID!`, `masteryPoints: Float`, `pointsPossible: Float`,
/// `title: String!`, `updatedAt: DateTime`, `vendorGuid: String`,
/// `canEdit: Boolean!`.
#[derive(Debug, Clone)]
pub struct LearningOutcome {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) display_name: Option<String>,
    pub(crate) context_id: Option<i64>,
    pub(crate) context_type: Option<String>,
    pub(crate) calculation_int: Option<i32>,
    pub(crate) calculation_method: Option<String>,
    pub(crate) mastery_points: Option<f64>,
    pub(crate) points_possible: Option<f64>,
    pub(crate) assessed: bool,
    pub(crate) vendor_guid: Option<String>,
    pub(crate) workflow_state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl LearningOutcome {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn title(&self) -> &str {
        &self.title
    }

    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    async fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// Canvas `LearningOutcome.contextId`.
    async fn context_id(&self) -> Option<ID> {
        self.context_id.map(|id| ID(id.to_string()))
    }

    /// Canvas `LearningOutcome.contextType`.
    async fn context_type(&self) -> Option<&str> {
        self.context_type.as_deref()
    }

    async fn calculation_int(&self) -> Option<i32> {
        self.calculation_int
    }

    async fn calculation_method(&self) -> Option<&str> {
        self.calculation_method.as_deref()
    }

    async fn mastery_points(&self) -> Option<f64> {
        self.mastery_points
    }

    async fn points_possible(&self) -> Option<f64> {
        self.points_possible
    }

    /// Canvas `LearningOutcome.assessed` — whether the outcome has been
    /// assessed.
    async fn assessed(&self) -> bool {
        self.assessed
    }

    async fn vendor_guid(&self) -> Option<&str> {
        self.vendor_guid.as_deref()
    }

    /// Canvas `LearningOutcome.workflowState`.
    async fn workflow_state(&self) -> &str {
        &self.workflow_state
    }

    /// Canvas `LearningOutcome.canEdit` — outcome management is an account-
    /// admin right in Canvas; the port has no outcome permission engine, so
    /// this follows the global-admin flag.
    async fn can_edit(&self, ctx: &Context<'_>) -> Result<bool, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        Ok(gctx.is_admin())
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Canvas `LearningOutcome.ratings` — the proficiency ratings of this
    /// outcome.
    async fn ratings(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Vec<ProficiencyRating>, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        crate::root_data::fetch_proficiency_ratings(&gctx.pool, self.id).await
    }
}

// ---------------------------------------------------------------------------
// LearningOutcomeGroup
// ---------------------------------------------------------------------------

/// Canvas GraphQL `LearningOutcomeGroup`
/// (`app/graphql/types/learning_outcome_group_type.rb`).
///
/// Canvas shape: `_id: ID!`, `canEdit: Boolean!`, `childGroupsCount: Int!`,
/// `contextId: ID`, `contextType: String`, `description: String`,
/// `id: ID!`, `title: String!`, `updatedAt: DateTime`,
/// `vendorGuid: String`, `outcomesCount: Int!`.
#[derive(Debug, Clone)]
pub struct LearningOutcomeGroup {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) context_id: Option<i64>,
    pub(crate) context_type: Option<String>,
    pub(crate) vendor_guid: Option<String>,
    pub(crate) workflow_state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl LearningOutcomeGroup {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn title(&self) -> &str {
        &self.title
    }

    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Canvas `LearningOutcomeGroup.contextId`.
    async fn context_id(&self) -> Option<ID> {
        self.context_id.map(|id| ID(id.to_string()))
    }

    /// Canvas `LearningOutcomeGroup.contextType`.
    async fn context_type(&self) -> Option<&str> {
        self.context_type.as_deref()
    }

    async fn vendor_guid(&self) -> Option<&str> {
        self.vendor_guid.as_deref()
    }

    /// Canvas `LearningOutcomeGroup.workflowState`.
    async fn workflow_state(&self) -> &str {
        &self.workflow_state
    }

    /// Canvas `LearningOutcomeGroup.canEdit`.
    async fn can_edit(&self, ctx: &Context<'_>) -> Result<bool, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        Ok(gctx.is_admin())
    }

    /// Canvas `LearningOutcomeGroup.childGroupsCount` — the port stores no
    /// parent/child hierarchy for outcome groups (deviation: always 0).
    async fn child_groups_count(&self) -> i32 {
        0
    }

    /// Canvas `LearningOutcomeGroup.outcomesCount` — the number of outcomes
    /// linked to this group.
    async fn outcomes_count(&self, ctx: &Context<'_>) -> Result<i32, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        crate::root_data::count_group_outcomes(&gctx.pool, self.id).await
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

// ---------------------------------------------------------------------------
// InboxSettings
// ---------------------------------------------------------------------------

/// Canvas GraphQL `InboxSettings`
/// (`app/graphql/types/inbox_settings_type.rb`).
///
/// Canvas shape: `_id: ID!`, `createdAt: DateTime`,
/// `outOfOfficeFirstDate: DateTime`, `outOfOfficeLastDate: DateTime`,
/// `outOfOfficeMessage: String`, `outOfOfficeSubject: String`,
/// `signature: String`, `updatedAt: DateTime`, `useOutOfOffice: Boolean!`,
/// `useSignature: Boolean!`, `userId: ID!`.
#[derive(Debug, Clone)]
pub struct InboxSettings {
    pub(crate) id: i64,
    pub(crate) user_id: i64,
    pub(crate) use_signature: bool,
    pub(crate) signature: Option<String>,
    pub(crate) use_out_of_office: bool,
    pub(crate) out_of_office_first_date: Option<DateTime<Utc>>,
    pub(crate) out_of_office_last_date: Option<DateTime<Utc>>,
    pub(crate) out_of_office_subject: Option<String>,
    pub(crate) out_of_office_message: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl InboxSettings {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `InboxSettings.userId`.
    async fn user_id(&self) -> ID {
        ID(self.user_id.to_string())
    }

    async fn use_signature(&self) -> bool {
        self.use_signature
    }

    async fn signature(&self) -> Option<&str> {
        self.signature.as_deref()
    }

    async fn use_out_of_office(&self) -> bool {
        self.use_out_of_office
    }

    async fn out_of_office_first_date(&self) -> Option<DateTime<Utc>> {
        self.out_of_office_first_date
    }

    async fn out_of_office_last_date(&self) -> Option<DateTime<Utc>> {
        self.out_of_office_last_date
    }

    async fn out_of_office_subject(&self) -> Option<&str> {
        self.out_of_office_subject.as_deref()
    }

    async fn out_of_office_message(&self) -> Option<&str> {
        self.out_of_office_message.as_deref()
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

// ---------------------------------------------------------------------------
// OutcomeCalculationMethod / OutcomeProficiency / ProficiencyRating
// ---------------------------------------------------------------------------

/// Canvas GraphQL `OutcomeCalculationMethod`
/// (`app/graphql/types/outcome_calculation_method_type.rb`).
///
/// Canvas shape: `_id: ID!`, `calculationInt: Int`,
/// `calculationMethod: String!`, `contextId: ID!`, `contextType: String!`,
/// `id: ID!`, `locked: Boolean!`.
#[derive(Debug, Clone)]
pub struct OutcomeCalculationMethod {
    pub(crate) id: i64,
    pub(crate) context_id: i64,
    pub(crate) context_type: String,
    pub(crate) calculation_int: Option<i32>,
    pub(crate) calculation_method: String,
    pub(crate) locked: bool,
}

#[Object]
impl OutcomeCalculationMethod {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn context_id(&self) -> ID {
        ID(self.context_id.to_string())
    }

    async fn context_type(&self) -> &str {
        &self.context_type
    }

    async fn calculation_int(&self) -> Option<i32> {
        self.calculation_int
    }

    async fn calculation_method(&self) -> &str {
        &self.calculation_method
    }

    /// Canvas `OutcomeCalculationMethod.locked` — whether the calculation
    /// method is inherited from a parent context and cannot be changed here.
    /// The port has no parent-context chain, so this is false for a
    /// context-owned method.
    async fn locked(&self) -> bool {
        self.locked
    }
}

/// Canvas GraphQL `OutcomeProficiency`
/// (`app/graphql/types/outcome_proficiency_type.rb`).
///
/// Canvas shape: `_id: ID!`, `contextId: ID!`, `contextType: String!`,
/// `id: ID!`, `locked: Boolean!`, `masteryPoints: Float!`,
/// `proficiencyRatingsConnection: ProficiencyRatingConnection!`.
#[derive(Debug, Clone)]
pub struct OutcomeProficiency {
    pub(crate) id: i64,
    pub(crate) context_id: i64,
    pub(crate) context_type: String,
    pub(crate) mastery_points: f64,
    pub(crate) locked: bool,
}

#[Object]
impl OutcomeProficiency {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn context_id(&self) -> ID {
        ID(self.context_id.to_string())
    }

    async fn context_type(&self) -> &str {
        &self.context_type
    }

    async fn mastery_points(&self) -> f64 {
        self.mastery_points
    }

    /// Canvas `OutcomeProficiency.locked`.
    async fn locked(&self) -> bool {
        self.locked
    }

    /// Canvas `OutcomeProficiency.proficiencyRatingsConnection` — the rating
    /// scale of this context.
    async fn proficiency_ratings_connection(
        &self,
        ctx: &Context<'_>,
    ) -> Result<ProficiencyRatingConnection, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        crate::root_data::fetch_proficiency_rating_connection(
            &gctx.pool,
            self.context_id,
            &self.context_type,
        )
        .await
    }
}

/// Canvas GraphQL `ProficiencyRating`
/// (`app/graphql/types/proficiency_rating_type.rb`).
///
/// Canvas shape: `_id: ID!`, `color: String`, `description: String`,
/// `mastery: Boolean!`, `points: Float!`.
#[derive(Debug, Clone)]
pub struct ProficiencyRating {
    pub(crate) id: i64,
    pub(crate) description: String,
    pub(crate) points: f64,
    pub(crate) mastery: bool,
}

#[Object]
impl ProficiencyRating {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn description(&self) -> &str {
        &self.description
    }

    async fn points(&self) -> f64 {
        self.points
    }

    async fn mastery(&self) -> bool {
        self.mastery
    }

    /// Canvas `ProficiencyRating.color` — star-db has no colour column
    /// (deviation: always null).
    async fn color(&self) -> Option<&str> {
        None
    }
}

/// Relay edge for a proficiency rating connection.
#[derive(Debug, Clone, SimpleObject)]
pub struct ProficiencyRatingEdge {
    pub cursor: String,
    pub node: ProficiencyRating,
}

/// Canvas `ProficiencyRatingConnection`.
#[derive(Debug, Clone, SimpleObject)]
pub struct ProficiencyRatingConnection {
    pub edges: Vec<ProficiencyRatingEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Rubric
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Rubric` (`app/graphql/types/rubric_type.rb`).
///
/// Canvas shape: `_id: ID!`, `buttonDisplay: String!`,
/// `canUpdateRubric: Boolean!`, `criteria: [RubricCriterion!]!`,
/// `criteriaCount: Int!`, `freeFormCriterionComments: Boolean!`,
/// `hasRubricAssociations: Boolean!`, `hidePoints: Boolean`,
/// `hideScoreTotal: Boolean!`, `id: ID!`, `pointsPossible: Float`,
/// `ratingOrder: String!`, `title: String`, `unassessed: Boolean!`,
/// `workflowState: String!`.
#[derive(Debug, Clone)]
pub struct Rubric {
    pub(crate) id: i64,
    pub(crate) course_id: i64,
    pub(crate) title: String,
    pub(crate) points_possible: f64,
    pub(crate) workflow_state: String,
    pub(crate) criteria: Vec<RubricCriterion>,
}

#[Object]
impl Rubric {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    /// Canvas `Rubric.title`.
    async fn title(&self) -> &str {
        &self.title
    }

    /// Canvas `Rubric.pointsPossible`.
    async fn points_possible(&self) -> f64 {
        self.points_possible
    }

    /// Canvas `Rubric.workflowState`.
    async fn workflow_state(&self) -> &str {
        &self.workflow_state
    }

    /// Canvas `Rubric.criteria` — the criteria that make up this rubric.
    async fn criteria(&self) -> Vec<RubricCriterion> {
        self.criteria.clone()
    }

    /// Canvas `Rubric.criteriaCount`.
    async fn criteria_count(&self) -> i32 {
        self.criteria.len() as i32
    }

    /// Canvas `Rubric.ratingOrder` — the order the ratings are listed in
    /// ("ascending" / "descending"). The port stores no order column; the
    /// criteria are returned by position (deviation).
    async fn rating_order(&self) -> &str {
        "ascending"
    }

    /// Canvas `Rubric.buttonDisplay` — how the rubric button is labelled.
    async fn button_display(&self) -> &str {
        "rubric"
    }

    /// Canvas `Rubric.canUpdateRubric` — rubric management follows the
    /// course's management rights; the port has no rubric permission
    /// engine, so this follows the global-admin flag.
    async fn can_update_rubric(&self, ctx: &Context<'_>) -> Result<bool, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        Ok(gctx.is_admin())
    }

    /// Canvas `Rubric.freeFormCriterionComments`.
    async fn free_form_criterion_comments(&self) -> bool {
        false
    }

    /// Canvas `Rubric.hasRubricAssociations`.
    async fn has_rubric_associations(&self) -> bool {
        false
    }

    /// Canvas `Rubric.hidePoints`.
    async fn hide_points(&self) -> Option<bool> {
        None
    }

    /// Canvas `Rubric.hideScoreTotal`.
    async fn hide_score_total(&self) -> bool {
        false
    }

    /// Canvas `Rubric.unassessed`.
    async fn unassessed(&self) -> bool {
        false
    }
}

/// Canvas GraphQL `RubricCriterion`
/// (`app/graphql/types/rubric_criterion_type.rb`).
///
/// Canvas shape: `_id: ID!`, `criterionUseRange: Boolean!`,
/// `description: String`, `ignoreForScoring: Boolean!`,
/// `learningOutcomeId: ID`, `longDescription: String`, `masteryPoints: Float`,
/// `outcome: LearningOutcome`, `points: Float`,
/// `ratings: [RubricRating!]`.
#[derive(Debug, Clone)]
pub struct RubricCriterion {
    pub(crate) id: i64,
    pub(crate) rubric_id: i64,
    pub(crate) description: String,
    pub(crate) points: f64,
    pub(crate) position: i32,
    pub(crate) ratings: Vec<RubricRating>,
}

#[Object]
impl RubricCriterion {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn rubric_id(&self) -> ID {
        ID(self.rubric_id.to_string())
    }

    /// Canvas `RubricCriterion.description`.
    async fn description(&self) -> &str {
        &self.description
    }

    /// Canvas `RubricCriterion.points`.
    async fn points(&self) -> f64 {
        self.points
    }

    /// Canvas `RubricCriterion.position` — the criterion's order in the
    /// rubric.
    async fn position(&self) -> i32 {
        self.position
    }

    /// Canvas `RubricCriterion.criterionUseRange`.
    async fn criterion_use_range(&self) -> bool {
        false
    }

    /// Canvas `RubricCriterion.ignoreForScoring`.
    async fn ignore_for_scoring(&self) -> bool {
        false
    }

    /// Canvas `RubricCriterion.learningOutcomeId` — star-db's
    /// `rubric_criteria` table has no outcome link (deviation: null).
    async fn learning_outcome_id(&self) -> Option<ID> {
        None
    }

    /// Canvas `RubricCriterion.longDescription`.
    async fn long_description(&self) -> Option<&str> {
        None
    }

    /// Canvas `RubricCriterion.masteryPoints`.
    async fn mastery_points(&self) -> Option<f64> {
        None
    }

    /// Canvas `RubricCriterion.ratings`.
    async fn ratings(&self) -> Vec<RubricRating> {
        self.ratings.clone()
    }
}

/// Canvas GraphQL `RubricRating`
/// (`app/graphql/types/rubric_rating_type.rb`).
///
/// Canvas shape: `_id: ID!`, `criterionId: ID!`, `description: String!`,
/// `id: ID!`, `points: Float!`.
#[derive(Debug, Clone)]
pub struct RubricRating {
    pub(crate) id: i64,
    pub(crate) criterion_id: i64,
    pub(crate) description: String,
    pub(crate) points: f64,
    pub(crate) position: i32,
}

#[Object]
impl RubricRating {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn criterion_id(&self) -> ID {
        ID(self.criterion_id.to_string())
    }

    async fn description(&self) -> &str {
        &self.description
    }

    async fn points(&self) -> f64 {
        self.points
    }

    async fn position(&self) -> i32 {
        self.position
    }
}

// ---------------------------------------------------------------------------
// PeerReviewSubAssignment
// ---------------------------------------------------------------------------

/// Canvas GraphQL `PeerReviewSubAssignment`
/// (`app/graphql/types/peer_review_sub_assignment_type.rb`).
///
/// The port's `assignments` table carries `assignment_type` (migration 0344)
/// so a peer-review sub-assignment is an assignment row flagged
/// `peer_review`. Canvas' type has ~60 fields; this slice implements the
/// identity, course and date fields star-db backs. The rest (allocation
/// rules, overrides, checkpoints, provisional grading, ...) land with the
/// peer-review allocation slice.
#[derive(Debug, Clone)]
pub struct PeerReviewSubAssignment {
    pub(crate) id: i64,
    pub(crate) course_id: i64,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) points_possible: f64,
    pub(crate) due_at: Option<DateTime<Utc>>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl PeerReviewSubAssignment {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    #[graphql(name = "_id")]

    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `PeerReviewSubAssignment.name`.
    async fn name(&self) -> &str {
        &self.name
    }

    /// Canvas `PeerReviewSubAssignment.description`.
    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Canvas `PeerReviewSubAssignment.pointsPossible`.
    async fn points_possible(&self) -> f64 {
        self.points_possible
    }

    /// Canvas `PeerReviewSubAssignment.dueAt`.
    async fn due_at(&self) -> Option<DateTime<Utc>> {
        self.due_at
    }

    /// Canvas `PeerReviewSubAssignment.courseId`.
    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    /// Canvas `PeerReviewSubAssignment.assignmentType` — always
    /// `PEER_REVIEW_SUB_ASSIGNMENT` for this type.
    async fn assignment_type(&self) -> AssignmentTypeEnum {
        AssignmentTypeEnum::PeerReviewSubAssignment
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

// ---------------------------------------------------------------------------
// AssignmentTypeEnum
// ---------------------------------------------------------------------------

/// Canvas `AssignmentTypeEnum`
/// (`app/graphql/types/assignment_type_enum_type.rb`): `ASSIGNMENT` and
/// `PEER_REVIEW_SUB_ASSIGNMENT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AssignmentTypeEnum {
    Assignment,
    PeerReviewSubAssignment,
}

impl AssignmentTypeEnum {
    /// The `assignments.assignment_type` value this enum member maps onto.
    pub fn assignment_type(self) -> &'static str {
        match self {
            AssignmentTypeEnum::Assignment => "assignment",
            AssignmentTypeEnum::PeerReviewSubAssignment => "peer_review",
        }
    }
}

// ---------------------------------------------------------------------------
// AuditLogs
// ---------------------------------------------------------------------------

/// Canvas GraphQL `AuditLogs` (`app/graphql/types/audit_logs_type.rb`).
///
/// Canvas shape: `mutationLogs(after, assetString!, before, endTime, first,
/// last, startTime): MutationLogConnection`.
///
/// **Deviation.** Canvas serves audit logs from DynamoDB; star-lms has no
/// auditor tables, so the connection is always empty.
#[derive(Debug, Clone, Default)]
pub struct AuditLogs;

#[Object]
impl AuditLogs {
    /// Canvas `AuditLogs.mutationLogs`.
    async fn mutation_logs(&self, asset_string: String) -> MutationLogConnection {
        let _ = asset_string;
        MutationLogConnection {
            edges: Vec::new(),
            nodes: Vec::new(),
            page_info: PageInfo {
                has_next_page: false,
                end_cursor: None,
            },
        }
    }
}

/// Canvas GraphQL `MutationLog`
/// (`app/graphql/types/mutation_log_type.rb`).
#[derive(Debug, Clone, SimpleObject)]
pub struct MutationLog {
    pub asset_string: ID,
    pub mutation_id: ID,
    pub mutation_name: String,
    pub timestamp: Option<DateTime<Utc>>,
}

/// Canvas `MutationLogEdge`.
#[derive(Debug, Clone, SimpleObject)]
pub struct MutationLogEdge {
    pub cursor: String,
    pub node: Option<MutationLog>,
}

/// Canvas `MutationLogConnection`.
#[derive(Debug, Clone, SimpleObject)]
pub struct MutationLogConnection {
    pub edges: Vec<MutationLogEdge>,
    pub nodes: Vec<MutationLog>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// courseInstructorsConnection
// ---------------------------------------------------------------------------

/// Canvas GraphQL `InstructorWithEnrollments`
/// (`app/graphql/types/instructor_connection_type.rb`).
///
/// Canvas shape: `enrollments: [InstructorEnrollmentInfo!]!`,
/// `user: InstructorUserInfo!`.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorWithEnrollments {
    pub enrollments: Vec<InstructorEnrollmentInfo>,
    pub user: InstructorUserInfo,
}

/// Canvas GraphQL `InstructorEnrollmentInfo`.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorEnrollmentInfo {
    pub course: InstructorCourseInfo,
    pub enrollment_state: String,
    pub role: String,
    #[graphql(name = "type")]
    pub enrollment_type: String,
}

/// Canvas GraphQL `InstructorCourseInfo`.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorCourseInfo {
    pub id: ID,
    pub name: String,
    pub course_code: Option<String>,
}

/// Canvas GraphQL `InstructorUserInfo`.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorUserInfo {
    pub id: ID,
    pub name: String,
    pub email: Option<String>,
    pub short_name: Option<String>,
    pub sortable_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Canvas `InstructorWithEnrollmentsEdge`.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorWithEnrollmentsEdge {
    pub cursor: String,
    pub node: Option<InstructorWithEnrollments>,
}

/// Canvas GraphQL `InstructorWithEnrollmentsConnection`.
///
/// Canvas shape: `edges: [InstructorWithEnrollmentsEdge]`,
/// `nodes: [InstructorWithEnrollments]`, `pageInfo: TotalCountPageInfo!`.
#[derive(Debug, Clone, SimpleObject)]
pub struct InstructorWithEnrollmentsConnection {
    pub edges: Vec<InstructorWithEnrollmentsEdge>,
    pub nodes: Vec<InstructorWithEnrollments>,
    pub page_info: TotalCountPageInfo,
}

/// Canvas GraphQL `TotalCountPageInfo`
/// (`app/graphql/types/page_info_type.rb`).
#[derive(Debug, Clone, SimpleObject)]
pub struct TotalCountPageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub end_cursor: Option<String>,
    pub start_cursor: Option<String>,
    pub total_count: Option<i32>,
    pub total_nr_of_pages: Option<i32>,
}

// ---------------------------------------------------------------------------
// Node id plumbing
// ---------------------------------------------------------------------------

/// The `Node` interface id of every type in this module.
///
/// The async-graphql `Interface` derive calls `node_id(ctx)` on the concrete
/// type of each variant, so each node-able type implements [`NodeId`].
macro_rules! node_id_from_id_field {
    ($ty:ty) => {
        impl $crate::node::NodeId for $ty {
            async fn node_id(
                &self,
                _ctx: &async_graphql::Context<'_>,
            ) -> Result<async_graphql::ID, async_graphql::Error> {
                Ok(async_graphql::ID(self.id.to_string()))
            }
        }
    };
}

// ---------------------------------------------------------------------------
// SubmissionComment
// ---------------------------------------------------------------------------

/// Canvas GraphQL `SubmissionComment`
/// (`app/graphql/types/submission_comment_type.rb`).
///
/// Canvas shape: `_id: ID!`, `assignment: Assignment`,
/// `attachments: [File!]!`, `attempt: Int!`, `author: User`,
/// `authorVisibleName: String`, `canReply: Boolean`, `comment: String`,
/// `course: Course`, `createdAt: DateTime!`, `draft: Boolean!`,
/// `htmlComment: String`, `id: ID!`, `mediaCommentId: String`,
/// `mediaObject: MediaObject`, `provisional: Boolean!`,
/// `publishable: Boolean!`, `read: Boolean!`, `submissionId: ID!`,
/// `updatedAt: DateTime`.
///
/// star-db's `submission_comments` table (migration 0026) carries the
/// submission, the author, the comment text and the workflow state; the
/// remaining Canvas fields are resolved from the submission row or are
/// constants.
#[derive(Debug, Clone)]
pub struct SubmissionComment {
    pub(crate) id: i64,
    pub(crate) submission_id: i64,
    pub(crate) author_id: i64,
    pub(crate) comment: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl SubmissionComment {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `SubmissionComment._id` — the legacy canvas id.
    #[graphql(name = "_id")]
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `SubmissionComment.submissionId`.
    async fn submission_id(&self) -> ID {
        ID(self.submission_id.to_string())
    }

    /// Canvas `SubmissionComment.comment`.
    async fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    /// Canvas `SubmissionComment.attempt` — star-lms stores one attempt per
    /// submission row.
    async fn attempt(&self) -> i32 {
        1
    }

    /// Canvas `SubmissionComment.draft`.
    async fn draft(&self) -> bool {
        false
    }

    /// Canvas `SubmissionComment.read` — a freshly created comment is read
    /// by its author.
    async fn read(&self) -> bool {
        true
    }

    /// Canvas `SubmissionComment.provisional`.
    async fn provisional(&self) -> bool {
        false
    }

    /// Canvas `SubmissionComment.publishable`.
    async fn publishable(&self) -> bool {
        true
    }

    /// Canvas `SubmissionComment.attachments` — star-lms has no attachment
    /// link on a submission comment.
    async fn attachments(&self) -> Vec<crate::files::File> {
        Vec::new()
    }

    /// Canvas `SubmissionComment.author` — the user who wrote the comment.
    async fn author(&self, ctx: &Context<'_>) -> Result<Option<crate::User>, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        crate::fetch_user(&gctx.pool, self.author_id).await
    }

    /// Canvas `SubmissionComment.authorVisibleName`.
    async fn author_visible_name(&self) -> Option<&str> {
        None
    }

    /// Canvas `SubmissionComment.htmlComment`.
    async fn html_comment(&self) -> Option<&str> {
        None
    }

    /// Canvas `SubmissionComment.canReply`.
    async fn can_reply(&self) -> Option<bool> {
        Some(true)
    }

    /// Canvas `SubmissionComment.mediaCommentId`.
    async fn media_comment_id(&self) -> Option<&str> {
        None
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

// ---------------------------------------------------------------------------
// Node id plumbing for every type the `Node` interface can return
// ---------------------------------------------------------------------------

node_id_from_id_field!(AccountNotification);
node_id_from_id_field!(AssignmentGroup);
node_id_from_id_field!(InstitutionalTag);
node_id_from_id_field!(InstitutionalTagCategory);
node_id_from_id_field!(InternalSetting);
node_id_from_id_field!(LearningOutcome);
node_id_from_id_field!(LearningOutcomeGroup);
node_id_from_id_field!(OutcomeCalculationMethod);
node_id_from_id_field!(OutcomeProficiency);
node_id_from_id_field!(PeerReviewSubAssignment);
node_id_from_id_field!(Rubric);

node_id_from_id_field!(crate::Account);
node_id_from_id_field!(crate::Assignment);
node_id_from_id_field!(crate::Conversation);
node_id_from_id_field!(crate::Course);
node_id_from_id_field!(crate::DiscussionTopic);
node_id_from_id_field!(crate::Enrollment);
node_id_from_id_field!(crate::File);
node_id_from_id_field!(crate::Folder);
node_id_from_id_field!(crate::Group);
node_id_from_id_field!(crate::GroupSet);
node_id_from_id_field!(crate::Module);
node_id_from_id_field!(crate::ModuleItem);
node_id_from_id_field!(crate::Page);
node_id_from_id_field!(crate::Progress);
node_id_from_id_field!(crate::CourseSection);
node_id_from_id_field!(crate::Submission);
node_id_from_id_field!(crate::EnrollmentTerm);
node_id_from_id_field!(crate::User);
