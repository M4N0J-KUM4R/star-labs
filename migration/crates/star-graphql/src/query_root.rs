//! The Canvas GraphQL query root.
//!
//! Canvas' root query is `Types::QueryType`
//! (`canvas-src/app/graphql/types/query_type.rb`); its 28 fields are the
//! contract this file implements. Field names, argument names, argument
//! types and nullability come from Canvas' `schema.graphql`:
//!
//! ```text
//! type Query {
//!   account(id: ID, sisId: String): Account
//!   accountNotifications(accountId: ID): [AccountNotification!]!
//!   allCourses: [Course!]
//!   assignment(id: ID, includeTypes: [AssignmentTypeEnum!] = [ASSIGNMENT], sisId: String): Assignment
//!   assignmentGroup(id: ID, sisId: String): AssignmentGroup
//!   auditLogs: AuditLogs
//!   course(id: ID, sisId: String): Course
//!   courseInstructorsConnection(after: String, before: String, courseIds: [ID!]!, enrollmentTypes: [String!], first: Int, last: Int, observedUserId: ID): InstructorWithEnrollmentsConnection
//!   courses(ids: [ID!], sisIds: [String!]): [Course!]
//!   enrollmentInvitations(includeEnrollmentUuid: String): [Enrollment!]!
//!   folder(id: ID!): Folder
//!   institutionalTag(id: ID!): InstitutionalTag
//!   institutionalTagCategory(id: ID!): InstitutionalTagCategory
//!   internalSetting(id: ID, name: String): InternalSetting
//!   internalSettings: [InternalSetting!]
//!   learningOutcome(id: ID!): LearningOutcome
//!   learningOutcomeGroup(id: ID!): LearningOutcomeGroup
//!   legacyNode(_id: ID!, type: NodeType!): Node
//!   moduleItem(id: ID!): ModuleItem
//!   myInboxSettings: InboxSettings
//!   node(id: ID!): Node
//!   outcomeCalculationMethod(id: ID!): OutcomeCalculationMethod
//!   outcomeProficiency(id: ID!): OutcomeProficiency
//!   peerReviewSubAssignment(id: ID!): PeerReviewSubAssignment
//!   rubric(id: ID!): Rubric
//!   submission(anonymousId: ID, assignmentId: ID, id: ID, userId: ID): Submission
//!   term(id: ID, sisId: String): Term
//!   user(id: ID!): User
//! }
//! ```
//!
//! Three more fields are exposed by star-lms and are **not** in Canvas:
//! `enrollmentTerm`, `courseSection` and `progress`. They stay for the
//! existing star-lms clients; the Canvas-named `term` is the one that
//! matches Canvas.

use async_graphql::{Context, Object, ID};

use crate::{
    course_user_fields::Account,
    node::{decode_relay_id, parse_relay_or_legacy_id, Node, NodeType},
    root_data,
    root_types::{
        AccountNotification, AssignmentGroup, AssignmentTypeEnum, AuditLogs, InstitutionalTag,
        InstitutionalTagCategory, InternalSetting, LearningOutcome, LearningOutcomeGroup,
        OutcomeCalculationMethod, OutcomeProficiency, PeerReviewSubAssignment, Rubric,
    },
    terms_sections::{CourseSection, EnrollmentTerm},
    Course, Enrollment, GraphQLContext, User,
};

/// The GraphQL query root.
pub struct Query;

#[Object]
impl Query {
    // -----------------------------------------------------------------
    // account
    // -----------------------------------------------------------------

    /// Canvas `Query.account` — `account(id: ID, sisId: String): Account`.
    ///
    /// Canvas raises `Must specify exactly one of id or sisId` when both or
    /// neither are given. `id` accepts "a graphql or legacy id".
    async fn account(
        &self,
        ctx: &Context<'_>,
        id: Option<ID>,
        sis_id: Option<String>,
    ) -> Result<Option<Account>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        match (id, sis_id) {
            (Some(_), Some(_)) | (None, None) => Err(async_graphql::Error::new(
                "Must specify exactly one of id or sisId",
            )),
            (Some(id), None) => {
                let account_id = parse_relay_or_legacy_id(&id.0, "Account")?;
                crate::course_user_fields::fetch_account(&gctx.pool, account_id).await
            }
            (None, Some(sis_id)) => {
                crate::course_user_fields::fetch_account_by_sis_id(&gctx.pool, &sis_id).await
            }
        }
    }

    // -----------------------------------------------------------------
    // accountNotifications
    // -----------------------------------------------------------------

    /// Canvas `Query.accountNotifications` —
    /// `accountNotifications(accountId: ID): [AccountNotification!]!`.
    async fn account_notifications(
        &self,
        ctx: &Context<'_>,
        account_id: Option<ID>,
    ) -> Result<Vec<AccountNotification>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let account_id = match account_id {
            Some(id) => Some(parse_relay_or_legacy_id(&id.0, "Account")?),
            None => None,
        };
        root_data::fetch_account_notifications(&gctx.pool, gctx.auth_user.id, account_id).await
    }

    // -----------------------------------------------------------------
    // allCourses
    // -----------------------------------------------------------------

    /// Canvas `Query.allCourses` — `allCourses: [Course!]`.
    ///
    /// "All courses viewable by the current user": the courses the caller
    /// has an active enrollment in.
    async fn all_courses(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<Vec<Course>>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            root_data::fetch_courses_for_user(&gctx.pool, gctx.auth_user.id).await?,
        ))
    }

    // -----------------------------------------------------------------
    // assignment
    // -----------------------------------------------------------------

    /// Canvas `Query.assignment` —
    /// `assignment(id: ID, includeTypes: [AssignmentTypeEnum!] = [ASSIGNMENT], sisId: String): Assignment`.
    async fn assignment(
        &self,
        ctx: &Context<'_>,
        id: Option<ID>,
        include_types: Option<Vec<AssignmentTypeEnum>>,
        sis_id: Option<String>,
    ) -> Result<Option<crate::assignment::Assignment>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        match (id, sis_id) {
            (Some(_), Some(_)) | (None, None) => Err(async_graphql::Error::new(
                "Must specify exactly one of id or sisId",
            )),
            (Some(id), None) => {
                let assignment_id = parse_relay_or_legacy_id(&id.0, "Assignment")?;
                let types = include_types.unwrap_or_else(|| vec![AssignmentTypeEnum::Assignment]);
                root_data::fetch_assignment_by_id(&gctx.pool, assignment_id, &types).await
            }
            (None, Some(_sis_id)) => {
                // star-db's `assignments` table has no SIS column, so the SIS
                // lookup finds nothing (deviation: always null).
                Ok(None)
            }
        }
    }

    // -----------------------------------------------------------------
    // assignmentGroup
    // -----------------------------------------------------------------

    /// Canvas `Query.assignmentGroup` —
    /// `assignmentGroup(id: ID, sisId: String): AssignmentGroup`.
    async fn assignment_group(
        &self,
        ctx: &Context<'_>,
        id: Option<ID>,
        sis_id: Option<String>,
    ) -> Result<Option<AssignmentGroup>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        match (id, sis_id) {
            (Some(_), Some(_)) | (None, None) => Err(async_graphql::Error::new(
                "Must specify exactly one of id or sisId",
            )),
            (Some(id), None) => {
                let group_id = parse_relay_or_legacy_id(&id.0, "AssignmentGroup")?;
                root_data::fetch_assignment_group(&gctx.pool, group_id).await
            }
            (None, Some(_sis_id)) => {
                // star-db's `assignment_groups` table has no SIS column.
                Ok(None)
            }
        }
    }

    // -----------------------------------------------------------------
    // auditLogs
    // -----------------------------------------------------------------

    /// Canvas `Query.auditLogs` — `auditLogs: AuditLogs`.
    async fn audit_logs(&self) -> Option<AuditLogs> {
        Some(AuditLogs)
    }

    // -----------------------------------------------------------------
    // course
    // -----------------------------------------------------------------

    /// Canvas `Query.course` — `course(id: ID, sisId: String): Course`.
    ///
    /// Visible to enrolled users (any role) and admins; unknown or
    /// unauthorized ids resolve to null, like Canvas.
    async fn course(
        &self,
        ctx: &Context<'_>,
        id: Option<ID>,
        sis_id: Option<String>,
    ) -> Result<Option<Course>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        match (id, sis_id) {
            (Some(_), Some(_)) | (None, None) => Err(async_graphql::Error::new(
                "Must specify exactly one of id or sisId",
            )),
            (Some(id), None) => {
                let course_id = parse_relay_or_legacy_id(&id.0, "Course")?;
                crate::fetch_course_authorized(
                    &gctx.pool,
                    gctx.auth_user.id,
                    gctx.is_admin(),
                    course_id,
                )
                .await
            }
            (None, Some(_sis_id)) => {
                // star-db's `courses` table has no SIS column.
                Ok(None)
            }
        }
    }

    // -----------------------------------------------------------------
    // courseInstructorsConnection
    // -----------------------------------------------------------------

    /// Canvas `Query.courseInstructorsConnection` —
    /// `courseInstructorsConnection(after: String, before: String, courseIds: [ID!]!, enrollmentTypes: [String!], first: Int, last: Int, observedUserId: ID): InstructorWithEnrollmentsConnection`.
    ///
    /// Canvas intersects the requested `courseIds` with the courses the
    /// caller participates in and filters `enrollmentTypes` down to
    /// `TeacherEnrollment` / `TaEnrollment`.
    #[allow(clippy::too_many_arguments)]
    async fn course_instructors_connection(
        &self,
        ctx: &Context<'_>,
        after: Option<String>,
        before: Option<String>,
        course_ids: Vec<ID>,
        enrollment_types: Option<Vec<String>>,
        first: Option<i32>,
        last: Option<i32>,
        observed_user_id: Option<ID>,
    ) -> Result<Option<root_data::InstructorConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;

        // Canvas' connection is forward-paginated here: `before` and `last`
        // are accepted for contract parity but not honoured.
        let _ = (&before, &last);

        let mut ids = Vec::with_capacity(course_ids.len());
        for raw in &course_ids {
            ids.push(parse_relay_or_legacy_id(&raw.0, "Course")?);
        }

        // Only the courses the caller participates in are visible.
        let visible = root_data::fetch_course_ids_for_user(&gctx.pool, gctx.auth_user.id).await?;
        let ids: Vec<i64> = ids.into_iter().filter(|id| visible.contains(id)).collect();

        let types: Vec<String> = match enrollment_types {
            Some(types) if !types.is_empty() => types
                .into_iter()
                .filter(|t| t == "TeacherEnrollment" || t == "TaEnrollment")
                .collect(),
            _ => vec!["TeacherEnrollment".to_string(), "TaEnrollment".to_string()],
        };

        let _ = observed_user_id;

        Ok(Some(
            root_data::fetch_course_instructors(&gctx.pool, &ids, &types, first, after.as_deref())
                .await?,
        ))
    }

    // -----------------------------------------------------------------
    // courses
    // -----------------------------------------------------------------

    /// Canvas `Query.courses` — `courses(ids: [ID!], sisIds: [String!]): [Course!]`.
    async fn courses(
        &self,
        ctx: &Context<'_>,
        ids: Option<Vec<ID>>,
        sis_ids: Option<Vec<String>>,
    ) -> Result<Option<Vec<Course>>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        match (ids, sis_ids) {
            (Some(_), Some(_)) | (None, None) => Err(async_graphql::Error::new(
                "Must specify exactly one of ids or sisIds",
            )),
            (Some(ids), None) => {
                if ids.len() > 100 {
                    return Err(async_graphql::Error::new(
                        "Cannot request more than 100 courses at once",
                    ));
                }
                let mut parsed = Vec::with_capacity(ids.len());
                for raw in &ids {
                    parsed.push(parse_relay_or_legacy_id(&raw.0, "Course")?);
                }
                Ok(Some(
                    root_data::fetch_visible_courses(&gctx.pool, gctx.auth_user.id, &parsed)
                        .await?,
                ))
            }
            (None, Some(_sis_ids)) => {
                // star-db's `courses` table has no SIS column.
                Ok(Some(Vec::new()))
            }
        }
    }

    // -----------------------------------------------------------------
    // enrollmentInvitations
    // -----------------------------------------------------------------

    /// Canvas `Query.enrollmentInvitations` —
    /// `enrollmentInvitations(includeEnrollmentUuid: String): [Enrollment!]!`.
    async fn enrollment_invitations(
        &self,
        ctx: &Context<'_>,
        include_enrollment_uuid: Option<String>,
    ) -> Result<Vec<Enrollment>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        root_data::fetch_enrollment_invitations(
            &gctx.pool,
            gctx.auth_user.id,
            include_enrollment_uuid.as_deref(),
        )
        .await
    }

    // -----------------------------------------------------------------
    // folder
    // -----------------------------------------------------------------

    /// Canvas `Query.folder` — `folder(id: ID!): Folder`.
    ///
    /// The folder's context gates visibility: enrolled users of the context
    /// course (any role) and admins; everyone else gets null.
    async fn folder(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<crate::files::Folder>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let folder_id = parse_relay_or_legacy_id(&id.0, "Folder")?;
        crate::files::fetch_folder_authorized(
            &gctx.pool,
            gctx.auth_user.id,
            gctx.is_admin(),
            folder_id,
        )
        .await
    }

    // -----------------------------------------------------------------
    // institutionalTag / institutionalTagCategory
    // -----------------------------------------------------------------

    /// Canvas `Query.institutionalTag` — `institutionalTag(id: ID!): InstitutionalTag`.
    async fn institutional_tag(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<InstitutionalTag>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let tag_id = parse_relay_or_legacy_id(&id.0, "InstitutionalTag")?;
        root_data::fetch_institutional_tag(&gctx.pool, tag_id).await
    }

    /// Canvas `Query.institutionalTagCategory` —
    /// `institutionalTagCategory(id: ID!): InstitutionalTagCategory`.
    async fn institutional_tag_category(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<InstitutionalTagCategory>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let category_id = parse_relay_or_legacy_id(&id.0, "InstitutionalTagCategory")?;
        root_data::fetch_institutional_tag_category(&gctx.pool, category_id).await
    }

    // -----------------------------------------------------------------
    // internalSetting / internalSettings
    // -----------------------------------------------------------------

    /// Canvas `Query.internalSetting` —
    /// `internalSetting(id: ID, name: String): InternalSetting`.
    ///
    /// Canvas raises `Must specify exactly one of id or name` when both or
    /// neither are given.
    async fn internal_setting(
        &self,
        ctx: &Context<'_>,
        id: Option<ID>,
        name: Option<String>,
    ) -> Result<Option<InternalSetting>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        match (id, name) {
            (Some(_), Some(_)) | (None, None) => Err(async_graphql::Error::new(
                "Must specify exactly one of id or name",
            )),
            (Some(id), None) => {
                let setting_id = parse_relay_or_legacy_id(&id.0, "InternalSetting")?;
                root_data::fetch_internal_setting(&gctx.pool, setting_id).await
            }
            (None, Some(name)) => {
                root_data::fetch_internal_setting_by_name(&gctx.pool, &name).await
            }
        }
    }

    /// Canvas `Query.internalSettings` — `internalSettings: [InternalSetting!]`.
    ///
    /// Canvas returns `[]` unless the caller may manage internal settings.
    async fn internal_settings(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<Vec<InternalSetting>>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !gctx.is_admin() {
            return Ok(Some(Vec::new()));
        }
        Ok(Some(root_data::fetch_internal_settings(&gctx.pool).await?))
    }

    // -----------------------------------------------------------------
    // learningOutcome / learningOutcomeGroup
    // -----------------------------------------------------------------

    /// Canvas `Query.learningOutcome` — `learningOutcome(id: ID!): LearningOutcome`.
    async fn learning_outcome(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<LearningOutcome>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let outcome_id = parse_relay_or_legacy_id(&id.0, "LearningOutcome")?;
        root_data::fetch_learning_outcome(&gctx.pool, outcome_id).await
    }

    /// Canvas `Query.learningOutcomeGroup` —
    /// `learningOutcomeGroup(id: ID!): LearningOutcomeGroup`.
    async fn learning_outcome_group(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<LearningOutcomeGroup>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let group_id = parse_relay_or_legacy_id(&id.0, "LearningOutcomeGroup")?;
        root_data::fetch_learning_outcome_group(&gctx.pool, group_id).await
    }

    // -----------------------------------------------------------------
    // legacyNode / node
    // -----------------------------------------------------------------

    /// Canvas `Query.legacyNode` — `legacyNode(_id: ID!, type: NodeType!): Node`.
    ///
    /// "Fetches an object given its type and legacy ID".
    async fn legacy_node(
        &self,
        ctx: &Context<'_>,
        #[graphql(name = "_id")] _id: ID,
        r#type: NodeType,
    ) -> Result<Option<Node>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let id = parse_relay_or_legacy_id(&_id.0, r#type.type_name())?;
        root_data::load_node(&gctx.pool, r#type, id).await
    }

    /// Canvas `Query.node` — `node(id: ID!): Node`.
    ///
    /// Canvas' `node` takes the **global** relay id —
    /// `base64("Type-id")` — not the legacy id. `legacyNode` takes the
    /// legacy id.
    async fn node(&self, ctx: &Context<'_>, id: ID) -> Result<Option<Node>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let (type_name, raw_id) =
            decode_relay_id(&id.0).ok_or_else(|| async_graphql::Error::new("invalid id"))?;
        let node_type = NodeType::from_type_name(&type_name)
            .ok_or_else(|| async_graphql::Error::new("invalid id"))?;
        let legacy_id = raw_id
            .parse::<i64>()
            .map_err(|_| async_graphql::Error::new("invalid id"))?;
        root_data::load_node(&gctx.pool, node_type, legacy_id).await
    }

    // -----------------------------------------------------------------
    // moduleItem
    // -----------------------------------------------------------------

    /// Canvas `Query.moduleItem` — `moduleItem(id: ID!): ModuleItem`.
    ///
    /// Visible to enrolled users of the item's course (any role) and admins;
    /// everyone else gets null.
    async fn module_item(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<crate::modules::ModuleItem>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let item_id = parse_relay_or_legacy_id(&id.0, "ModuleItem")?;
        crate::modules::fetch_module_item(&gctx.pool, gctx.auth_user.id, gctx.is_admin(), item_id)
            .await
    }

    // -----------------------------------------------------------------
    // myInboxSettings
    // -----------------------------------------------------------------

    /// Canvas `Query.myInboxSettings` — `myInboxSettings: InboxSettings`.
    async fn my_inbox_settings(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<crate::root_types::InboxSettings>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        root_data::fetch_inbox_settings(&gctx.pool, gctx.auth_user.id).await
    }

    // -----------------------------------------------------------------
    // outcomeCalculationMethod / outcomeProficiency
    // -----------------------------------------------------------------

    /// Canvas `Query.outcomeCalculationMethod` —
    /// `outcomeCalculationMethod(id: ID!): OutcomeCalculationMethod`.
    async fn outcome_calculation_method(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<OutcomeCalculationMethod>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let method_id = parse_relay_or_legacy_id(&id.0, "OutcomeCalculationMethod")?;
        root_data::fetch_outcome_calculation_method(&gctx.pool, method_id).await
    }

    /// Canvas `Query.outcomeProficiency` —
    /// `outcomeProficiency(id: ID!): OutcomeProficiency`.
    async fn outcome_proficiency(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<OutcomeProficiency>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let proficiency_id = parse_relay_or_legacy_id(&id.0, "OutcomeProficiency")?;
        root_data::fetch_outcome_proficiency(&gctx.pool, proficiency_id).await
    }

    // -----------------------------------------------------------------
    // peerReviewSubAssignment
    // -----------------------------------------------------------------

    /// Canvas `Query.peerReviewSubAssignment` —
    /// `peerReviewSubAssignment(id: ID!): PeerReviewSubAssignment`.
    async fn peer_review_sub_assignment(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<PeerReviewSubAssignment>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let assignment_id = parse_relay_or_legacy_id(&id.0, "PeerReviewSubAssignment")?;
        root_data::fetch_peer_review_sub_assignment(&gctx.pool, assignment_id).await
    }

    // -----------------------------------------------------------------
    // rubric
    // -----------------------------------------------------------------

    /// Canvas `Query.rubric` — `rubric(id: ID!): Rubric`.
    async fn rubric(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<Rubric>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let rubric_id = parse_relay_or_legacy_id(&id.0, "Rubric")?;
        root_data::fetch_rubric(&gctx.pool, rubric_id).await
    }

    // -----------------------------------------------------------------
    // submission
    // -----------------------------------------------------------------

    /// Canvas `Query.submission` —
    /// `submission(anonymousId: ID, assignmentId: ID, id: ID, userId: ID): Submission`.
    ///
    /// Canvas raises `Must specify an id or an assignment_id and user_id or
    /// an assignment_id and an anonymous_id` for any other combination.
    async fn submission(
        &self,
        ctx: &Context<'_>,
        anonymous_id: Option<ID>,
        assignment_id: Option<ID>,
        id: Option<ID>,
        user_id: Option<ID>,
    ) -> Result<Option<crate::submission::Submission>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;

        match (id.as_ref(), assignment_id.as_ref(), user_id.as_ref(), anonymous_id.as_ref()) {
            (Some(_), None, None, None) => {
                let submission_id = parse_relay_or_legacy_id(&id.as_ref().unwrap().0, "Submission")?;
                let submission = crate::submission::fetch_submission(&gctx.pool, submission_id).await?;
                match submission {
                    Some(submission) if can_read_submission(gctx, &submission).await? => {
                        Ok(Some(submission))
                    }
                    _ => Ok(None),
                }
            }
            (None, Some(assignment_id), Some(user_id), None) => {
                let assignment_id = parse_relay_or_legacy_id(&assignment_id.0, "Assignment")?;
                let user_id = parse_relay_or_legacy_id(&user_id.0, "User")?;
                crate::submission::fetch_submission_by_assignment_and_user(
                    &gctx.pool,
                    assignment_id,
                    user_id,
                )
                .await
            }
            (None, Some(assignment_id), None, Some(_anonymous_id)) => {
                let assignment_id = parse_relay_or_legacy_id(&assignment_id.0, "Assignment")?;
                let anonymous = anonymous_id.map(|id| id.0).unwrap_or_default();
                root_data::fetch_submission_by_assignment_and_anonymous_id(
                    &gctx.pool,
                    assignment_id,
                    &anonymous,
                )
                .await
            }
            _ => Err(async_graphql::Error::new(
                "Must specify an id or an assignment_id and user_id or an assignment_id and an anonymous_id",
            )),
        }
    }

    // -----------------------------------------------------------------
    // term
    // -----------------------------------------------------------------

    /// Canvas `Query.term` — `term(id: ID, sisId: String): Term`.
    async fn term(
        &self,
        ctx: &Context<'_>,
        id: Option<ID>,
        sis_id: Option<String>,
    ) -> Result<Option<EnrollmentTerm>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        match (id, sis_id) {
            (Some(_), Some(_)) | (None, None) => Err(async_graphql::Error::new(
                "Must specify exactly one of id or sisId",
            )),
            (Some(id), None) => {
                let term_id = parse_relay_or_legacy_id(&id.0, "Term")?;
                crate::terms_sections::fetch_term(&gctx.pool, term_id).await
            }
            (None, Some(_sis_id)) => {
                // star-db's `terms` table has no SIS column.
                Ok(None)
            }
        }
    }

    // -----------------------------------------------------------------
    // user
    // -----------------------------------------------------------------

    /// Canvas `Query.user` — `user(id: ID!): User`.
    ///
    /// Any authenticated caller may read the basic profile shape;
    /// `enrollmentsConnection` enforces self/admin/shared-teacher.
    async fn user(&self, ctx: &Context<'_>, id: ID) -> Result<Option<User>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let user_id = parse_relay_or_legacy_id(&id.0, "User")?;
        crate::fetch_user(&gctx.pool, user_id).await
    }

    // -----------------------------------------------------------------
    // star-lms extensions (not in Canvas)
    // -----------------------------------------------------------------

    /// `enrollmentTerm(id: ID!)` — star-lms exposure of the Canvas `Term`
    /// object. Canvas reaches the term through `term(id:)`; this alias
    /// stays for the existing star-lms clients.
    async fn enrollment_term(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<EnrollmentTerm>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let term_id = parse_relay_or_legacy_id(&id.0, "Term")?;
        crate::terms_sections::fetch_term(&gctx.pool, term_id).await
    }

    /// `courseSection(id: ID!)` — star-lms exposure of the Canvas `Section`
    /// object. Canvas has no `courseSection` root field.
    async fn course_section(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<CourseSection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let section_id = parse_relay_or_legacy_id(&id.0, "Section")?;
        crate::terms_sections::fetch_section(
            &gctx.pool,
            gctx.auth_user.id,
            gctx.is_admin(),
            section_id,
        )
        .await
    }

    /// `progress(id: ID!)` — star-lms exposure of the Canvas `Progress`
    /// orphan type. Canvas reaches Progress only through mutation payloads;
    /// the REST progress endpoints have no GraphQL root in Canvas.
    async fn progress(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<crate::pages::Progress>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let progress_id = parse_relay_or_legacy_id(&id.0, "Progress")?;
        let progress = crate::pages::fetch_progress(&gctx.pool, progress_id).await?;
        match progress {
            Some(p) if gctx.is_admin() || p.owner_is(gctx.auth_user.id) => Ok(Some(p)),
            _ => Ok(None),
        }
    }
    async fn media_object(
        &self,
        ctx: &Context<'_>,
        media_id: String,
    ) -> Result<Option<crate::outcomes_media::MediaObject>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        crate::outcomes_media::fetch_media_object_for_caller(&gctx.pool, gctx, &media_id).await
    }
}

/// Whether the caller may read this submission.
///
/// Canvas gates submissions through the assignment's read permissions:
/// a student sees their own, a teacher sees their students', an admin sees
/// everything.
async fn can_read_submission(
    gctx: &GraphQLContext,
    submission: &crate::submission::Submission,
) -> Result<bool, async_graphql::Error> {
    if gctx.is_admin() {
        return Ok(true);
    }
    if submission.user_id == ID(gctx.auth_user.id.to_string()) {
        return Ok(true);
    }
    let assignment_id: i64 = submission
        .assignment_id
        .0
        .parse()
        .map_err(|_| async_graphql::Error::new("invalid assignment id"))?;
    crate::user_teaches_course_by_assignment(&gctx.pool, gctx.auth_user.id, assignment_id).await
}

// ---------------------------------------------------------------------------
// Root-field contract
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::Query;
    use async_graphql::{EmptySubscription, Schema};

    /// Canvas' 28 root query fields with their argument list and return
    /// type, exactly as `canvas-src/schema.graphql` declares them.
    ///
    /// The argument list is rendered as `name:type` pairs separated by
    /// commas; the type is the GraphQL rendering of the argument's type
    /// (`ID`, `String`, `[ID!]!`, ...).
    const CANVAS_QUERY_ROOT: [(&str, &str, &str); 28] = [
        ("account", "id:ID,sisId:String", "Account"),
        ("accountNotifications", "accountId:ID", "[AccountNotification!]!"),
        ("allCourses", "", "[Course!]"),
        (
            "assignment",
            "id:ID,includeTypes:[AssignmentTypeEnum!],sisId:String",
            "Assignment",
        ),
        ("assignmentGroup", "id:ID,sisId:String", "AssignmentGroup"),
        ("auditLogs", "", "AuditLogs"),
        ("course", "id:ID,sisId:String", "Course"),
        (
            "courseInstructorsConnection",
            "after:String,before:String,courseIds:[ID!]!,enrollmentTypes:[String!],first:Int,last:Int,observedUserId:ID",
            "InstructorWithEnrollmentsConnection",
        ),
        ("courses", "ids:[ID!],sisIds:[String!]", "[Course!]"),
        (
            "enrollmentInvitations",
            "includeEnrollmentUuid:String",
            "[Enrollment!]!",
        ),
        ("folder", "id:ID!", "Folder"),
        ("institutionalTag", "id:ID!", "InstitutionalTag"),
        ("institutionalTagCategory", "id:ID!", "InstitutionalTagCategory"),
        ("internalSetting", "id:ID,name:String", "InternalSetting"),
        ("internalSettings", "", "[InternalSetting!]"),
        ("learningOutcome", "id:ID!", "LearningOutcome"),
        ("learningOutcomeGroup", "id:ID!", "LearningOutcomeGroup"),
        ("legacyNode", "_id:ID!,type:NodeType!", "Node"),
        ("moduleItem", "id:ID!", "ModuleItem"),
        ("myInboxSettings", "", "InboxSettings"),
        ("node", "id:ID!", "Node"),
        ("outcomeCalculationMethod", "id:ID!", "OutcomeCalculationMethod"),
        ("outcomeProficiency", "id:ID!", "OutcomeProficiency"),
        ("peerReviewSubAssignment", "id:ID!", "PeerReviewSubAssignment"),
        ("rubric", "id:ID!", "Rubric"),
        (
            "submission",
            "anonymousId:ID,assignmentId:ID,id:ID,userId:ID",
            "Submission",
        ),
        ("term", "id:ID,sisId:String", "Term"),
        ("user", "id:ID!", "User"),
    ];

    /// The schema the HTTP handler serves.
    fn schema() -> Schema<Query, crate::Mutation, EmptySubscription> {
        Schema::build(Query, crate::Mutation::new(), EmptySubscription).finish()
    }

    /// The `type Query { ... }` block of the schema, one field per entry.
    fn query_root_fields() -> Vec<String> {
        let sdl = schema().sdl();
        let start = sdl
            .find("type Query {")
            .expect("the schema has a Query root");
        let block = &sdl[start..];
        let end = block.find("\n}").expect("the Query block is closed");

        let mut fields = Vec::new();
        let mut current = String::new();
        let mut depth = 0usize;
        let mut in_description = false;
        for line in block["type Query {".len()..end].lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // The SDL carries the doc comments as `"""` description blocks.
            if line.starts_with("\"\"\"") {
                if !(line.len() > 3 && line.ends_with("\"\"\"")) {
                    in_description = !in_description;
                }
                continue;
            }
            if in_description {
                continue;
            }
            if current.is_empty() {
                current.push_str(line);
            } else {
                current.push(' ');
                current.push_str(line);
            }
            depth += line.matches('(').count();
            depth -= line.matches(')').count();
            if depth == 0 && (current.contains(':') || !current.contains('(')) {
                fields.push(std::mem::take(&mut current));
            }
        }
        fields
    }

    /// Split `name(args): Type` into its three parts.
    fn split_field(entry: &str) -> (String, String, String) {
        let (head, return_type) = entry
            .rsplit_once(": ")
            .unwrap_or_else(|| panic!("cannot split `{entry}`"));
        let (name, args) = match head.split_once('(') {
            Some((name, rest)) => {
                let args = rest
                    .strip_suffix(')')
                    .unwrap_or_else(|| panic!("cannot split `{entry}`"));
                (name.trim().to_string(), args.to_string())
            }
            None => (head.trim().to_string(), String::new()),
        };
        (name, args, return_type.trim().to_string())
    }

    #[test]
    fn query_root_matches_canvas_field_names_args_and_types() {
        let fields = query_root_fields();
        let rendered: Vec<(String, String, String)> =
            fields.iter().map(|f| split_field(f)).collect();

        for (name, args, return_type) in CANVAS_QUERY_ROOT {
            let field = rendered
                .iter()
                .find(|(field_name, _, _)| field_name == name)
                .unwrap_or_else(|| {
                    panic!("Canvas root field `{name}` is missing from the query root")
                });

            // `id: ID` -> `id:ID`
            let rendered_args: Vec<String> = field
                .1
                .split(',')
                .filter(|a| !a.trim().is_empty())
                .map(|a| match a.split_once(':') {
                    Some((name, ty)) => format!("{}:{}", name.trim(), ty.trim()),
                    None => a.trim().to_string(),
                })
                .collect();
            assert_eq!(
                rendered_args.join(","),
                args,
                "argument list of `{name}` does not match Canvas"
            );
            assert_eq!(
                field.2, return_type,
                "return type of `{name}` does not match Canvas"
            );
        }
    }

    #[test]
    fn query_root_only_adds_the_three_documented_star_lms_fields() {
        let fields = query_root_fields();
        let canvas: Vec<&str> = CANVAS_QUERY_ROOT.iter().map(|(name, _, _)| *name).collect();

        for entry in &fields {
            let (name, _, _) = split_field(entry);
            assert!(
                canvas.contains(&name.as_str())
                    || name == "enrollmentTerm"
                    || name == "courseSection"
                    || name == "progress",
                "unexpected root field `{name}`"
            );
        }
        assert_eq!(
            fields.len(),
            canvas.len() + 3,
            "the query root must expose Canvas' 28 fields plus the three star-lms extensions"
        );
    }
}

#[cfg(test)]
mod mutation_tests {
    use async_graphql::{EmptySubscription, Schema};

    use crate::Mutation;

    /// The Canvas root mutations this slice implements. The names are
    /// Canvas' own (`canvas-src/app/graphql/mutations/*.rb`).
    const IMPLEMENTED_MUTATIONS: [&str; 18] = [
        "acceptEnrollmentInvitation",
        "createDiscussionTopic",
        "createInternalSetting",
        "createLearningOutcome",
        "createLearningOutcomeGroup",
        "createSubmissionComment",
        "deleteDiscussionTopic",
        "deleteInternalSetting",
        "dismissAccountNotification",
        "rejectEnrollmentInvitation",
        "subscribeToDiscussionTopic",
        "updateDiscussionTopic",
        "updateInternalSetting",
        "updateLearningOutcome",
        "updateLearningOutcomeGroup",
        "updateMyInboxSettings",
        "createAssignment",
        "updateAssignment",
    ];

    fn schema() -> Schema<crate::Query, Mutation, EmptySubscription> {
        Schema::build(crate::Query, Mutation::new(), EmptySubscription).finish()
    }

    /// The field names of the `type Mutation { ... }` block.
    fn mutation_fields() -> Vec<String> {
        let sdl = schema().sdl();
        let start = sdl.find("type Mutation {").expect("a Mutation root");
        let block = &sdl[start..];
        let end = block.find("\n}").expect("the Mutation block is closed");

        let mut fields = Vec::new();
        let mut in_description = false;
        for line in block["type Mutation {".len()..end].lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with("\"\"\"") {
                if !(line.len() > 3 && line.ends_with("\"\"\"")) {
                    in_description = !in_description;
                }
                continue;
            }
            if in_description {
                continue;
            }
            let name = line
                .split(['(', ':'])
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            if !name.is_empty() {
                fields.push(name);
            }
        }
        fields
    }

    #[test]
    fn mutation_root_uses_canvas_mutation_names() {
        let fields = mutation_fields();
        for name in IMPLEMENTED_MUTATIONS {
            assert!(
                fields.iter().any(|f| f == name),
                "the mutation `{name}` is missing from the mutation root"
            );
        }
    }

    #[test]
    fn legacy_id_fields_keep_their_leading_underscore() {
        // Canvas exposes the legacy id as `_id` next to the relay `id`.
        let sdl = schema().sdl();
        for type_name in ["File", "Discussion", "Folder", "SubmissionComment"] {
            let header = format!("type {type_name} ");
            let block = sdl
                .split(&header)
                .nth(1)
                .unwrap_or_else(|| panic!("the schema has a `{type_name}` type"));
            let block = &block[block.find('{').expect("a block")..];
            let block = &block[..block.find("\n}").expect("closed")];
            assert!(
                block.contains("_id: ID!"),
                "`{type_name}` does not expose the Canvas `_id` field"
            );
        }
    }
}
