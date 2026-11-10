//! star-outcomes — Canvas learning outcomes, groups, links, and results.
//!
//! Implements the following Canvas API v1 endpoints:
//! - `POST /courses/{courseId}/outcome_groups` — create a group (teacher)
//! - `GET  /courses/{courseId}/outcome_groups` — list groups (enrolled)
//! - `GET  /outcome_groups/{id}` — show a group (enrolled)
//! - `PUT  /outcome_groups/{id}` — update a group (teacher)
//! - `DELETE /outcome_groups/{id}` — soft-delete a group (teacher)
//! - `POST /courses/{courseId}/outcomes` — create an outcome (teacher)
//! - `GET  /courses/{courseId}/outcomes` — list outcomes, excludes deleted (enrolled)
//! - `POST /outcome_groups/{id}/outcomes` — link (or create and link) an outcome (teacher)
//! - `GET  /outcome_groups/{id}/outcomes` — list OutcomeLinks of a group (enrolled)
//! - `POST /courses/{courseId}/outcome_groups/{groupId}/outcomes` — course-scoped link alias (teacher)
//! - `GET  /courses/{courseId}/outcome_groups/{groupId}/outcomes` — course-scoped outcomes (enrolled)
//! - `GET  /courses/{courseId}/root_outcome_group` — redirect to the course root group (enrolled)
//! - `GET  /courses/{courseId}/outcome_group_links` — all course OutcomeLinks (enrolled)
//! - `GET  /accounts/{accountId}/outcome_groups` — account groups (account admin)
//! - `GET  /courses/{courseId}/outcome_groups/{id}` — course-scoped group show (enrolled)
//! - `POST /outcomes/{outcomeId}/results` — record a result (teacher)
//! - `GET  /outcomes/{outcomeId}/results` — list results (teacher or the user themself)

use std::collections::{HashMap, HashSet};

use axum::{
    extract::{Path, Query, State},
    http::{header::LOCATION, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A learning outcome. Title is required; description and display_name are optional.
/// Workflow_state is always active|archived|retired|deleted.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LearningOutcome {
    pub id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating an outcome.
#[derive(Debug, Deserialize)]
pub struct CreateOutcome {
    pub title: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
}

/// A learning outcome group, optionally tied to a course.
///
/// Matches the fields of the Canvas `OutcomeGroup` payload; `url`,
/// `subgroups_url`, `outcomes_url`, `import_url`, and `can_edit` are added by
/// the `group_json` helper.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LearningOutcomeGroup {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub course_id: Option<i64>,
    pub context_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_outcome_group_id: Option<i64>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_guid: Option<String>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating an outcome group.
#[derive(Debug, Default, Deserialize)]
pub struct CreateOutcomeGroup {
    pub title: String,
    pub description: Option<String>,
    pub vendor_guid: Option<String>,
}

/// Request body for `PUT /outcome_groups/{id}` (Canvas `outcome_groups_incoming_params`).
///
/// Omitted fields are left unchanged, matching Canvas. An explicit JSON `null`
/// is treated like an omitted field for the nullable text columns.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateOutcomeGroup {
    pub title: Option<String>,
    pub description: Option<String>,
    pub vendor_guid: Option<String>,
    pub parent_outcome_group_id: Option<i64>,
}

/// A link between an outcome and a group.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LearningOutcomeLink {
    pub id: i64,
    pub outcome_id: i64,
    pub outcome_group_id: i64,
    pub context_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<i64>,
    pub workflow_state: String,
}

/// Request body for linking an outcome to a group.
#[derive(Debug, Deserialize)]
pub struct LinkOutcomeToGroup {
    pub outcome_id: i64,
}

/// Request body for `POST /outcome_groups/{id}/outcomes`.
///
/// Canvas links an existing outcome when `outcome_id` is present. Without it,
/// a new outcome is created from `title` (plus optional `display_name` and
/// `description`) and linked into the group. `move_from` re-links an outcome
/// from another group when linking an existing outcome.
#[derive(Debug, Default, Deserialize)]
pub struct LinkOutcome {
    pub outcome_id: Option<i64>,
    pub move_from: Option<i64>,
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
}

/// A result recording a user's score on an outcome.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LearningOutcomeResult {
    pub id: i64,
    pub outcome_id: i64,
    pub user_id: i64,
    pub score: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for recording a result.
#[derive(Debug, Deserialize)]
pub struct CreateOutcomeResult {
    pub user_id: i64,
    pub score: f64,
}

/// Query params for filtering outcome results by user.
#[derive(Debug, Deserialize)]
pub struct OutcomeResultsQuery {
    pub user_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

async fn check_enrolled_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND workflow_state = 'active' LIMIT 1")
            .bind(user_id)
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

async fn check_teacher_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' AND workflow_state = 'active' LIMIT 1",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Load an active outcome group or return 404 (Canvas scopes indexes to active groups).
async fn fetch_outcome_group(
    pool: &PgPool,
    group_id: i64,
) -> Result<LearningOutcomeGroup, ApiError> {
    let row: Option<LearningOutcomeGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, context_type, context_id, parent_outcome_group_id,
               title, description, vendor_guid, workflow_state, created_at, updated_at
        FROM outcome_groups
        WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

/// Canvas `read_outcomes`: enrolled in the owning course, admin, or admin for
/// global (account) groups.
async fn check_group_read_access(
    pool: &PgPool,
    user_id: i64,
    group: &LearningOutcomeGroup,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    match group.course_id {
        Some(course_id) => check_enrolled_or_admin(pool, user_id, course_id, auth).await,
        None => Ok(is_admin(auth)),
    }
}

/// Canvas `manage_outcomes`: teacher in the owning course, admin, or admin for
/// global (account) groups.
async fn check_group_manage_access(
    pool: &PgPool,
    user_id: i64,
    group: &LearningOutcomeGroup,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    match group.course_id {
        Some(course_id) => check_teacher_or_admin(pool, user_id, course_id, auth).await,
        None => Ok(is_admin(auth)),
    }
}

async fn check_teacher_or_admin_for_group(
    pool: &PgPool,
    user_id: i64,
    group_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    let group = fetch_outcome_group(pool, group_id).await?;
    check_group_manage_access(pool, user_id, &group, auth).await
}

/// Depth cap for account-tree walks (same guard as star-accounts).
const MAX_ACCOUNT_TREE_DEPTH: i64 = 10;

/// Is the user an admin of this specific account?
async fn is_account_admin(pool: &PgPool, user_id: i64, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `read_outcomes` / `manage_outcomes` on an account: global admin, or
/// an admin of the account itself or any ancestor account.
async fn can_access_account(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let mut current = Some(account_id);
    let mut depth = 0;
    while let Some(id) = current {
        if depth >= MAX_ACCOUNT_TREE_DEPTH {
            return Ok(false);
        }
        depth += 1;
        if is_account_admin(pool, user_id, id).await? {
            return Ok(true);
        }
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        current = parent.and_then(|(parent_id,)| parent_id);
    }

    Ok(false)
}

/// Canvas `context.root_outcome_group`: the context's active root group,
/// created on first access (titled after the context, like Canvas).
///
/// The upsert is backed by the partial unique index from migration 0059, so
/// concurrent requests converge on a single row; a soft-deleted root is
/// revived rather than duplicated.
async fn ensure_root_outcome_group(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    title: &str,
) -> Result<LearningOutcomeGroup, ApiError> {
    let course_id = (context_type == "Course").then_some(context_id);
    let row: LearningOutcomeGroup = sqlx::query_as(
        r#"
        INSERT INTO outcome_groups
            (course_id, context_type, context_id, title, root_group)
        VALUES ($1, $2, $3, $4, TRUE)
        ON CONFLICT (context_type, context_id) WHERE root_group
        DO UPDATE SET workflow_state = 'active', updated_at = now()
        RETURNING id, course_id, context_type, context_id, parent_outcome_group_id,
                  title, description, vendor_guid, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(context_type)
    .bind(context_id)
    .bind(title)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Canvas `outcome_group_json`: the stored group plus the opaque URLs and
/// `can_edit` flag that the API contract exposes.
fn group_json(group: &LearningOutcomeGroup, can_edit: bool) -> serde_json::Value {
    // Canvas routes the group under its owning context: courses, accounts, or
    // the global namespace for global (context-less) groups.
    let base = match (
        group.course_id,
        group.context_type.as_str(),
        group.context_id,
    ) {
        (Some(course_id), _, _) => {
            format!("/api/v1/courses/{course_id}/outcome_groups/{}", group.id)
        }
        (None, "Account", Some(account_id)) => {
            format!("/api/v1/accounts/{account_id}/outcome_groups/{}", group.id)
        }
        _ => format!("/api/v1/outcome_groups/{}", group.id),
    };
    let url = base.clone();
    let subgroups_url = format!("{base}/subgroups");
    let outcomes_url = format!("{base}/outcomes");
    let import_url = format!("{base}/import");

    serde_json::json!({
        "id": group.id,
        "course_id": group.course_id,
        "context_type": group.context_type,
        "context_id": group.context_id,
        "parent_outcome_group_id": group.parent_outcome_group_id,
        "title": group.title,
        "description": group.description,
        "vendor_guid": group.vendor_guid,
        "workflow_state": group.workflow_state,
        "url": url,
        "subgroups_url": subgroups_url,
        "outcomes_url": outcomes_url,
        "import_url": import_url,
        "can_edit": can_edit,
        "created_at": group.created_at,
        "updated_at": group.updated_at,
    })
}

/// Canvas `outcome_link_json`: the link plus its abbreviated group and outcome.
fn link_json(
    link: &LearningOutcomeLink,
    group: &serde_json::Value,
    outcome: &LearningOutcome,
    can_unlink: bool,
    assessed: bool,
) -> serde_json::Value {
    // Canvas builds the link URL from the owning context, so account links
    // resolve under /accounts and global links under /outcome_groups.
    let url = match (link.context_type.as_str(), link.context_id) {
        ("Course", Some(context_id)) => format!(
            "/api/v1/courses/{context_id}/outcome_groups/{}/outcomes/{}",
            link.outcome_group_id, link.outcome_id
        ),
        ("Account", Some(context_id)) => format!(
            "/api/v1/accounts/{context_id}/outcome_groups/{}/outcomes/{}",
            link.outcome_group_id, link.outcome_id
        ),
        _ => format!(
            "/api/v1/outcome_groups/{}/outcomes/{}",
            link.outcome_group_id, link.outcome_id
        ),
    };

    serde_json::json!({
        "id": link.id,
        "outcome_id": link.outcome_id,
        "outcome_group_id": link.outcome_group_id,
        "context_type": link.context_type,
        "context_id": link.context_id,
        "workflow_state": link.workflow_state,
        "url": url,
        "outcome_group": group,
        "outcome": outcome,
        "can_unlink": can_unlink,
        "assessed": assessed,
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

// --- Outcome Groups ---

/// POST /courses/{courseId}/outcome_groups → 201
async fn create_outcome_group(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateOutcomeGroup>,
) -> Result<(StatusCode, Json<LearningOutcomeGroup>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }

    let row: LearningOutcomeGroup = sqlx::query_as(
        r#"
        INSERT INTO outcome_groups
            (course_id, context_type, context_id, title, description, vendor_guid)
        VALUES ($1, 'Course', $1, $2, $3, $4)
        RETURNING id, course_id, context_type, context_id, parent_outcome_group_id,
                  title, description, vendor_guid, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.vendor_guid)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/outcome_groups (enrolled)
async fn list_outcome_groups(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<LearningOutcomeGroup>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<LearningOutcomeGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, context_type, context_id, parent_outcome_group_id,
               title, description, vendor_guid, workflow_state, created_at, updated_at
        FROM outcome_groups
        WHERE course_id = $1 AND workflow_state = 'active'
        ORDER BY created_at ASC, id ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /accounts/{accountId}/outcome_groups (account admin)
///
/// Canvas `index` for account contexts: every active Account group owned by
/// the account. `can_edit` is true for the account admins allowed to manage
/// them.
async fn list_account_outcome_groups(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // Canvas resolves the context first, so an unknown account is 404 even
    // for admins.
    let account_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;
    if !account_exists {
        return Err(ApiError::NotFound);
    }

    let can_manage = can_access_account(&state.db, auth.id, account_id, &auth).await?;
    if !can_manage {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<LearningOutcomeGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, context_type, context_id, parent_outcome_group_id,
               title, description, vendor_guid, workflow_state, created_at, updated_at
        FROM outcome_groups
        WHERE context_type = 'Account'
          AND context_id = $1
          AND workflow_state = 'active'
        ORDER BY id ASC
        "#,
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.iter()
            .map(|group| group_json(group, can_manage))
            .collect(),
    ))
}

/// GET /outcome_groups/{id} (enrolled)
async fn show_outcome_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let group = fetch_outcome_group(&state.db, group_id).await?;
    if !check_group_read_access(&state.db, auth.id, &group, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let can_edit = check_group_manage_access(&state.db, auth.id, &group, &auth).await?;
    Ok(Json(group_json(&group, can_edit)))
}

/// GET /courses/{courseId}/outcome_groups/{id} (enrolled)
///
/// Canvas scopes the lookup to the context (`context_outcome_groups.find`), so
/// a group owned by another course is 404, not 403.
async fn show_course_outcome_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let group = fetch_outcome_group(&state.db, group_id).await?;
    if group.course_id != Some(course_id) {
        return Err(ApiError::NotFound);
    }

    let can_edit = check_group_manage_access(&state.db, auth.id, &group, &auth).await?;
    Ok(Json(group_json(&group, can_edit)))
}

/// GET /courses/{courseId}/root_outcome_group (enrolled)
///
/// Canvas `OutcomeGroupsApiController#redirect`: 302 to the root group's
/// canonical URL. The group JSON is included in the body so clients that do
/// not follow redirects can still consume the response.
async fn root_outcome_group(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let name: Option<String> = sqlx::query_scalar("SELECT name FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;
    let name = name.ok_or(ApiError::NotFound)?;

    let group = ensure_root_outcome_group(&state.db, "Course", course_id, &name).await?;
    let can_edit = check_group_manage_access(&state.db, auth.id, &group, &auth).await?;
    let location = format!("/api/v1/courses/{course_id}/outcome_groups/{}", group.id);

    Ok((
        StatusCode::FOUND,
        [(LOCATION, location)],
        Json(group_json(&group, can_edit)),
    )
        .into_response())
}

/// PUT /outcome_groups/{id} (teacher)
///
/// Canvas semantics: omitted fields are left unchanged; a new parent must
/// belong to the same context and must not create a cycle.
async fn update_outcome_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<UpdateOutcomeGroup>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let group = fetch_outcome_group(&state.db, group_id).await?;
    if !check_group_manage_access(&state.db, auth.id, &group, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let payload = payload.map(|Json(fields)| fields).unwrap_or_default();

    if payload
        .title
        .as_deref()
        .is_some_and(|title| title.trim().is_empty())
    {
        return Err(ApiError::Validation("title cannot be blank".into()));
    }

    if let Some(parent_id) = payload.parent_outcome_group_id {
        if parent_id == group_id {
            return Err(ApiError::BadRequest(
                "cannot move an outcome group under itself".into(),
            ));
        }

        let parent: Option<(i64, Option<i64>, String)> = sqlx::query_as(
            "SELECT id, course_id, workflow_state FROM outcome_groups WHERE id = $1",
        )
        .bind(parent_id)
        .fetch_optional(&state.db)
        .await?;
        let (_, parent_course_id, parent_state) = parent.ok_or(ApiError::NotFound)?;
        if parent_state == "deleted" {
            return Err(ApiError::NotFound);
        }
        if parent_course_id != group.course_id {
            return Err(ApiError::BadRequest(
                "parent outcome group must belong to the same context".into(),
            ));
        }

        let in_subtree: bool = sqlx::query_scalar(
            r#"
            WITH RECURSIVE subtree AS (
                SELECT id FROM outcome_groups WHERE id = $1
                UNION ALL
                SELECT child.id
                FROM outcome_groups child
                JOIN subtree ON child.parent_outcome_group_id = subtree.id
            )
            SELECT EXISTS (SELECT 1 FROM subtree WHERE id = $2)
            "#,
        )
        .bind(group_id)
        .bind(parent_id)
        .fetch_one(&state.db)
        .await?;
        if in_subtree {
            return Err(ApiError::BadRequest(
                "cannot move an outcome group under its own descendant".into(),
            ));
        }
    }

    let row: Option<LearningOutcomeGroup> = sqlx::query_as(
        r#"
        UPDATE outcome_groups
        SET title = COALESCE($2, title),
            description = COALESCE($3, description),
            vendor_guid = COALESCE($4, vendor_guid),
            parent_outcome_group_id = COALESCE($5, parent_outcome_group_id),
            updated_at = now()
        WHERE id = $1 AND workflow_state <> 'deleted'
        RETURNING id, course_id, context_type, context_id, parent_outcome_group_id,
                  title, description, vendor_guid, workflow_state, created_at, updated_at
        "#,
    )
    .bind(group_id)
    .bind(payload.title.as_deref())
    .bind(payload.description.as_deref())
    .bind(payload.vendor_guid.as_deref())
    .bind(payload.parent_outcome_group_id)
    .fetch_optional(&state.db)
    .await?;

    let row = row.ok_or(ApiError::NotFound)?;
    Ok(Json(group_json(&row, true)))
}

/// DELETE /outcome_groups/{id} (teacher) — soft-delete
async fn delete_outcome_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !check_teacher_or_admin_for_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        "UPDATE outcome_groups SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(group_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

// --- Outcomes ---

/// POST /courses/{courseId}/outcomes → 201
async fn create_outcome(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateOutcome>,
) -> Result<(StatusCode, Json<LearningOutcome>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }

    let row: LearningOutcome = sqlx::query_as(
        r#"
        INSERT INTO learning_outcomes (title, description, display_name)
        VALUES ($1, $2, $3)
        RETURNING id, title, description, display_name, workflow_state, created_at, updated_at
        "#,
    )
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.display_name)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/outcomes (enrolled) — excludes deleted
async fn list_outcomes(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<LearningOutcome>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Outcomes are linked to courses through outcome_groups that have that course_id.
    // We return outcomes that are linked via active links to groups belonging to this course.
    let rows: Vec<LearningOutcome> = sqlx::query_as(
        r#"
        SELECT DISTINCT lo.id, lo.title, lo.description, lo.display_name, lo.workflow_state,
               lo.created_at, lo.updated_at
        FROM learning_outcomes lo
        JOIN outcome_links ol ON ol.outcome_id = lo.id
        JOIN outcome_groups og ON og.id = ol.outcome_group_id
        WHERE og.course_id = $1
          AND og.workflow_state = 'active'
          AND ol.workflow_state = 'active'
          AND lo.workflow_state = 'active'
        ORDER BY lo.created_at ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

// --- Outcome Links ---

/// POST /outcome_groups/{id}/outcomes → 201
///
/// Mirrors Canvas `outcome_groups_api#link`: link an existing outcome
/// (`outcome_id`, optionally relinking from `move_from`) or create and link a
/// new outcome (`title` + optional `display_name` / `description`).
async fn link_outcome_by_group_id(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<LinkOutcome>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let group = fetch_outcome_group(&state.db, group_id).await?;
    if !check_group_manage_access(&state.db, auth.id, &group, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let outcome_id = match payload.outcome_id {
        Some(outcome_id) => {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM learning_outcomes WHERE id = $1 AND workflow_state = 'active')",
            )
            .bind(outcome_id)
            .fetch_one(&state.db)
            .await?;
            if !exists {
                return Err(ApiError::NotFound);
            }
            outcome_id
        }
        None => {
            let title = payload
                .title
                .as_deref()
                .ok_or_else(|| ApiError::Validation("outcome_id or title is required".into()))?;
            if title.trim().is_empty() {
                return Err(ApiError::Validation("title cannot be blank".into()));
            }
            let outcome: LearningOutcome = sqlx::query_as(
                r#"
                INSERT INTO learning_outcomes (title, description, display_name)
                VALUES ($1, $2, $3)
                RETURNING id, title, description, display_name, workflow_state, created_at, updated_at
                "#,
            )
            .bind(title)
            .bind(&payload.description)
            .bind(&payload.display_name)
            .fetch_one(&state.db)
            .await?;
            outcome.id
        }
    };

    if let Some(from_group_id) = payload.move_from {
        // Canvas moves the existing link instead of creating a second one.
        let moved: Option<LearningOutcomeLink> = sqlx::query_as(
            r#"
            UPDATE outcome_links
            SET outcome_group_id = $1,
                context_type = $2,
                context_id = $3,
                workflow_state = 'active',
                updated_at = now()
            WHERE outcome_id = $4 AND outcome_group_id = $5
            RETURNING id, outcome_id, outcome_group_id, context_type, context_id, workflow_state
            "#,
        )
        .bind(group_id)
        .bind(&group.context_type)
        .bind(group.context_id)
        .bind(outcome_id)
        .bind(from_group_id)
        .fetch_optional(&state.db)
        .await?;
        let link = moved.ok_or_else(|| ApiError::BadRequest("outcome link not found".into()))?;
        let json = link_response(&state.db, &group, &link, true).await?;
        return Ok((StatusCode::OK, Json(json)));
    }

    let link: LearningOutcomeLink = sqlx::query_as(
        r#"
        INSERT INTO outcome_links (outcome_id, outcome_group_id, context_type, context_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (outcome_id, outcome_group_id)
        DO UPDATE SET workflow_state = 'active', updated_at = now()
        RETURNING id, outcome_id, outcome_group_id, context_type, context_id, workflow_state
        "#,
    )
    .bind(outcome_id)
    .bind(group_id)
    .bind(&group.context_type)
    .bind(group.context_id)
    .fetch_one(&state.db)
    .await?;

    let json = link_response(&state.db, &group, &link, true).await?;
    Ok((StatusCode::CREATED, Json(json)))
}

/// Build the Canvas `OutcomeLink` payload for a stored link.
async fn link_response(
    pool: &PgPool,
    group: &LearningOutcomeGroup,
    link: &LearningOutcomeLink,
    can_unlink: bool,
) -> Result<serde_json::Value, ApiError> {
    let outcome: Option<LearningOutcome> = sqlx::query_as(
        r#"
        SELECT id, title, description, display_name, workflow_state, created_at, updated_at
        FROM learning_outcomes
        WHERE id = $1
        "#,
    )
    .bind(link.outcome_id)
    .fetch_optional(pool)
    .await?;
    let outcome = outcome.ok_or(ApiError::NotFound)?;
    let assessed: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM learning_outcome_results WHERE outcome_id = $1)",
    )
    .bind(link.outcome_id)
    .fetch_one(pool)
    .await?;

    Ok(link_json(
        link,
        &group_json(group, can_unlink),
        &outcome,
        can_unlink,
        assessed,
    ))
}

/// GET /outcome_groups/{id}/outcomes (enrolled) — Canvas `OutcomeLink` list.
async fn list_group_outcome_links(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let group = fetch_outcome_group(&state.db, group_id).await?;
    if !check_group_read_access(&state.db, auth.id, &group, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let can_manage = check_group_manage_access(&state.db, auth.id, &group, &auth).await?;

    let links: Vec<LearningOutcomeLink> = sqlx::query_as(
        r#"
        SELECT id, outcome_id, outcome_group_id, context_type, context_id, workflow_state
        FROM outcome_links
        WHERE outcome_group_id = $1 AND workflow_state = 'active'
        ORDER BY id ASC
        "#,
    )
    .bind(group_id)
    .fetch_all(&state.db)
    .await?;

    let outcomes: Vec<LearningOutcome> = sqlx::query_as(
        r#"
        SELECT lo.id, lo.title, lo.description, lo.display_name, lo.workflow_state,
               lo.created_at, lo.updated_at
        FROM learning_outcomes lo
        JOIN outcome_links ol ON ol.outcome_id = lo.id
        WHERE ol.outcome_group_id = $1
          AND ol.workflow_state = 'active'
          AND lo.workflow_state = 'active'
        ORDER BY lo.title ASC, lo.id ASC
        "#,
    )
    .bind(group_id)
    .fetch_all(&state.db)
    .await?;

    let outcome_ids: Vec<i64> = outcomes.iter().map(|outcome| outcome.id).collect();
    let assessed_ids: Vec<i64> = if outcome_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar(
            "SELECT DISTINCT outcome_id FROM learning_outcome_results WHERE outcome_id = ANY($1)",
        )
        .bind(outcome_ids)
        .fetch_all(&state.db)
        .await?
    };
    let assessed: HashSet<i64> = assessed_ids.into_iter().collect();

    let group_payload = group_json(&group, can_manage);
    let rows: Vec<serde_json::Value> = outcomes
        .iter()
        .filter_map(|outcome| {
            let link = links
                .iter()
                .find(|link| link.outcome_id == outcome.id && link.workflow_state == "active")?;
            Some(link_json(
                link,
                &group_payload,
                outcome,
                can_manage,
                assessed.contains(&outcome.id),
            ))
        })
        .collect();

    Ok(Json(rows))
}

/// GET /courses/{courseId}/outcome_group_links (enrolled)
///
/// Canvas `link_index`: every active OutcomeLink in the course, ordered by id.
/// The `outcome_style` / `outcome_group_style` query parameters are accepted
/// but ignored — this route always returns the same detail level as
/// `GET /outcome_groups/{id}/outcomes`.
async fn list_course_outcome_group_links(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let links: Vec<LearningOutcomeLink> = sqlx::query_as(
        r#"
        SELECT ol.id, ol.outcome_id, ol.outcome_group_id,
               ol.context_type, ol.context_id, ol.workflow_state
        FROM outcome_links ol
        JOIN outcome_groups og ON og.id = ol.outcome_group_id
        WHERE og.course_id = $1
          AND og.workflow_state = 'active'
          AND ol.workflow_state = 'active'
        ORDER BY ol.id ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    if links.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let group_ids: Vec<i64> = links.iter().map(|link| link.outcome_group_id).collect();
    let groups: Vec<LearningOutcomeGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, context_type, context_id, parent_outcome_group_id,
               title, description, vendor_guid, workflow_state, created_at, updated_at
        FROM outcome_groups
        WHERE id = ANY($1) AND workflow_state = 'active'
        "#,
    )
    .bind(&group_ids)
    .fetch_all(&state.db)
    .await?;
    let groups: HashMap<i64, LearningOutcomeGroup> =
        groups.into_iter().map(|group| (group.id, group)).collect();

    let outcome_ids: Vec<i64> = links.iter().map(|link| link.outcome_id).collect();
    let outcomes: Vec<LearningOutcome> = sqlx::query_as(
        r#"
        SELECT id, title, description, display_name, workflow_state, created_at, updated_at
        FROM learning_outcomes
        WHERE id = ANY($1) AND workflow_state = 'active'
        "#,
    )
    .bind(&outcome_ids)
    .fetch_all(&state.db)
    .await?;
    let outcomes: HashMap<i64, LearningOutcome> = outcomes
        .into_iter()
        .map(|outcome| (outcome.id, outcome))
        .collect();

    let assessed_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT outcome_id FROM learning_outcome_results WHERE outcome_id = ANY($1)",
    )
    .bind(&outcome_ids)
    .fetch_all(&state.db)
    .await?;
    let assessed: HashSet<i64> = assessed_ids.into_iter().collect();

    let can_manage = check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let rows: Vec<serde_json::Value> = links
        .iter()
        .filter_map(|link| {
            let group = groups.get(&link.outcome_group_id)?;
            let outcome = outcomes.get(&link.outcome_id)?;
            Some(link_json(
                link,
                &group_json(group, can_manage),
                outcome,
                can_manage,
                assessed.contains(&outcome.id),
            ))
        })
        .collect();

    Ok(Json(rows))
}

/// POST /courses/{courseId}/outcome_groups/{groupId}/outcomes {outcome_id} → 201
async fn link_outcome_to_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<LinkOutcomeToGroup>,
) -> Result<(StatusCode, Json<LearningOutcomeLink>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify group belongs to this course
    let group_row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM outcome_groups WHERE id = $1 AND course_id = $2 AND workflow_state = 'active'",
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    if group_row.is_none() {
        return Err(ApiError::NotFound);
    }

    let row: LearningOutcomeLink = sqlx::query_as(
        r#"
        INSERT INTO outcome_links (outcome_id, outcome_group_id, context_type, context_id)
        VALUES ($1, $2, 'Course', $3)
        ON CONFLICT (outcome_id, outcome_group_id)
        DO UPDATE SET workflow_state = 'active', updated_at = now()
        RETURNING id, outcome_id, outcome_group_id, context_type, context_id, workflow_state
        "#,
    )
    .bind(payload.outcome_id)
    .bind(group_id)
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/outcome_groups/{groupId}/outcomes (enrolled)
async fn list_group_outcomes(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<LearningOutcome>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify group belongs to this course
    let group_row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM outcome_groups WHERE id = $1 AND course_id = $2 AND workflow_state = 'active'",
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    if group_row.is_none() {
        return Err(ApiError::NotFound);
    }

    let rows: Vec<LearningOutcome> = sqlx::query_as(
        r#"
        SELECT lo.id, lo.title, lo.description, lo.display_name, lo.workflow_state,
               lo.created_at, lo.updated_at
        FROM learning_outcomes lo
        JOIN outcome_links ol ON ol.outcome_id = lo.id
        WHERE ol.outcome_group_id = $1
          AND ol.workflow_state = 'active'
          AND lo.workflow_state = 'active'
        ORDER BY lo.created_at ASC
        "#,
    )
    .bind(group_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

// --- Results ---

/// POST /outcomes/{outcomeId}/results {user_id, score} → 201
async fn create_outcome_result(
    State(state): State<AppState>,
    Path(outcome_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateOutcomeResult>,
) -> Result<(StatusCode, Json<LearningOutcomeResult>), ApiError> {
    // Only teachers or admins can record results
    // We need to verify the user is a teacher in the course that owns this outcome
    let outcome_row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT og.course_id
        FROM learning_outcomes lo
        JOIN outcome_links ol ON ol.outcome_id = lo.id
        JOIN outcome_groups og ON og.id = ol.outcome_group_id
        WHERE lo.id = $1 AND lo.workflow_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(outcome_id)
    .fetch_optional(&state.db)
    .await?;

    let course_id = match outcome_row {
        Some((cid,)) => cid,
        None => return Err(ApiError::NotFound),
    };

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: LearningOutcomeResult = sqlx::query_as(
        r#"
        INSERT INTO learning_outcome_results (outcome_id, user_id, score)
        VALUES ($1, $2, $3)
        RETURNING id, outcome_id, user_id, score, created_at, updated_at
        "#,
    )
    .bind(outcome_id)
    .bind(payload.user_id)
    .bind(payload.score)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /outcomes/{outcomeId}/results?user_id= (teacher or the user themself)
async fn list_outcome_results(
    State(state): State<AppState>,
    Path(outcome_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<OutcomeResultsQuery>,
) -> Result<Json<Vec<LearningOutcomeResult>>, ApiError> {
    // Verify outcome exists
    let outcome_row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT og.course_id
        FROM learning_outcomes lo
        JOIN outcome_links ol ON ol.outcome_id = lo.id
        JOIN outcome_groups og ON og.id = ol.outcome_group_id
        WHERE lo.id = $1 AND lo.workflow_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(outcome_id)
    .fetch_optional(&state.db)
    .await?;

    let (course_id,) = outcome_row.ok_or(ApiError::NotFound)?;

    // If filtering by user_id, check permissions
    if let Some(uid) = params.user_id {
        if uid != auth.id && !is_admin(&auth) {
            // Check if the requesting user is a teacher in the course
            if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
                return Err(ApiError::Forbidden);
            }
        }
        let rows: Vec<LearningOutcomeResult> = sqlx::query_as(
            r#"
            SELECT id, outcome_id, user_id, score, created_at, updated_at
            FROM learning_outcome_results
            WHERE outcome_id = $1 AND user_id = $2
            ORDER BY created_at ASC
            "#,
        )
        .bind(outcome_id)
        .bind(uid)
        .fetch_all(&state.db)
        .await?;
        Ok(Json(rows))
    } else {
        // No user_id filter: require teacher/admin
        if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
            return Err(ApiError::Forbidden);
        }
        let rows: Vec<LearningOutcomeResult> = sqlx::query_as(
            r#"
            SELECT id, outcome_id, user_id, score, created_at, updated_at
            FROM learning_outcome_results
            WHERE outcome_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(outcome_id)
        .fetch_all(&state.db)
        .await?;
        Ok(Json(rows))
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // outcome groups
        .route(
            "/courses/{courseId}/outcome_groups",
            get(list_outcome_groups).post(create_outcome_group),
        )
        .route(
            "/outcome_groups/{id}",
            get(show_outcome_group)
                .put(update_outcome_group)
                .delete(delete_outcome_group),
        )
        // context-scoped group navigation (Canvas outcome_groups_api)
        .route(
            "/courses/{courseId}/root_outcome_group",
            get(root_outcome_group),
        )
        .route(
            "/courses/{courseId}/outcome_group_links",
            get(list_course_outcome_group_links),
        )
        .route(
            "/courses/{courseId}/outcome_groups/{id}",
            get(show_course_outcome_group),
        )
        .route(
            "/accounts/{accountId}/outcome_groups",
            get(list_account_outcome_groups),
        )
        // outcomes
        .route(
            "/courses/{courseId}/outcomes",
            get(list_outcomes).post(create_outcome),
        )
        // linking outcomes to groups
        .route(
            "/outcome_groups/{id}/outcomes",
            get(list_group_outcome_links).post(link_outcome_by_group_id),
        )
        .route(
            "/courses/{courseId}/outcome_groups/{groupId}/outcomes",
            get(list_group_outcomes).post(link_outcome_to_group),
        )
        // results
        .route(
            "/outcomes/{outcomeId}/results",
            get(list_outcome_results).post(create_outcome_result),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Path;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::Json;
    use chrono::Utc;
    use sqlx::PgPool;
    use star_auth::AuthUser;

    #[tokio::test]
    async fn only_active_enrollments_grant_course_outcome_permissions() {
        let Some(database_url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("connect DATABASE_URL");
        star_db::MIGRATOR.run(&pool).await.expect("run migrations");

        let prefix = format!(
            "outcomes-authz-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis()
        );
        let course_id: i64 = sqlx::query_scalar(
            "INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id",
        )
        .bind(format!("{prefix} course"))
        .bind(&prefix)
        .fetch_one(&pool)
        .await
        .expect("seed course");

        async fn seed_user(pool: &sqlx::PgPool, prefix: &str, label: &str) -> i64 {
            sqlx::query_scalar(
                "INSERT INTO users (name, email, password_hash) VALUES ($1, $2, $3) RETURNING id",
            )
            .bind(label)
            .bind(format!("{prefix}-{label}@test.local"))
            .bind("unused-test-hash")
            .fetch_one(pool)
            .await
            .expect("seed user")
        }

        async fn enroll(
            pool: &sqlx::PgPool,
            user_id: i64,
            course_id: i64,
            role: &str,
            state: &str,
        ) {
            sqlx::query(
                "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, $3, $4)",
            )
            .bind(user_id)
            .bind(course_id)
            .bind(role)
            .bind(state)
            .execute(pool)
            .await
            .expect("seed enrollment");
        }

        let active_student = seed_user(&pool, &prefix, "active-student").await;
        let inactive_student = seed_user(&pool, &prefix, "inactive-student").await;
        let active_teacher = seed_user(&pool, &prefix, "active-teacher").await;
        let inactive_teacher = seed_user(&pool, &prefix, "inactive-teacher").await;
        enroll(&pool, active_student, course_id, "student", "active").await;
        enroll(&pool, inactive_student, course_id, "student", "inactive").await;
        enroll(&pool, active_teacher, course_id, "teacher", "active").await;
        enroll(&pool, inactive_teacher, course_id, "teacher", "inactive").await;

        let auth = |id: i64, role: &str| AuthUser {
            id,
            role: role.to_string(),
        };
        assert!(check_enrolled_or_admin(
            &pool,
            active_student,
            course_id,
            &auth(active_student, "student")
        )
        .await
        .expect("active student check"));
        assert!(!check_enrolled_or_admin(
            &pool,
            inactive_student,
            course_id,
            &auth(inactive_student, "student")
        )
        .await
        .expect("inactive student check"));
        assert!(check_teacher_or_admin(
            &pool,
            active_teacher,
            course_id,
            &auth(active_teacher, "teacher")
        )
        .await
        .expect("active teacher check"));
        assert!(!check_teacher_or_admin(
            &pool,
            inactive_teacher,
            course_id,
            &auth(inactive_teacher, "teacher")
        )
        .await
        .expect("inactive teacher check"));

        sqlx::query("DELETE FROM courses WHERE id = $1")
            .bind(course_id)
            .execute(&pool)
            .await
            .expect("cleanup course");
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind([
                active_student,
                inactive_student,
                active_teacher,
                inactive_teacher,
            ])
            .execute(&pool)
            .await
            .expect("cleanup users");
    }

    // -----------------------------------------------------------------------
    // Phase 2: outcome group / outcome link parity
    // -----------------------------------------------------------------------

    #[test]
    fn group_json_exposes_canvas_urls_and_can_edit() {
        let created_at = Utc::now();
        let group = LearningOutcomeGroup {
            id: 7,
            course_id: Some(3),
            context_type: "Course".to_string(),
            context_id: Some(3),
            parent_outcome_group_id: None,
            title: "Group".to_string(),
            description: Some("desc".to_string()),
            vendor_guid: None,
            workflow_state: "active".to_string(),
            created_at,
            updated_at: created_at,
        };

        let json = group_json(&group, true);
        assert_eq!(json["id"], 7);
        assert_eq!(json["url"], "/api/v1/courses/3/outcome_groups/7");
        assert_eq!(
            json["subgroups_url"],
            "/api/v1/courses/3/outcome_groups/7/subgroups"
        );
        assert_eq!(
            json["outcomes_url"],
            "/api/v1/courses/3/outcome_groups/7/outcomes"
        );
        assert_eq!(
            json["import_url"],
            "/api/v1/courses/3/outcome_groups/7/import"
        );
        assert_eq!(json["description"], "desc");
        assert_eq!(json["can_edit"], true);

        let global = LearningOutcomeGroup {
            course_id: None,
            context_type: "Account".to_string(),
            context_id: None,
            ..group
        };
        let json = group_json(&global, false);
        assert_eq!(json["url"], "/api/v1/outcome_groups/7");
        assert_eq!(json["can_edit"], false);
    }

    async fn test_pool() -> Option<PgPool> {
        let Some(database_url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("skipping: DATABASE_URL not set");
            return None;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
            .expect("connect DATABASE_URL");
        star_db::MIGRATOR.run(&pool).await.expect("run migrations");
        Some(pool)
    }

    /// Seed a course plus an active teacher enrollment.
    async fn seed_course_with_teacher(pool: &PgPool, label: &str) -> (i64, i64) {
        let prefix = format!(
            "outcomes-{label}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_millis()
        );
        let course_id: i64 = sqlx::query_scalar(
            "INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id",
        )
        .bind(format!("{prefix} course"))
        .bind(&prefix)
        .fetch_one(pool)
        .await
        .expect("seed course");
        let teacher_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (name, email, password_hash) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind("Outcomes Teacher")
        .bind(format!("{prefix}@test.local"))
        .bind("unused-test-hash")
        .fetch_one(pool)
        .await
        .expect("seed teacher");
        sqlx::query(
            "INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'teacher')",
        )
        .bind(teacher_id)
        .bind(course_id)
        .execute(pool)
        .await
        .expect("seed enrollment");
        (course_id, teacher_id)
    }

    async fn teardown(pool: &PgPool, course_id: i64, user_ids: &[i64]) {
        sqlx::query("DELETE FROM courses WHERE id = $1")
            .bind(course_id)
            .execute(pool)
            .await
            .expect("cleanup course");
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(user_ids.to_vec())
            .execute(pool)
            .await
            .expect("cleanup users");
    }

    fn test_state(pool: &PgPool) -> AppState {
        AppState {
            db: pool.clone(),
            jwt_secret: "test-secret".to_string(),
        }
    }

    async fn create_group(
        state: &AppState,
        course_id: i64,
        auth: &AuthUser,
        title: &str,
    ) -> LearningOutcomeGroup {
        let (status, Json(group)) = create_outcome_group(
            State(state.clone()),
            Path(course_id),
            auth.clone(),
            Json(CreateOutcomeGroup {
                title: title.to_string(),
                description: None,
                vendor_guid: None,
            }),
        )
        .await
        .expect("create outcome group");
        assert_eq!(status, StatusCode::CREATED);
        group
    }

    #[tokio::test]
    async fn group_show_and_update_share_course_storage() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (course_id, teacher_id) = seed_course_with_teacher(&pool, "show-update").await;
        let auth = AuthUser {
            id: teacher_id,
            role: "teacher".to_string(),
        };
        let state = test_state(&pool);

        let group = create_group(&state, course_id, &auth, "Created Group").await;

        // GET /outcome_groups/{id} sees groups created by the course route.
        let Json(shown) = show_outcome_group(State(state.clone()), Path(group.id), auth.clone())
            .await
            .expect("show outcome group");
        assert_eq!(shown["id"], group.id);
        assert_eq!(shown["title"], "Created Group");
        assert_eq!(shown["context_type"], "Course");
        assert_eq!(
            shown["url"],
            format!("/api/v1/courses/{course_id}/outcome_groups/{}", group.id)
        );
        assert_eq!(shown["can_edit"], true);

        // PUT /outcome_groups/{id} updates only the provided fields.
        let Json(updated) = update_outcome_group(
            State(state.clone()),
            Path(group.id),
            auth.clone(),
            Some(Json(UpdateOutcomeGroup {
                title: Some("Renamed Group".to_string()),
                description: Some("updated description".to_string()),
                vendor_guid: Some("customid9000".to_string()),
                parent_outcome_group_id: None,
            })),
        )
        .await
        .expect("update outcome group");
        assert_eq!(updated["title"], "Renamed Group");
        assert_eq!(updated["description"], "updated description");
        assert_eq!(updated["vendor_guid"], "customid9000");
        assert_eq!(updated["workflow_state"], "active");

        teardown(&pool, course_id, &[teacher_id]).await;
    }

    #[tokio::test]
    async fn group_scoped_link_routes_expose_canvas_outcome_links() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (course_id, teacher_id) = seed_course_with_teacher(&pool, "links").await;
        let auth = AuthUser {
            id: teacher_id,
            role: "teacher".to_string(),
        };
        let state = test_state(&pool);

        let group = create_group(&state, course_id, &auth, "Link Group").await;

        // POST creates the outcome and the link in one call (Canvas `link`).
        let (status, Json(link)) = link_outcome_by_group_id(
            State(state.clone()),
            Path(group.id),
            auth.clone(),
            Json(LinkOutcome {
                outcome_id: None,
                move_from: None,
                title: Some("Critical Thinking".to_string()),
                display_name: Some("CT".to_string()),
                description: Some("Think critically".to_string()),
            }),
        )
        .await
        .expect("create and link outcome");
        assert_eq!(status, StatusCode::CREATED);
        let outcome_id = link["outcome_id"].as_i64().expect("outcome id");
        assert_eq!(link["outcome"]["title"], "Critical Thinking");
        assert_eq!(link["outcome_group"]["id"], group.id);
        assert_eq!(link["can_unlink"], true);
        assert_eq!(link["assessed"], false);

        // GET returns the same link in Canvas OutcomeLink shape.
        let Json(list) =
            list_group_outcome_links(State(state.clone()), Path(group.id), auth.clone())
                .await
                .expect("list group outcome links");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["id"], link["id"]);
        assert_eq!(list[0]["outcome"]["id"], outcome_id);
        assert_eq!(
            list[0]["url"],
            format!(
                "/api/v1/courses/{course_id}/outcome_groups/{}/outcomes/{outcome_id}",
                group.id
            )
        );

        // The pre-existing course-scoped listing sees the same link.
        let Json(outcomes) = list_group_outcomes(
            State(state.clone()),
            Path((course_id, group.id)),
            auth.clone(),
        )
        .await
        .expect("legacy group outcomes");
        assert!(outcomes.iter().any(|outcome| outcome.id == outcome_id));

        sqlx::query("DELETE FROM learning_outcomes WHERE id = $1")
            .bind(outcome_id)
            .execute(&pool)
            .await
            .expect("cleanup outcome");
        teardown(&pool, course_id, &[teacher_id]).await;
    }

    #[tokio::test]
    async fn group_update_rejects_parent_outside_context_or_cycle() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (course_id, teacher_id) = seed_course_with_teacher(&pool, "cycle").await;
        let (other_course_id, other_teacher_id) =
            seed_course_with_teacher(&pool, "cycle-other").await;
        let auth = AuthUser {
            id: teacher_id,
            role: "teacher".to_string(),
        };
        let state = test_state(&pool);

        let parent = create_group(&state, course_id, &auth, "Parent Group").await;
        let child: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO outcome_groups
                (course_id, context_type, context_id, parent_outcome_group_id, title)
            VALUES ($1, 'Course', $1, $2, 'Child Group')
            RETURNING id
            "#,
        )
        .bind(course_id)
        .bind(parent.id)
        .fetch_one(&pool)
        .await
        .expect("seed child group");

        // A group cannot become a child of its own descendant.
        let err = update_outcome_group(
            State(state.clone()),
            Path(parent.id),
            auth.clone(),
            Some(Json(UpdateOutcomeGroup {
                parent_outcome_group_id: Some(child),
                ..UpdateOutcomeGroup::default()
            })),
        )
        .await
        .expect_err("cycle must be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)));

        // The new parent must live in the same context (course).
        let other_auth = AuthUser {
            id: other_teacher_id,
            role: "teacher".to_string(),
        };
        let other_group =
            create_group(&state, other_course_id, &other_auth, "Other Course Group").await;
        let err = update_outcome_group(
            State(state.clone()),
            Path(parent.id),
            auth.clone(),
            Some(Json(UpdateOutcomeGroup {
                parent_outcome_group_id: Some(other_group.id),
                ..UpdateOutcomeGroup::default()
            })),
        )
        .await
        .expect_err("cross-context parent must be rejected");
        assert!(matches!(err, ApiError::BadRequest(_)));

        teardown(&pool, course_id, &[teacher_id]).await;
        teardown(&pool, other_course_id, &[other_teacher_id]).await;
    }

    // -----------------------------------------------------------------------
    // Phase 3: context-scoped outcome navigation
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn root_outcome_group_is_created_once_and_course_scoped() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (course_id, teacher_id) = seed_course_with_teacher(&pool, "root").await;
        let auth = AuthUser {
            id: teacher_id,
            role: "teacher".to_string(),
        };
        let state = test_state(&pool);

        // The first request lazily creates the root group and redirects to it.
        let response = root_outcome_group(State(state.clone()), Path(course_id), auth.clone())
            .await
            .expect("root outcome group");
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .expect("location header")
            .to_string();

        let (root_id, root_course_id, root_title): (i64, Option<i64>, String) = sqlx::query_as(
            "SELECT id, course_id, title FROM outcome_groups
             WHERE course_id = $1 AND root_group AND workflow_state = 'active'",
        )
        .bind(course_id)
        .fetch_one(&pool)
        .await
        .expect("root group row");
        assert_eq!(root_course_id, Some(course_id));
        assert_eq!(
            location,
            format!("/api/v1/courses/{course_id}/outcome_groups/{root_id}")
        );

        // Canvas titles the root group after its context.
        let course_name: String = sqlx::query_scalar("SELECT name FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_one(&pool)
            .await
            .expect("course name");
        assert_eq!(root_title, course_name);

        // A second request reuses the row instead of creating another root.
        let again = root_outcome_group(State(state.clone()), Path(course_id), auth.clone())
            .await
            .expect("root outcome group again");
        assert_eq!(again.status(), StatusCode::FOUND);
        assert_eq!(
            again
                .headers()
                .get(axum::http::header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some(location.as_str())
        );
        let root_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM outcome_groups WHERE course_id = $1 AND root_group",
        )
        .bind(course_id)
        .fetch_one(&pool)
        .await
        .expect("root group count");
        assert_eq!(root_count, 1);

        // The course-scoped show route resolves the redirect target.
        let Json(shown) =
            show_course_outcome_group(State(state.clone()), Path((course_id, root_id)), auth)
                .await
                .expect("show root outcome group");
        assert_eq!(shown["id"], root_id);
        assert_eq!(shown["url"], location);
        assert_eq!(shown["can_edit"], true);

        teardown(&pool, course_id, &[teacher_id]).await;
    }

    #[tokio::test]
    async fn course_outcome_group_show_rejects_other_courses() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (course_id, teacher_id) = seed_course_with_teacher(&pool, "scope").await;
        let (other_course_id, other_teacher_id) =
            seed_course_with_teacher(&pool, "scope-other").await;
        let auth = AuthUser {
            id: teacher_id,
            role: "teacher".to_string(),
        };
        let state = test_state(&pool);

        let group = create_group(&state, course_id, &auth, "Scoped Group").await;
        let Json(shown) =
            show_course_outcome_group(State(state.clone()), Path((course_id, group.id)), auth)
                .await
                .expect("show own group");
        assert_eq!(shown["id"], group.id);

        // Admin bypasses enrollment, so the 404 proves the context scoping.
        let admin = AuthUser {
            id: teacher_id,
            role: "admin".to_string(),
        };
        let err = show_course_outcome_group(
            State(state.clone()),
            Path((other_course_id, group.id)),
            admin,
        )
        .await
        .expect_err("group of another course must 404");
        assert!(matches!(err, ApiError::NotFound));

        teardown(&pool, course_id, &[teacher_id]).await;
        teardown(&pool, other_course_id, &[other_teacher_id]).await;
    }

    #[tokio::test]
    async fn course_outcome_group_links_lists_only_course_links() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let (course_id, teacher_id) = seed_course_with_teacher(&pool, "link-index").await;
        let (other_course_id, other_teacher_id) =
            seed_course_with_teacher(&pool, "link-index-other").await;
        let auth = AuthUser {
            id: teacher_id,
            role: "teacher".to_string(),
        };
        let other_auth = AuthUser {
            id: other_teacher_id,
            role: "teacher".to_string(),
        };
        let state = test_state(&pool);

        let group = create_group(&state, course_id, &auth, "Course Links").await;
        let other_group =
            create_group(&state, other_course_id, &other_auth, "Other Course Links").await;

        let (_, Json(link)) = link_outcome_by_group_id(
            State(state.clone()),
            Path(group.id),
            auth.clone(),
            Json(LinkOutcome {
                outcome_id: None,
                move_from: None,
                title: Some("Link Index Outcome".to_string()),
                display_name: None,
                description: None,
            }),
        )
        .await
        .expect("link course outcome");
        let outcome_id = link["outcome_id"].as_i64().expect("outcome id");

        let (_, Json(other_link)) = link_outcome_by_group_id(
            State(state.clone()),
            Path(other_group.id),
            other_auth,
            Json(LinkOutcome {
                outcome_id: None,
                move_from: None,
                title: Some("Other Course Outcome".to_string()),
                display_name: None,
                description: None,
            }),
        )
        .await
        .expect("link other course outcome");
        let other_outcome_id = other_link["outcome_id"].as_i64().expect("other outcome id");

        // Only the links of the requested course are returned.
        let Json(rows) =
            list_course_outcome_group_links(State(state.clone()), Path(course_id), auth.clone())
                .await
                .expect("list course links");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], link["id"]);
        assert_eq!(rows[0]["outcome_id"], outcome_id);
        assert_eq!(rows[0]["outcome_group"]["id"], group.id);
        assert_eq!(
            rows[0]["url"],
            format!(
                "/api/v1/courses/{course_id}/outcome_groups/{}/outcomes/{outcome_id}",
                group.id
            )
        );
        assert_eq!(rows[0]["can_unlink"], true);
        assert_eq!(rows[0]["assessed"], false);

        // Assessing the outcome flips the Canvas `assessed` flag.
        sqlx::query(
            "INSERT INTO learning_outcome_results (outcome_id, user_id, score)
             VALUES ($1, $2, $3)",
        )
        .bind(outcome_id)
        .bind(teacher_id)
        .bind(4.0_f64)
        .execute(&pool)
        .await
        .expect("seed outcome result");
        let Json(rows) =
            list_course_outcome_group_links(State(state.clone()), Path(course_id), auth)
                .await
                .expect("list assessed course links");
        assert_eq!(rows[0]["assessed"], true);

        sqlx::query("DELETE FROM learning_outcome_results WHERE outcome_id = ANY($1)")
            .bind(vec![outcome_id, other_outcome_id])
            .execute(&pool)
            .await
            .expect("cleanup results");
        sqlx::query("DELETE FROM learning_outcomes WHERE id = ANY($1)")
            .bind(vec![outcome_id, other_outcome_id])
            .execute(&pool)
            .await
            .expect("cleanup outcomes");
        teardown(&pool, course_id, &[teacher_id]).await;
        teardown(&pool, other_course_id, &[other_teacher_id]).await;
    }

    #[tokio::test]
    async fn account_outcome_groups_require_account_admin() {
        let Some(pool) = test_pool().await else {
            return;
        };
        let prefix = format!(
            "outcomes-account-{}-{}",
            std::process::id(),
            Utc::now().timestamp_millis()
        );
        let account_id: i64 =
            sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
                .bind(format!("{prefix} account"))
                .fetch_one(&pool)
                .await
                .expect("seed account");
        let account_admin_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (name, email, password_hash) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind("Account Outcomes Admin")
        .bind(format!("{prefix}-admin@test.local"))
        .bind("unused-test-hash")
        .fetch_one(&pool)
        .await
        .expect("seed account admin");
        let outsider_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (name, email, password_hash) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind("Account Outcomes Outsider")
        .bind(format!("{prefix}-outsider@test.local"))
        .bind("unused-test-hash")
        .fetch_one(&pool)
        .await
        .expect("seed outsider");
        sqlx::query(
            "INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')",
        )
        .bind(account_id)
        .bind(account_admin_id)
        .execute(&pool)
        .await
        .expect("grant account admin");

        let active_group_id: i64 = sqlx::query_scalar(
            "INSERT INTO outcome_groups (course_id, context_type, context_id, title)
             VALUES (NULL, 'Account', $1, $2) RETURNING id",
        )
        .bind(account_id)
        .bind(format!("{prefix} active group"))
        .fetch_one(&pool)
        .await
        .expect("seed account group");
        let deleted_group_id: i64 = sqlx::query_scalar(
            "INSERT INTO outcome_groups (course_id, context_type, context_id, title, workflow_state)
             VALUES (NULL, 'Account', $1, $2, 'deleted') RETURNING id",
        )
        .bind(account_id)
        .bind(format!("{prefix} deleted group"))
        .fetch_one(&pool)
        .await
        .expect("seed deleted account group");

        let state = test_state(&pool);
        let account_admin = AuthUser {
            id: account_admin_id,
            role: "student".to_string(),
        };
        let Json(rows) =
            list_account_outcome_groups(State(state.clone()), Path(account_id), account_admin)
                .await
                .expect("account admin lists groups");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], active_group_id);
        assert_eq!(rows[0]["context_type"], "Account");
        assert_eq!(
            rows[0]["url"],
            format!("/api/v1/accounts/{account_id}/outcome_groups/{active_group_id}")
        );
        assert_eq!(rows[0]["can_edit"], true);

        // Non-admins may not read account outcomes.
        let outsider = AuthUser {
            id: outsider_id,
            role: "student".to_string(),
        };
        let err = list_account_outcome_groups(State(state.clone()), Path(account_id), outsider)
            .await
            .expect_err("non-admin must be forbidden");
        assert!(matches!(err, ApiError::Forbidden));

        // Global admins always may.
        let global_admin = AuthUser {
            id: outsider_id,
            role: "admin".to_string(),
        };
        let Json(rows) =
            list_account_outcome_groups(State(state.clone()), Path(account_id), global_admin)
                .await
                .expect("global admin lists groups");
        assert_eq!(rows.len(), 1);

        sqlx::query("DELETE FROM outcome_groups WHERE id = ANY($1)")
            .bind(vec![active_group_id, deleted_group_id])
            .execute(&pool)
            .await
            .expect("cleanup account groups");
        sqlx::query("DELETE FROM accounts WHERE id = $1")
            .bind(account_id)
            .execute(&pool)
            .await
            .expect("cleanup account");
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(vec![account_admin_id, outsider_id])
            .execute(&pool)
            .await
            .expect("cleanup users");
    }
}
