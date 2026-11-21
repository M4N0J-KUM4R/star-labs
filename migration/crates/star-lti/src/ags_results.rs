//! LTI AGS Results service — `GET /api/lti/courses/{course_id}/line_items/
//! {line_item_id}/results` and `.../results/{id}` (issue #94).
//!
//! Canvas source: `app/controllers/lti/ims/results_controller.rb`,
//! `app/serializers/lti/ims/results_serializer.rb`, route
//! `config/routes.rb:3210-3213`.
//!
//! Auth chain (LtiServices + AdvantageServices + GradebookServices):
//! token → developer key → `result.readonly` scope (401s), context exists
//! (404), tool bound (401), line item in course (404).
//!
//! Index semantics (results_controller.rb:69-76):
//! - a `user_id` query param filters to that user's result (empty list when
//!   none); the param is the lti_id or Canvas user id — star-lms resolves
//!   the numeric user id (no `users.lti_id` column, documented deviation);
//! - `limit` caps the page (Canvas `pagination_args` per_page);
//! - when the `user_id` names a real context member who is NOT a student,
//!   Canvas returns `[]` early (`user.present? && !context.user_is_student?`)
//!   — matched here for known non-student users;
//! - ordering follows Api.paginate's id-ascending order.
//!
//! Serializer (results_serializer.rb:23-40): every field except `id` and
//! `scoreOf` is compacted (omitted when null); `extensions` (the
//! `.../lti/submission` claim) is merged in when present.
//!
//! MIME `application/vnd.ims.lis.v2.resultcontainer+json`
//! (results_controller.rb:63).

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_common::AppState;

use crate::ags_auth::{
    advantage_error, verify_advantage_access, verify_advantage_course, verify_advantage_tool,
    SCOPE_AGS_RESULT_READ_ONLY,
};

/// Canvas MIME type (results_controller.rb:63).
const RESULT_CONTAINER_MIME: &str = "application/vnd.ims.lis.v2.resultcontainer+json";

#[derive(Debug, Deserialize)]
pub struct ListResultsQuery {
    /// Canvas `user_id` — the lti_user_id or Canvas user id.
    user_id: Option<String>,
    /// Canvas `limit` — page size cap.
    limit: Option<i64>,
    /// Offset-based paging for multi-page tests.
    offset: Option<i64>,
}

/// One result row joined with what the serializer needs.
#[derive(Debug, sqlx::FromRow)]
struct ResultRow {
    id: i64,
    user_id: i64,
    result_score: Option<f64>,
    result_maximum: Option<f64>,
    comment: Option<String>,
    extensions: Option<Value>,
}

/// The results serializer shape (results_serializer.rb:23-40): `id` and
/// `scoreOf` are the fully-qualified line-item/result URLs; nulls are
/// compacted out; `extensions` merges in when present.
fn result_json(row: &ResultRow, course_id: i64, line_item_id: i64) -> Value {
    let line_item_url = format!("/api/lti/courses/{course_id}/line_items/{line_item_id}");
    let mut map = Map::new();
    map.insert(
        "id".into(),
        json!(format!("{line_item_url}/results/{}", row.id)),
    );
    map.insert("scoreOf".into(), json!(line_item_url));
    map.insert("userId".into(), json!(row.user_id.to_string()));
    if let Some(score) = row.result_score {
        map.insert("resultScore".into(), json!(score));
    }
    if let Some(max) = row.result_maximum {
        map.insert("resultMaximum".into(), json!(max));
    }
    if let Some(comment) = &row.comment {
        map.insert("comment".into(), json!(comment));
    }
    // `.merge(result.extensions)` — the JSONB claim keys merge over the base.
    if let Some(Value::Object(ext)) = &row.extensions {
        for (k, v) in ext {
            map.insert(k.clone(), v.clone());
        }
    }
    Value::Object(map)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/courses/{course_id}/line_items/{line_item_id}/results",
            get(index_results),
        )
        .route(
            "/api/lti/courses/{course_id}/line_items/{line_item_id}/results/{id}",
            get(show_result),
        )
}

/// Shared pre-handler: the AdvantageServices chain + line-item-in-course
/// (404). Returns (line_item course check ok, response on failure).
async fn verify_request(
    state: &AppState,
    course_id: i64,
    line_item_id: i64,
    authorization: Option<&str>,
) -> Result<(), Response> {
    let auth = verify_advantage_access(state, authorization, SCOPE_AGS_RESULT_READ_ONLY).await?;
    verify_advantage_course(&state.db, &auth, course_id).await?;
    verify_advantage_tool(&state.db, &auth).await?;

    // verify_line_item_in_context — 404 when missing or another course's.
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM lti_line_items \
         WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(line_item_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;

    row.ok_or_else(|| advantage_error(StatusCode::NOT_FOUND, "not found"))?;
    Ok(())
}

async fn index_results(
    State(state): State<AppState>,
    Path((course_id, line_item_id)): Path<(i64, i64)>,
    Query(params): Query<ListResultsQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if let Err(resp) = verify_request(&state, course_id, line_item_id, authorization).await {
        return resp;
    }

    // The user filter: resolve to an id; a known non-student context member
    // short-circuits to `[]` (results_controller.rb:69); an unknown id
    // yields an empty page too.
    let filter_user: Option<i64> = match params.user_id.as_deref() {
        None => None,
        Some(s) => match s.parse::<i64>() {
            Ok(id) => match known_user(&state.db, id).await {
                Ok(Some(true)) => Some(id),             // student → filter by them
                Ok(Some(false)) => return empty_page(), // non-student → []
                Ok(None) => None,                       // unknown user → []
                Err(resp) => return resp,
            },
            Err(_) => None,
        },
    };

    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);

    let rows: Vec<ResultRow> = match sqlx::query_as(
        "SELECT id, user_id, result_score, result_maximum, comment, extensions \
         FROM lti_results \
         WHERE lti_line_item_id = $1 AND workflow_state = 'active' \
           AND ($2::bigint IS NULL OR user_id = $2) \
         ORDER BY id LIMIT $3 OFFSET $4",
    )
    .bind(line_item_id)
    .bind(filter_user)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    };

    let data: Vec<Value> = rows
        .iter()
        .map(|row| result_json(row, course_id, line_item_id))
        .collect();

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, RESULT_CONTAINER_MIME)],
        Json(data),
    )
        .into_response()
}

fn empty_page() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, RESULT_CONTAINER_MIME)],
        Json(json!([])),
    )
        .into_response()
}

/// Whether the user exists and is a student somewhere (context membership
/// for the early-return): Ok(Some(is_student)), Ok(None) = unknown user.
async fn known_user(pool: &PgPool, user_id: i64) -> Result<Option<bool>, Response> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1::bigint FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;
    if row.is_none() {
        return Ok(None);
    }
    let student: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM enrollments \
         WHERE user_id = $1 AND role = 'student' AND workflow_state = 'active' LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;
    Ok(Some(student.is_some()))
}

async fn show_result(
    State(state): State<AppState>,
    Path((course_id, line_item_id, id)): Path<(i64, i64, i64)>,
    headers: axum::http::HeaderMap,
) -> Response {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if let Err(resp) = verify_request(&state, course_id, line_item_id, authorization).await {
        return resp;
    }

    // verify_result_in_line_item — the result must belong to this line item.
    let row: Option<ResultRow> = sqlx::query_as(
        "SELECT id, user_id, result_score, result_maximum, comment, extensions \
         FROM lti_results \
         WHERE id = $1 AND lti_line_item_id = $2 AND workflow_state = 'active'",
    )
    .bind(id)
    .bind(line_item_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);

    match row {
        Some(row) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, RESULT_CONTAINER_MIME)],
            Json(result_json(&row, course_id, line_item_id)),
        )
            .into_response(),
        None => advantage_error(StatusCode::NOT_FOUND, "not found"),
    }
}
