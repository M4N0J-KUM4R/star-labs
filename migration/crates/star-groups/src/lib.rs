//! star-groups — Canvas-style groups and group memberships for star-lms.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
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

/// Canvas group_membership_json shape returned by POST /groups/{groupId}/users
/// and POST /groups/{groupId}/invite.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GroupMembershipJson {
    pub id: i64,
    pub group_id: i64,
    pub user_id: i64,
    pub workflow_state: String,
    pub moderator: bool,
    /// Canvas includes this only on create (include=just_created).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub just_created: Option<bool>,
}

/// Request body for POST /groups/{groupId}/invite (Canvas `invitees[]`).
#[derive(Debug, Deserialize)]
pub struct InviteGroupUsers {
    #[serde(default)]
    pub invitees: Vec<String>,
}

/// Request body for DELETE /groups/{groupId}/users (Canvas `user_ids[]`).
#[derive(Debug, Default, Deserialize)]
pub struct BulkRemoveGroupUsers {
    #[serde(default)]
    pub user_ids: Vec<i64>,
}

/// Query params for DELETE /groups/{groupId}/users fallback
/// (`?user_ids=1,2`).
#[derive(Debug, Deserialize)]
pub struct BulkRemoveGroupUsersQuery {
    pub user_ids: Option<String>,
}

/// Query params for GET /groups/{groupId}/users.
#[derive(Debug, Deserialize)]
pub struct ListGroupUsersQuery {
    pub search_term: Option<String>,
}

/// Query params for GET /users/self/groups.
#[derive(Debug, Deserialize)]
pub struct SelfGroupsQuery {
    pub context_type: Option<String>,
}

/// User row joined with the group membership state (Canvas User shape).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GroupUser {
    pub id: i64,
    pub name: String,
    pub sortable_name: String,
    pub email: String,
    pub role: String,
    /// State of this user's membership in the group.
    pub workflow_state: String,
}

/// Group-scoped file metadata (Canvas File shape; no upload/storage).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GroupFile {
    pub id: i64,
    pub group_id: i64,
    pub folder: String,
    pub filename: String,
    pub display_name: String,
    pub content_type: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

/// Query params for GET /accounts/{accountId}/group_categories. Canvas filters
/// on `collaboration_state` (all | collaborative | non_collaborative, default
/// collaborative).
#[derive(Debug, Deserialize)]
pub struct AccountCategoriesQuery {
    pub collaboration_state: Option<String>,
}

/// One member returned by POST /group_categories/{id}/assign_unassigned_members
/// (Canvas `new_members` entry; sections are not exposed by this service).
#[derive(Debug, Clone, Serialize)]
pub struct AssignedMember {
    pub user_id: i64,
    pub name: String,
    pub display_name: String,
}

/// One group's share of the assignment result (Canvas response array item).
#[derive(Debug, Clone, Serialize)]
pub struct AssignedGroupMembers {
    pub id: i64,
    pub new_members: Vec<AssignedMember>,
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

/// Fetch the join_level of an existing, non-deleted group.
async fn fetch_active_group_join_level(
    pool: &PgPool,
    group_id: i64,
) -> Result<Option<String>, ApiError> {
    let group: Option<(String,)> = sqlx::query_as(
        "SELECT join_level FROM groups WHERE id = $1 AND workflow_state != 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?;
    Ok(group.map(|(join_level,)| join_level))
}

/// Canvas group :read: admins, course teachers, or any active member.
async fn can_read_group(
    pool: &PgPool,
    user_id: i64,
    group_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_teacher_in_group(pool, user_id, group_id, auth).await? {
        return Ok(true);
    }

    let member: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM group_memberships
        WHERE group_id = $1 AND user_id = $2 AND workflow_state != 'deleted'
        LIMIT 1
        "#,
    )
    .bind(group_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(member.is_some())
}

/// Canvas UserList trims invitee identifiers; emails match case-insensitively.
fn normalize_invitee(value: &str) -> String {
    value.trim().to_string()
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

async fn ensure_account_exists(pool: &PgPool, account_id: i64) -> Result<(), ApiError> {
    let account: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    account.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Account-scoped counterpart of `category_name_taken`.
async fn account_category_name_taken(
    pool: &PgPool,
    account_id: i64,
    name: &str,
    exclude_id: Option<i64>,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM group_categories
        WHERE account_id = $1 AND name = $2 AND workflow_state != 'deleted'
          AND ($3::BIGINT IS NULL OR id != $3)
        LIMIT 1
        "#,
    )
    .bind(account_id)
    .bind(name)
    .bind(exclude_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
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
        // Canvas groups#users / group_memberships#create/destroy_bulk
        .route("/groups/{groupId}/users", get(list_group_users))
        .route("/groups/{groupId}/users", post(add_group_user))
        .route("/groups/{groupId}/users", delete(bulk_remove_group_users))
        .route(
            "/groups/{groupId}/users/{userId}",
            delete(remove_group_user),
        )
        // Canvas groups#invite and group file metadata
        .route("/groups/{groupId}/invite", post(invite_group_users))
        .route("/groups/{groupId}/files", get(list_group_files))
        // Canvas groups#index (current user's active groups)
        .route("/users/self/groups", get(list_self_groups))
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
        .route("/group_categories/{id}", patch(update_group_category))
        .route("/group_categories/{id}", delete(delete_group_category))
        .route("/group_categories/{id}/groups", get(list_category_groups))
        // Canvas group categories in an account context (accounts/group_categories)
        .route(
            "/accounts/{accountId}/group_categories",
            get(list_account_group_categories),
        )
        .route(
            "/accounts/{accountId}/group_categories",
            post(create_account_group_category),
        )
        // Canvas group_categories#assign_unassigned_members
        .route(
            "/group_categories/{id}/assign_unassigned_members",
            post(assign_unassigned_members),
        )
        // Canvas groups#create without a course route (category context)
        .route("/groups", post(create_group_standalone))
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
// Canvas group users / invitations / files handlers
// ---------------------------------------------------------------------------

/// GET /groups/{groupId}/users — list the users in a group (Canvas groups#users).
/// Includes invited/requested memberships; excludes soft-deleted ones.
async fn list_group_users(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(query): Query<ListGroupUsersQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<GroupUser>>, ApiError> {
    if fetch_active_group_join_level(&state.db, group_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound);
    }

    if !can_read_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let search_term = query.search_term.and_then(|term| {
        let term = term.trim().to_string();
        if term.is_empty() {
            None
        } else {
            Some(term)
        }
    });
    if let Some(term) = search_term.as_deref() {
        if term.chars().count() < 2 {
            return Err(ApiError::BadRequest(
                "search_term must be at least 2 characters".into(),
            ));
        }
    }

    let users: Vec<GroupUser> = sqlx::query_as(
        r#"
        SELECT u.id, u.name, COALESCE(u.sortable_name, u.name) AS sortable_name,
               u.email, u.role, gm.workflow_state
        FROM group_memberships gm
        JOIN users u ON u.id = gm.user_id
        WHERE gm.group_id = $1
          AND gm.workflow_state != 'deleted'
          AND (
              $2::TEXT IS NULL
              OR u.name ILIKE '%' || $2 || '%'
              OR u.email ILIKE '%' || $2 || '%'
          )
        ORDER BY u.name
        "#,
    )
    .bind(group_id)
    .bind(search_term)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(users))
}

/// POST /groups/{groupId}/users — add a user to a group
/// (Canvas group_memberships#create). Existing memberships are returned as-is.
async fn add_group_user(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateMembership>,
) -> Result<Json<GroupMembershipJson>, ApiError> {
    let Some(join_level) = fetch_active_group_join_level(&state.db, group_id).await? else {
        return Err(ApiError::NotFound);
    };

    if !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let user_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(payload.user_id)
        .fetch_optional(&state.db)
        .await?;
    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // `xmax = 0` distinguishes a fresh insert from a conflict update, which
    // drives Canvas' `just_created` flag. Deleted memberships are revived.
    let membership: GroupMembershipJson = sqlx::query_as(
        r#"
        INSERT INTO group_memberships (group_id, user_id, workflow_state)
        VALUES ($1, $2, $3)
        ON CONFLICT (group_id, user_id) DO UPDATE
          SET workflow_state = CASE
                WHEN group_memberships.workflow_state = 'deleted'
                  THEN EXCLUDED.workflow_state
                ELSE group_memberships.workflow_state
              END,
              updated_at = now()
        RETURNING id, group_id, user_id, workflow_state, moderator,
                  (xmax = 0) AS just_created
        "#,
    )
    .bind(group_id)
    .bind(payload.user_id)
    .bind(membership_initial_workflow_state(&join_level))
    .fetch_one(&state.db)
    .await?;

    Ok(Json(membership))
}

/// DELETE /groups/{groupId}/users/{userId} — leave/remove a membership
/// (Canvas group_memberships#destroy). `userId` may be "self".
async fn remove_group_user(
    State(state): State<AppState>,
    Path((group_id, user_ref)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    if fetch_active_group_join_level(&state.db, group_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound);
    }

    let user_id = if user_ref == "self" {
        auth.id
    } else {
        user_ref
            .parse::<i64>()
            .map_err(|_| ApiError::BadRequest("user id must be an integer or 'self'".into()))?
    };

    let membership: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM group_memberships \
         WHERE group_id = $1 AND user_id = $2 AND workflow_state != 'deleted'",
    )
    .bind(group_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;

    let Some((membership_id,)) = membership else {
        return Err(ApiError::NotFound);
    };

    // Members may leave on their own; managing others needs group manage.
    if user_id != auth.id && !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    sqlx::query(
        "UPDATE group_memberships SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(membership_id)
    .execute(&state.db)
    .await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// DELETE /groups/{groupId}/users — bulk remove memberships
/// (Canvas group_memberships#destroy_bulk, `user_ids[]`).
async fn bulk_remove_group_users(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(query): Query<BulkRemoveGroupUsersQuery>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    if fetch_active_group_join_level(&state.db, group_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound);
    }

    if !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let payload: BulkRemoveGroupUsers = if body.is_empty() {
        BulkRemoveGroupUsers::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|_| ApiError::BadRequest("invalid JSON body".into()))?
    };

    let mut user_ids = payload.user_ids;
    if user_ids.is_empty() {
        if let Some(list) = query.user_ids {
            user_ids = list
                .split(',')
                .filter_map(|value| value.trim().parse::<i64>().ok())
                .collect();
        }
    }
    if user_ids.is_empty() {
        return Err(ApiError::BadRequest("user_ids is required".into()));
    }

    let deleted_user_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        UPDATE group_memberships
        SET workflow_state = 'deleted', updated_at = now()
        WHERE group_id = $1 AND user_id = ANY($2) AND workflow_state != 'deleted'
        RETURNING user_id
        "#,
    )
    .bind(group_id)
    .bind(&user_ids)
    .fetch_all(&state.db)
    .await?;

    // Canvas reports memberships the caller could not remove; here that is
    // any requested user id that had no active membership in the group.
    let unauthorized_user_ids: Vec<i64> = user_ids
        .into_iter()
        .filter(|user_id| !deleted_user_ids.contains(user_id))
        .collect();

    Ok(Json(serde_json::json!({
        "message": "Bulk delete completed",
        "deleted_user_ids": deleted_user_ids,
        "unauthorized_user_ids": unauthorized_user_ids,
    })))
}

/// POST /groups/{groupId}/invite — invite users by email or user id
/// (Canvas groups#invite, `invitees[]`). Matches existing users; unknown
/// emails are recorded in group_invitations.
async fn invite_group_users(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<InviteGroupUsers>,
) -> Result<Json<Vec<GroupMembershipJson>>, ApiError> {
    if fetch_active_group_join_level(&state.db, group_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound);
    }

    if !is_teacher_in_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let invitees: Vec<String> = payload
        .invitees
        .into_iter()
        .map(|invitee| normalize_invitee(&invitee))
        .filter(|invitee| !invitee.is_empty())
        .collect();
    if invitees.is_empty() {
        return Err(ApiError::BadRequest("invitees is required".into()));
    }

    let mut memberships = Vec::new();
    for invitee in invitees {
        // Canvas UserList resolves user ids and email addresses.
        let user: Option<(i64, String)> = if let Ok(user_id) = invitee.parse::<i64>() {
            sqlx::query_as("SELECT id, email FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&state.db)
                .await?
        } else {
            sqlx::query_as("SELECT id, email FROM users WHERE lower(email) = lower($1)")
                .bind(&invitee)
                .fetch_optional(&state.db)
                .await?
        };

        let email = match &user {
            Some((_, email)) => email.clone(),
            None => invitee.to_lowercase(),
        };
        sqlx::query(
            r#"
            INSERT INTO group_invitations (group_id, email, user_id, invited_by_id)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(group_id)
        .bind(&email)
        .bind(user.as_ref().map(|(user_id, _)| *user_id))
        .bind(auth.id)
        .execute(&state.db)
        .await?;

        let Some((user_id, _)) = user else {
            continue;
        };

        // Canvas invite_user: invite pending memberships; active ones stay put.
        let membership: GroupMembershipJson = sqlx::query_as(
            r#"
            INSERT INTO group_memberships (group_id, user_id, workflow_state)
            VALUES ($1, $2, 'invited')
            ON CONFLICT (group_id, user_id) DO UPDATE
              SET workflow_state = CASE
                    WHEN group_memberships.workflow_state = 'deleted' THEN 'invited'
                    ELSE group_memberships.workflow_state
                  END,
                  updated_at = now()
            RETURNING id, group_id, user_id, workflow_state, moderator,
                      NULL::BOOLEAN AS just_created
            "#,
        )
        .bind(group_id)
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
        memberships.push(membership);
    }

    Ok(Json(memberships))
}

/// GET /groups/{groupId}/files — group file metadata (Canvas files#api_index).
async fn list_group_files(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<GroupFile>>, ApiError> {
    if fetch_active_group_join_level(&state.db, group_id)
        .await?
        .is_none()
    {
        return Err(ApiError::NotFound);
    }

    if !can_read_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let files: Vec<GroupFile> = sqlx::query_as(
        r#"
        SELECT id, group_id, folder, filename,
               COALESCE(display_name, filename) AS display_name,
               content_type, size, created_at, updated_at
        FROM group_files
        WHERE group_id = $1
        ORDER BY display_name
        "#,
    )
    .bind(group_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(files))
}

/// GET /users/self/groups — the caller's active groups (Canvas groups#index).
async fn list_self_groups(
    State(state): State<AppState>,
    Query(query): Query<SelfGroupsQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<Group>>, ApiError> {
    let context_type = match query.context_type.as_deref() {
        None | Some("") => None,
        Some("Course") => Some("Course"),
        Some("Account") => Some("Account"),
        Some(_) => {
            return Err(ApiError::BadRequest(
                "context_type must be 'Course' or 'Account'".into(),
            ))
        }
    };

    let groups: Vec<Group> = sqlx::query_as(
        r#"
        SELECT g.id, g.course_id, g.name, g.description, g.workflow_state,
               g.join_level, g.group_category_id, g.created_at, g.updated_at
        FROM groups g
        JOIN group_memberships gm ON gm.group_id = g.id
        WHERE gm.user_id = $1
          AND gm.workflow_state != 'deleted'
          AND g.workflow_state != 'deleted'
          AND (
              $2::TEXT IS NULL
              OR ($2 = 'Course' AND g.course_id IS NOT NULL)
              OR ($2 = 'Account' AND g.course_id IS NULL)
          )
        ORDER BY g.name
        "#,
    )
    .bind(auth.id)
    .bind(context_type)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(groups))
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

// ---------------------------------------------------------------------------
// Account group categories / standalone group / assignment handlers
// ---------------------------------------------------------------------------

/// GET /accounts/{accountId}/group_categories — account-scoped categories
/// (Canvas group_categories#index, `collaboration_state` filter).
async fn list_account_group_categories(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    Query(query): Query<AccountCategoriesQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<GroupCategory>>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    ensure_account_exists(&state.db, account_id).await?;

    let collaboration_state = query
        .collaboration_state
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "collaborative".to_string());

    // None means "all"; Some(false) is the Canvas default.
    let non_collaborative = match collaboration_state.as_str() {
        "collaborative" => Some(false),
        "non_collaborative" => Some(true),
        "all" => None,
        _ => {
            return Err(ApiError::BadRequest(
                "Invalid collaboration_state parameter".into(),
            ))
        }
    };

    let sql = format!(
        r#"
        SELECT {GROUP_CATEGORY_COLUMNS} FROM group_categories
        WHERE account_id = $1 AND workflow_state != 'deleted'
          AND ($2::BOOLEAN IS NULL OR non_collaborative = $2)
        ORDER BY id
        "#
    );
    let categories: Vec<GroupCategoryRow> = sqlx::query_as(&sql)
        .bind(account_id)
        .bind(non_collaborative)
        .fetch_all(&state.db)
        .await?;

    let mut response = Vec::with_capacity(categories.len());
    for category in categories {
        let is_member = category_is_member(&state.db, category.id, auth.id).await?;
        response.push(group_category_json(category, is_member));
    }
    Ok(Json(response))
}

/// POST /accounts/{accountId}/group_categories — create an account category
/// (Canvas group_categories#create; admin only). Course-only flags
/// (`self_signup`, `group_limit`, `auto_leader`) are ignored, like Canvas.
async fn create_account_group_category(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateGroupCategory>,
) -> Result<Json<GroupCategory>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    ensure_account_exists(&state.db, account_id).await?;

    let name = payload.name.trim().to_string();
    validate_group_category_fields(&name, None, None, None)?;

    if account_category_name_taken(&state.db, account_id, &name, None).await? {
        return Err(ApiError::BadRequest(format!("{name} is already in use.")));
    }

    let sql = format!(
        r#"
        INSERT INTO group_categories (account_id, name, non_collaborative)
        VALUES ($1, $2, $3)
        RETURNING {GROUP_CATEGORY_COLUMNS}
        "#
    );
    let category: GroupCategoryRow = sqlx::query_as(&sql)
        .bind(account_id)
        .bind(&name)
        .bind(payload.non_collaborative)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(group_category_json(category, false)))
}

/// POST /groups — create a group from its category's context (Canvas
/// groups#create). Course categories need a teacher/admin; account categories
/// and categories-less creates need an admin.
async fn create_group_standalone(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateGroup>,
) -> Result<Json<Group>, ApiError> {
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }

    let (course_id, account_id) = match payload.group_category_id {
        Some(category_id) => {
            let category = fetch_group_category(&state.db, category_id).await?;
            if !can_manage_group_category(&state.db, auth.id, &category, &auth).await? {
                return Err(ApiError::Forbidden);
            }
            (category.course_id, category.account_id)
        }
        // Canvas falls back to the domain root account's communities category;
        // this service has no domain root, so a bare POST /groups is admin-only.
        None => {
            if !is_admin(&auth) {
                return Err(ApiError::Forbidden);
            }
            (None, None)
        }
    };

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
        INSERT INTO groups
            (course_id, account_id, name, description, join_level, group_category_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, course_id, name, description, workflow_state, join_level,
                  group_category_id, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(account_id)
    .bind(&name)
    .bind(&payload.description)
    .bind(&join_level)
    .bind(payload.group_category_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(group))
}

/// POST /group_categories/{id}/assign_unassigned_members — distribute course
/// students without a group in this category evenly across its active groups
/// (Canvas group_categories#assign_unassigned_members).
///
/// Deviation: Canvas runs asynchronously and returns a Progress unless
/// `sync=true`; this service always assigns synchronously and returns the
/// membership summary, and ignores `group_by_section`.
async fn assign_unassigned_members(
    State(state): State<AppState>,
    Path(category_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<AssignedGroupMembers>>, ApiError> {
    let category = fetch_group_category(&state.db, category_id).await?;

    if !can_manage_group_category(&state.db, auth.id, &category, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if category.role.as_deref() == Some("student_organized") {
        return Err(ApiError::BadRequest(
            "student organized categories cannot assign members".into(),
        ));
    }
    let Some(course_id) = category.course_id else {
        return Err(ApiError::BadRequest(
            "group category must belong to a course".into(),
        ));
    };
    if category.self_signup.as_deref() == Some("restricted") {
        return Err(ApiError::BadRequest(
            "restricted self signup categories cannot assign members".into(),
        ));
    }

    let group_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM groups \
         WHERE group_category_id = $1 AND workflow_state != 'deleted' ORDER BY id",
    )
    .bind(category_id)
    .fetch_all(&state.db)
    .await?;
    if group_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let unassigned: Vec<(i64, String)> = sqlx::query_as(
        r#"
        SELECT u.id, u.name
        FROM users u
        JOIN enrollments e ON e.user_id = u.id
        WHERE e.course_id = $1 AND e.role = 'student'
          AND NOT EXISTS (
              SELECT 1
              FROM group_memberships gm
              JOIN groups g ON g.id = gm.group_id
              WHERE g.group_category_id = $2
                AND g.workflow_state != 'deleted'
                AND gm.user_id = u.id
                AND gm.workflow_state != 'deleted'
          )
        GROUP BY u.id, u.name
        ORDER BY u.id
        "#,
    )
    .bind(course_id)
    .bind(category_id)
    .fetch_all(&state.db)
    .await?;

    // Round-robin over the active groups keeps the split balanced.
    let mut assignment: Vec<AssignedGroupMembers> = group_ids
        .iter()
        .map(|id| AssignedGroupMembers {
            id: *id,
            new_members: Vec::new(),
        })
        .collect();

    for (index, (user_id, name)) in unassigned.into_iter().enumerate() {
        let slot = index % group_ids.len();
        sqlx::query(
            r#"
            INSERT INTO group_memberships (group_id, user_id, workflow_state)
            VALUES ($1, $2, 'accepted')
            ON CONFLICT (group_id, user_id) DO NOTHING
            "#,
        )
        .bind(group_ids[slot])
        .bind(user_id)
        .execute(&state.db)
        .await?;

        assignment[slot].new_members.push(AssignedMember {
            user_id,
            name: name.clone(),
            display_name: name,
        });
    }

    Ok(Json(
        assignment
            .into_iter()
            .filter(|group| !group.new_members.is_empty())
            .collect(),
    ))
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

    #[test]
    fn invitee_identifiers_are_trimmed() {
        assert_eq!(
            normalize_invitee("  leonard@example.com "),
            "leonard@example.com"
        );
        assert_eq!(normalize_invitee("42"), "42");
        assert_eq!(normalize_invitee("   "), "");
    }

    #[test]
    fn membership_json_omits_just_created_when_absent() {
        let membership = GroupMembershipJson {
            id: 9,
            group_id: 3,
            user_id: 7,
            workflow_state: "invited".to_string(),
            moderator: false,
            just_created: None,
        };
        let value = serde_json::to_value(&membership).expect("serialize membership");
        assert!(value.get("just_created").is_none());
        assert_eq!(value["workflow_state"], "invited");
        assert_eq!(value["moderator"], false);

        let created = GroupMembershipJson {
            just_created: Some(true),
            ..membership
        };
        let value = serde_json::to_value(&created).expect("serialize membership");
        assert_eq!(value["just_created"], true);
    }
}
