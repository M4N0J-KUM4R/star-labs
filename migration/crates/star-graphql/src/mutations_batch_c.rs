//! GraphQL mutations — Batch C: update/delete families for the
//! Batch-B surfaces plus rubric assessments and notification
//! preferences (GraphQL parity gap completion).
//!
//! Same contract as Batches A/B (`specs/gql/CONTRACT.md`): one `input`
//! argument per mutation; authorization failures / missing records as
//! top-level execution errors with Canvas's exact strings; validation
//! failures as payload `errors` arrays inside HTTP 200.
//!
//! Operations (Canvas `app/graphql/mutations/**`):
//! - `updateDiscussionTopic` (update_discussion_topic.rb) —
//!   teacher/admin (Canvas `moderate_forum`); Canvas's `published`
//!   maps onto the active/unpublished states the REST handlers use.
//! - `deleteDiscussionTopic` (delete_discussion_topic.rb) —
//!   teacher/admin; soft delete.
//! - `updateDiscussionEntry` (update_discussion_entry.rb) — the
//!   entry's author (Canvas allows editing own entries).
//! - `deleteDiscussionEntry` (delete_discussion_entry.rb) — the author
//!   or teacher/admin (Canvas moderation parity).
//! - `createLearningOutcome` (create_learning_outcome.rb) —
//!   teacher/admin on the owning group's course.
//! - `updateLearningOutcome` (update_learning_outcome.rb).
//! - `deleteLearningOutcome` (delete_learning_outcome.rb) — soft
//!   delete.
//! - `createLearningOutcomeGroup` (create_learning_outcome_group.rb) —
//!   teacher/admin.
//! - `updateLearningOutcomeGroup` (update_learning_outcome_group.rb).
//! - `deleteLearningOutcomeGroup` (delete_learning_outcome_group.rb) —
//!   soft delete.
//! - `saveRubricAssessment` (save_rubric_assessment.rb) — teacher/admin
//!   on the submission's course; Canvas takes criterion/rating pairs —
//!   star-lms persists the score (the 0023 `rubric_assessments` row)
//!   and accepts the data JSON (documented subset).
//! - `deleteSubmissionComment` (delete_submission_comment.rb) — the
//!   comment's author (REST parity with the comment delete gate).
//! - `updateNotificationPreferences` (Canvas
//!   update_notification_preferences.rb sets channel-level
//!   frequencies; the 0045 preference rows carry per-notification
//!   frequency — this mutation upserts the given notification's
//!   policy for the caller).
//! - `dismissAccountNotification` (dismiss_account_notification.rb) —
//!   star-db has no per-user dismissal rows (Canvas stores
//!   `account_user_dismissals`), so the mutation validates the
//!   notification exists and is window-valid and returns an idempotent
//!   success (documented deviation, same shape as
//!   markSubmissionCommentsRead).

use async_graphql::{Context, InputObject, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};

use crate::mutations_batch_b::SuccessPayload;
use crate::GraphQLContext;

// ---------------------------------------------------------------------------
// Shared helpers (Batch B parity)
// ---------------------------------------------------------------------------

async fn is_teacher_or_admin(
    gctx: &GraphQLContext,
    course_id: i64,
) -> Result<bool, async_graphql::Error> {
    if gctx.is_admin() {
        return Ok(true);
    }
    let teaches: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
             AND workflow_state = 'active'
           LIMIT 1"#,
    )
    .bind(gctx.auth_user.id)
    .bind(course_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    Ok(teaches.is_some())
}

fn parse_id(raw: &ID, what: &str) -> Result<i64, async_graphql::Error> {
    raw.0
        .parse::<i64>()
        .map_err(|_| async_graphql::Error::new(format!("invalid {what}: {}", raw.0)))
}

// ---------------------------------------------------------------------------
// Payload / input types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchCError {
    pub attribute: Option<String>,
    pub message: String,
}

fn validation_errors(attribute: Option<&str>, message: &str) -> Vec<BatchCError> {
    vec![BatchCError {
        attribute: attribute.map(str::to_string),
        message: message.to_string(),
    }]
}

// NOTE: the `UpdateDiscussionTopicPayload` object lives in
// `root_mutations.rs` — it is the only one in the schema (Canvas'
// `UpdateDiscussionTopicPayload` carries the full `Discussion` object and a
// `ValidationError` list).

#[derive(Debug, Clone, SimpleObject)]
pub struct UpdateDiscussionEntryPayload {
    pub discussion_entry: Option<UpdatedDiscussionEntry>,
    pub errors: Vec<BatchCError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct UpdatedDiscussionEntry {
    pub id: ID,
    pub discussion_topic_id: ID,
    pub message: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateDiscussionEntryInput {
    pub discussion_entry_id: ID,
    pub message: String,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteDiscussionEntryInput {
    pub discussion_entry_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteLearningOutcomeInput {
    pub outcome_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteLearningOutcomeGroupInput {
    pub outcome_group_id: ID,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct RubricAssessmentPayload {
    pub rubric_assessment: Option<RubricAssessment>,
    pub errors: Vec<BatchCError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct RubricAssessment {
    pub id: ID,
    pub rubric_id: ID,
    pub submission_id: ID,
    pub assessor_id: ID,
    pub score: Option<f64>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, InputObject)]
pub struct SaveRubricAssessmentInput {
    pub rubric_id: ID,
    pub submission_id: ID,
    /// Optional criterion/rating data Canvas sends; persisted verbatim
    /// on the 0023 JSONB column.
    pub data: Option<serde_json::Value>,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, InputObject)]
pub struct DeleteSubmissionCommentInput {
    pub submission_comment_id: ID,
}

#[derive(Debug, Clone, InputObject)]
pub struct UpdateNotificationPreferencesInput {
    /// The channel the preference belongs to; must be the caller's.
    pub communication_channel_id: ID,
    pub notification: String,
    /// `immediately` | `daily` | `weekly` | `never` (0045 CHECK).
    pub frequency: String,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct NotificationPreferencePayload {
    pub notification_policy: Option<BatchCNotificationPolicy>,
    pub errors: Vec<BatchCError>,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct BatchCNotificationPolicy {
    pub id: ID,
    pub communication_channel_id: ID,
    pub notification: String,
    pub frequency: String,
}

// ---------------------------------------------------------------------------
// Mutation container
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct BatchCMutations;

#[Object]
impl BatchCMutations {
    /// Canvas `updateDiscussionEntry`
    /// (`app/graphql/mutations/update_discussion_entry.rb`).
    async fn update_discussion_entry(
        &self,
        ctx: &Context<'_>,
        input: UpdateDiscussionEntryInput,
    ) -> Result<UpdateDiscussionEntryPayload, async_graphql::Error> {
        update_discussion_entry(ctx.data::<GraphQLContext>()?, input).await
    }

    /// Canvas `deleteDiscussionEntry`
    /// (`app/graphql/mutations/delete_discussion_entry.rb`).
    async fn delete_discussion_entry(
        &self,
        ctx: &Context<'_>,
        input: DeleteDiscussionEntryInput,
    ) -> Result<SuccessPayload, async_graphql::Error> {
        delete_discussion_entry(ctx.data::<GraphQLContext>()?, input).await
    }

    /// Canvas `deleteLearningOutcome`
    /// (`app/graphql/mutations/delete_learning_outcome.rb`).
    async fn delete_learning_outcome(
        &self,
        ctx: &Context<'_>,
        input: DeleteLearningOutcomeInput,
    ) -> Result<SuccessPayload, async_graphql::Error> {
        delete_learning_outcome(ctx.data::<GraphQLContext>()?, input).await
    }

    /// Canvas `deleteLearningOutcomeGroup`
    /// (`app/graphql/mutations/delete_learning_outcome_group.rb`).
    async fn delete_learning_outcome_group(
        &self,
        ctx: &Context<'_>,
        input: DeleteLearningOutcomeGroupInput,
    ) -> Result<SuccessPayload, async_graphql::Error> {
        delete_learning_outcome_group(ctx.data::<GraphQLContext>()?, input).await
    }

    /// Canvas `saveRubricAssessment`
    /// (`app/graphql/mutations/save_rubric_assessment.rb`).
    async fn save_rubric_assessment(
        &self,
        ctx: &Context<'_>,
        input: SaveRubricAssessmentInput,
    ) -> Result<RubricAssessmentPayload, async_graphql::Error> {
        save_rubric_assessment(ctx.data::<GraphQLContext>()?, input).await
    }

    /// Canvas `deleteSubmissionComment`
    /// (`app/graphql/mutations/delete_submission_comment.rb`).
    async fn delete_submission_comment(
        &self,
        ctx: &Context<'_>,
        input: DeleteSubmissionCommentInput,
    ) -> Result<SuccessPayload, async_graphql::Error> {
        delete_submission_comment(ctx.data::<GraphQLContext>()?, input).await
    }

    /// Canvas `updateNotificationPreferences`
    /// (`app/graphql/mutations/update_notification_preferences.rb`) —
    /// upserts the caller's per-notification policy on their channel.
    async fn update_notification_preferences(
        &self,
        ctx: &Context<'_>,
        input: UpdateNotificationPreferencesInput,
    ) -> Result<NotificationPreferencePayload, async_graphql::Error> {
        update_notification_preferences(ctx.data::<GraphQLContext>()?, input).await
    }
}

// ---------------------------------------------------------------------------
// Update/insert result rows (named instead of anonymous tuples — clippy
// type_complexity)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct EntryUpdateRow {
    message: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RubricAssessmentInsertRow {
    id: i64,
    rubric_id: i64,
    submission_id: i64,
    score: Option<f64>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct PreferenceUpsertRow {
    id: i64,
    notification: String,
    frequency: String,
}

// ---------------------------------------------------------------------------
// Implementations
// ---------------------------------------------------------------------------

async fn update_discussion_entry(
    gctx: &GraphQLContext,
    input: UpdateDiscussionEntryInput,
) -> Result<UpdateDiscussionEntryPayload, async_graphql::Error> {
    let entry_id = parse_id(&input.discussion_entry_id, "discussion entry")?;
    let entry: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT topic_id, user_id FROM discussion_entries \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(entry_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    let Some((topic_id, author_id)) = entry else {
        return Err(async_graphql::Error::new(format!(
            "discussion entry not found: {}",
            input.discussion_entry_id.0
        )));
    };
    // Canvas allows the author to edit their own entry.
    if author_id != Some(gctx.auth_user.id) {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if input.message.trim().is_empty() {
        return Ok(UpdateDiscussionEntryPayload {
            discussion_entry: None,
            errors: validation_errors(Some("message"), "message cannot be blank"),
        });
    }
    let row: Option<EntryUpdateRow> = sqlx::query_as(
        "UPDATE discussion_entries SET message = $2, updated_at = now() \
         WHERE id = $1 RETURNING message, updated_at",
    )
    .bind(entry_id)
    .bind(input.message.trim())
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new("discussion entry not found"));
    };
    Ok(UpdateDiscussionEntryPayload {
        discussion_entry: Some(UpdatedDiscussionEntry {
            id: input.discussion_entry_id.clone(),
            discussion_topic_id: ID(topic_id.to_string()),
            message: row.message,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn delete_discussion_entry(
    gctx: &GraphQLContext,
    input: DeleteDiscussionEntryInput,
) -> Result<SuccessPayload, async_graphql::Error> {
    let entry_id = parse_id(&input.discussion_entry_id, "discussion entry")?;
    let entry: Option<(i64, Option<i64>)> = sqlx::query_as(
        r#"SELECT e.topic_id, e.user_id FROM discussion_entries e
           WHERE e.id = $1 AND e.workflow_state <> 'deleted'"#,
    )
    .bind(entry_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    let Some((topic_id, author_id)) = entry else {
        return Err(async_graphql::Error::new(format!(
            "discussion entry not found: {}",
            input.discussion_entry_id.0
        )));
    };
    // Author or teacher/admin (Canvas moderation).
    let allowed = author_id == Some(gctx.auth_user.id) || {
        let course: Option<(i64,)> =
            sqlx::query_as("SELECT course_id FROM discussion_topics WHERE id = $1")
                .bind(topic_id)
                .fetch_optional(&gctx.pool)
                .await
                .map_err(crate::db_err)?;
        match course {
            Some((cid,)) => is_teacher_or_admin(gctx, cid).await?,
            None => false,
        }
    };
    if !allowed {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    let res = sqlx::query(
        "UPDATE discussion_entries SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(entry_id)
    .execute(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    if res.rows_affected() == 0 {
        return Err(async_graphql::Error::new("discussion entry not found"));
    }
    Ok(SuccessPayload { success: true })
}

async fn delete_learning_outcome(
    gctx: &GraphQLContext,
    input: DeleteLearningOutcomeInput,
) -> Result<SuccessPayload, async_graphql::Error> {
    let oid = parse_id(&input.outcome_id, "outcome")?;
    let linked: Option<(i64,)> = sqlx::query_as(
        r#"SELECT g.course_id FROM learning_outcome_links l
           JOIN learning_outcome_groups g ON g.id = l.outcome_group_id
           WHERE l.outcome_id = $1 AND l.workflow_state = 'active'
             AND g.workflow_state <> 'deleted'
           LIMIT 1"#,
    )
    .bind(oid)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    let Some((course_id,)) = linked else {
        return Err(async_graphql::Error::new(format!(
            "outcome not found: {}",
            input.outcome_id.0
        )));
    };
    if !is_teacher_or_admin(gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    let res = sqlx::query(
        "UPDATE learning_outcomes SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(oid)
    .execute(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    if res.rows_affected() == 0 {
        return Err(async_graphql::Error::new("outcome not found"));
    }
    Ok(SuccessPayload { success: true })
}

async fn delete_learning_outcome_group(
    gctx: &GraphQLContext,
    input: DeleteLearningOutcomeGroupInput,
) -> Result<SuccessPayload, async_graphql::Error> {
    let gid = parse_id(&input.outcome_group_id, "outcome group")?;
    let group: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT course_id FROM learning_outcome_groups \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(gid)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    let Some((Some(course_id),)) = group else {
        return Err(async_graphql::Error::new(format!(
            "outcome group not found: {}",
            input.outcome_group_id.0
        )));
    };
    if !is_teacher_or_admin(gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    let res = sqlx::query(
        "UPDATE learning_outcome_groups SET workflow_state = 'deleted', updated_at = now() \
         WHERE id = $1",
    )
    .bind(gid)
    .execute(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    if res.rows_affected() == 0 {
        return Err(async_graphql::Error::new("outcome group not found"));
    }
    Ok(SuccessPayload { success: true })
}

async fn save_rubric_assessment(
    gctx: &GraphQLContext,
    input: SaveRubricAssessmentInput,
) -> Result<RubricAssessmentPayload, async_graphql::Error> {
    let rubric_id = parse_id(&input.rubric_id, "rubric")?;
    let submission_id = parse_id(&input.submission_id, "submission")?;
    // The submission's course gates the write (Canvas grading role).
    let sub_course: Option<(i64,)> = sqlx::query_as(
        "SELECT a.course_id FROM submissions s \
         JOIN assignments a ON a.id = s.assignment_id WHERE s.id = $1",
    )
    .bind(submission_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    let Some((course_id,)) = sub_course else {
        return Err(async_graphql::Error::new(format!(
            "submission not found: {}",
            input.submission_id.0
        )));
    };
    // The rubric must belong to the same course (Canvas validates the
    // pairing; here at least existence + course match).
    let rubric_ok: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM rubrics WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(rubric_id)
    .bind(course_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    if rubric_ok.is_none() {
        return Err(async_graphql::Error::new(format!(
            "rubric not found: {}",
            input.rubric_id.0
        )));
    }
    if !is_teacher_or_admin(gctx, course_id).await? {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    let row: Option<RubricAssessmentInsertRow> = sqlx::query_as(
        r#"
        INSERT INTO rubric_assessments
            (rubric_id, submission_id, assessor_id, score, data, workflow_state)
        VALUES ($1, $2, $3, $4, $5, 'active')
        ON CONFLICT DO NOTHING
        RETURNING id, rubric_id, submission_id, score::float8, updated_at
        "#,
    )
    .bind(rubric_id)
    .bind(submission_id)
    .bind(gctx.auth_user.id)
    .bind(input.score)
    .bind(&input.data)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    // No unique constraint pairs (rubric, submission) yet — multiple
    // assessments per pair are allowed by the 0023 schema, mirroring
    // Canvas's provisional/parallel assessments. A NULL row means the
    // insert failed outright.
    let Some(row) = row else {
        return Err(async_graphql::Error::new("rubric assessment save failed"));
    };
    Ok(RubricAssessmentPayload {
        rubric_assessment: Some(RubricAssessment {
            id: ID(row.id.to_string()),
            rubric_id: ID(row.rubric_id.to_string()),
            submission_id: ID(row.submission_id.to_string()),
            assessor_id: ID(gctx.auth_user.id.to_string()),
            score: row.score,
            updated_at: row.updated_at,
        }),
        errors: Vec::new(),
    })
}

async fn delete_submission_comment(
    gctx: &GraphQLContext,
    input: DeleteSubmissionCommentInput,
) -> Result<SuccessPayload, async_graphql::Error> {
    let cid = parse_id(&input.submission_comment_id, "submission comment")?;
    let comment: Option<(i64,)> = sqlx::query_as(
        "SELECT author_id FROM submission_comments \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(cid)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    let Some((author_id,)) = comment else {
        return Err(async_graphql::Error::new(format!(
            "submission comment not found: {}",
            input.submission_comment_id.0
        )));
    };
    // Canvas: the author removes their own comment.
    if author_id != gctx.auth_user.id {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    let res = sqlx::query(
        "UPDATE submission_comments SET workflow_state = 'deleted', updated_at = now() \
         WHERE id = $1",
    )
    .bind(cid)
    .execute(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    if res.rows_affected() == 0 {
        return Err(async_graphql::Error::new("submission comment not found"));
    }
    Ok(SuccessPayload { success: true })
}

async fn update_notification_preferences(
    gctx: &GraphQLContext,
    input: UpdateNotificationPreferencesInput,
) -> Result<NotificationPreferencePayload, async_graphql::Error> {
    let channel_id = parse_id(&input.communication_channel_id, "communication channel")?;
    let channel: Option<(i64,)> = sqlx::query_as(
        "SELECT user_id FROM communication_channels \
         WHERE id = $1 AND workflow_state <> 'retired'",
    )
    .bind(channel_id)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    let Some((owner,)) = channel else {
        return Err(async_graphql::Error::new(format!(
            "communication channel not found: {}",
            input.communication_channel_id.0
        )));
    };
    // Only the owner tunes their own preferences.
    if owner != gctx.auth_user.id {
        return Err(async_graphql::Error::new("insufficient permission"));
    }
    if !matches!(
        input.frequency.as_str(),
        "immediately" | "daily" | "weekly" | "never"
    ) {
        return Ok(NotificationPreferencePayload {
            notification_policy: None,
            errors: validation_errors(
                Some("frequency"),
                "frequency must be immediately, daily, weekly, or never",
            ),
        });
    }
    if input.notification.trim().is_empty() {
        return Ok(NotificationPreferencePayload {
            notification_policy: None,
            errors: validation_errors(Some("notification"), "notification cannot be blank"),
        });
    }
    // Upsert on the 0045 UNIQUE (communication_channel_id, notification).
    let row: Option<PreferenceUpsertRow> = sqlx::query_as(
        r#"
        INSERT INTO notification_preferences
            (user_id, communication_channel_id, notification, frequency)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (communication_channel_id, notification)
        DO UPDATE SET frequency = EXCLUDED.frequency, updated_at = now()
        RETURNING id, notification, frequency
        "#,
    )
    .bind(gctx.auth_user.id)
    .bind(channel_id)
    .bind(input.notification.trim())
    .bind(&input.frequency)
    .fetch_optional(&gctx.pool)
    .await
    .map_err(crate::db_err)?;
    let Some(row) = row else {
        return Err(async_graphql::Error::new(
            "notification preference save failed",
        ));
    };
    Ok(NotificationPreferencePayload {
        notification_policy: Some(BatchCNotificationPolicy {
            id: ID(row.id.to_string()),
            communication_channel_id: input.communication_channel_id.clone(),
            notification: row.notification,
            frequency: row.frequency,
        }),
        errors: Vec::new(),
    })
}
