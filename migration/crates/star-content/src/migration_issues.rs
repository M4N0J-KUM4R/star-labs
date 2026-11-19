//! migration_issues — Canvas `MigrationIssuesController` parity.
//!
//! Routes (Canvas config/routes.rb `scope(controller: :migration_issues)`,
//! one block per context in `%w[account course group user]`):
//! - GET  /courses/{courseId}/content_migrations/{id}/migration_issues
//! - POST /courses/{courseId}/content_migrations/{id}/migration_issues
//! - GET  /courses/{courseId}/content_migrations/{id}/migration_issues/{issueId}
//! - PUT  /courses/{courseId}/content_migrations/{id}/migration_issues/{issueId}
//! and the same four actions under /accounts/{accountId},
//! /groups/{groupId} and /users/{userId} (16 routes total).
//!
//! Canvas controller (app/controllers/migration_issues_controller.rb):
//! `before_action :require_context` answers 404 for an unknown context, and
//! `before_action :require_content_migration` answers 404 unless the
//! migration belongs to the context, both *before* the permission check —
//! so a caller without permissions still sees the 404s. index/show require
//! the granular manage-course-content set (any-of
//! manage_course_content_add/edit/delete), update requires
//! manage_course_content_edit. Update accepts only
//! `workflow_state=active|resolved`; anything else is 403
//! `{"message":"Must send a valid workflow state"}` and save errors 400.
//! Index orders issues by created_at (`by_created_at`) and paginates.
//!
//! Canvas JSON (`lib/api/v1/content_migration.rb#migration_issue_json`):
//! id, description, workflow_state, fix_issue_html_url, issue_type,
//! created_at, updated_at, content_migration_url. Canvas builds
//! content_migration_url with the *course* URL helper for every context
//! (`api_v1_course_content_migration_url(migration.context_id, …)`), so
//! account/group/user contexts still get `/api/v1/courses/{context_id}/…`;
//! that long-standing quirk is replicated verbatim. `error_message` is only
//! exposed to `:read_errors` holders (site admins). This workspace also
//! carries the migration-issues-deep UI fields (content_migration_id,
//! issue_link, fixable_by_web_upload, fixable_by_admin, unresolved).
//!
//! Deviations from Canvas:
//! - Canvas has no `create` action behind POST migration_issues; this route
//!   creates an issue and returns it with HTTP 200 so the API is complete.
//! - An invalid `issue_type` is a 422 validation error instead of a DB error.
//! - Permissions use the workspace's simplified role model (the Canvas
//!   granular set collapses to one gate per context): course contexts need a
//!   teacher/ta/designer enrollment, account contexts an `account_users` row
//!   (Canvas account admins), group contexts the parent course's
//!   teachers/ta/designers, and user contexts the user themselves (Canvas
//!   `User#set_policy` grants the manage_course_content_* rights to self).
//!   Global admins pass everywhere.
//! - Course contexts attach migrations through the 0062 `course_id` column;
//!   account/group/user contexts use the polymorphic `context_type` /
//!   `context_id` columns added by migration 0126 (Canvas stores the class
//!   name in `context_type`).

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::content_migrations::{ensure_course, is_admin, manages_course, pagination, ts};

const ISSUE_TYPES: [&str; 3] = ["todo", "warning", "error"];

const ISSUE_SELECT: &str = "SELECT id, content_migration_id, description, \
        workflow_state, fix_issue_html_url, issue_type, error_message, \
        issue_link, fixable_by_web_upload, fixable_by_admin, created_at, updated_at \
     FROM migration_issues";

#[derive(Debug, Clone, sqlx::FromRow)]
struct IssueRow {
    id: i64,
    content_migration_id: i64,
    description: Option<String>,
    workflow_state: String,
    fix_issue_html_url: Option<String>,
    issue_type: String,
    error_message: Option<String>,
    issue_link: Option<String>,
    fixable_by_web_upload: bool,
    fixable_by_admin: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Build the Canvas `migration_issue_json`. `context_id` is the id from the
/// request path (course/account/group/user): Canvas always renders the
/// course-shaped URL with it, whatever the context type.
fn issue_json(row: &IssueRow, context_id: i64, include_errors: bool) -> Value {
    let mut json = json!({
        "id": row.id,
        "content_migration_id": row.content_migration_id,
        "content_migration_url": format!(
            "/api/v1/courses/{context_id}/content_migrations/{}",
            row.content_migration_id
        ),
        "description": row.description,
        "workflow_state": row.workflow_state,
        "fix_issue_html_url": row.fix_issue_html_url,
        "issue_type": row.issue_type,
        "issue_link": row.issue_link,
        "fixable_by_web_upload": row.fixable_by_web_upload,
        "fixable_by_admin": row.fixable_by_admin,
        "unresolved": row.workflow_state == "active",
        "created_at": ts(&row.created_at),
        "updated_at": ts(&row.updated_at),
    });
    // Canvas exposes the site-admin error message to `:read_errors` holders.
    if include_errors {
        json["error_message"] = json!(row.error_message);
    }
    json
}

// ---------------------------------------------------------------------------
// Context resolution (Canvas `require_context` / `require_content_migration`)
// ---------------------------------------------------------------------------

/// The non-course contexts of the Canvas migration_issues route block.
#[derive(Clone, Copy)]
enum Ctx {
    Account,
    Group,
    User,
}

impl Ctx {
    /// Canvas polymorphic `context_type` values (Ruby class names).
    fn context_type(self) -> &'static str {
        match self {
            Ctx::Account => "Account",
            Ctx::Group => "Group",
            Ctx::User => "User",
        }
    }
}

/// 404 unless the context exists (Canvas `require_context` resolving through
/// `api_find(Account.active)` / `api_find(Group.active)` / `api_find(User.active)`).
async fn ensure_context(db: &PgPool, ctx: Ctx, context_id: i64) -> Result<(), ApiError> {
    let sql = match ctx {
        Ctx::Account => "SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted'",
        Ctx::Group => "SELECT 1 FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
        Ctx::User => "SELECT 1 FROM users WHERE id = $1",
    };
    let row: Option<(i32,)> = sqlx::query_as(sql)
        .bind(context_id)
        .fetch_optional(db)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// The workspace's stand-in for the Canvas granular manage-course-content
/// set (any-of `manage_course_content_add/edit/delete`, module docs):
/// account admins for Account contexts, the parent course's
/// teachers/ta/designers for Group contexts, the user themselves for User
/// contexts; global admins pass everywhere.
async fn manages_context(
    db: &PgPool,
    ctx: Ctx,
    context_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let sql = match ctx {
        Ctx::Account => {
            "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 LIMIT 1"
        }
        Ctx::Group => {
            "SELECT 1 FROM enrollments e JOIN groups g ON g.course_id = e.course_id \
             WHERE g.id = $1 AND e.user_id = $2 \
               AND e.role IN ('teacher', 'ta', 'designer') LIMIT 1"
        }
        Ctx::User => return Ok(auth.id == context_id),
    };
    let row: Option<(i32,)> = sqlx::query_as(sql)
        .bind(context_id)
        .bind(auth.id)
        .fetch_optional(db)
        .await?;
    Ok(row.is_some())
}

/// 404 unless the migration belongs to the course (Canvas
/// `require_content_migration` over `course.content_migrations`).
async fn ensure_course_migration(db: &PgPool, course_id: i64, id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM content_migrations WHERE id = $1 AND course_id = $2")
            .bind(id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// 404 unless the migration belongs to the account/group/user context
/// (Canvas `require_content_migration` over the polymorphic
/// `context.content_migrations` association; migration 0126 columns).
async fn ensure_ctx_migration(
    db: &PgPool,
    ctx: Ctx,
    context_id: i64,
    id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM content_migrations \
         WHERE id = $1 AND context_type = $2 AND context_id = $3",
    )
    .bind(id)
    .bind(ctx.context_type())
    .bind(context_id)
    .fetch_optional(db)
    .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Shared actions (Canvas index/show/create/update)
// ---------------------------------------------------------------------------

async fn find_issue(db: &PgPool, migration_id: i64, id: i64) -> Result<IssueRow, ApiError> {
    let sql = format!("{ISSUE_SELECT} WHERE id = $1 AND content_migration_id = $2");
    sqlx::query_as(&sql)
        .bind(id)
        .bind(migration_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Canvas `index`: the migration's issues ordered by created_at
/// (`by_created_at`), paginated with the workspace defaults.
async fn issues_page(
    db: &PgPool,
    migration_id: i64,
    context_id: i64,
    params: &HashMap<String, String>,
    auth: &AuthUser,
) -> Result<Json<Value>, ApiError> {
    let (per_page, offset) = pagination(params);
    let sql = format!(
        "{ISSUE_SELECT} WHERE content_migration_id = $1 \
         ORDER BY created_at, id LIMIT $2 OFFSET $3"
    );
    let rows: Vec<IssueRow> = sqlx::query_as(&sql)
        .bind(migration_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(db)
        .await?;

    let include_errors = is_admin(auth);
    Ok(Json(Value::Array(
        rows.iter()
            .map(|row| issue_json(row, context_id, include_errors))
            .collect(),
    )))
}

/// Canvas `show`: a single issue scoped to the migration (404 otherwise).
async fn issue_payload(
    db: &PgPool,
    migration_id: i64,
    issue_id: i64,
    context_id: i64,
    auth: &AuthUser,
) -> Result<Json<Value>, ApiError> {
    let row = find_issue(db, migration_id, issue_id).await?;
    Ok(Json(issue_json(&row, context_id, is_admin(auth))))
}

fn json_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(text)) => {
            matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "true" | "t" | "1" | "yes" | "on"
            )
        }
        Some(Value::Number(number)) => number.as_f64().map(|v| v != 0.0).unwrap_or(false),
        _ => false,
    }
}

fn json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Workspace `create` (Canvas has no create action; see module docs).
async fn insert_issue(
    db: &PgPool,
    migration_id: i64,
    context_id: i64,
    payload: &Value,
    auth: &AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let issue_type = json_string(payload.get("issue_type")).unwrap_or_else(|| "error".to_string());
    if !ISSUE_TYPES.contains(&issue_type.as_str()) {
        return Err(ApiError::Validation("Invalid issue_type".to_string()));
    }
    let workflow_state = json_string(payload.get("workflow_state"))
        .filter(|state| state == "active" || state == "resolved")
        .unwrap_or_else(|| "active".to_string());

    let issue_id: i64 = sqlx::query_scalar(
        "INSERT INTO migration_issues \
         (content_migration_id, description, workflow_state, fix_issue_html_url, \
          issue_type, error_message, issue_link, fixable_by_web_upload, fixable_by_admin) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(migration_id)
    .bind(json_string(payload.get("description")))
    .bind(workflow_state)
    .bind(json_string(payload.get("fix_issue_html_url")))
    .bind(issue_type)
    .bind(json_string(payload.get("error_message")))
    .bind(json_string(payload.get("issue_link")))
    .bind(json_bool(payload.get("fixable_by_web_upload")))
    .bind(json_bool(payload.get("fixable_by_admin")))
    .fetch_one(db)
    .await?;

    let row = find_issue(db, migration_id, issue_id).await?;
    // Canvas renders created resources with HTTP 200.
    Ok((
        StatusCode::OK,
        Json(issue_json(&row, context_id, is_admin(auth))),
    ))
}

/// Canvas `update`: only the workflow_state can change; an invalid value is
/// 403 with Canvas' message.
async fn change_issue(
    db: &PgPool,
    migration_id: i64,
    issue_id: i64,
    context_id: i64,
    payload: &Value,
    auth: &AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let _ = find_issue(db, migration_id, issue_id).await?;

    let workflow_state = json_string(payload.get("workflow_state")).unwrap_or_default();
    if workflow_state != "active" && workflow_state != "resolved" {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Must send a valid workflow state" })),
        ));
    }

    sqlx::query(
        "UPDATE migration_issues SET workflow_state = $1, updated_at = now() WHERE id = $2",
    )
    .bind(&workflow_state)
    .bind(issue_id)
    .execute(db)
    .await?;

    let row = find_issue(db, migration_id, issue_id).await?;
    Ok((
        StatusCode::OK,
        Json(issue_json(&row, context_id, is_admin(auth))),
    ))
}

// ---------------------------------------------------------------------------
// Course-context handlers (Canvas course block of the route set)
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/content_migrations/{id}/migration_issues — Canvas
/// `index`. Check order follows the Canvas before_actions: context 404,
/// migration 404, then the permission 403.
async fn list_issues(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    ensure_course_migration(&state.db, course_id, id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    issues_page(&state.db, id, course_id, &params, &auth).await
}

/// POST /courses/{courseId}/content_migrations/{id}/migration_issues —
/// Canvas has no create action (see module docs).
async fn create_issue(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    ensure_course_migration(&state.db, course_id, id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    insert_issue(&state.db, id, course_id, &payload, &auth).await
}

/// GET /courses/{courseId}/content_migrations/{id}/migration_issues/{issueId}
/// — Canvas `show`, scoped to the migration.
async fn show_issue(
    State(state): State<AppState>,
    Path((course_id, migration_id, issue_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    ensure_course_migration(&state.db, course_id, migration_id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    issue_payload(&state.db, migration_id, issue_id, course_id, &auth).await
}

/// PUT /courses/{courseId}/content_migrations/{id}/migration_issues/{issueId}
/// — Canvas `update`: only the workflow_state can change.
async fn update_issue(
    State(state): State<AppState>,
    Path((course_id, migration_id, issue_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    ensure_course_migration(&state.db, course_id, migration_id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    change_issue(
        &state.db,
        migration_id,
        issue_id,
        course_id,
        &payload,
        &auth,
    )
    .await
}

// ---------------------------------------------------------------------------
// Account / group / user context handlers (the remaining Canvas blocks)
// ---------------------------------------------------------------------------

macro_rules! ctx_handlers {
    ($list:ident, $create:ident, $show:ident, $update:ident, $ctx:expr) => {
        async fn $list(
            State(state): State<AppState>,
            Path((context_id, id)): Path<(i64, i64)>,
            Query(params): Query<HashMap<String, String>>,
            auth: AuthUser,
        ) -> Result<Json<Value>, ApiError> {
            ensure_context(&state.db, $ctx, context_id).await?;
            ensure_ctx_migration(&state.db, $ctx, context_id, id).await?;
            if !manages_context(&state.db, $ctx, context_id, &auth).await? {
                return Err(ApiError::Forbidden);
            }
            issues_page(&state.db, id, context_id, &params, &auth).await
        }

        async fn $create(
            State(state): State<AppState>,
            Path((context_id, id)): Path<(i64, i64)>,
            auth: AuthUser,
            body: Option<Json<Value>>,
        ) -> Result<(StatusCode, Json<Value>), ApiError> {
            ensure_context(&state.db, $ctx, context_id).await?;
            ensure_ctx_migration(&state.db, $ctx, context_id, id).await?;
            if !manages_context(&state.db, $ctx, context_id, &auth).await? {
                return Err(ApiError::Forbidden);
            }
            let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
            insert_issue(&state.db, id, context_id, &payload, &auth).await
        }

        async fn $show(
            State(state): State<AppState>,
            Path((context_id, migration_id, issue_id)): Path<(i64, i64, i64)>,
            auth: AuthUser,
        ) -> Result<Json<Value>, ApiError> {
            ensure_context(&state.db, $ctx, context_id).await?;
            ensure_ctx_migration(&state.db, $ctx, context_id, migration_id).await?;
            if !manages_context(&state.db, $ctx, context_id, &auth).await? {
                return Err(ApiError::Forbidden);
            }
            issue_payload(&state.db, migration_id, issue_id, context_id, &auth).await
        }

        async fn $update(
            State(state): State<AppState>,
            Path((context_id, migration_id, issue_id)): Path<(i64, i64, i64)>,
            auth: AuthUser,
            body: Option<Json<Value>>,
        ) -> Result<(StatusCode, Json<Value>), ApiError> {
            ensure_context(&state.db, $ctx, context_id).await?;
            ensure_ctx_migration(&state.db, $ctx, context_id, migration_id).await?;
            if !manages_context(&state.db, $ctx, context_id, &auth).await? {
                return Err(ApiError::Forbidden);
            }
            let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
            change_issue(
                &state.db,
                migration_id,
                issue_id,
                context_id,
                &payload,
                &auth,
            )
            .await
        }
    };
}

ctx_handlers!(
    list_account_issues,
    create_account_issue,
    show_account_issue,
    update_account_issue,
    Ctx::Account
);
ctx_handlers!(
    list_group_issues,
    create_group_issue,
    show_group_issue,
    update_group_issue,
    Ctx::Group
);
ctx_handlers!(
    list_user_issues,
    create_user_issue,
    show_user_issue,
    update_user_issue,
    Ctx::User
);

/// Build the migration_issues router: the Canvas route block for each of
/// account, course, group and user contexts.
pub fn router() -> Router<AppState> {
    Router::new()
        // Canvas course context block
        .route(
            "/courses/{courseId}/content_migrations/{id}/migration_issues",
            get(list_issues).post(create_issue),
        )
        .route(
            "/courses/{courseId}/content_migrations/{id}/migration_issues/{issueId}",
            get(show_issue).put(update_issue),
        )
        // Canvas account context block
        .route(
            "/accounts/{accountId}/content_migrations/{id}/migration_issues",
            get(list_account_issues).post(create_account_issue),
        )
        .route(
            "/accounts/{accountId}/content_migrations/{id}/migration_issues/{issueId}",
            get(show_account_issue).put(update_account_issue),
        )
        // Canvas group context block
        .route(
            "/groups/{groupId}/content_migrations/{id}/migration_issues",
            get(list_group_issues).post(create_group_issue),
        )
        .route(
            "/groups/{groupId}/content_migrations/{id}/migration_issues/{issueId}",
            get(show_group_issue).put(update_group_issue),
        )
        // Canvas user context block
        .route(
            "/users/{userId}/content_migrations/{id}/migration_issues",
            get(list_user_issues).post(create_user_issue),
        )
        .route(
            "/users/{userId}/content_migrations/{id}/migration_issues/{issueId}",
            get(show_user_issue).put(update_user_issue),
        )
}
