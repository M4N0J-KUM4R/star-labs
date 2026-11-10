//! star-groups — Canvas-style groups and group memberships for star-lms.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};
use std::collections::HashSet;

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

/// Canvas derives a new membership's initial state from the group's join level.
fn membership_initial_workflow_state(join_level: &str) -> &'static str {
    match join_level {
        "parent_context_auto_join" => "accepted",
        "parent_context_request" => "requested",
        _ => "invited",
    }
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

/// Group category row as stored in Postgres.
#[derive(Debug, Clone, sqlx::FromRow)]
struct GroupCategoryRow {
    id: i64,
    course_id: Option<i64>,
    account_id: Option<i64>,
    name: String,
    role: Option<String>,
    self_signup: Option<String>,
    self_signup_end_at: Option<DateTime<Utc>>,
    group_limit: Option<i32>,
    auto_leader: Option<String>,
    non_collaborative: bool,
    created_at: DateTime<Utc>,
}

/// Group category response matching the Canvas API v1 shape.
#[derive(Debug, Clone, Serialize)]
pub struct GroupCategory {
    pub id: i64,
    pub name: String,
    pub role: Option<String>,
    pub self_signup: Option<String>,
    pub self_signup_end_at: Option<DateTime<Utc>>,
    pub group_limit: Option<i32>,
    pub auto_leader: Option<String>,
    pub context_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub course_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<i64>,
    pub non_collaborative: bool,
    pub protected: bool,
    pub allows_multiple_memberships: bool,
    pub is_member: bool,
    pub created_at: DateTime<Utc>,
}

/// Request to create a group category.
#[derive(Debug, Deserialize)]
pub struct CreateGroupCategory {
    pub name: String,
    #[serde(default)]
    pub self_signup: Option<String>,
    #[serde(default)]
    pub self_signup_end_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub group_limit: Option<i32>,
    #[serde(default)]
    pub auto_leader: Option<String>,
    #[serde(default)]
    pub non_collaborative: bool,
    /// Canvas creates this many groups inside the new category (Course only).
    #[serde(default)]
    pub create_group_count: Option<i32>,
}

/// Request to update a group category. Absent fields are left unchanged.
#[derive(Debug, Deserialize)]
pub struct UpdateGroupCategory {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub self_signup: Option<String>,
    #[serde(default)]
    pub self_signup_end_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub group_limit: Option<i32>,
    #[serde(default)]
    pub auto_leader: Option<String>,
}

const GROUP_CATEGORY_COLUMNS: &str = "id, course_id, account_id, name, role, self_signup, \
     self_signup_end_at, group_limit, auto_leader, non_collaborative, created_at";

/// Canvas protects built-in categories with a role other than "imported".
fn is_protected_role(role: Option<&str>) -> bool {
    matches!(role, Some(r) if r != "imported")
}

/// Canvas "communities" and "student_organized" categories allow several memberships.
fn role_allows_multiple_memberships(role: Option<&str>) -> bool {
    matches!(role, Some("communities") | Some("student_organized"))
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn validate_group_category_fields(
    name: &str,
    self_signup: Option<&str>,
    auto_leader: Option<&str>,
    group_limit: Option<i32>,
) -> Result<(), ApiError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("Name is required".into()));
    }
    if name.chars().count() > 255 {
        return Err(ApiError::BadRequest("Enter a shorter category name".into()));
    }
    if let Some(value) = self_signup {
        if value != "enabled" && value != "restricted" {
            return Err(ApiError::BadRequest(
                "Self-signup needs to be one of the following values: null, 'enabled', 'restricted'"
                    .into(),
            ));
        }
    }
    if let Some(value) = auto_leader {
        if value != "first" && value != "random" {
            return Err(ApiError::BadRequest(
                "AutoLeader type needs to be one of the following values: null, 'first', 'random'"
                    .into(),
            ));
        }
    }
    if let Some(value) = group_limit {
        if value <= 1 {
            return Err(ApiError::BadRequest("Must be greater than 1".into()));
        }
    }
    Ok(())
}

fn group_category_json(row: GroupCategoryRow, is_member: bool) -> GroupCategory {
    let context_type = if row.course_id.is_some() {
        "Course"
    } else {
        "Account"
    };
    GroupCategory {
        id: row.id,
        name: row.name,
        protected: is_protected_role(row.role.as_deref()),
        allows_multiple_memberships: role_allows_multiple_memberships(row.role.as_deref()),
        role: row.role,
        self_signup: row.self_signup,
        self_signup_end_at: row.self_signup_end_at,
        group_limit: row.group_limit,
        auto_leader: row.auto_leader,
        context_type: context_type.to_string(),
        course_id: row.course_id,
        account_id: row.account_id,
        non_collaborative: row.non_collaborative,
        is_member,
        created_at: row.created_at,
    }
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
    let group: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT course_id FROM groups WHERE id = $1")
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

/// Check if the user may read a course's groups/categories (any enrollment).
async fn check_course_read_access(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1")
            .bind(user_id)
            .bind(course_id)
            .fetch_optional(pool)
            .await?;

    Ok(row.is_some())
}

/// Authorization for managing a group category: teacher on its course, or admin.
async fn can_manage_group_category(
    pool: &PgPool,
    user_id: i64,
    category: &GroupCategoryRow,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    match category.course_id {
        Some(course_id) => check_teacher_or_admin_for_course(pool, user_id, course_id, auth).await,
        // Account-level categories: only admin can manage.
        None => Ok(is_admin(auth)),
    }
}

/// Authorization for viewing a group category and its groups.
async fn can_read_group_category(
    pool: &PgPool,
    user_id: i64,
    category: &GroupCategoryRow,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    match category.course_id {
        Some(course_id) => check_course_read_access(pool, user_id, course_id, auth).await,
        None => Ok(is_admin(auth)),
    }
}

async fn fetch_group_category(
    pool: &PgPool,
    category_id: i64,
) -> Result<GroupCategoryRow, ApiError> {
    let sql = format!(
        "SELECT {GROUP_CATEGORY_COLUMNS} FROM group_categories \
         WHERE id = $1 AND workflow_state != 'deleted'"
    );
    let category: Option<GroupCategoryRow> = sqlx::query_as(&sql)
        .bind(category_id)
        .fetch_optional(pool)
        .await?;
    category.ok_or(ApiError::NotFound)
}

/// Canvas validates category names are unique per context (ignoring deleted ones).
async fn category_name_taken(
    pool: &PgPool,
    course_id: i64,
    name: &str,
    exclude_id: Option<i64>,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM group_categories
        WHERE course_id = $1 AND name = $2 AND workflow_state != 'deleted'
          AND ($3::BIGINT IS NULL OR id != $3)
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .bind(name)
    .bind(exclude_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas category "is_member" is true when the user belongs to an active group
/// inside the category.
async fn category_is_member(
    pool: &PgPool,
    category_id: i64,
    user_id: i64,
) -> Result<bool, ApiError> {
    let is_member: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM group_memberships gm
            JOIN groups g ON g.id = gm.group_id
            WHERE g.group_category_id = $1
              AND g.workflow_state != 'deleted'
              AND gm.user_id = $2
              AND gm.workflow_state != 'deleted'
        )
        "#,
    )
    .bind(category_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(is_member)
}

/// Create the `create_group_count` groups Canvas creates along with a category.
/// Canvas English singularizes the category name; this keeps the raw name.
async fn create_category_groups(
    pool: &PgPool,
    course_id: i64,
    category_id: i64,
    category_name: &str,
    count: i32,
) -> Result<(), ApiError> {
    for index in 1..=count {
        sqlx::query("INSERT INTO groups (course_id, name, group_category_id) VALUES ($1, $2, $3)")
            .bind(course_id)
            .bind(format!("{category_name} {index}"))
            .bind(category_id)
            .execute(pool)
            .await?;
    }
    Ok(())
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
        .route("/groups/{id}", patch(update_group))
        .route("/groups/{id}", delete(delete_group))
        // Membership routes
        .route("/groups/{groupId}/memberships", post(create_membership))
        .route("/groups/{groupId}/memberships", get(list_memberships))
        .route(
            "/groups/{groupId}/memberships/{membershipId}",
            delete(delete_membership),
        )
        // Group category routes
        .route(
            "/courses/{courseId}/group_categories",
            post(create_group_category),
        )
        .route(
            "/courses/{courseId}/group_categories",
            get(list_group_categories),
        )
        .route("/group_categories/{id}", get(get_group_category))
        .route("/group_categories/{id}", put(update_group_category))
        .route("/group_categories/{id}", delete(delete_group_category))
        .route("/group_categories/{id}/groups", get(list_category_groups))
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
    let course_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
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

    let join_level = if [
        "invitation_only",
        "parent_context_auto_join",
        "parent_context_request",
    ]
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
    .bind(payload.group_category_id)
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
    let course_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;

    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Non-admin must be enrolled in the course
    if !is_admin(&auth) {
        let enrolled: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1",
        )
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
            let enrolled: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1",
            )
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

/// PUT/PATCH /groups/{id} — update a group (teacher/admin only)
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
    if ![
        "invitation_only",
        "parent_context_auto_join",
        "parent_context_request",
    ]
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
    let existing: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
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
    let group: Option<(String,)> = sqlx::query_as(
        "SELECT join_level FROM groups WHERE id = $1 AND workflow_state != 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;

    let Some((join_level,)) = group else {
        return Err(ApiError::NotFound);
    };

    if !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify user exists
    let user_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
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
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(group_id)
    .bind(payload.user_id)
    .bind(membership_initial_workflow_state(&join_level))
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23505") => {
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
    let group_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
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
    let group_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
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
    let membership_exists: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM group_memberships WHERE id = $1 AND group_id = $2")
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

// ---------------------------------------------------------------------------
// Group category handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/group_categories — create a category (teacher/admin only)
async fn create_group_category(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateGroupCategory>,
) -> Result<Json<GroupCategory>, ApiError> {
    let course_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;
    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    if !check_teacher_or_admin_for_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let name = payload.name.trim().to_string();
    let self_signup = normalize_optional(payload.self_signup);
    let auto_leader = normalize_optional(payload.auto_leader);
    validate_group_category_fields(
        &name,
        self_signup.as_deref(),
        auto_leader.as_deref(),
        payload.group_limit,
    )?;

    if category_name_taken(&state.db, course_id, &name, None).await? {
        return Err(ApiError::BadRequest(format!("{name} is already in use.")));
    }

    // Canvas only honors self_signup_end_at when self signup is enabled.
    let self_signup_end_at = if self_signup.as_deref() == Some("enabled") {
        payload.self_signup_end_at
    } else {
        None
    };

    let sql = format!(
        r#"
        INSERT INTO group_categories
            (course_id, name, self_signup, self_signup_end_at, group_limit,
             auto_leader, non_collaborative)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING {GROUP_CATEGORY_COLUMNS}
        "#
    );
    let category: GroupCategoryRow = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(&name)
        .bind(&self_signup)
        .bind(self_signup_end_at)
        .bind(payload.group_limit)
        .bind(&auto_leader)
        .bind(payload.non_collaborative)
        .fetch_one(&state.db)
        .await?;

    // Canvas creates create_group_count groups inline when self signup is off.
    if self_signup.is_none() {
        if let Some(count) = payload.create_group_count {
            if count > 0 {
                create_category_groups(&state.db, course_id, category.id, &name, count).await?;
            }
        }
    }

    Ok(Json(group_category_json(category, false)))
}

/// GET /courses/{courseId}/group_categories — list categories (teacher/admin only)
/// Excludes deleted categories.
async fn list_group_categories(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<GroupCategory>>, ApiError> {
    let course_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;
    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    if !check_teacher_or_admin_for_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "SELECT {GROUP_CATEGORY_COLUMNS} FROM group_categories \
         WHERE course_id = $1 AND workflow_state != 'deleted' ORDER BY id"
    );
    let categories: Vec<GroupCategoryRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .fetch_all(&state.db)
        .await?;

    let member_category_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT g.group_category_id
        FROM group_memberships gm
        JOIN groups g ON g.id = gm.group_id
        WHERE gm.user_id = $1
          AND gm.workflow_state != 'deleted'
          AND g.workflow_state != 'deleted'
          AND g.course_id = $2
          AND g.group_category_id IS NOT NULL
        "#,
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;
    let member_ids: HashSet<i64> = member_category_ids.into_iter().collect();

    Ok(Json(
        categories
            .into_iter()
            .map(|category| {
                let is_member = member_ids.contains(&category.id);
                group_category_json(category, is_member)
            })
            .collect(),
    ))
}

/// GET /group_categories/{id} — get a single category (enrolled/admin)
async fn get_group_category(
    State(state): State<AppState>,
    Path(category_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<GroupCategory>, ApiError> {
    let category = fetch_group_category(&state.db, category_id).await?;

    if !can_read_group_category(&state.db, auth.id, &category, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let is_member = category_is_member(&state.db, category.id, auth.id).await?;
    Ok(Json(group_category_json(category, is_member)))
}

/// PUT /group_categories/{id} — update a category (teacher/admin only)
async fn update_group_category(
    State(state): State<AppState>,
    Path(category_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateGroupCategory>,
) -> Result<Json<GroupCategory>, ApiError> {
    let existing = fetch_group_category(&state.db, category_id).await?;

    if !can_manage_group_category(&state.db, auth.id, &existing, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let name = payload
        .name
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|| existing.name.clone());
    let self_signup = match payload.self_signup {
        Some(value) => normalize_optional(Some(value)),
        None => existing.self_signup.clone(),
    };
    let auto_leader = match payload.auto_leader {
        Some(value) => normalize_optional(Some(value)),
        None => existing.auto_leader.clone(),
    };
    let group_limit = payload.group_limit.or(existing.group_limit);

    validate_group_category_fields(
        &name,
        self_signup.as_deref(),
        auto_leader.as_deref(),
        group_limit,
    )?;

    if let Some(course_id) = existing.course_id {
        if category_name_taken(&state.db, course_id, &name, Some(category_id)).await? {
            return Err(ApiError::BadRequest(format!("{name} is already in use.")));
        }
    }

    // Canvas clears the deadline unless self signup remains enabled.
    let self_signup_end_at = if self_signup.as_deref() == Some("enabled") {
        payload.self_signup_end_at.or(existing.self_signup_end_at)
    } else {
        None
    };

    let sql = format!(
        r#"
        UPDATE group_categories
        SET name = $2, self_signup = $3, self_signup_end_at = $4,
            group_limit = $5, auto_leader = $6, updated_at = now()
        WHERE id = $1
        RETURNING {GROUP_CATEGORY_COLUMNS}
        "#
    );
    let category: GroupCategoryRow = sqlx::query_as(&sql)
        .bind(category_id)
        .bind(&name)
        .bind(&self_signup)
        .bind(self_signup_end_at)
        .bind(group_limit)
        .bind(&auto_leader)
        .fetch_one(&state.db)
        .await?;

    let is_member = category_is_member(&state.db, category.id, auth.id).await?;
    Ok(Json(group_category_json(category, is_member)))
}

/// DELETE /group_categories/{id} — soft-delete a category and its groups
/// (teacher/admin only). Canvas responds 200 with the deleted category body;
/// protected categories respond 401.
async fn delete_group_category(
    State(state): State<AppState>,
    Path(category_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<GroupCategory>, ApiError> {
    let existing = fetch_group_category(&state.db, category_id).await?;

    if !can_manage_group_category(&state.db, auth.id, &existing, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas refuses to delete protected categories with 401 "unauthorized".
    if is_protected_role(existing.role.as_deref()) {
        return Err(ApiError::Unauthorized);
    }

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE groups SET workflow_state = 'deleted', updated_at = now() \
         WHERE group_category_id = $1 AND workflow_state != 'deleted'",
    )
    .bind(category_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE group_categories SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(category_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(group_category_json(existing, false)))
}

/// GET /group_categories/{id}/groups — list active groups in a category
/// (enrolled/admin), ordered by name like Canvas.
async fn list_category_groups(
    State(state): State<AppState>,
    Path(category_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Group>>, ApiError> {
    let category = fetch_group_category(&state.db, category_id).await?;

    if !can_read_group_category(&state.db, auth.id, &category, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let groups: Vec<Group> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, description, workflow_state, join_level,
               group_category_id, created_at, updated_at
        FROM groups
        WHERE group_category_id = $1 AND workflow_state != 'deleted'
        ORDER BY name
        "#,
    )
    .bind(category_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(groups))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_state_follows_group_join_level() {
        assert_eq!(
            membership_initial_workflow_state("invitation_only"),
            "invited"
        );
        assert_eq!(
            membership_initial_workflow_state("parent_context_request"),
            "requested"
        );
        assert_eq!(
            membership_initial_workflow_state("parent_context_auto_join"),
            "accepted"
        );
    }

    #[test]
    fn group_category_flags_follow_role() {
        // Canvas protected?: role present and not "imported".
        assert!(!is_protected_role(None));
        assert!(!is_protected_role(Some("imported")));
        assert!(is_protected_role(Some("communities")));
        assert!(is_protected_role(Some("student_organized")));

        // Canvas allows_multiple_memberships?: communities/student_organized.
        assert!(role_allows_multiple_memberships(Some("communities")));
        assert!(role_allows_multiple_memberships(Some("student_organized")));
        assert!(!role_allows_multiple_memberships(Some("imported")));
        assert!(!role_allows_multiple_memberships(None));

        let row = GroupCategoryRow {
            id: 7,
            course_id: Some(3),
            account_id: None,
            name: "Project Groups".to_string(),
            role: Some("communities".to_string()),
            self_signup: None,
            self_signup_end_at: None,
            group_limit: None,
            auto_leader: None,
            non_collaborative: false,
            created_at: Utc::now(),
        };
        let json = group_category_json(row, true);
        assert_eq!(json.context_type, "Course");
        assert_eq!(json.course_id, Some(3));
        assert!(json.protected);
        assert!(json.allows_multiple_memberships);
        assert!(json.is_member);
    }

    #[test]
    fn group_category_validation_rejects_invalid_values() {
        assert!(validate_group_category_fields("Groups", None, None, None).is_ok());
        assert!(validate_group_category_fields("  ", None, None, None).is_err());
        assert!(validate_group_category_fields("Groups", Some("sometimes"), None, None).is_err());
        assert!(
            validate_group_category_fields("Groups", None, Some("alphabetical"), None).is_err()
        );
        assert!(
            validate_group_category_fields("Groups", Some("enabled"), Some("random"), None).is_ok()
        );
        assert!(validate_group_category_fields("Groups", None, None, Some(1)).is_err());
        assert!(validate_group_category_fields("Groups", Some("enabled"), None, Some(2)).is_ok());
    }
}
