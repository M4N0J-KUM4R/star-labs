//! Handlers for LTI tool proxy endpoints.
//!
//! Canvas controllers:
//!   `lti/tool_proxy_controller`        — 4 context-scoped routes per context type
//!   `lti/ims/tool_proxy_controller`    — 3 routes
//!
//! Routes:
//!   DELETE /courses/:course_id/tool_proxies/:tool_proxy_id
//!   PUT    /courses/:course_id/tool_proxies/:tool_proxy_id
//!   PUT    /courses/:course_id/tool_proxies/:tool_proxy_id/update
//!   DELETE /courses/:course_id/tool_proxies/:tool_proxy_id/update
//!   DELETE /accounts/:account_id/tool_proxies/:tool_proxy_id
//!   PUT    /accounts/:account_id/tool_proxies/:tool_proxy_id
//!   PUT    /accounts/:account_id/tool_proxies/:tool_proxy_id/update
//!   DELETE /accounts/:account_id/tool_proxies/:tool_proxy_id/update
//!   GET    /tool_proxy/:tool_proxy_guid
//!   POST   /courses/:course_id/tool_proxy
//!   POST   /accounts/:account_id/tool_proxy

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{can_manage_account, is_admin};

/// Row type for `lti_tool_proxies`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ToolProxyRow {
    guid: String,
    developer_key_id: Option<i64>,
    raw_data: Value,
}

const TOOL_PROXY_COLUMNS: &str = "guid, developer_key_id, raw_data";

/// Check whether the user can manage LTI tools in the given context.
async fn can_manage_tool_proxy(
    pool: &PgPool,
    auth: &AuthUser,
    context_type: &str,
    context_id: i64,
) -> Result<(), ApiError> {
    if is_admin(auth) {
        return Ok(());
    }
    match context_type {
        "Account" => {
            if can_manage_account(pool, auth, context_id).await? {
                Ok(())
            } else {
                Err(ApiError::Forbidden)
            }
        }
        "Course" => {
            // For courses, check if user is a teacher or account admin.
            let row: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
                    .bind(context_id)
                    .fetch_optional(pool)
                    .await?;
            if let Some((Some(account_id),)) = row {
                if can_manage_account(pool, auth, account_id).await? {
                    return Ok(());
                }
            }
            // Check if teacher in course
            let row: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 AND role = 'teacher' LIMIT 1",
            )
            .bind(context_id)
            .bind(auth.id)
            .fetch_optional(pool)
            .await?;
            if row.is_some() {
                Ok(())
            } else {
                Err(ApiError::Forbidden)
            }
        }
        _ => Err(ApiError::Forbidden),
    }
}

/// Look up a tool proxy by its id in a context.
async fn find_tool_proxy_in_context(
    pool: &PgPool,
    tool_proxy_id: i64,
    context_type: &str,
    context_id: i64,
) -> Result<ToolProxyRow, ApiError> {
    let sql = format!(
        "SELECT {TOOL_PROXY_COLUMNS} FROM lti_tool_proxies \
         WHERE id = $1 AND context_type = $2 AND context_id = $3"
    );
    let row: Option<ToolProxyRow> = sqlx::query_as(&sql)
        .bind(tool_proxy_id)
        .bind(context_type)
        .bind(context_id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// Look up a tool proxy by its GUID.
async fn find_tool_proxy_by_guid(pool: &PgPool, guid: &str) -> Result<ToolProxyRow, ApiError> {
    let row: Option<ToolProxyRow> = sqlx::query_as(
        format!("SELECT {TOOL_PROXY_COLUMNS} FROM lti_tool_proxies WHERE guid = $1").as_str(),
    )
    .bind(guid)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // lti/tool_proxy context-scoped (course)
        .route(
            "/courses/{course_id}/tool_proxies/{tool_proxy_id}",
            put(update_tool_proxy_course).delete(delete_tool_proxy_course),
        )
        .route(
            "/courses/{course_id}/tool_proxies/{tool_proxy_id}/update",
            put(accept_tool_proxy_update_course).delete(dismiss_tool_proxy_update_course),
        )
        // lti/tool_proxy context-scoped (account)
        .route(
            "/accounts/{account_id}/tool_proxies/{tool_proxy_id}",
            put(update_tool_proxy_account).delete(delete_tool_proxy_account),
        )
        .route(
            "/accounts/{account_id}/tool_proxies/{tool_proxy_id}/update",
            put(accept_tool_proxy_update_account).delete(dismiss_tool_proxy_update_account),
        )
        // lti/ims/tool_proxy
        .route("/tool_proxy/{tool_proxy_guid}", get(show_tool_proxy_guid))
        .route(
            "/courses/{course_id}/tool_proxy",
            post(create_tool_proxy_course),
        )
        .route(
            "/accounts/{account_id}/tool_proxy",
            post(create_tool_proxy_account),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `PUT /courses/{course_id}/tool_proxies/{tool_proxy_id}` — update workflow state.
async fn update_tool_proxy_course(
    State(state): State<AppState>,
    Path((course_id, tool_proxy_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Course", course_id).await?;
    let _ = find_tool_proxy_in_context(&state.db, tool_proxy_id, "Course", course_id).await?;

    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE lti_tool_proxies SET workflow_state = $2, updated_at = now() \
         WHERE id = $1 RETURNING workflow_state",
    )
    .bind(tool_proxy_id)
    .bind("active")
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some((workflow_state,)) => Ok(Json(json!({ "workflow_state": workflow_state }))),
        None => Err(ApiError::NotFound),
    }
}

/// `DELETE /courses/{course_id}/tool_proxies/{tool_proxy_id}` — soft delete.
async fn delete_tool_proxy_course(
    State(state): State<AppState>,
    Path((course_id, tool_proxy_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Course", course_id).await?;

    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE lti_tool_proxies SET workflow_state = 'deleted', updated_at = now() \
         WHERE id = $1 AND context_type = 'Course' AND context_id = $2 \
         RETURNING workflow_state",
    )
    .bind(tool_proxy_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some((workflow_state,)) => Ok(Json(json!({ "workflow_state": workflow_state }))),
        None => Err(ApiError::NotFound),
    }
}

/// `PUT /courses/{course_id}/tool_proxies/{tool_proxy_id}/update` — accept update.
async fn accept_tool_proxy_update_course(
    State(state): State<AppState>,
    Path((course_id, tool_proxy_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Course", course_id).await?;

    // Check if there is an update_payload to process.
    let row: Option<(Value,)> = sqlx::query_as(
        "SELECT update_payload FROM lti_tool_proxies \
         WHERE id = $1 AND context_type = 'Course' AND context_id = $2",
    )
    .bind(tool_proxy_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some((_payload,)) => {
            // Clear update_payload after acceptance.
            sqlx::query(
                "UPDATE lti_tool_proxies SET update_payload = NULL, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(tool_proxy_id)
            .execute(&state.db)
            .await?;
            Ok(Json(json!({ "status": "Success" })))
        }
        _ => Ok(Json(json!({ "status": "Failed" }))),
    }
}

/// `DELETE /courses/{course_id}/tool_proxies/{tool_proxy_id}/update` — dismiss update.
async fn dismiss_tool_proxy_update_course(
    State(state): State<AppState>,
    Path((course_id, tool_proxy_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Course", course_id).await?;

    sqlx::query(
        "UPDATE lti_tool_proxies SET update_payload = NULL, updated_at = now() \
         WHERE id = $1 AND context_type = 'Course' AND context_id = $2",
    )
    .bind(tool_proxy_id)
    .bind(course_id)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({ "status": "success" })))
}

/// `PUT /accounts/{account_id}/tool_proxies/{tool_proxy_id}` — update workflow state.
async fn update_tool_proxy_account(
    State(state): State<AppState>,
    Path((account_id, tool_proxy_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Account", account_id).await?;
    let _ = find_tool_proxy_in_context(&state.db, tool_proxy_id, "Account", account_id).await?;

    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE lti_tool_proxies SET workflow_state = $2, updated_at = now() \
         WHERE id = $1 RETURNING workflow_state",
    )
    .bind(tool_proxy_id)
    .bind("active")
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some((workflow_state,)) => Ok(Json(json!({ "workflow_state": workflow_state }))),
        None => Err(ApiError::NotFound),
    }
}

/// `DELETE /accounts/{account_id}/tool_proxies/{tool_proxy_id}` — soft delete.
async fn delete_tool_proxy_account(
    State(state): State<AppState>,
    Path((account_id, tool_proxy_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Account", account_id).await?;

    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE lti_tool_proxies SET workflow_state = 'deleted', updated_at = now() \
         WHERE id = $1 AND context_type = 'Account' AND context_id = $2 \
         RETURNING workflow_state",
    )
    .bind(tool_proxy_id)
    .bind(account_id)
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some((workflow_state,)) => Ok(Json(json!({ "workflow_state": workflow_state }))),
        None => Err(ApiError::NotFound),
    }
}

/// `PUT /accounts/{account_id}/tool_proxies/{tool_proxy_id}/update` — accept update.
async fn accept_tool_proxy_update_account(
    State(state): State<AppState>,
    Path((account_id, tool_proxy_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Account", account_id).await?;

    let row: Option<(Value,)> = sqlx::query_as(
        "SELECT update_payload FROM lti_tool_proxies \
         WHERE id = $1 AND context_type = 'Account' AND context_id = $2",
    )
    .bind(tool_proxy_id)
    .bind(account_id)
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some((_payload,)) => {
            sqlx::query(
                "UPDATE lti_tool_proxies SET update_payload = NULL, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(tool_proxy_id)
            .execute(&state.db)
            .await?;
            Ok(Json(json!({ "status": "Success" })))
        }
        _ => Ok(Json(json!({ "status": "Failed" }))),
    }
}

/// `DELETE /accounts/{account_id}/tool_proxies/{tool_proxy_id}/update` — dismiss update.
async fn dismiss_tool_proxy_update_account(
    State(state): State<AppState>,
    Path((account_id, tool_proxy_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Account", account_id).await?;

    sqlx::query(
        "UPDATE lti_tool_proxies SET update_payload = NULL, updated_at = now() \
         WHERE id = $1 AND context_type = 'Account' AND context_id = $2",
    )
    .bind(tool_proxy_id)
    .bind(account_id)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({ "status": "success" })))
}

/// `GET /tool_proxy/:tool_proxy_guid` — show tool proxy by GUID.
/// Canvas requires OAuth authentication via the tool proxy's shared secret.
/// In this implementation, we require a valid auth token and check the
/// developer key matches.
async fn show_tool_proxy_guid(
    State(state): State<AppState>,
    Path(tool_proxy_guid): Path<String>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let row = find_tool_proxy_by_guid(&state.db, &tool_proxy_guid).await?;

    // Verify the developer key belongs to the authenticated user or is an admin.
    if !is_admin(&auth) {
        let row: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM developer_keys WHERE id = $1 AND user_id = $2 LIMIT 1")
                .bind(row.developer_key_id)
                .bind(auth.id)
                .fetch_optional(&state.db)
                .await?;
        if row.is_none() {
            return Err(ApiError::Forbidden);
        }
    }

    Ok((
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/vnd.ims.lti.v2.toolproxy+json",
        )],
        Json(row.raw_data),
    )
        .into_response())
}

/// `POST /courses/:course_id/tool_proxy` — create a tool proxy for a course.
async fn create_tool_proxy_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<Value>,
) -> Result<Response, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Course", course_id).await?;

    let uuid = format!(
        "{:x}-{:x}",
        chrono::Utc::now().timestamp_millis(),
        rand::random::<u64>()
    );
    let raw_data = payload.clone();

    let row: ToolProxyRow = sqlx::query_as(
        format!(
            "INSERT INTO lti_tool_proxies \
             (guid, context_type, context_id, raw_data, workflow_state, developer_key_id) \
             VALUES ($1, 'Course', $2, $3, 'active', $4) \
             RETURNING {TOOL_PROXY_COLUMNS}"
        )
        .as_str(),
    )
    .bind(&uuid)
    .bind(course_id)
    .bind(&raw_data)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    let body = json!({
        "@context": "http://purl.imsglobal.org/ctx/lti/v2/ToolProxyId",
        "@type": "ToolProxy",
        "@id": null,
        "tool_proxy_guid": row.guid,
    });

    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// `POST /accounts/:account_id/tool_proxy` — create a tool proxy for an account.
async fn create_tool_proxy_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<Value>,
) -> Result<Response, ApiError> {
    can_manage_tool_proxy(&state.db, &auth, "Account", account_id).await?;

    let uuid = format!(
        "{:x}-{:x}",
        chrono::Utc::now().timestamp_millis(),
        rand::random::<u64>()
    );
    let raw_data = payload.clone();

    let row: ToolProxyRow = sqlx::query_as(
        format!(
            "INSERT INTO lti_tool_proxies \
             (guid, context_type, context_id, raw_data, workflow_state, developer_key_id) \
             VALUES ($1, 'Account', $2, $3, 'active', $4) \
             RETURNING {TOOL_PROXY_COLUMNS}"
        )
        .as_str(),
    )
    .bind(&uuid)
    .bind(account_id)
    .bind(&raw_data)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    let body = json!({
        "@context": "http://purl.imsglobal.org/ctx/lti/v2/ToolProxyId",
        "@type": "ToolProxy",
        "@id": null,
        "tool_proxy_guid": row.guid,
    });

    Ok((StatusCode::CREATED, Json(body)).into_response())
}
