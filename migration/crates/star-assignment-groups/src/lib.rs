//! star-assignment-groups: assignment group management for star-lms.
//!
//! Implements Canvas API v1 Assignment Groups endpoint.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
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

/// AssignmentGroup record matching Canvas API v1 shape.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AssignmentGroup {
    pub id: i64,
    pub course_id: i64,
    pub name: String,
    pub position: i32,
    pub group_weight: f64,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request to create an assignment group.
#[derive(Debug, Deserialize)]
pub struct CreateAssignmentGroup {
    pub name: String,
    #[serde(default)]
    pub group_weight: Option<f64>,
    #[serde(default = "default_position")]
    pub position: Option<i32>,
}

fn default_position() -> Option<i32> {
    Some(1)
}

/// Request to update an assignment group.
#[derive(Debug, Deserialize)]
pub struct UpdateAssignmentGroup {
    pub name: Option<String>,
    pub position: Option<i32>,
    pub group_weight: Option<f64>,
}

/// Assignment record returned when listing assignments in a group.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GroupAssignment {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub points_possible: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1"#,
    )
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
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/assignment_groups — create (teacher)
async fn create_assignment_group(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateAssignmentGroup>,
) -> Result<(StatusCode, Json<AssignmentGroup>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.name.trim().is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }

    let row: AssignmentGroup = sqlx::query_as(
        r#"
        INSERT INTO assignment_groups (course_id, name, group_weight, position)
        VALUES ($1, $2, COALESCE($3, 0), COALESCE($4, 1))
        RETURNING id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.name)
    .bind(payload.group_weight)
    .bind(payload.position)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/assignment_groups — list (enrolled)
async fn list_assignment_groups(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<AssignmentGroup>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<AssignmentGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        FROM assignment_groups
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        ORDER BY position ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /courses/{courseId}/assignment_groups/{id} — get one (enrolled)
async fn get_assignment_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<AssignmentGroup>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<AssignmentGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        FROM assignment_groups
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    row.map(Json).ok_or(ApiError::NotFound)
}

/// PUT /courses/{courseId}/assignment_groups/{id} — update (teacher)
async fn update_assignment_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateAssignmentGroup>,
) -> Result<Json<AssignmentGroup>, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Fetch existing to merge fields
    let existing: Option<AssignmentGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        FROM assignment_groups
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    let existing = existing.ok_or(ApiError::NotFound)?;

    let name = payload.name.unwrap_or(existing.name);
    let position = payload.position.unwrap_or(existing.position);
    let group_weight = payload.group_weight.unwrap_or(existing.group_weight);

    let updated: AssignmentGroup = sqlx::query_as(
        r#"
        UPDATE assignment_groups
        SET name = $3, position = $4, group_weight = $5, updated_at = now()
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        RETURNING id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .bind(&name)
    .bind(position)
    .bind(group_weight)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /courses/{courseId}/assignment_groups/{id} — soft-delete (teacher)
async fn delete_assignment_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        r#"
        UPDATE assignment_groups
        SET workflow_state = 'deleted', updated_at = now()
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// GET /courses/{courseId}/assignment_groups/{groupId}/assignments — assignments in group (enrolled)
async fn list_group_assignments(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<GroupAssignment>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify group exists and is not deleted
    let group: Option<(i64,)> = sqlx::query_as(
        r#"SELECT id FROM assignment_groups WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'"#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    if group.is_none() {
        return Err(ApiError::NotFound);
    }

    let rows: Vec<GroupAssignment> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        FROM assignments
        WHERE assignment_group_id = $1 AND course_id = $2
        ORDER BY created_at ASC
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/assignment_groups", post(create_assignment_group))
        .route("/courses/{courseId}/assignment_groups", get(list_assignment_groups))
        .route("/courses/{courseId}/assignment_groups/{id}", get(get_assignment_group))
        .route("/courses/{courseId}/assignment_groups/{id}", put(update_assignment_group))
        .route("/courses/{courseId}/assignment_groups/{id}", delete(delete_assignment_group))
        .route("/courses/{courseId}/assignment_groups/{groupId}/assignments", get(list_group_assignments))
}
