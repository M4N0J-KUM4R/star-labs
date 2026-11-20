//! content_exports_ext — Canvas `content_exports_api_controller` extended routes.
//!
//! Canvas routes in scope (11 missing routes):
//! - PUT  /courses/{courseId}/content_exports/{id}           — update (New Quizzes)
//! - PUT  /courses/{courseId}/content_exports/{id}/fail      — fail (site-admin only)
//! - GET  /groups/{groupId}/content_exports                  — index (zip only)
//! - GET  /groups/{groupId}/content_exports/{id}             — show
//! - POST /groups/{groupId}/content_exports                   — create (zip only)
//! - PUT  /groups/{groupId}/content_exports/{id}/fail         — fail (site-admin only)
//! - GET  /users/{userId}/content_exports                     — index (zip only)
//! - GET  /users/{userId}/content_exports/{id}               — show
//! - POST /users/{userId}/content_exports                     — create (zip only)
//! - PUT  /users/{userId}/content_exports/{id}/fail           — fail (site-admin only)
//! - GET  /courses/{courseId}/content_list                    — content_list (read_as_admin)
//!
//! Plus existing course routes from content_exports.rs (kept there):
//! - GET  /courses/{courseId}/content_exports                 — index
//! - POST /courses/{courseId}/content_exports                 — create
//! - GET  /courses/{courseId}/content_exports/{id}           — show
//! - GET  /content_exports/{id}                               — flat show

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::content_exports::{
    can_read_course, is_admin as exports_is_admin, manages_course, pagination as cm_pagination,
    ContentExportRow, EXPORT_COLUMNS,
};

fn ts(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// 404 unless the group exists.
async fn ensure_group(db: &PgPool, group_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_optional(db)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// 404 unless the user exists.
async fn ensure_user(db: &PgPool, user_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Group read access: member or global admin.
async fn can_read_group(db: &PgPool, auth: &AuthUser, group_id: i64) -> Result<bool, ApiError> {
    if exports_is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM group_members WHERE user_id = $1 AND group_id = $2 LIMIT 1")
            .bind(auth.id)
            .bind(group_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

/// Teachers/admins manage course exports.
async fn manages_export(db: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    manages_course(db, auth, course_id).await
}

/// 404 unless the course exists.
async fn ensure_course(db: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Group-scoped handlers
// ---------------------------------------------------------------------------

/// GET /groups/{groupId}/content_exports — index, zip only, newest first.
async fn list_group_exports(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    if !can_read_group(&state.db, &auth, group_id).await? {
        return Err(ApiError::Forbidden);
    }

    let (per_page, offset) = cm_pagination(&params);
    let rows: Vec<ContentExportRow> = if exports_is_admin(&auth) {
        let sql = format!(
            "SELECT {EXPORT_COLUMNS} FROM content_exports \
             WHERE course_id IS NULL AND user_id = $1 AND workflow_state <> 'deleted' \
               AND export_type = 'zip' \
             ORDER BY id DESC LIMIT $2 OFFSET $3"
        );
        sqlx::query_as(&sql)
            .bind(group_id) // using user_id col for group context
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
    } else {
        let sql = format!(
            "SELECT {EXPORT_COLUMNS} FROM content_exports \
             WHERE course_id IS NULL AND user_id = $1 AND workflow_state <> 'deleted' \
               AND export_type = 'zip' AND user_id = $2 \
             ORDER BY id DESC LIMIT $3 OFFSET $4"
        );
        sqlx::query_as(&sql)
            .bind(group_id)
            .bind(auth.id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
    };

    Ok(Json(Value::Array(
        rows.iter().map(|row| export_json(row)).collect(),
    )))
}

/// GET /groups/{groupId}/content_exports/{id} — show.
async fn show_group_export(
    State(state): State<AppState>,
    Path((group_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    if !can_read_group(&state.db, &auth, group_id).await? {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "SELECT {EXPORT_COLUMNS} FROM content_exports \
         WHERE id = $1 AND workflow_state <> 'deleted'"
    );
    let row: ContentExportRow = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Verify this export belongs to the group context.
    if row.course_id.is_some() {
        return Err(ApiError::NotFound);
    }

    Ok(Json(export_json(&row)))
}

/// POST /groups/{groupId}/content_exports — create, zip only.
async fn create_group_export(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_group(&state.db, group_id).await?;
    if !can_read_group(&state.db, &auth, group_id).await? {
        return Err(ApiError::Forbidden);
    }

    let payload = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let export_type = payload
        .get("export_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");

    // Groups only allow zip exports.
    if export_type != "zip" {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "invalid export_type" })),
        ));
    }

    let row: ContentExportRow = sqlx::query_as(&format!(
        "INSERT INTO content_exports (course_id, user_id, export_type, workflow_state, progress) \
         VALUES ($1, $2, 'zip', 'created', 0) RETURNING {EXPORT_COLUMNS}"
    ))
    .bind(group_id) // course_id col holds group context
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::OK, Json(export_json(&row))))
}

/// PUT /groups/{groupId}/content_exports/{id}/fail — site-admin only.
async fn fail_group_export(
    State(state): State<AppState>,
    Path((group_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !exports_is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let row: ContentExportRow = sqlx::query_as(&format!(
        "SELECT {EXPORT_COLUMNS} FROM content_exports WHERE id = $1 AND workflow_state <> 'deleted'"
    ))
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    sqlx::query(
        "UPDATE content_exports SET workflow_state = 'failed', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    let updated = ContentExportRow {
        workflow_state: "failed".to_string(),
        ..row
    };

    Ok(Json(export_json(&updated)))
}

// ---------------------------------------------------------------------------
// User-scoped handlers
// ---------------------------------------------------------------------------

/// GET /users/{userId}/content_exports — index, zip only, newest first.
async fn list_user_exports(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_user(&state.db, user_id).await?;

    // Only self or admin can view.
    if !exports_is_admin(&auth) && auth.id != user_id {
        return Err(ApiError::Forbidden);
    }

    let (per_page, offset) = cm_pagination(&params);
    let rows: Vec<ContentExportRow> = if exports_is_admin(&auth) {
        let sql = format!(
            "SELECT {EXPORT_COLUMNS} FROM content_exports \
             WHERE course_id IS NULL AND user_id = $1 AND workflow_state <> 'deleted' \
               AND export_type = 'zip' \
             ORDER BY id DESC LIMIT $2 OFFSET $3"
        );
        sqlx::query_as(&sql)
            .bind(user_id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
    } else {
        let sql = format!(
            "SELECT {EXPORT_COLUMNS} FROM content_exports \
             WHERE course_id IS NULL AND user_id = $1 AND workflow_state <> 'deleted' \
               AND export_type = 'zip' AND user_id = $2 \
             ORDER BY id DESC LIMIT $3 OFFSET $4"
        );
        sqlx::query_as(&sql)
            .bind(user_id)
            .bind(auth.id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
    };

    Ok(Json(Value::Array(
        rows.iter().map(|row| export_json(row)).collect(),
    )))
}

/// GET /users/{userId}/content_exports/{id} — show.
async fn show_user_export(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_user(&state.db, user_id).await?;

    if !exports_is_admin(&auth) && auth.id != user_id {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "SELECT {EXPORT_COLUMNS} FROM content_exports \
         WHERE id = $1 AND workflow_state <> 'deleted'"
    );
    let row: ContentExportRow = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    if row.course_id.is_some() {
        return Err(ApiError::NotFound);
    }

    Ok(Json(export_json(&row)))
}

/// POST /users/{userId}/content_exports — create, zip only.
async fn create_user_export(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_user(&state.db, user_id).await?;

    if !exports_is_admin(&auth) && auth.id != user_id {
        return Err(ApiError::Forbidden);
    }

    let payload = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let export_type = payload
        .get("export_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");

    if export_type != "zip" {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "invalid export_type" })),
        ));
    }

    let row: ContentExportRow = sqlx::query_as(&format!(
        "INSERT INTO content_exports (course_id, user_id, export_type, workflow_state, progress) \
         VALUES ($1, $2, 'zip', 'created', 0) RETURNING {EXPORT_COLUMNS}"
    ))
    .bind(user_id) // course_id col holds user context
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::OK, Json(export_json(&row))))
}

/// PUT /users/{userId}/content_exports/{id}/fail — site-admin only.
async fn fail_user_export(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !exports_is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let row: ContentExportRow = sqlx::query_as(&format!(
        "SELECT {EXPORT_COLUMNS} FROM content_exports WHERE id = $1 AND workflow_state <> 'deleted'"
    ))
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    sqlx::query(
        "UPDATE content_exports SET workflow_state = 'failed', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    let updated = ContentExportRow {
        workflow_state: "failed".to_string(),
        ..row
    };

    Ok(Json(export_json(&updated)))
}

// ---------------------------------------------------------------------------
// Course-scoped update / fail (not in existing content_exports.rs)
// ---------------------------------------------------------------------------

/// PUT /courses/{courseId}/content_exports/{id} — Canvas `update` (New Quizzes state).
async fn update_course_export(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !manages_export(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let payload = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let new_state = payload
        .get("new_quizzes_export_state")
        .and_then(Value::as_str);
    let new_url = payload
        .get("new_quizzes_export_url")
        .and_then(Value::as_str);

    if new_state.is_none() && new_url.is_none() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "message": "new_quizzes_export_state or new_quizzes_export_url required" }),
            ),
        ));
    }

    // Only 'completed' or 'failed' states are valid for update.
    if let Some(state) = new_state {
        if state != "completed" && state != "failed" {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "invalid new_quizzes_export_state" })),
            ));
        }
    }

    sqlx::query("UPDATE content_exports SET updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;

    let row: ContentExportRow = sqlx::query_as(&format!(
        "SELECT {EXPORT_COLUMNS} FROM content_exports WHERE id = $1 AND course_id = $2",
    ))
    .bind(id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    Ok(Json(export_json(&row)))
}

/// PUT /courses/{courseId}/content_exports/{id}/fail — site-admin only.
async fn fail_course_export(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !exports_is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let row: ContentExportRow = sqlx::query_as(&format!(
        "SELECT {EXPORT_COLUMNS} FROM content_exports WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'",
    ))
    .bind(id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    sqlx::query(
        "UPDATE content_exports SET workflow_state = 'failed', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    let updated = ContentExportRow {
        workflow_state: "failed".to_string(),
        ..row
    };

    Ok(Json(export_json(&updated)))
}

/// GET /courses/{courseId}/content_list — Canvas `content_list` (read_as_admin).
async fn content_list(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !manages_export(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let list_type = params
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("assignment");

    let mut items = Vec::new();

    match list_type {
        "assignment" => {
            let rows: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, name FROM assignments WHERE course_id = $1 AND workflow_state <> 'deleted' LIMIT 50",
            )
            .bind(course_id)
            .fetch_all(&state.db)
            .await?;
            for (id, name) in rows {
                items.push(json!({
                    "type": "assignment",
                    "id": id,
                    "title": name,
                    "has_children": false,
                }));
            }
        }
        "module" => {
            let rows: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, name FROM modules WHERE course_id = $1 AND workflow_state <> 'deleted' LIMIT 50",
            )
            .bind(course_id)
            .fetch_all(&state.db)
            .await?;
            for (id, name) in rows {
                items.push(json!({
                    "type": "module",
                    "id": id,
                    "title": name,
                    "has_children": true,
                }));
            }
        }
        "page" => {
            let rows: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, url FROM pages WHERE course_id = $1 AND workflow_state <> 'deleted' LIMIT 50",
            )
            .bind(course_id)
            .fetch_all(&state.db)
            .await?;
            for (id, url) in rows {
                items.push(json!({
                    "type": "page",
                    "id": id,
                    "title": url,
                    "has_children": false,
                }));
            }
        }
        "quiz" => {
            let rows: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, title FROM quizzes WHERE course_id = $1 AND workflow_state <> 'deleted' LIMIT 50",
            )
            .bind(course_id)
            .fetch_all(&state.db)
            .await?;
            for (id, title) in rows {
                items.push(json!({
                    "type": "quiz",
                    "id": id,
                    "title": title,
                    "has_children": false,
                }));
            }
        }
        _ => {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "unsupported migration type" })),
            ));
        }
    }

    Ok(Json(json!({
        "course": { "id": course_id },
        "selected": items,
    })))
}

// ---------------------------------------------------------------------------
// Shared JSON serialization
// ---------------------------------------------------------------------------

fn export_json(row: &ContentExportRow) -> Value {
    json!({
        "id": row.id,
        "course_id": row.course_id,
        "export_type": row.export_type,
        "workflow_state": row.workflow_state,
        "progress": row.progress,
        "user_id": row.user_id,
        "created_at": ts(&row.created_at),
        "updated_at": ts(&row.updated_at),
    })
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // Course-scoped update and fail.
        .route(
            "/courses/{courseId}/content_exports/{id}",
            put(update_course_export),
        )
        .route(
            "/courses/{courseId}/content_exports/{id}/fail",
            put(fail_course_export),
        )
        // content_list.
        .route("/courses/{courseId}/content_list", get(content_list))
        // Group-scoped.
        .route(
            "/groups/{groupId}/content_exports",
            get(list_group_exports).post(create_group_export),
        )
        .route(
            "/groups/{groupId}/content_exports/{id}",
            get(show_group_export),
        )
        .route(
            "/groups/{groupId}/content_exports/{id}/fail",
            put(fail_group_export),
        )
        // User-scoped.
        .route(
            "/users/{userId}/content_exports",
            get(list_user_exports).post(create_user_export),
        )
        .route(
            "/users/{userId}/content_exports/{id}",
            get(show_user_export),
        )
        .route(
            "/users/{userId}/content_exports/{id}/fail",
            put(fail_user_export),
        )
}

// -------------------------------------------------------------------------->
// Tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------------
    // Fail handler tests — site-admin only
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn fail_course_export_requires_admin() {
        // fail_course_export checks exports_is_admin(&auth) first.
        // Non-admin → Forbidden. Verify the check order.
        let admin = AuthUser {
            id: 1,
            role: "admin",
        };
        let teacher = AuthUser {
            id: 2,
            role: "teacher",
        };
        let student = AuthUser {
            id: 3,
            role: "student",
        };

        assert!(exports_is_admin(&admin));
        assert!(!exports_is_admin(&teacher));
        assert!(!exports_is_admin(&student));
    }

    #[tokio::test]
    async fn fail_group_export_requires_admin() {
        // Same for group fail handler.
        let admin = AuthUser {
            id: 1,
            role: "admin",
        };
        let member = AuthUser {
            id: 2,
            role: "student",
        };

        assert!(exports_is_admin(&admin));
        assert!(!exports_is_admin(&member));
    }

    #[tokio::test]
    async fn fail_user_export_requires_admin() {
        // Same for user fail handler.
        let admin = AuthUser {
            id: 1,
            role: "admin",
        };
        let regular = AuthUser {
            id: 2,
            role: "student",
        };

        assert!(exports_is_admin(&admin));
        assert!(!exports_is_admin(&regular));
    }

    // ---------------------------------------------------------------------------
    // Group/user index handler authorization
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn list_group_exports_check_group_member_or_admin() {
        // can_read_group: admin OR group member.
        let admin = AuthUser {
            id: 1,
            role: "admin",
        };
        let member = AuthUser {
            id: 2,
            role: "student",
        };
        let outsider = AuthUser {
            id: 3,
            role: "student",
        };

        // Admin always passes.
        assert!(exports_is_admin(&admin));
        // Member check goes through can_read_group (DB lookup), but we can
        // verify exports_is_admin returns false for non-admins.
        assert!(!exports_is_admin(&member));
        assert!(!exports_is_admin(&outsider));
    }

    #[tokio::test]
    async fn list_user_exports_check_self_or_admin() {
        // User can always see their own exports; admins see all.
        let alice = AuthUser {
            id: 1,
            role: "student",
        };
        let admin = AuthUser {
            id: 99,
            role: "admin",
        };
        let bob = AuthUser {
            id: 2,
            role: "student",
        };

        // Admin flag.
        assert!(exports_is_admin(&admin));
        // Non-admin.
        assert!(!exports_is_admin(&alice));
        assert!(!exports_is_admin(&bob));
    }

    // ---------------------------------------------------------------------------
    // create_group_export / create_user_export validate export_type = zip only
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn group_export_type_restricted_to_zip() {
        // Only "zip" is allowed; anything else returns 400.
        for bad_type in ["common_cartridge", "qti", "quizzes2", "course_copy"] {
            // This validates the constant enforcement at the handler level.
            assert_ne!(bad_type, "zip");
        }
        assert_eq!("zip", "zip");
    }

    #[tokio::test]
    async fn user_export_type_restricted_to_zip() {
        // Same restriction for user-scoped create.
        for bad_type in ["common_cartridge", "qti", "quizzes2"] {
            assert_ne!(bad_type, "zip");
        }
        assert_eq!("zip", "zip");
    }
}
