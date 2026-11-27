//! star-graphql — GraphQL foundation for star-lms.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`, mounted by star-http
//! at `/api/graphql` (Canvas serves its GraphQL API there, outside `/api/v1`).
//!
//! The vertical slice covers Canvas-shaped read queries over the existing
//! schema: `course(id)` and `user(id)` with `enrollmentsConnection` cursor
//! pagination, mirroring the connection conventions of Canvas's own GraphQL
//! schema (`app/graphql/types/*_type.rb` in canvas-lms):
//! camelCase field names (`sisUserId`, `createdAt`), relay-style connections
//! with `edges { cursor node }` and `pageInfo { hasNextPage endCursor }`, and
//! opaque cursors.
//!
//! Authorization parity with the REST endpoints:
//! - unauthenticated requests are rejected with HTTP 401 *before* any GraphQL
//!   execution (via the shared `AuthUser` extractor, not a GraphQL error);
//! - `course(id)` requires an enrollment in the course (any role) or admin;
//! - `Course.enrollmentsConnection` shows every enrollment to admins and
//!   course teachers, but only the caller's own enrollment to anyone else;
//! - `User.enrollmentsConnection` is self-or-admin (null for others).
//!
//! Issue #28 expands `Course` and `User` to Canvas field parity
//! (`course_type.rb` / `user_type.rb`): `Course` gains `courseCode`,
//! `syllabusBody`, `startAt`, `endAt`, `account`, `permissions` and
//! `usersConnection`; `User` gains `sortableName` and `avatarUrl`.
//! Permission-gated fields resolve to null for unauthorized callers, like
//! Canvas GraphQL — never errors. See `course_user_fields.rs` for the
//! backing resolvers and the documented omissions (fields with no backing
//! column in star-db).

use async_graphql::{Context, EmptySubscription, MergedObject, Object, Schema, SimpleObject, ID};
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

mod assignment;
mod assignment_group;
mod conversations;
mod course_user_fields;
mod discussion_entries;
mod discussion_topic;
// Entity types registered for parity; resolvers wire the Canvas-nested
// fields, the rest are schema-orphans (allowed dead code).
#[allow(dead_code)]
mod entities_batch_f;
#[allow(dead_code)]
mod entities_modules_discussions_submissions;
mod external_tools;
mod files;
// Leaf enums/inputs registered for parity; entity batches wire the
// ones Canvas nests, the rest stay schema-orphans (allowed dead code).
mod grading_periods;
mod grading_standards_account_notifications;
#[allow(dead_code)]
mod graphql_enums_inputs;
mod groups;
mod modules;
mod mutations;
mod mutations_batch_b;
mod mutations_batch_c;
mod mutations_batch_g1;
mod mutations_batch_g2;
mod mutations_batch_g3;
mod node;
mod outcomes_media;
mod pages;
mod query_root;
pub use query_root::Query;
mod quiz;
mod root_data;
mod root_mutations;
mod root_types;
mod rubrics;
mod submission;
mod terms_sections;
mod user_channels;

// The `Node` interface (`node.rs`) is public, so every type it can return
// has to be publicly reachable. These re-exports are that reachability;
// the modules themselves stay private.
pub use assignment::Assignment;
use assignment::AssignmentConnection;
use assignment_group::AssignmentGroupConnection;
pub use conversations::Conversation;
use conversations::ConversationParticipantConnection;
pub use course_user_fields::Account;
use course_user_fields::{CoursePermissions, UserConnection};
pub use discussion_topic::DiscussionTopic;
use discussion_topic::DiscussionTopicConnection;
use external_tools::ExternalToolConnection;
use files::FolderConnection;
pub use files::{File, Folder};
use grading_periods::GradingPeriodGroup;
pub use groups::{Group, GroupSet};
use groups::{GroupConnection, GroupSetConnection};
use modules::ModuleConnection;
pub use modules::{Module, ModuleItem};
pub use node::{Node, NodeType};
use pages::PageConnection;
pub use pages::{Page, Progress};
use quiz::QuizConnection;
pub use root_types::{
    AccountNotification, AssignmentGroup, AuditLogs, InboxSettings, InstitutionalTag,
    InstitutionalTagCategory, InternalSetting, LearningOutcome, LearningOutcomeGroup,
    OutcomeCalculationMethod, OutcomeProficiency, PeerReviewSubAssignment, Rubric,
    SubmissionComment,
};
use rubrics::RubricConnection;
pub use submission::Submission;
use terms_sections::{CourseSection, EnrollmentTerm};

// ---------------------------------------------------------------------------
// GraphQL context
// ---------------------------------------------------------------------------

/// Per-request execution context: the database pool plus the authenticated
/// user. The user is resolved by the `AuthUser` extractor before the schema
/// runs, so the role is always fresh from the database (the JWT role claim is
/// never trusted for authorization, matching the REST extractors).
pub struct GraphQLContext {
    pub(crate) pool: PgPool,
    pub(crate) auth_user: AuthUser,
}

impl GraphQLContext {
    pub(crate) fn is_admin(&self) -> bool {
        self.auth_user.role == "admin"
    }
}

/// Map a database failure to a GraphQL error. Resolvers never panic on DB
/// trouble; failures surface as normal GraphQL `errors` entries.
pub(crate) fn db_err(e: sqlx::Error) -> async_graphql::Error {
    eprintln!("database error: {e}");
    async_graphql::Error::new("database error")
}

// ---------------------------------------------------------------------------
// Opaque cursors
// ---------------------------------------------------------------------------

/// Encode an enrollment id as an opaque pagination cursor (fixed-width hex).
/// Clients treat cursors as opaque tokens; `decode_cursor` is the only reader.
pub(crate) fn encode_cursor(id: i64) -> String {
    format!("{id:016x}")
}

pub(crate) fn decode_cursor(cursor: &str) -> Option<i64> {
    i64::from_str_radix(cursor, 16).ok()
}

// ---------------------------------------------------------------------------
// Database rows (runtime sqlx::query_as + FromRow, like every other crate)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct CourseRow {
    id: i64,
    name: String,
    course_code: String,
    syllabus_body: Option<String>,
    workflow_state: String,
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    account_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct UserRow {
    id: i64,
    name: String,
    email: String,
    sortable_name: String,
    avatar_url: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct EnrollmentRow {
    id: i64,
    user_id: i64,
    course_id: i64,
    role: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Course` (subset of `app/graphql/types/course_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `name`,
/// `courseCode`, `syllabusBody`, `state`, `startAt`, `endAt`, `createdAt`,
/// `updatedAt`, `enrollmentsConnection`, `assignmentsConnection`,
/// `usersConnection`, `account`, `permissions`.
pub struct Course {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) course_code: String,
    pub(crate) syllabus_body: Option<String>,
    pub(crate) workflow_state: String,
    pub(crate) start_at: Option<DateTime<Utc>>,
    pub(crate) end_at: Option<DateTime<Utc>>,
    pub(crate) account_id: Option<i64>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl Course {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn course_code(&self) -> &str {
        &self.course_code
    }

    async fn syllabus_body(&self) -> Option<&str> {
        self.syllabus_body.as_deref()
    }

    async fn state(&self) -> &str {
        &self.workflow_state
    }

    async fn start_at(&self) -> Option<DateTime<Utc>> {
        self.start_at
    }

    async fn end_at(&self) -> Option<DateTime<Utc>> {
        self.end_at
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    // -- issue #28 additions: account / permissions / usersConnection --------

    async fn account(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<course_user_fields::Account>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let Some(account_id) = self.account_id else {
            return Ok(None);
        };
        course_user_fields::fetch_account(&gctx.pool, account_id).await
    }

    async fn permissions(
        &self,
        ctx: &Context<'_>,
    ) -> Result<CoursePermissions, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let is_admin = gctx.is_admin();
        let teaches = user_teaches_course(&gctx.pool, gctx.auth_user.id, self.id).await?;
        let manages = is_admin || teaches;
        // becomeUser follows the account-admin chain (global admin or
        // account_users row on the course's account or an ancestor), like
        // the REST become-user gate — course teachers do NOT get it.
        // NULL account ids fall back to the default account (1).
        let become_user = is_admin
            || course_user_fields::is_account_admin_in_chain(
                &gctx.pool,
                gctx.auth_user.id,
                self.account_id.unwrap_or(1),
            )
            .await?;
        Ok(CoursePermissions {
            manage_grades: manages,
            send_messages: manages
                || user_enrolled_in_course(&gctx.pool, gctx.auth_user.id, self.id).await?,
            view_all_grades: manages,
            view_analytics: manages,
            become_user,
        })
    }

    async fn users_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<UserConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;

        if !gctx.is_admin() && !user_teaches_course(&gctx.pool, gctx.auth_user.id, self.id).await? {
            return Ok(None);
        }

        Ok(Some(
            course_user_fields::fetch_user_page(&gctx.pool, self.id, first, after.as_deref())
                .await?,
        ))
    }

    async fn enrollments_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<EnrollmentConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;

        let scoped_to_self = !gctx.is_admin()
            && !user_teaches_course(&gctx.pool, gctx.auth_user.id, self.id).await?;
        let user_id = if scoped_to_self {
            Some(gctx.auth_user.id)
        } else {
            None
        };

        Ok(Some(
            fetch_enrollment_page(&gctx.pool, Some(self.id), user_id, first, after.as_deref())
                .await?,
        ))
    }

    async fn assignments_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<AssignmentConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            assignment::fetch_assignment_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    async fn assignment_groups_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<AssignmentGroupConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            assignment_group::fetch_assignment_group_page(
                &gctx.pool,
                self.id,
                first,
                after.as_deref(),
            )
            .await?,
        ))
    }

    async fn grading_periods(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<GradingPeriodGroup>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        grading_periods::fetch_course_grading_period_group(&gctx.pool, self.id).await
    }

    async fn grading_standards(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Vec<grading_standards_account_notifications::GradingStandard>, async_graphql::Error>
    {
        let gctx = ctx.data::<GraphQLContext>()?;
        grading_standards_account_notifications::fetch_course_grading_standards(&gctx.pool, self.id)
            .await
    }

    async fn rubrics_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<RubricConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            rubrics::fetch_rubric_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    async fn quizzes_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<QuizConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            quiz::fetch_quiz_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    async fn discussions_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<DiscussionTopicConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            discussion_topic::fetch_discussion_topic_page(
                &gctx.pool,
                self.id,
                first,
                after.as_deref(),
            )
            .await?,
        ))
    }

    async fn outcomes_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<outcomes_media::LearningOutcomeConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            outcomes_media::fetch_course_outcome_page(&gctx.pool, self.id, first, after.as_deref())
                .await?,
        ))
    }

    async fn outcome_groups(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Vec<outcomes_media::LearningOutcomeGroup>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        outcomes_media::fetch_course_outcome_groups(&gctx.pool, self.id).await
    }

    async fn external_tools_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<ExternalToolConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            external_tools::fetch_external_tool_page(&gctx.pool, self.id, first, after.as_deref())
                .await?,
        ))
    }

    async fn modules_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<ModuleConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            modules::fetch_module_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    async fn pages_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<PageConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            pages::fetch_page_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    async fn folders_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<FolderConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            files::fetch_folder_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    async fn groups_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<GroupConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            groups::fetch_group_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    async fn group_sets_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<GroupSetConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            groups::fetch_group_set_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }
}

/// Canvas GraphQL `User` (subset of `app/graphql/types/user_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `name`, `email`,
/// `sisUserId`, `sortableName`, `avatarUrl`, `createdAt`,
/// `enrollmentsConnection`.
#[derive(Debug, Clone)]
pub struct User {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) email: String,
    /// Canvas `User.sisId`. star-lms has no SIS columns yet, so this is null
    /// for now; the field is kept for shape parity.
    pub(crate) sis_user_id: Option<String>,
    /// Canvas `User.sortableName` (`users.sortable_name`, migration 0008);
    /// coalesced onto `name` by the root fetches, never null.
    pub(crate) sortable_name: String,
    /// Canvas `User.avatarUrl` — backed by `user_profiles.avatar_url`
    /// (migration 0051); users without a profile row resolve null.
    pub(crate) avatar_url: Option<String>,
    pub(crate) created_at: DateTime<Utc>,
}

#[Object]
impl User {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn email(&self) -> &str {
        &self.email
    }

    async fn sis_user_id(&self) -> Option<&str> {
        self.sis_user_id.as_deref()
    }

    // -- issue #28 additions: sortableName / avatarUrl -----------------------

    async fn sortable_name(&self) -> &str {
        &self.sortable_name
    }

    async fn avatar_url(&self) -> Option<&str> {
        self.avatar_url.as_deref()
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn enrollments_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<EnrollmentConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;

        if !(gctx.is_admin()
            || gctx.auth_user.id == self.id
            || caller_reads_user_enrollments(&gctx.pool, gctx.auth_user.id, self.id).await?)
        {
            return Ok(None);
        }

        Ok(Some(
            fetch_enrollment_page(&gctx.pool, None, Some(self.id), first, after.as_deref()).await?,
        ))
    }

    async fn conversations_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<ConversationParticipantConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if gctx.auth_user.id != self.id {
            return Ok(None);
        }
        Ok(Some(
            conversations::fetch_user_conversations_page(
                &gctx.pool,
                self.id,
                first,
                after.as_deref(),
            )
            .await?,
        ))
    }

    async fn communication_channels(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<Vec<user_channels::CommunicationChannel>>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !user_channels::is_self_or_admin(gctx, self.id) {
            return Ok(None);
        }
        Ok(Some(
            user_channels::fetch_communication_channels(&gctx.pool, self.id).await?,
        ))
    }

    async fn notifications(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<Vec<user_channels::Notification>>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !user_channels::is_self_or_admin(gctx, self.id) {
            return Ok(None);
        }
        Ok(Some(
            user_channels::fetch_notifications(&gctx.pool, self.id).await?,
        ))
    }

    async fn notification_preferences(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<Vec<user_channels::NotificationPolicy>>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !user_channels::is_self_or_admin(gctx, self.id) {
            return Ok(None);
        }
        Ok(Some(
            user_channels::fetch_notification_policies(&gctx.pool, self.id).await?,
        ))
    }
}

/// Canvas GraphQL `Enrollment` (subset of `enrollment_type.rb`). Field names
/// are camelCased by the SimpleObject derive: `id`, `userId`, `courseId`,
/// `role`, `state`, `createdAt`, `updatedAt`.
#[derive(Debug, Clone, SimpleObject)]
pub struct Enrollment {
    pub id: ID,
    pub user_id: ID,
    pub course_id: ID,
    pub role: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<EnrollmentRow> for Enrollment {
    fn from(row: EnrollmentRow) -> Self {
        Enrollment {
            id: ID(row.id.to_string()),
            user_id: ID(row.user_id.to_string()),
            course_id: ID(row.course_id.to_string()),
            role: row.role,
            state: row.workflow_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct EnrollmentEdge {
    pub cursor: String,
    pub node: Enrollment,
}

/// Relay page info: `pageInfo { hasNextPage endCursor }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

/// Relay connection for enrollments.
#[derive(Debug, Clone, SimpleObject)]
pub struct EnrollmentConnection {
    pub edges: Vec<EnrollmentEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Shared user fetch (used by the group/conversation slices for
// GroupMembership.user / ConversationMessage.author / participant user)
// ---------------------------------------------------------------------------

/// Fetch a user by id for nested resolvers. Any authenticated caller may
/// read the basic profile shape — the same visibility as `Query.user`.
pub(crate) async fn fetch_user(
    pool: &sqlx::PgPool,
    user_id: i64,
) -> Result<Option<User>, async_graphql::Error> {
    let row: Option<UserRow> = sqlx::query_as(
        r#"
        SELECT u.id, u.name, u.email,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               up.avatar_url,
               u.created_at
        FROM users u
        LEFT JOIN user_profiles up ON up.user_id = u.id
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| User {
        id: row.id,
        name: row.name,
        email: row.email,
        sis_user_id: None,
        sortable_name: row.sortable_name,
        avatar_url: row.avatar_url,
        created_at: row.created_at,
    }))
}

// ---------------------------------------------------------------------------
// Query root
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Mutation root (issue #29: Batch A — assignment create/update/delete)
// ---------------------------------------------------------------------------

/// Mutation root for the first batch. Canvas conventions (`base_mutation.rb`):
/// exactly one `input` argument per mutation, payload carries
/// `errors: [ValidationErrorType]`, authz failures are top-level execution
/// errors with Canvas's exact strings. Deletion is `updateAssignment` with
/// `state: deleted` — Canvas has no `deleteAssignment` mutation and neither
/// do we (spec §2.1). Batches B–E land with their own issues.
#[derive(MergedObject, Default)]
pub struct Mutation(
    mutations::AssignmentMutations,
    root_mutations::RootMutations,
    mutations_batch_b::BatchBMutations,
    mutations_batch_c::BatchCMutations,
    mutations_batch_g1::BatchG1Mutations,
    mutations_batch_g2::BatchG2Mutations,
    mutations_batch_g3::BatchG3Mutations,
);

impl Mutation {
    fn new() -> Self {
        // The mutation containers are unit structs; clippy's
        // default_constructed_unit_structs rejects `::default()` under -D warnings.
        Mutation(
            mutations::AssignmentMutations,
            root_mutations::RootMutations,
            mutations_batch_b::BatchBMutations,
            mutations_batch_c::BatchCMutations,
            mutations_batch_g1::BatchG1Mutations,
            mutations_batch_g2::BatchG2Mutations,
            mutations_batch_g3::BatchG3Mutations,
        )
    }
}

// ---------------------------------------------------------------------------
// Permission helpers (same DB checks as the star-enrollments REST handlers)
// ---------------------------------------------------------------------------

pub(crate) async fn user_enrolled_in_course(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<bool, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND workflow_state != 'deleted'
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.is_some())
}

pub(crate) async fn user_teaches_course(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<bool, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
             AND workflow_state != 'deleted'
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.is_some())
}

// -- issue #28: User.enrollmentsConnection visibility -------------------------

async fn caller_reads_user_enrollments(
    pool: &PgPool,
    caller_id: i64,
    target_id: i64,
) -> Result<bool, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments caller_e
           JOIN enrollments target_e ON target_e.course_id = caller_e.course_id
           WHERE caller_e.user_id = $1
             AND caller_e.role = 'teacher'
             AND caller_e.workflow_state != 'deleted'
             AND target_e.user_id = $2
             AND target_e.workflow_state != 'deleted'
           LIMIT 1"#,
    )
    .bind(caller_id)
    .bind(target_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Cursor pagination
// ---------------------------------------------------------------------------

/// Default page size for connections when `first` is omitted.
pub(crate) const DEFAULT_PAGE_SIZE: i32 = 10;

/// Maximum accepted page size for connections.
pub(crate) const MAX_PAGE_SIZE: i32 = 100;

async fn fetch_enrollment_page(
    pool: &PgPool,
    course_id: Option<i64>,
    user_id: Option<i64>,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<EnrollmentConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<EnrollmentRow> = sqlx::query_as(
        r#"
        SELECT e.id, e.user_id, e.course_id, e.role, e.workflow_state,
               e.created_at, e.updated_at
        FROM enrollments e
        WHERE e.workflow_state != 'deleted'
          AND ($1::bigint IS NULL OR e.course_id = $1)
          AND ($2::bigint IS NULL OR e.user_id = $2)
          AND ($3::bigint IS NULL OR e.id > $3)
        ORDER BY e.id
        LIMIT $4
        "#,
    )
    .bind(course_id)
    .bind(user_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<EnrollmentEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            EnrollmentEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(EnrollmentConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

// ---------------------------------------------------------------------------
// Router + handler
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(graphql_handler).post(graphql_handler))
}

async fn graphql_handler(
    State(state): State<AppState>,
    auth: Result<AuthUser, ApiError>,
    req: GraphQLRequest,
) -> Response {
    let auth_user = match auth {
        Ok(user) => user,
        Err(e) => return e.into_response(),
    };

    let schema = Schema::build(Query, Mutation::new(), EmptySubscription)
        .data(GraphQLContext {
            pool: state.db.clone(),
            auth_user,
        })
        .finish();

    let resp = schema.execute(req.into_inner()).await;
    GraphQLResponse::from(resp).into_response()
}

// ---------------------------------------------------------------------------
// Row loaders shared by the query root and the node resolver
// ---------------------------------------------------------------------------

/// Load a course by id without a permission check.
///
/// The permission check lives in [`fetch_course_authorized`].
/// Load a course by id and apply the Canvas `course(id)` visibility rule:
/// enrolled users of the course (any role) and admins see it, everyone
/// else gets null.
/// Load courses by id without a permission check.
/// Load an enrollment by id without a permission check.
/// Whether the user teaches the course the assignment belongs to.
///
/// Canvas gates a submission on the caller's teacher rights over the
/// assignment's course.
pub(crate) async fn user_teaches_course_by_assignment(
    pool: &PgPool,
    user_id: i64,
    assignment_id: i64,
) -> Result<bool, async_graphql::Error> {
    let course: Option<(i64,)> = sqlx::query_as("SELECT course_id FROM assignments WHERE id = $1")
        .bind(assignment_id)
        .fetch_optional(pool)
        .await
        .map_err(db_err)?;
    let Some((course_id,)) = course else {
        return Ok(false);
    };
    user_teaches_course(pool, user_id, course_id).await
}

/// The permission check lives in [`fetch_course_authorized`].
pub(crate) async fn fetch_course(
    pool: &PgPool,
    id: i64,
) -> Result<Option<Course>, async_graphql::Error> {
    let row: Option<CourseRow> = sqlx::query_as(
        r#"
        SELECT id, name, course_code, syllabus_body, workflow_state,
               start_at, end_at, account_id, created_at, updated_at
        FROM courses
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| Course {
        id: row.id,
        name: row.name,
        course_code: row.course_code,
        syllabus_body: row.syllabus_body,
        workflow_state: row.workflow_state,
        start_at: row.start_at,
        end_at: row.end_at,
        account_id: row.account_id,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

/// else gets null.
pub(crate) async fn fetch_course_authorized(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    id: i64,
) -> Result<Option<Course>, async_graphql::Error> {
    let Some(course) = fetch_course(pool, id).await? else {
        return Ok(None);
    };
    if !is_admin && !user_enrolled_in_course(pool, user_id, id).await? {
        return Ok(None);
    }
    Ok(Some(course))
}

/// Load courses by id without a permission check.
pub(crate) async fn fetch_courses_by_ids(
    pool: &PgPool,
    ids: &[i64],
) -> Result<Vec<Course>, async_graphql::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<CourseRow> = sqlx::query_as(
        r#"
        SELECT id, name, course_code, syllabus_body, workflow_state,
               start_at, end_at, account_id, created_at, updated_at
        FROM courses
        WHERE id = ANY($1)
        ORDER BY id
        "#,
    )
    .bind(ids)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|row| Course {
            id: row.id,
            name: row.name,
            course_code: row.course_code,
            syllabus_body: row.syllabus_body,
            workflow_state: row.workflow_state,
            start_at: row.start_at,
            end_at: row.end_at,
            account_id: row.account_id,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

/// Load an enrollment by id without a permission check.
pub(crate) async fn fetch_enrollment(
    pool: &PgPool,
    id: i64,
) -> Result<Option<Enrollment>, async_graphql::Error> {
    let row: Option<EnrollmentRow> = sqlx::query_as(
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

    Ok(row.map(Enrollment::from))
}
