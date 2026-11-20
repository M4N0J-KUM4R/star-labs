//! Canvas parity for LTI AGS Line Items (`Lti::IMS::LineItemsController`).
//!
//! Canvas source: `app/controllers/lti/ims/line_items_controller.rb`.
//!
//! Routes (mounted under `/api/lti`):
//! - `GET    /api/lti/courses/{courseId}/line_items`      — list line items
//! - `POST   /api/lti/courses/{courseId}/line_items`      — create line item
//! - `GET    /api/lti/courses/{courseId}/line_items/{id}` — show line item
//! - `PUT    /api/lti/courses/{courseId}/line_items/{id}` — update line item
//! - `DELETE /api/lti/courses/{courseId}/line_items/{id}` — delete line item
//!
//! AGS = Assignment and Grade Services. Line items map to Canvas assignments
//! with LTI-specific extensions (scoreMaximum, label, resourceId, tag).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::can_manage_account;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SUBSCRIPTION_COLUMNS: &str =
    "id, course_id, created_at, updated_at, submission_types, due_at, points_possible, \
     workflow_state, title, description, is_concluded";

const LINE_ITEM_COLUMNS: &str =
    "id, course_id, assignment_id, label, resource_id, tag, score_maximum, \
     created_at, updated_at, workflow_state";

// ---------------------------------------------------------------------------
// Domain rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct LineItemRow {
    id: i64,
    course_id: i64,
    assignment_id: Option<i64>,
    label: String,
    resource_id: Option<String>,
    tag: Option<String>,
    score_maximum: f64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    workflow_state: String,
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ListLineItemsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// Body for `POST /api/lti/courses/{courseId}/line_items`.
#[derive(Debug, Deserialize)]
pub struct CreateLineItemRequest {
    #[serde(rename = "submissionType")]
    submission_type: Option<String>,
    #[serde(rename = "dueAt")]
    due_at: Option<String>,
    #[serde(rename = "scoreMaximum")]
    score_maximum: f64,
    pub label: String,
    #[serde(rename = "resourceId")]
    resource_id: Option<String>,
    pub tag: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLineItemRequest {
    #[serde(rename = "submissionType")]
    submission_type: Option<String>,
    #[serde(rename = "dueAt")]
    due_at: Option<String>,
    #[serde(rename = "scoreMaximum")]
    score_maximum: Option<f64>,
    pub label: Option<String>,
    #[serde(rename = "resourceId")]
    resource_id: Option<String>,
    pub tag: Option<String>,
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

async fn require_course(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

async fn find_active_line_item(pool: &PgPool, id: i64) -> Result<LineItemRow, ApiError> {
    let row: Option<LineItemRow> = sqlx::query_as(&format!(
        "SELECT {LINE_ITEM_COLUMNS} FROM lti_line_items \
         WHERE id = $1 AND workflow_state <> 'deleted'"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

async fn find_line_item_in_course(
    pool: &PgPool,
    id: i64,
    course_id: i64,
) -> Result<LineItemRow, ApiError> {
    let item = find_active_line_item(pool, id).await?;
    if item.course_id != course_id {
        return Err(ApiError::NotFound);
    }
    Ok(item)
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

fn line_item_json(item: &LineItemRow) -> Value {
    json!({
        "id": item.id,
        "label": item.label,
        "resourceId": item.resource_id,
        "tag": item.tag,
        "scoreMaximum": item.score_maximum,
        "createdAt": item.created_at,
        "updatedAt": item.updated_at,
        "workflowState": item.workflow_state,
    })
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/courses/{course_id}/line_items",
            get(list_line_items).post(create_line_item),
        )
        .route(
            "/api/lti/courses/{course_id}/line_items/{id}",
            get(show_line_item)
                .put(update_line_item)
                .delete(destroy_line_item),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_line_items(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(_params): Query<ListLineItemsQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_account(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "SELECT {LINE_ITEM_COLUMNS} FROM lti_line_items \
         WHERE course_id = $1 AND workflow_state <> 'deleted' \
         ORDER BY created_at DESC"
    );
    let rows: Vec<LineItemRow> =
        sqlx::query_as(&sql).bind(course_id).fetch_all(&state.db).await?;

    let data: Vec<Value> = rows.iter().map(line_item_json).collect();
    Ok(Json(data))
}

async fn create_line_item(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateLineItemRequest>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_account(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let label = payload.label;
    if label.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, Json(json!({ "error": "label is required" })))
            .into_response());
    }

    let score_maximum = payload.score_maximum;
    if score_maximum < 0.0 {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "scoreMaximum must be non-negative" })),
        ))
            .into_response();
    }

    let resource_id = payload.resource_id;
    let tag = payload.tag;
    let now = Utc::now();

    // Create a matching Canvas assignment and get its id back.
    let assignment_title = label.clone();
    let insert_assign = sqlx::query_as::<_, (i64,)>(
        "INSERT INTO assignments (course_id, title, points_possible, workflow_state, \
         created_at, updated_at) VALUES ($1, $2, $3, 'published', $4, $4) \
         RETURNING id",
    )
    .bind(course_id)
    .bind(&assignment_title)
    .bind(score_maximum)
    .bind(now)
    .fetch_one(&state.db)
    .await;

    let assignment_id: i64 = match insert_assign {
        Ok(row) => row.0,
        Err(_) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "Failed to create Canvas assignment" })),
            ))
                .into_response();
        }
    };

    let sql = format!(
        "INSERT INTO lti_line_items \
         (course_id, assignment_id, label, resource_id, tag, score_maximum, \
          created_at, updated_at, workflow_state) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'active') \
         RETURNING {LINE_ITEM_COLUMNS}"
    );
    let row: LineItemRow = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(assignment_id)
        .bind(&label)
        .bind(&resource_id)
        .bind(&tag)
        .bind(score_maximum)
        .bind(now)
        .bind(now)
        .fetch_one(&state.db)
        .await?;

    Ok((StatusCode::CREATED, Json(line_item_json(&row))).into_response())
}

async fn show_line_item(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_account(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let item = find_line_item_in_course(&state.db, id, course_id).await?;
    Ok(Json(line_item_json(&item)))
}

async fn update_line_item(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateLineItemRequest>,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_account(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let existing = find_line_item_in_course(&state.db, id, course_id).await?;

    let label = payload.label.unwrap_or(existing.label);
    let score_maximum = payload.score_maximum.unwrap_or(existing.score_maximum);
    let resource_id = payload.resource_id.or(existing.resource_id);
    let tag = payload.tag.or(existing.tag);

    // Sync score to Canvas assignment.
    if let Some(assignment_id) = existing.assignment_id {
        let _ = sqlx::query(
            "UPDATE assignments SET points_possible = $1, updated_at = now() WHERE id = $2",
        )
        .bind(score_maximum)
        .bind(assignment_id)
        .execute(&state.db)
        .await;
    }

    let sql = format!(
        "UPDATE lti_line_items \
         SET label = $2, resource_id = $3, tag = $4, score_maximum = $5, \
             updated_at = now() \
         WHERE id = $1 AND workflow_state <> 'deleted' \
         RETURNING {LINE_ITEM_COLUMNS}"
    );
    let row: LineItemRow = sqlx::query_as(&sql)
        .bind(id)
        .bind(&label)
        .bind(&resource_id)
        .bind(&tag)
        .bind(score_maximum)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(line_item_json(&row)))
}

async fn destroy_line_item(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_account(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _item = find_line_item_in_course(&state.db, id, course_id).await?;

    let sql = format!(
        "UPDATE lti_line_items \
         SET workflow_state = 'deleted', updated_at = now() \
         WHERE id = $1 AND workflow_state <> 'deleted' \
         RETURNING {LINE_ITEM_COLUMNS}"
    );
    let row: Option<LineItemRow> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?;

    match row {
        Some(r) => Ok(Json(line_item_json(&r))),
        None => Err(ApiError::NotFound),
    }
}
