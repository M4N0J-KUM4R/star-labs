//! GraphQL mutations — Batch B: course-content and inbox operations
//! (GraphQL parity gap completion).
//!
//! Mirrors the Canvas mutation sources per `specs/gql/CONTRACT.md`:
//! every mutation takes exactly one `input` argument (Canvas
//! `HasSingleInputArgument`), authorization failures and missing
//! records are top-level execution errors with Canvas's exact strings
//! (which null out `data.<mutation>`), and validation failures come
//! back as payload-level `errors` arrays inside HTTP 200 — the same
//! channels the merged Batch A (`mutations.rs`) established.
//!
//! Operations (Canvas `app/graphql/mutations/**`):
//! - `createModule` (create_module.rb) — teacher/admin on the course.
//! - `createGroupSet` (create_group_set.rb) — teacher/admin.
//! - `createGroupInSet` (create_group_in_set.rb) — teacher/admin.
//! - `createDiscussionTopic` (create_discussion_topic.rb) — any
//!   enrolled user may post; teacher/admin may publish.
//! - `createDiscussionEntry` (create_discussion_entry.rb) — any
//!   enrolled user on a visible topic.
//! - `createConversation` (create_conversation.rb) — any user, mirror
//!   of the star-conversations REST create (participants + first
//!   message in one transaction).
//! - `addConversationMessage` (add_conversation_message.rb) —
//!   participants only.
//! - `deleteConversation` (delete_conversations.rb, single id) —
//!   participant soft-delete for that user only (REST parity).
//! - `createSubmission` (create_submission.rb) — the student submits
//!   their own (or a teacher submits on behalf, Canvas grading-role
//!   parity collapsed to teacher/admin).
//! - `createSubmissionComment` (create_submission_comment.rb) —
//!   author must see the submission (REST parity gate).
//! - `updateSubmissionGrade` (update_submission_grade.rb) —
//!   teacher/admin, `grade` string like Canvas.
//! - `markSubmissionCommentsRead` (mark_submission_comments_read.rb) —
//!   the submission's student.
//!
//! Authorization parity: all teacher/admin gates collapse onto the
//! same check the matching REST handlers use (teacher enrollment or
//! global admin), and read gates on enrollments — one role model,
//! like Batch A documented for assignments.

use async_graphql::{Context, InputObject, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::db_err;

// ---------------------------------------------------------------------------
// Shared permission checks (REST parity)
// ---------------------------------------------------------------------------

/// Teacher enrollment in the course, or global admin
/// (star-modules `require_teacher_or_admin`, star-conversations
/// equivalent).
async fn is_teacher_or_admin(
    pool: &PgPool,
    caller: &crate::GraphQLContext,
    course_id: i64,
) -> Result<bool, async_graphql::Error> {
    if caller.is_admin() {
        return Ok(true);
    }
    let teaches: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
             AND workflow_state = 'active'
           LIMIT 1"#,
    )
    .bind(caller.auth_user.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(teaches.is_some())
}

/// Any active enrollment in the course, or global admin.
async fn is_enrolled_or_admin(
    pool: &PgPool,
    caller: &crate::GraphQLContext,
    course_id: i64,
) -> Result<bool, async_graphql::Error> {
    if caller.is_admin() {
        return Ok(true);
    }
    let enrolled: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND workflow_state = 'active'
           LIMIT 1"#,
    )
    .bind(caller.auth_user.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(enrolled.is_some())
}

fn parse_id(raw: &ID, what: &str) -> Result<i64, async_graphql::Error> {
    raw.0
        .parse::<i64>()
        .map_err(|_| async_graphql::Error::new(format!("invalid {what}: {}", raw.0)))
}

// ---------------------------------------------------------------------------
// Payload / input types
// ---------------------------------------------------------------------------

/// Shared `errors: [{ attribute, message }]` shape (Batch A contract;
/// Canvas ValidationError parity).
#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBError {
    pub attribute: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ModulePayload {
    pub module: Option<BatchBModule>,
    pub errors: Vec<BatchBError>,
}

/// Module node shape (Canvas `module_type.rb` subset; the read-side
/// `Module` in `modules.rs` has private fields, so the mutation payload
/// carries its own row).
#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBModule {
    pub id: ID,
    pub course_id: ID,
    pub name: String,
    pub position: i32,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateModuleInput {
    pub course_id: ID,
    pub name: String,
    pub position: Option<i32>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GroupSetPayload {
    pub group_set: Option<BatchBGroupSet>,
    pub errors: Vec<BatchBError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBGroupSet {
    pub id: ID,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateGroupSetInput {
    pub course_id: ID,
    pub name: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GroupPayload {
    pub group: Option<BatchBGroup>,
    pub errors: Vec<BatchBError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBGroup {
    pub id: ID,
    pub group_set_id: Option<ID>,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateGroupInSetInput {
    pub group_set_id: ID,
    pub name: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionTopicPayload {
    pub discussion_topic: Option<BatchBDiscussionTopic>,
    pub errors: Vec<BatchBError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBDiscussionTopic {
    pub id: ID,
    pub course_id: ID,
    pub title: String,
    pub message: Option<String>,
    pub published: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateDiscussionTopicInput {
    pub course_id: ID,
    pub title: String,
    pub message: Option<String>,
    pub published: Option<bool>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionEntryPayload {
    pub discussion_entry: Option<BatchBDiscussionEntry>,
    pub errors: Vec<BatchBError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBDiscussionEntry {
    pub id: ID,
    pub discussion_topic_id: ID,
    pub parent_id: Option<ID>,
    pub user_id: Option<ID>,
    pub message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateDiscussionEntryInput {
    pub discussion_topic_id: ID,
    pub message: String,
    pub parent_id: Option<ID>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationPayload {
    pub conversation: Option<BatchBConversation>,
    pub errors: Vec<BatchBError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBConversation {
    pub id: ID,
    pub subject: String,
    /// The conversation's messages (Canvas returns the created
    /// conversation with its messages).
    pub messages: Vec<BatchBConversationMessage>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBConversationMessage {
    pub id: ID,
    pub conversation_id: ID,
    pub author_id: ID,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateConversationInput {
    pub subject: String,
    pub body: String,
    pub recipient_ids: Vec<ID>,
}

#[derive(Debug, Clone, InputObject)]
pub struct AddConversationMessageInput {
    pub conversation_id: ID,
    pub body: String,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteConversationInput {
    pub conversation_id: ID,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct SuccessPayload {
    pub success: bool,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionPayload {
    pub submission: Option<BatchBSubmission>,
    pub errors: Vec<BatchBError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBSubmission {
    pub id: ID,
    pub assignment_id: ID,
    pub user_id: ID,
    pub body: Option<String>,
    pub score: Option<f64>,
    pub grade: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateSubmissionInput {
    pub assignment_id: ID,
    pub user_id: Option<ID>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, InputObject)]
pub struct CreateSubmissionCommentInput {
    pub submission_id: ID,
    pub comment: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionCommentPayload {
    pub submission_comment: Option<BatchBSubmissionComment>,
    pub errors: Vec<BatchBError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchBSubmissionComment {
    pub id: ID,
    pub submission_id: ID,
    pub author_id: ID,
    pub comment: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateSubmissionGradeInput {
    pub submission_id: ID,
    /// Canvas `grade` is a string ("85", "pass") — parity kept.
    pub grade: Option<String>,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, InputObject)]
pub struct MarkSubmissionCommentsReadInput {
    pub submission_id: ID,
}

// ---------------------------------------------------------------------------
// Mutation container
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct BatchBMutations;

#[Object]
impl BatchBMutations {
    /// Canvas `createModule` (`app/graphql/mutations/create_module.rb`).
    async fn create_module(
        &self,
        ctx: &Context<'_>,
        input: CreateModuleInput,
    ) -> Result<ModulePayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        create_module(gctx, input).await
    }

    /// Canvas `createGroupSet` (`app/graphql/mutations/create_group_set.rb`).
    async fn create_group_set(
        &self,
        ctx: &Context<'_>,
        input: CreateGroupSetInput,
    ) -> Result<GroupSetPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        create_group_set(gctx, input).await
    }

    /// Canvas `createGroupInSet`
    /// (`app/graphql/mutations/create_group_in_set.rb`).
    async fn create_group_in_set(
        &self,
        ctx: &Context<'_>,
        input: CreateGroupInSetInput,
    ) -> Result<GroupPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        create_group_in_set(gctx, input).await
    }

    /// Canvas `createDiscussionTopic`
    /// (`app/graphql/mutations/create_discussion_topic.rb`).
    async fn create_discussion_topic(
        &self,
        ctx: &Context<'_>,
        input: CreateDiscussionTopicInput,
    ) -> Result<DiscussionTopicPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        create_discussion_topic(gctx, input).await
    }

    /// Canvas `createDiscussionEntry`
    /// (`app/graphql/mutations/create_discussion_entry.rb`).
    async fn create_discussion_entry(
        &self,
        ctx: &Context<'_>,
        input: CreateDiscussionEntryInput,
    ) -> Result<DiscussionEntryPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        create_discussion_entry(gctx, input).await
    }

    /// Canvas `createConversation`
    /// (`app/graphql/mutations/create_conversation.rb`).
    async fn create_conversation(
        &self,
        ctx: &Context<'_>,
        input: CreateConversationInput,
    ) -> Result<ConversationPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        create_conversation(gctx, input).await
    }

    /// Canvas `addConversationMessage`
    /// (`app/graphql/mutations/add_conversation_message.rb`).
    async fn add_conversation_message(
        &self,
        ctx: &Context<'_>,
        input: AddConversationMessageInput,
    ) -> Result<ConversationPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        add_conversation_message(gctx, input).await
    }

    /// Canvas `deleteConversation` (the single-id form of
    /// `app/graphql/mutations/delete_conversations.rb`).
    async fn delete_conversation(
        &self,
        ctx: &Context<'_>,
        input: DeleteConversationInput,
    ) -> Result<SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        delete_conversation(gctx, input).await
    }

    /// Canvas `createSubmission`
    /// (`app/graphql/mutations/create_submission.rb`).
    async fn create_submission(
        &self,
        ctx: &Context<'_>,
        input: CreateSubmissionInput,
    ) -> Result<SubmissionPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        create_submission(gctx, input).await
    }

    /// Canvas `createSubmissionComment`
    /// (`app/graphql/mutations/create_submission_comment.rb`).
    async fn create_submission_comment(
        &self,
        ctx: &Context<'_>,
        input: CreateSubmissionCommentInput,
    ) -> Result<SubmissionCommentPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        create_submission_comment(gctx, input).await
    }

    /// Canvas `updateSubmissionGrade`
    /// (`app/graphql/mutations/update_submission_grade.rb`).
    async fn update_submission_grade(
        &self,
        ctx: &Context<'_>,
        input: UpdateSubmissionGradeInput,
    ) -> Result<SubmissionPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        update_submission_grade(gctx, input).await
    }

    /// Canvas `markSubmissionCommentsRead`
    /// (`app/graphql/mutations/mark_submission_comments_read.rb`).
    async fn mark_submission_comments_read(
        &self,
        ctx: &Context<'_>,
        input: MarkSubmissionCommentsReadInput,
    ) -> Result<SuccessPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        mark_submission_comments_read(gctx, input).await
    }
}

// ---------------------------------------------------------------------------
// Insert/update result rows (named instead of anonymous tuples — clippy
// type_complexity)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct ModuleInsertRow {
    id: i64,
    course_id: i64,
    name: String,
    position: i32,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct GroupSetInsertRow {
    id: i64,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct GroupInsertRow {
    id: i64,
    group_category_id: Option<i64>,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct TopicInsertRow {
    id: i64,
    course_id: i64,
    title: String,
    message: Option<String>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct EntryInsertRow {
    id: i64,
    topic_id: i64,
    parent_id: Option<i64>,
    user_id: Option<i64>,
    message: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct MessageInsertRow {
    id: i64,
    author_id: i64,
    body: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SubmissionInsertRow {
    id: i64,
    assignment_id: i64,
    user_id: i64,
    body: Option<String>,
    score: Option<f64>,
    submitted_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct CommentInsertRow {
    id: i64,
    submission_id: i64,
    author_id: i64,
    comment: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Implementations
// ---------------------------------------------------------------------------

fn validation_errors(attribute: Option<&str>, message: &str) -> Vec<BatchBError> {
    vec![BatchBError {
        attribute: attribute.map(str::to_string),
        message: message.to_string(),
    }]
}

async fn create_module(
    gctx: &crate::GraphQLContext,
    input: CreateModuleInput,
) -> Result<ModulePayload, async_graphql::Error> {
    let course_id = parse_id(&input.course_id, "course")?;
    let exists: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
    if exists.is_none() {
        return Err(async_graphql::Error::new(format!(
            "invalid course: {}",
            input.course_id.0
        )));
    }
    if !is_teacher_or_admin(&gctx.pool, gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if input.name.trim().is_empty() {
        return Ok(ModulePayload {
            module: None,
            errors: validation_errors(Some("name"), "name cannot be blank"),
        });
    }
    let row: Option<ModuleInsertRow> = sqlx::query_as(
        r#"
        INSERT INTO modules (course_id, name, position, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id, course_id, name, position, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(input.name.trim())
    .bind(input.position.unwrap_or(0))
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new("module create failed"));
    };
    Ok(ModulePayload {
        module: Some(BatchBModule {
            id: ID(row.id.to_string()),
            course_id: ID(row.course_id.to_string()),
            name: row.name,
            position: row.position,
            state: row.workflow_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn create_group_set(
    gctx: &crate::GraphQLContext,
    input: CreateGroupSetInput,
) -> Result<GroupSetPayload, async_graphql::Error> {
    let course_id = parse_id(&input.course_id, "course")?;
    let exists: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
    if exists.is_none() {
        return Err(async_graphql::Error::new(format!(
            "invalid course: {}",
            input.course_id.0
        )));
    }
    if !is_teacher_or_admin(&gctx.pool, gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if input.name.trim().is_empty() {
        return Ok(GroupSetPayload {
            group_set: None,
            errors: validation_errors(Some("name"), "name cannot be blank"),
        });
    }
    let row: Option<GroupSetInsertRow> = sqlx::query_as(
        r#"
        INSERT INTO group_categories (course_id, name)
        VALUES ($1, $2)
        RETURNING id, name, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(input.name.trim())
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new("group set create failed"));
    };
    Ok(GroupSetPayload {
        group_set: Some(BatchBGroupSet {
            id: ID(row.id.to_string()),
            name: row.name,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn create_group_in_set(
    gctx: &crate::GraphQLContext,
    input: CreateGroupInSetInput,
) -> Result<GroupPayload, async_graphql::Error> {
    let set_id = parse_id(&input.group_set_id, "group set")?;
    let course: Option<(Option<i64>,)> = sqlx::query_as(
        r#"SELECT course_id FROM group_categories
           WHERE id = $1 AND workflow_state <> 'deleted'"#,
    )
    .bind(set_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some((Some(course_id),)) = course else {
        return Err(async_graphql::Error::new(format!(
            "group set not found: {}",
            input.group_set_id.0
        )));
    };
    if !is_teacher_or_admin(&gctx.pool, gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if input.name.trim().is_empty() {
        return Ok(GroupPayload {
            group: None,
            errors: validation_errors(Some("name"), "name cannot be blank"),
        });
    }
    let row: Option<GroupInsertRow> = sqlx::query_as(
        r#"
        INSERT INTO groups (course_id, group_category_id, name)
        VALUES ($1, $2, $3)
        RETURNING id, group_category_id, name, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(set_id)
    .bind(input.name.trim())
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new("group create failed"));
    };
    Ok(GroupPayload {
        group: Some(BatchBGroup {
            id: ID(row.id.to_string()),
            group_set_id: row.group_category_id.map(|g| ID(g.to_string())),
            name: row.name,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn create_discussion_topic(
    gctx: &crate::GraphQLContext,
    input: CreateDiscussionTopicInput,
) -> Result<DiscussionTopicPayload, async_graphql::Error> {
    let course_id = parse_id(&input.course_id, "course")?;
    let exists: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
    if exists.is_none() {
        return Err(async_graphql::Error::new(format!(
            "invalid course: {}",
            input.course_id.0
        )));
    }
    // Canvas: any participant may create a topic; publishing requires
    // a teacher. Non-teachers get an unpublished topic (Canvas
    // `published` param is silently ignored without the right — REST
    // parity in star-discussions).
    let teacher = is_teacher_or_admin(&gctx.pool, gctx, course_id).await?;
    if !teacher && !is_enrolled_or_admin(&gctx.pool, gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if input.title.trim().is_empty() {
        return Ok(DiscussionTopicPayload {
            discussion_topic: None,
            errors: validation_errors(Some("title"), "title cannot be blank"),
        });
    }
    let published = teacher && input.published.unwrap_or(false);
    let row: Option<TopicInsertRow> = sqlx::query_as(
        r#"
        INSERT INTO discussion_topics (course_id, title, message, workflow_state)
        VALUES ($1, $2, $3, $4)
        RETURNING id, course_id, title, message, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(input.title.trim())
    .bind(&input.message)
    .bind(if published { "active" } else { "unpublished" })
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new("discussion topic create failed"));
    };
    Ok(DiscussionTopicPayload {
        discussion_topic: Some(BatchBDiscussionTopic {
            id: ID(row.id.to_string()),
            course_id: ID(row.course_id.to_string()),
            title: row.title,
            message: row.message,
            published: row.workflow_state == "active",
            created_at: row.created_at,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn create_discussion_entry(
    gctx: &crate::GraphQLContext,
    input: CreateDiscussionEntryInput,
) -> Result<DiscussionEntryPayload, async_graphql::Error> {
    let topic_id = parse_id(&input.discussion_topic_id, "discussion topic")?;
    let topic: Option<(i64, String)> = sqlx::query_as(
        r#"SELECT course_id, workflow_state FROM discussion_topics
           WHERE id = $1 AND workflow_state <> 'deleted'"#,
    )
    .bind(topic_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some((course_id, state)) = topic else {
        return Err(async_graphql::Error::new(format!(
            "discussion topic not found: {}",
            input.discussion_topic_id.0
        )));
    };
    if !is_enrolled_or_admin(&gctx.pool, gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if input.message.trim().is_empty() {
        return Ok(DiscussionEntryPayload {
            discussion_entry: None,
            errors: validation_errors(Some("message"), "message cannot be blank"),
        });
    }
    // Validate the parent belongs to the same topic when given.
    let parent_id = match &input.parent_id {
        None => None,
        Some(raw) => {
            let pid = parse_id(raw, "parent entry")?;
            let ok: Option<(i32,)> =
                sqlx::query_as("SELECT 1 FROM discussion_entries WHERE id = $1 AND topic_id = $2")
                    .bind(pid)
                    .bind(topic_id)
                    .fetch_optional(&gctx.pool)
                    .await
                    .map_err(db_err)?;
            if ok.is_none() {
                return Err(async_graphql::Error::new(format!(
                    "parent entry not found: {}",
                    raw.0
                )));
            }
            Some(pid)
        }
    };
    let row: Option<EntryInsertRow> = sqlx::query_as(
        r#"
        INSERT INTO discussion_entries (topic_id, parent_id, user_id, message)
        VALUES ($1, $2, $3, $4)
        RETURNING id, topic_id, parent_id, user_id, message, created_at, updated_at
        "#,
    )
    .bind(topic_id)
    .bind(parent_id)
    .bind(gctx.auth_user.id)
    .bind(input.message.trim())
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new("discussion entry create failed"));
    };
    let _ = state;
    Ok(DiscussionEntryPayload {
        discussion_entry: Some(BatchBDiscussionEntry {
            id: ID(row.id.to_string()),
            discussion_topic_id: ID(row.topic_id.to_string()),
            parent_id: row.parent_id.map(|p| ID(p.to_string())),
            user_id: row.user_id.map(|u| ID(u.to_string())),
            message: row.message,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn create_conversation(
    gctx: &crate::GraphQLContext,
    input: CreateConversationInput,
) -> Result<ConversationPayload, async_graphql::Error> {
    if input.subject.trim().is_empty() {
        return Ok(ConversationPayload {
            conversation: None,
            errors: validation_errors(Some("subject"), "subject cannot be blank"),
        });
    }
    if input.body.trim().is_empty() {
        return Ok(ConversationPayload {
            conversation: None,
            errors: validation_errors(Some("body"), "body cannot be blank"),
        });
    }
    let mut recipient_ids = Vec::new();
    for raw in &input.recipient_ids {
        let uid = parse_id(raw, "recipient")?;
        let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
            .bind(uid)
            .fetch_optional(&gctx.pool)
            .await
            .map_err(db_err)?;
        if exists.is_none() {
            return Err(async_graphql::Error::new(format!(
                "recipient not found: {}",
                raw.0
            )));
        }
        if uid != gctx.auth_user.id && !recipient_ids.contains(&uid) {
            recipient_ids.push(uid);
        }
    }
    if recipient_ids.is_empty() {
        return Ok(ConversationPayload {
            conversation: None,
            errors: validation_errors(Some("recipients"), "must have at least one recipient"),
        });
    }

    let mut tx = gctx.pool.begin().await.map_err(db_err)?;
    let (conv_id, subject): (i64, String) =
        sqlx::query_as("INSERT INTO conversations (subject) VALUES ($1) RETURNING id, subject")
            .bind(input.subject.trim())
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;

    let mut participant_ids = vec![gctx.auth_user.id];
    participant_ids.extend(&recipient_ids);
    let now = Utc::now();
    for &uid in &participant_ids {
        let read_at = if uid == gctx.auth_user.id {
            Some(now)
        } else {
            None
        };
        sqlx::query(
            r#"INSERT INTO conversation_participants
               (conversation_id, user_id, workflow_state, last_message_at, read_at)
               VALUES ($1, $2, 'active', now(), $3)"#,
        )
        .bind(conv_id)
        .bind(uid)
        .bind(read_at)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    let msg: MessageInsertRow = sqlx::query_as(
        r#"INSERT INTO conversation_messages (conversation_id, author_id, body)
           VALUES ($1, $2, $3) RETURNING id, author_id, body, created_at"#,
    )
    .bind(conv_id)
    .bind(gctx.auth_user.id)
    .bind(input.body.trim())
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;

    Ok(ConversationPayload {
        conversation: Some(BatchBConversation {
            id: ID(conv_id.to_string()),
            subject,
            messages: vec![BatchBConversationMessage {
                id: ID(msg.id.to_string()),
                conversation_id: ID(conv_id.to_string()),
                author_id: ID(msg.author_id.to_string()),
                body: msg.body,
                created_at: msg.created_at,
            }],
        }),
        errors: Vec::new(),
    })
}

async fn add_conversation_message(
    gctx: &crate::GraphQLContext,
    input: AddConversationMessageInput,
) -> Result<ConversationPayload, async_graphql::Error> {
    let conv_id = parse_id(&input.conversation_id, "conversation")?;
    let participates: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM conversation_participants
           WHERE conversation_id = $1 AND user_id = $2 AND workflow_state = 'active'"#,
    )
    .bind(conv_id)
    .bind(gctx.auth_user.id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    if participates.is_none() {
        return Err(async_graphql::Error::new(format!(
            "conversation not found: {}",
            input.conversation_id.0
        )));
    }
    if input.body.trim().is_empty() {
        return Ok(ConversationPayload {
            conversation: None,
            errors: validation_errors(Some("body"), "body cannot be blank"),
        });
    }
    let mut tx = gctx.pool.begin().await.map_err(db_err)?;
    let msg: MessageInsertRow = sqlx::query_as(
        r#"INSERT INTO conversation_messages (conversation_id, author_id, body)
           VALUES ($1, $2, $3) RETURNING id, author_id, body, created_at"#,
    )
    .bind(conv_id)
    .bind(gctx.auth_user.id)
    .bind(input.body.trim())
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;
    sqlx::query(
        "UPDATE conversation_participants SET last_message_at = now() \
         WHERE conversation_id = $1 AND user_id = $2",
    )
    .bind(conv_id)
    .bind(gctx.auth_user.id)
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    let (subject,): (String,) = sqlx::query_as("SELECT subject FROM conversations WHERE id = $1")
        .bind(conv_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;

    Ok(ConversationPayload {
        conversation: Some(BatchBConversation {
            id: ID(conv_id.to_string()),
            subject,
            messages: vec![BatchBConversationMessage {
                id: ID(msg.id.to_string()),
                conversation_id: ID(conv_id.to_string()),
                author_id: ID(msg.author_id.to_string()),
                body: msg.body,
                created_at: msg.created_at,
            }],
        }),
        errors: Vec::new(),
    })
}

async fn delete_conversation(
    gctx: &crate::GraphQLContext,
    input: DeleteConversationInput,
) -> Result<SuccessPayload, async_graphql::Error> {
    let conv_id = parse_id(&input.conversation_id, "conversation")?;
    let result = sqlx::query(
        r#"UPDATE conversation_participants SET workflow_state = 'deleted'
           WHERE conversation_id = $1 AND user_id = $2 AND workflow_state = 'active'"#,
    )
    .bind(conv_id)
    .bind(gctx.auth_user.id)
    .execute(&gctx.pool)
    .await
    .map_err(db_err)?;
    if result.rows_affected() == 0 {
        return Err(async_graphql::Error::new(format!(
            "conversation not found: {}",
            input.conversation_id.0
        )));
    }
    Ok(SuccessPayload { success: true })
}

/// Load a submission + its assignment's course for authorization.
async fn load_submission_course(
    pool: &PgPool,
    submission_id: i64,
) -> Result<Option<(i64, i64, i64)>, async_graphql::Error> {
    // (submission_id, user_id, course_id)
    let row: Option<(i64, i64, i64)> = sqlx::query_as(
        r#"SELECT s.id, s.user_id, a.course_id
           FROM submissions s JOIN assignments a ON a.id = s.assignment_id
           WHERE s.id = $1"#,
    )
    .bind(submission_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row)
}

async fn create_submission(
    gctx: &crate::GraphQLContext,
    input: CreateSubmissionInput,
) -> Result<SubmissionPayload, async_graphql::Error> {
    let assignment_id = parse_id(&input.assignment_id, "assignment")?;
    let (course_id,): (i64,) = sqlx::query_as("SELECT course_id FROM assignments WHERE id = $1")
        .bind(assignment_id)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| {
            async_graphql::Error::new(format!("assignment not found: {assignment_id}"))
        })?;

    let target = match &input.user_id {
        Some(raw) => parse_id(raw, "user")?,
        None => gctx.auth_user.id,
    };
    // Self-submission needs an enrollment; submitting for someone else
    // needs teacher/admin (Canvas grading role, collapsed).
    if target == gctx.auth_user.id {
        if !is_enrolled_or_admin(&gctx.pool, gctx, course_id).await? {
            return Err(async_graphql::Error::new("insufficient permission"));
        }
    } else if !is_teacher_or_admin(&gctx.pool, gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if input
        .body
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        return Ok(SubmissionPayload {
            submission: None,
            errors: validation_errors(Some("body"), "body cannot be blank"),
        });
    }

    // Upsert on (assignment_id, user_id) — Canvas resubmits replace the
    // body (star-assignments REST parity on the UNIQUE constraint).
    let row: Option<SubmissionInsertRow> = sqlx::query_as(
        r#"
        INSERT INTO submissions (assignment_id, user_id, body, submitted_at)
        VALUES ($1, $2, $3, now())
        ON CONFLICT (assignment_id, user_id)
        DO UPDATE SET body = EXCLUDED.body, submitted_at = now(), updated_at = now()
        RETURNING id, assignment_id, user_id, body, score::float8, submitted_at, created_at, updated_at
        "#,
    )
    .bind(assignment_id)
    .bind(target)
    .bind(input.body.as_deref().map(str::trim))
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new("submission create failed"));
    };
    Ok(SubmissionPayload {
        submission: Some(BatchBSubmission {
            id: ID(row.id.to_string()),
            assignment_id: ID(row.assignment_id.to_string()),
            user_id: ID(row.user_id.to_string()),
            body: row.body,
            score: row.score,
            grade: row.score.map(|s| format!("{}", s as i64)),
            submitted_at: row.submitted_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn create_submission_comment(
    gctx: &crate::GraphQLContext,
    input: CreateSubmissionCommentInput,
) -> Result<SubmissionCommentPayload, async_graphql::Error> {
    let submission_id = parse_id(&input.submission_id, "submission")?;
    let Some((sid, student_id, course_id)) =
        load_submission_course(&gctx.pool, submission_id).await?
    else {
        return Err(async_graphql::Error::new(format!(
            "submission not found: {}",
            input.submission_id.0
        )));
    };
    // The submission's student or a teacher/admin of the course (REST
    // star-submission-comments visibility gate).
    let allowed =
        student_id == gctx.auth_user.id || is_teacher_or_admin(&gctx.pool, gctx, course_id).await?;
    if !allowed {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if input.comment.trim().is_empty() {
        return Ok(SubmissionCommentPayload {
            submission_comment: None,
            errors: validation_errors(Some("comment"), "comment cannot be blank"),
        });
    }
    let row: Option<CommentInsertRow> = sqlx::query_as(
        r#"
        INSERT INTO submission_comments (submission_id, author_id, comment)
        VALUES ($1, $2, $3)
        RETURNING id, submission_id, author_id, comment, created_at, updated_at
        "#,
    )
    .bind(sid)
    .bind(gctx.auth_user.id)
    .bind(input.comment.trim())
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new(
            "submission comment create failed",
        ));
    };
    Ok(SubmissionCommentPayload {
        submission_comment: Some(BatchBSubmissionComment {
            id: ID(row.id.to_string()),
            submission_id: ID(row.submission_id.to_string()),
            author_id: ID(row.author_id.to_string()),
            comment: row.comment,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn update_submission_grade(
    gctx: &crate::GraphQLContext,
    input: UpdateSubmissionGradeInput,
) -> Result<SubmissionPayload, async_graphql::Error> {
    let submission_id = parse_id(&input.submission_id, "submission")?;
    let Some((sid, _student_id, course_id)) =
        load_submission_course(&gctx.pool, submission_id).await?
    else {
        return Err(async_graphql::Error::new(format!(
            "submission not found: {}",
            input.submission_id.0
        )));
    };
    if !is_teacher_or_admin(&gctx.pool, gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    // Canvas takes `grade` (string) or `score`; score wins when both
    // are given (Canvas `update_submission_grade.rb` computes score
    // from grade via the grading scheme — star-lms has no scheme
    // engine, so a numeric grade string is parsed, other strings are a
    // documented deviation that stores only the passed score).
    let score: Option<f64> = match input.score {
        Some(s) => Some(s),
        None => match &input.grade {
            Some(g) => g.trim().parse::<f64>().ok(),
            None => None,
        },
    };
    if input.score.is_none() && input.grade.is_none() {
        return Ok(SubmissionPayload {
            submission: None,
            errors: validation_errors(Some("grade"), "grade or score is required"),
        });
    }
    let row: Option<SubmissionInsertRow> = sqlx::query_as(
        r#"
        UPDATE submissions
        SET score = $2::numeric, updated_at = now()
        WHERE id = $1
        RETURNING id, assignment_id, user_id, body, score::float8, submitted_at, created_at, updated_at
        "#,
    )
    .bind(sid)
    .bind(score)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new("submission not found"));
    };
    Ok(SubmissionPayload {
        submission: Some(BatchBSubmission {
            id: ID(row.id.to_string()),
            assignment_id: ID(row.assignment_id.to_string()),
            user_id: ID(row.user_id.to_string()),
            body: row.body,
            score: row.score,
            grade: row.score.map(|s| format!("{}", s as i64)),
            submitted_at: row.submitted_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn mark_submission_comments_read(
    gctx: &crate::GraphQLContext,
    input: MarkSubmissionCommentsReadInput,
) -> Result<SuccessPayload, async_graphql::Error> {
    let submission_id = parse_id(&input.submission_id, "submission")?;
    let Some((sid, student_id, course_id)) =
        load_submission_course(&gctx.pool, submission_id).await?
    else {
        return Err(async_graphql::Error::new(format!(
            "submission not found: {}",
            input.submission_id.0
        )));
    };
    // Canvas marks the current user's comment read state; the natural
    // subject is the submission's student (or a teacher reading the
    // thread — allow both, REST parity).
    let allowed =
        student_id == gctx.auth_user.id || is_teacher_or_admin(&gctx.pool, gctx, course_id).await?;
    if !allowed {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    // star-db has no per-user submission-comment read rows (the
    // 0225 viewed_submission_comments snapshot table is unused by the
    // workspace handlers) — record the read as a no-op success, like
    // Canvas idempotently flagging. Documented deviation.
    let _ = sid;
    Ok(SuccessPayload { success: true })
}
