//! star-accounts — Canvas-style account hierarchy + account admin management.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// --------------------------------------------------------------------------
// Domain types
// --------------------------------------------------------------------------

/// Account record — Canvas API v1 shape.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Account {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_account_id: Option<i64>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Response for a single account (includes parent_account_id).
#[derive(Debug, Clone, Serialize)]
pub struct AccountResponse {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_account_id: Option<i64>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Account> for AccountResponse {
    fn from(a: Account) -> Self {
        AccountResponse {
            id: a.id,
            name: a.name,
            parent_account_id: a.parent_account_id,
            workflow_state: a.workflow_state,
            created_at: a.created_at,
            updated_at: a.updated_at,
        }
    }
}

/// Account user — maps a user to an account with a role.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AccountUser {
    pub id: i64,
    pub account_id: i64,
    pub user_id: i64,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Lightweight account user row for joined queries.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AccountUserWithUser {
    pub id: i64,
    pub account_id: i64,
    pub user_id: i64,
    pub role: String,
    pub user_name: String,
    pub user_email: String,
}

/// Request to create a sub-account.
#[derive(Debug, Deserialize)]
pub struct CreateSubAccount {
    pub name: String,
}

/// Request to grant account admin.
#[derive(Debug, Deserialize)]
pub struct GrantAccountAdmin {
    pub role: Option<String>,
}

/// Pagination params.
#[derive(Debug, Deserialize)]
pub struct Pagination {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl Pagination {
    fn resolve(&self) -> (i64, i64, i64) {
        let page = self.page.unwrap_or(1).max(1);
        let per_page = self.per_page.unwrap_or(50).clamp(1, 100);
        (page, per_page, (page - 1) * per_page)
    }
}

/// Paginated accounts list.
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedAccounts {
    pub data: Vec<Account>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// Paginated account users list.
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedAccountUsers {
    pub data: Vec<AccountUserWithUser>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// Course summary for account course listing.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AccountCourse {
    pub id: i64,
    pub name: String,
    pub course_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Paginated courses under an account.
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedAccountCourses {
    pub data: Vec<AccountCourse>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

// --------------------------------------------------------------------------
// Account notifications — Canvas `Api::V1::AccountNotifications` shape
// --------------------------------------------------------------------------

/// Row loaded from `account_notifications` joined with its author.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountNotificationRow {
    pub id: i64,
    pub account_id: i64,
    pub subject: String,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub icon: Option<String>,
    pub message: String,
    pub user_id: Option<i64>,
    pub author_name: Option<String>,
}

/// Row loaded from `account_notification_roles`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountNotificationRoleRow {
    pub account_notification_id: i64,
    pub role_id: Option<i64>,
    pub role_name: String,
}

/// Author of a notification (`display_author` in Canvas).
#[derive(Debug, Clone, Serialize)]
pub struct AccountNotificationAuthor {
    pub id: i64,
    pub name: String,
}

/// Canvas account notification JSON:
/// `id, subject, start_at, end_at, icon, message, role_ids, roles, author`.
#[derive(Debug, Clone, Serialize)]
pub struct AccountNotificationResponse {
    pub id: i64,
    pub subject: String,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub icon: Option<String>,
    pub message: String,
    pub role_ids: Vec<Option<i64>>,
    pub roles: Vec<String>,
    pub author: Option<AccountNotificationAuthor>,
}

/// Request to create a notification. Canvas nests the attributes under
/// `account_notification[...]`; JSON clients send the same keys flat.
#[derive(Debug, Deserialize)]
pub struct CreateAccountNotification {
    pub subject: String,
    pub message: String,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub icon: Option<String>,
    /// Canvas sends role names (or ids) in `account_notification_roles[]`.
    #[serde(default, alias = "roles")]
    pub account_notification_roles: Option<Vec<String>>,
}

// --------------------------------------------------------------------------
// Account admins — Canvas `Api::V1::Admin` shape
// --------------------------------------------------------------------------

/// Row joining `account_users` with the promoted user.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountAdminRow {
    pub id: i64,
    pub account_id: i64,
    pub user_id: i64,
    pub role: String,
    pub user_name: String,
    pub user_sortable_name: Option<String>,
    pub user_email: String,
}

/// Minimal Canvas user_json embedded in an admin record.
#[derive(Debug, Clone, Serialize)]
pub struct AdminUser {
    pub id: i64,
    pub name: String,
    pub sortable_name: Option<String>,
    pub email: String,
}

/// Canvas Admin JSON: `id, role, role_id, user, workflow_state`.
#[derive(Debug, Clone, Serialize)]
pub struct AccountAdmin {
    pub id: i64,
    pub role: String,
    pub role_id: Option<i64>,
    pub user: AdminUser,
    pub workflow_state: String,
}

impl From<AccountAdminRow> for AccountAdmin {
    fn from(row: AccountAdminRow) -> Self {
        AccountAdmin {
            id: row.id,
            role: canvas_admin_role(&row.role).to_string(),
            role_id: None,
            user: AdminUser {
                id: row.user_id,
                name: row.user_name,
                sortable_name: row.user_sortable_name,
                email: row.user_email,
            },
            workflow_state: "active".to_string(),
        }
    }
}

/// Request to make a user an account admin.
#[derive(Debug, Deserialize)]
pub struct CreateAccountAdmin {
    pub user_id: i64,
    /// Deprecated Canvas param; only AccountAdmin is representable here.
    pub role: Option<String>,
}

// --------------------------------------------------------------------------
// Permission helpers
// --------------------------------------------------------------------------

const MAX_ACCOUNT_TREE_DEPTH: i64 = 10;

/// Check if the authenticated user is a global admin.
fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Check if the authenticated user is an account admin for the given account.
/// Queries account_users table. Falls back to false if not found.
async fn is_account_admin(pool: &PgPool, user_id: i64, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 LIMIT 1"#,
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(row.is_some())
}

/// Check if the user can view/manage the given account.
/// Global admin OR account admin for this account (or any parent in the chain).
/// Uses iterative traversal with max depth guard.
async fn can_access_account(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let mut current_id = Some(account_id);
    let mut depth = 0;

    while let Some(id) = current_id {
        if depth >= MAX_ACCOUNT_TREE_DEPTH {
            // Guard against circular reference attacks
            return Ok(false);
        }
        depth += 1;

        if is_account_admin(pool, user_id, id).await? {
            return Ok(true);
        }

        // Move up to parent
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as(r#"SELECT parent_account_id FROM accounts WHERE id = $1"#)
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(ApiError::from)?;

        current_id = parent.and_then(|(pid,)| pid);
    }

    Ok(false)
}

/// Check whether an account row exists.
async fn account_exists(pool: &PgPool, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM accounts WHERE id = $1"#)
        .bind(account_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(row.is_some())
}

/// Canvas exposes the built-in admin role as "AccountAdmin"; the local
/// `account_users.role` column stores the short "admin" form.
fn canvas_admin_role(role: &str) -> &str {
    match role {
        "admin" | "AccountAdmin" => "AccountAdmin",
        other => other,
    }
}

/// Validate the account notification payload the way Canvas's model does
/// (required fields plus `end_at >= start_at`); Canvas renders 400 on failure.
fn validate_notification(payload: &CreateAccountNotification) -> Result<(), ApiError> {
    if payload.subject.trim().is_empty() {
        return Err(ApiError::BadRequest("subject is required".into()));
    }
    if payload.message.trim().is_empty() {
        return Err(ApiError::BadRequest("message is required".into()));
    }
    if payload.end_at < payload.start_at {
        return Err(ApiError::BadRequest(
            "end_at must not precede start_at".into(),
        ));
    }
    Ok(())
}

/// Normalize `account_notification_roles` entries. Canvas accepts either a
/// role id or a role name per entry; ids are kept in `role_id` and the raw
/// entry in `role_name` because this schema has no `roles` table to resolve
/// names against. Duplicates are dropped while preserving request order.
fn notification_role_entries(payload: &CreateAccountNotification) -> Vec<(Option<i64>, String)> {
    let mut entries: Vec<(Option<i64>, String)> = Vec::new();
    for raw in payload.account_notification_roles.iter().flatten() {
        let name = raw.trim();
        if name.is_empty() || entries.iter().any(|(_, existing)| existing == name) {
            continue;
        }
        entries.push((name.parse::<i64>().ok(), name.to_string()));
    }
    entries
}

/// Load the role rows for a set of notification ids.
async fn fetch_notification_roles(
    pool: &PgPool,
    notification_ids: &[i64],
) -> Result<Vec<AccountNotificationRoleRow>, ApiError> {
    if notification_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<AccountNotificationRoleRow> = sqlx::query_as(
        r#"
        SELECT account_notification_id, role_id, role_name
        FROM account_notification_roles
        WHERE account_notification_id = ANY($1)
          AND workflow_state = 'active'
        ORDER BY id ASC
        "#,
    )
    .bind(notification_ids)
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows)
}

/// Convert notification rows + role rows into Canvas JSON responses.
fn notification_responses(
    rows: Vec<AccountNotificationRow>,
    roles: Vec<AccountNotificationRoleRow>,
    display_author: bool,
) -> Vec<AccountNotificationResponse> {
    let mut roles_by_notification: HashMap<i64, Vec<AccountNotificationRoleRow>> = HashMap::new();
    for role in roles {
        roles_by_notification
            .entry(role.account_notification_id)
            .or_default()
            .push(role);
    }

    rows.into_iter()
        .map(|row| {
            let notification_roles = roles_by_notification.remove(&row.id).unwrap_or_default();
            AccountNotificationResponse {
                id: row.id,
                subject: row.subject,
                start_at: row.start_at,
                end_at: row.end_at,
                icon: row.icon,
                message: row.message,
                role_ids: notification_roles.iter().map(|r| r.role_id).collect(),
                roles: notification_roles
                    .iter()
                    .map(|r| r.role_name.clone())
                    .collect(),
                author: match (display_author, row.user_id, row.author_name) {
                    (true, Some(id), Some(name)) => Some(AccountNotificationAuthor { id, name }),
                    _ => None,
                },
            }
        })
        .collect()
}

const NOTIFICATION_COLUMNS: &str = r#"
    n.id, n.account_id, n.subject, n.start_at, n.end_at, n.icon, n.message,
    n.user_id, u.name AS author_name
"#;

/// List a notification (or every active notification of an account), newest
/// `start_at` first, matching Canvas `for_account(...).order(start_at: :desc)`.
async fn fetch_notifications(
    pool: &PgPool,
    account_id: Option<i64>,
    notification_id: Option<i64>,
    limit: i64,
    offset: i64,
    display_author: bool,
) -> Result<Vec<AccountNotificationResponse>, ApiError> {
    let query = format!(
        r#"
        SELECT {NOTIFICATION_COLUMNS}
        FROM account_notifications n
        LEFT JOIN users u ON u.id = n.user_id
        WHERE n.workflow_state = 'active'
          AND ($1::bigint IS NULL OR n.account_id = $1)
          AND ($2::bigint IS NULL OR n.id = $2)
        ORDER BY n.start_at DESC, n.id DESC
        LIMIT $3 OFFSET $4
        "#
    );

    let rows: Vec<AccountNotificationRow> = sqlx::query_as(&query)
        .bind(account_id)
        .bind(notification_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(ApiError::from)?;

    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let roles = fetch_notification_roles(pool, &ids).await?;
    Ok(notification_responses(rows, roles, display_author))
}

/// Load one active notification in Canvas JSON shape.
async fn fetch_notification(
    pool: &PgPool,
    notification_id: i64,
    display_author: bool,
) -> Result<Option<AccountNotificationResponse>, ApiError> {
    let mut rows =
        fetch_notifications(pool, None, Some(notification_id), 1, 0, display_author).await?;
    Ok(rows.pop())
}

/// List the admins of an account, ordered by account_users id (Canvas order).
async fn fetch_admins(
    pool: &PgPool,
    account_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<AccountAdmin>, ApiError> {
    let rows: Vec<AccountAdminRow> = sqlx::query_as(
        r#"
        SELECT au.id, au.account_id, au.user_id, au.role,
               u.name AS user_name,
               u.sortable_name AS user_sortable_name,
               u.email AS user_email
        FROM account_users au
        JOIN users u ON u.id = au.user_id
        WHERE au.account_id = $1
        ORDER BY au.id ASC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(account_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows.into_iter().map(AccountAdmin::from).collect())
}

/// Load one account/admin pair in Canvas Admin JSON shape.
async fn fetch_admin(
    pool: &PgPool,
    account_id: i64,
    user_id: i64,
) -> Result<Option<AccountAdmin>, ApiError> {
    let row: Option<AccountAdminRow> = sqlx::query_as(
        r#"
        SELECT au.id, au.account_id, au.user_id, au.role,
               u.name AS user_name,
               u.sortable_name AS user_sortable_name,
               u.email AS user_email
        FROM account_users au
        JOIN users u ON u.id = au.user_id
        WHERE au.account_id = $1 AND au.user_id = $2
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(row.map(AccountAdmin::from))
}

// --------------------------------------------------------------------------
// Router
// --------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounts", get(list_accounts))
        .route("/accounts/{id}", get(get_account))
        .route("/accounts/{id}/sub_accounts", post(create_sub_account))
        .route("/accounts/{id}/courses", get(list_account_courses))
        .route("/accounts/{id}/users", get(list_account_users))
        .route("/accounts/{id}/users/{userId}", post(grant_account_admin))
        // Canvas Api::V1::AccountNotifications
        .route(
            "/accounts/{account_id}/account_notifications",
            get(list_account_notifications).post(create_account_notification),
        )
        .route(
            "/accounts/{account_id}/account_notifications/{id}",
            delete(delete_account_notification_scoped),
        )
        .route(
            "/account_notifications/{id}",
            delete(delete_account_notification),
        )
        // Canvas Api::V1::Admin
        .route(
            "/accounts/{account_id}/admins",
            get(list_account_admins).post(create_account_admin),
        )
        .route(
            "/accounts/{account_id}/admins/{user_id}",
            delete(delete_account_admin),
        )
}

// --------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------

/// GET /accounts — list all accounts (admin only).
async fn list_accounts(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<PaginatedAccounts>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let (page, per_page, offset) = pag.resolve();

    let (total,): (i64,) = sqlx::query_as(r#"SELECT COUNT(*) FROM accounts"#)
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?;

    let accounts: Vec<Account> = sqlx::query_as(
        r#"
        SELECT id, name, parent_account_id, workflow_state, created_at, updated_at
        FROM accounts
        ORDER BY id ASC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(PaginatedAccounts {
        data: accounts,
        page,
        per_page,
        total,
    }))
}

/// GET /accounts/{id} — get a single account (admin or account admin).
async fn get_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<AccountResponse>, ApiError> {
    if !can_access_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let account: Option<Account> = sqlx::query_as(
        r#"
        SELECT id, name, parent_account_id, workflow_state, created_at, updated_at
        FROM accounts WHERE id = $1
        "#,
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    account.ok_or(ApiError::NotFound).map(|a| Json(a.into()))
}

/// POST /accounts/{id}/sub_accounts — create a child account (admin only).
async fn create_sub_account(
    State(state): State<AppState>,
    Path(parent_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateSubAccount>,
) -> Result<(StatusCode, Json<AccountResponse>), ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Verify parent exists
    let parent_exists: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM accounts WHERE id = $1"#)
        .bind(parent_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;

    if parent_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    if payload.name.trim().is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }

    let account: Account = sqlx::query_as(
        r#"
        INSERT INTO accounts (name, parent_account_id)
        VALUES ($1, $2)
        RETURNING id, name, parent_account_id, workflow_state, created_at, updated_at
        "#,
    )
    .bind(&payload.name)
    .bind(parent_id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok((StatusCode::CREATED, Json(account.into())))
}

/// GET /accounts/{id}/courses — list all courses under this account tree (admin only).
/// Iterates all sub-accounts up to MAX_ACCOUNT_TREE_DEPTH.
async fn list_account_courses(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<PaginatedAccountCourses>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let (page, per_page, offset) = pag.resolve();

    // Collect all account IDs in this account tree (BFS, depth-guarded)
    let mut account_ids = vec![account_id];
    let mut depth = 0;
    let mut frontier = vec![account_id];

    while !frontier.is_empty() {
        if depth >= MAX_ACCOUNT_TREE_DEPTH {
            // Guard against circular reference attacks
            break;
        }
        depth += 1;

        let placeholders = frontier
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let children: Vec<(i64,)> = sqlx::query_as(&format!(
            r#"SELECT id FROM accounts WHERE parent_account_id IN ({placeholders})"#
        ))
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::from)?;

        frontier = children.into_iter().map(|(id,)| id).collect();
        account_ids.extend(frontier.iter().copied());
    }

    let placeholders = account_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let (total,): (i64,) = sqlx::query_as(&format!(
        r#"SELECT COUNT(*) FROM courses WHERE account_id IN ({placeholders})"#
    ))
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    let courses: Vec<AccountCourse> = sqlx::query_as(&format!(
        r#"
        SELECT id, name, course_code, description, created_at, updated_at
        FROM courses
        WHERE account_id IN ({placeholders})
        ORDER BY created_at DESC, id DESC
        LIMIT {per_page} OFFSET {offset}
        "#,
    ))
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(PaginatedAccountCourses {
        data: courses,
        page,
        per_page,
        total,
    }))
}

/// GET /accounts/{id}/users — list account admins (admin only).
async fn list_account_users(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<PaginatedAccountUsers>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Verify account exists
    let account_exists: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM accounts WHERE id = $1"#)
        .bind(account_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;

    if account_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    let (page, per_page, offset) = pag.resolve();

    let (total,): (i64,) =
        sqlx::query_as(r#"SELECT COUNT(*) FROM account_users WHERE account_id = $1"#)
            .bind(account_id)
            .fetch_one(&state.db)
            .await
            .map_err(ApiError::from)?;

    let users: Vec<AccountUserWithUser> = sqlx::query_as(
        r#"
        SELECT au.id, au.account_id, au.user_id, au.role, u.name as user_name, u.email as user_email
        FROM account_users au
        JOIN users u ON u.id = au.user_id
        WHERE au.account_id = $1
        ORDER BY au.created_at ASC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(account_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(PaginatedAccountUsers {
        data: users,
        page,
        per_page,
        total,
    }))
}

/// POST /accounts/{id}/users/{userId} — grant account admin role (admin only).
async fn grant_account_admin(
    State(state): State<AppState>,
    Path((account_id, user_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<GrantAccountAdmin>,
) -> Result<(StatusCode, Json<AccountUser>), ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Verify account exists
    let account_exists: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM accounts WHERE id = $1"#)
        .bind(account_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;

    if account_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Verify user exists
    let user_exists: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM users WHERE id = $1"#)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;

    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    let role = payload.role.as_deref().unwrap_or("admin");
    if role != "admin" {
        return Err(ApiError::Validation("role must be 'admin'".into()));
    }

    // Upsert: insert or update on conflict
    let account_user: AccountUser = sqlx::query_as(
        r#"
        INSERT INTO account_users (account_id, user_id, role)
        VALUES ($1, $2, $3)
        ON CONFLICT (account_id, user_id) DO UPDATE SET role = EXCLUDED.role, updated_at = now()
        RETURNING id, account_id, user_id, role, created_at, updated_at
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .bind(role)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok((StatusCode::CREATED, Json(account_user)))
}

// --------------------------------------------------------------------------
// Account notification handlers
// --------------------------------------------------------------------------

/// GET /accounts/{account_id}/account_notifications — list the account's
/// active notifications. Canvas returns a bare JSON array here.
async fn list_account_notifications(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<Vec<AccountNotificationResponse>>, ApiError> {
    if !can_access_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }

    let (_, per_page, offset) = pag.resolve();
    // Admins request the full listing, so Canvas's `include_all` view applies
    // and the author is displayed.
    let notifications =
        fetch_notifications(&state.db, Some(account_id), None, per_page, offset, true).await?;
    Ok(Json(notifications))
}

/// POST /accounts/{account_id}/account_notifications — create a notification.
async fn create_account_notification(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateAccountNotification>,
) -> Result<(StatusCode, Json<AccountNotificationResponse>), ApiError> {
    if !can_access_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    validate_notification(&payload)?;

    let role_entries = notification_role_entries(&payload);

    let mut tx = state.db.begin().await.map_err(ApiError::from)?;
    let (notification_id,): (i64,) = sqlx::query_as(
        r#"
        INSERT INTO account_notifications
            (account_id, user_id, subject, message, start_at, end_at, icon)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
    )
    .bind(account_id)
    .bind(auth.id)
    .bind(&payload.subject)
    .bind(&payload.message)
    .bind(payload.start_at)
    .bind(payload.end_at)
    .bind(&payload.icon)
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from)?;

    for (role_id, role_name) in &role_entries {
        sqlx::query(
            r#"
            INSERT INTO account_notification_roles
                (account_notification_id, role_id, role_name)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(notification_id)
        .bind(*role_id)
        .bind(role_name)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from)?;
    }
    tx.commit().await.map_err(ApiError::from)?;

    let notification = fetch_notification(&state.db, notification_id, false)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok((StatusCode::OK, Json(notification)))
}

/// Shared delete path for both Canvas routes. `expected_account_id` scopes the
/// lookup for `/accounts/{account_id}/account_notifications/{id}`.
async fn delete_account_notification_impl(
    state: &AppState,
    auth: &AuthUser,
    notification_id: i64,
    expected_account_id: Option<i64>,
) -> Result<Json<AccountNotificationResponse>, ApiError> {
    let account_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT account_id FROM account_notifications
        WHERE id = $1 AND workflow_state = 'active'
        "#,
    )
    .bind(notification_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;
    let account_id = account_id.ok_or(ApiError::NotFound)?;

    if let Some(expected) = expected_account_id {
        if expected != account_id {
            return Err(ApiError::NotFound);
        }
    }

    if !can_access_account(&state.db, auth.id, account_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let notification = fetch_notification(&state.db, notification_id, false)
        .await?
        .ok_or(ApiError::NotFound)?;

    // account_notification_roles rows cascade from the FK.
    sqlx::query(r#"DELETE FROM account_notifications WHERE id = $1"#)
        .bind(notification_id)
        .execute(&state.db)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(notification))
}

/// DELETE /account_notifications/{id} — remove an account notification.
async fn delete_account_notification(
    State(state): State<AppState>,
    Path(notification_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<AccountNotificationResponse>, ApiError> {
    delete_account_notification_impl(&state, &auth, notification_id, None).await
}

/// DELETE /accounts/{account_id}/account_notifications/{id} — Canvas-scoped
/// alias; 404s when the notification belongs to another account.
async fn delete_account_notification_scoped(
    State(state): State<AppState>,
    Path((account_id, notification_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<AccountNotificationResponse>, ApiError> {
    delete_account_notification_impl(&state, &auth, notification_id, Some(account_id)).await
}

// --------------------------------------------------------------------------
// Account admin handlers
// --------------------------------------------------------------------------

/// GET /accounts/{account_id}/admins — list account_users with their user.
async fn list_account_admins(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<Vec<AccountAdmin>>, ApiError> {
    if !can_access_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }

    let (_, per_page, offset) = pag.resolve();
    let admins = fetch_admins(&state.db, account_id, per_page, offset).await?;
    Ok(Json(admins))
}

/// POST /accounts/{account_id}/admins — flag an existing user as admin.
async fn create_account_admin(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateAccountAdmin>,
) -> Result<(StatusCode, Json<AccountAdmin>), ApiError> {
    if !can_access_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }

    if let Some(role) = payload.role.as_deref() {
        if role != "admin" && role != "AccountAdmin" {
            return Err(ApiError::Validation("role must be 'AccountAdmin'".into()));
        }
    }

    let user_exists: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM users WHERE id = $1"#)
        .bind(payload.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;
    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Canvas is idempotent here: an existing pair is returned unchanged.
    sqlx::query(
        r#"
        INSERT INTO account_users (account_id, user_id, role)
        VALUES ($1, $2, 'admin')
        ON CONFLICT (account_id, user_id) DO UPDATE
            SET role = 'admin', updated_at = now()
        "#,
    )
    .bind(account_id)
    .bind(payload.user_id)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    let admin = fetch_admin(&state.db, account_id, payload.user_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok((StatusCode::OK, Json(admin)))
}

/// DELETE /accounts/{account_id}/admins/{user_id} — remove the admin grant.
async fn delete_account_admin(
    State(state): State<AppState>,
    Path((account_id, user_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<AccountAdmin>, ApiError> {
    if !can_access_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let mut admin = fetch_admin(&state.db, account_id, user_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    sqlx::query(r#"DELETE FROM account_users WHERE account_id = $1 AND user_id = $2"#)
        .bind(account_id)
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(ApiError::from)?;

    // Canvas' admin_json after destroy reports workflow_state "deleted".
    admin.workflow_state = "deleted".to_string();
    Ok(Json(admin))
}

// --------------------------------------------------------------------------
// Unit tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pagination_defaults() {
        let pag = Pagination {
            page: None,
            per_page: None,
        };
        assert_eq!(pag.resolve(), (1, 50, 0));
    }

    #[test]
    fn test_pagination_page2() {
        let pag = Pagination {
            page: Some(2),
            per_page: Some(10),
        };
        assert_eq!(pag.resolve(), (2, 10, 10));
    }

    #[test]
    fn test_pagination_clamped() {
        let pag = Pagination {
            page: Some(0),
            per_page: Some(500),
        };
        assert_eq!(pag.resolve(), (1, 100, 0));
    }

    #[test]
    fn test_is_admin() {
        let auth = AuthUser {
            id: 1,
            role: "admin".into(),
        };
        assert!(is_admin(&auth));

        let auth = AuthUser {
            id: 2,
            role: "student".into(),
        };
        assert!(!is_admin(&auth));
    }
}
