//! star-groups — Canvas-style groups and group memberships for star-lms.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

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

/// Group record matching Canvas API v1 shape.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Group {
    pub id: i64,
    pub course_id: Option<i64>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub workflow_state: String,
    pub join_level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_category_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request to create a group.
#[derive(Debug, Deserialize)]
pub struct CreateGroup {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_join_level")]
    pub join_level: String,
    #[serde(default)]
    pub group_category_id: Option<i64>,
}

fn default_join_level() -> String {
    "invitation_only".to_string()
}

/// Request to update a group.
#[derive(Debug, Deserialize)]
pub struct UpdateGroup {
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub join_level: Option<String>,
}

/// Group membership record matching Canvas API v1 shape.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GroupMembership {
    pub id: i64,
    pub group_id: i64,
    pub user_id: i64,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Full membership with user info for responses.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MembershipWithUser {
    pub id: i64,
    pub group_id: i64,
    pub user_id: i64,
    pub user_name: String,
    pub user_email: String,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request to add a user to a group.
#[derive(Debug, Deserialize)]
pub struct CreateMembership {
    pub user_id: i64,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

fn is_teacher(auth: &AuthUser) -> bool {
    auth.role == "teacher" || auth.role == "admin"
}

/// Check if user is enrolled as teacher in a course (or is admin).
async fn check_teacher_or_admin_for_course(
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

/// Check if user is enrolled as teacher in a course (or is admin).
/// Returns false for non-admin if no teacher enrollment found.
async fn is_teacher_in_group(
    pool: &PgPool,
    user_id: i64,
    group_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    // Get the group's course_id first
    let group: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT course_id FROM groups WHERE id = $1",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?;

    let Some((course_id,)) = group else {
        return Ok(false);
    };

    if let Some(cid) = course_id {
        check_teacher_or_admin_for_course(pool, user_id, cid, auth).await
    } else {
        // Account-level group — only admin can manage
        Ok(false)
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // Course-scoped group routes
        .route("/courses/{courseId}/groups", post(create_group))
        .route("/courses/{courseId}/groups", get(list_groups))
        // Direct group routes
        .route("/groups/{id}", get(get_group))
        .route("/groups/{id}", put(update_group))
        .route("/groups/{id}", delete(delete_group))
        // Membership routes
        .route("/groups/{groupId}/memberships", post(create_membership))
        .route("/groups/{groupId}/memberships", get(list_memberships))
        .route("/groups/{groupId}/memberships/{membershipId}", delete(delete_membership))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/groups — create a group (teacher/admin only)
async fn create_group(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateGroup>,
) -> Result<(StatusCode, Json<Group>), ApiError> {
    if !is_teacher(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Verify course exists
    let course_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;

    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Check teacher or admin permission on the course
    if !check_teacher_or_admin_for_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.name.trim().is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }

    let join_level = if ["invitation_only", "parent_context_auto_join", "parent_context_request"]
        .contains(&payload.join_level.as_str())
    {
        payload.join_level.clone()
    } else {
        "invitation_only".to_string()
    };

    let group: Group = sqlx::query_as(
        r#"
        INSERT INTO groups (course_id, name, description, join_level, group_category_id)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, course_id, name, description, workflow_state, join_level,
                  group_category_id, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.name)
    .bind(&payload.description)
    .bind(&join_level)
    .bind(&payload.group_category_id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(group)))
}

/// GET /courses/{courseId}/groups — list groups for a course (enrolled users)
/// Excludes deleted groups.
async fn list_groups(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Group>>, ApiError> {
    // Verify course exists
    let course_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;

    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Non-admin must be enrolled in the course
    if !is_admin(&auth) {
        let enrolled: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1")
                .bind(auth.id)
                .bind(course_id)
                .fetch_optional(&state.db)
                .await?;

        if enrolled.is_none() {
            return Err(ApiError::Forbidden);
        }
    }

    let groups: Vec<Group> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, description, workflow_state, join_level,
               group_category_id, created_at, updated_at
        FROM groups
        WHERE course_id = $1 AND workflow_state != 'deleted'
        ORDER BY created_at DESC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(groups))
}

/// GET /groups/{id} — get a single group by ID.
async fn get_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Group>, ApiError> {
    let group: Option<Group> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, description, workflow_state, join_level,
               group_category_id, created_at, updated_at
        FROM groups WHERE id = $1
        "#,
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;

    let group = group.ok_or(ApiError::NotFound)?;

    // Check access: if the group belongs to a course, user must be enrolled or admin
    if let Some(course_id) = group.course_id {
        if !is_admin(&auth) {
            let enrolled: Option<(i64,)> =
                sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1")
                    .bind(auth.id)
                    .bind(course_id)
                    .fetch_optional(&state.db)
                    .await?;

            if enrolled.is_none() {
                return Err(ApiError::Forbidden);
            }
        }
    } else {
        // Account-level group — only admin can view
        if !is_admin(&auth) {
            return Err(ApiError::Forbidden);
        }
    }

    Ok(Json(group))
}

/// PUT /groups/{id} — update a group (teacher/admin only)
async fn update_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateGroup>,
) -> Result<Json<Group>, ApiError> {
    // Fetch existing group
    let existing: Option<Group> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, description, workflow_state, join_level,
               group_category_id, created_at, updated_at
        FROM groups WHERE id = $1
        "#,
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;

    let existing = existing.ok_or(ApiError::NotFound)?;

    if !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let name = payload.name.unwrap_or(existing.name);
    let description = payload.description.or(existing.description);
    let join_level = payload.join_level.unwrap_or(existing.join_level);

    // Validate join_level
    if !["invitation_only", "parent_context_auto_join", "parent_context_request"]
        .contains(&join_level.as_str())
    {
        return Err(ApiError::Validation(
            "join_level must be 'invitation_only', 'parent_context_auto_join', or 'parent_context_request'".into(),
        ));
    }

    let updated: Group = sqlx::query_as(
        r#"
        UPDATE groups
        SET name = $2, description = $3, join_level = $4, updated_at = now()
        WHERE id = $1
        RETURNING id, course_id, name, description, workflow_state, join_level,
                  group_category_id, created_at, updated_at
        "#,
    )
    .bind(group_id)
    .bind(&name)
    .bind(&description)
    .bind(&join_level)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /groups/{id} — soft-delete a group (teacher/admin only)
async fn delete_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Verify group exists
    let existing: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_optional(&state.db)
        .await?;

    if existing.is_none() {
        return Err(ApiError::NotFound);
    }

    if !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        "UPDATE groups SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(group_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /groups/{groupId}/memberships — add a user to a group (teacher/admin only)
async fn create_membership(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateMembership>,
) -> Result<(StatusCode, Json<MembershipWithUser>), ApiError> {
    // Verify group exists and is not deleted
    let group: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM groups WHERE id = $1 AND workflow_state != 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;

    if group.is_none() {
        return Err(ApiError::NotFound);
    }

    if !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify user exists
    let user_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(payload.user_id)
        .fetch_optional(&state.db)
        .await?;

    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Try insert; UNIQUE constraint triggers 23505 -> 409 Conflict
    let result = sqlx::query(
        r#"
        INSERT INTO group_memberships (group_id, user_id, workflow_state)
        VALUES ($1, $2, 'accepted')
        "#,
    )
    .bind(group_id)
    .bind(payload.user_id)
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err))
            if db_err.code().as_deref() == Some("23505") =>
        {
            return Err(ApiError::Conflict(format!(
                "user {} is already a member of group {}",
                payload.user_id, group_id
            )));
        }
        Err(e) => return Err(ApiError::Db(e.to_string())),
    }

    // Fetch the created membership with user info
    let membership: MembershipWithUser = sqlx::query_as(
        r#"
        SELECT gm.id, gm.group_id, gm.user_id, u.name as user_name, u.email as user_email,
               gm.workflow_state, gm.created_at, gm.updated_at
        FROM group_memberships gm
        JOIN users u ON u.id = gm.user_id
        WHERE gm.group_id = $1 AND gm.user_id = $2
        "#,
    )
    .bind(group_id)
    .bind(payload.user_id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(membership)))
}

/// GET /groups/{groupId}/memberships — list memberships (teacher/admin only)
/// Excludes deleted memberships.
async fn list_memberships(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<MembershipWithUser>>, ApiError> {
    // Verify group exists
    let group_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_optional(&state.db)
        .await?;

    if group_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    if !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let memberships: Vec<MembershipWithUser> = sqlx::query_as(
        r#"
        SELECT gm.id, gm.group_id, gm.user_id, u.name as user_name, u.email as user_email,
               gm.workflow_state, gm.created_at, gm.updated_at
        FROM group_memberships gm
        JOIN users u ON u.id = gm.user_id
        WHERE gm.group_id = $1 AND gm.workflow_state != 'deleted'
        ORDER BY gm.created_at DESC
        "#,
    )
    .bind(group_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(memberships))
}

/// DELETE /groups/{groupId}/memberships/{membershipId} — soft-delete a membership (teacher/admin only)
async fn delete_membership(
    State(state): State<AppState>,
    Path((group_id, membership_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Verify group exists
    let group_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_optional(&state.db)
        .await?;

    if group_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    if !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify membership exists and belongs to this group
    let membership_exists: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM group_memberships WHERE id = $1 AND group_id = $2",
    )
    .bind(membership_id)
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;

    if membership_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    let result = sqlx::query(
        "UPDATE group_memberships SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(membership_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}
