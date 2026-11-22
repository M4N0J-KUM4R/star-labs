//! error_reports — Canvas `ErrorsController` parity.
//!
//! Route:
//! - POST /error_reports — report a problem
//!
//! Canvas behavior mirrored here (`app/controllers/errors_controller.rb`):
//! - the route is public: `skip_before_action :require_user`.
//! - `error[username]` is a honeypot field: when it is present the request is
//!   answered with a 400 and an **empty body** (`head(:bad_request)`).
//! - the body carries the report under the `error` key: `subject` (the summary
//!   of the problem), `url`, `email`, `comments`, `description`, `backtrace`
//!   and `http_env`.
//! - the answer is `{"logged": true, "id": <id>}`. Canvas rescues a failed
//!   save and answers `{"logged": true, "id": null}` in that case, so a report
//!   without a subject is not stored.

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_common::{ApiError, AppState};

/// The root account of the installation.
async fn root_account_id(pool: &PgPool) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts \
         WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id,)| id))
}

/// POST /error_reports — Canvas `ErrorsController#create`.
async fn create_error_report(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let error = match payload.get("error") {
        Some(Value::Object(map)) => map.clone(),
        _ => Map::new(),
    };

    // The honeypot field catches the spambots; a human never fills it in.
    if let Some(Value::String(username)) = error.get("username") {
        if !username.trim().is_empty() {
            return Ok((StatusCode::BAD_REQUEST, Body::empty()).into_response());
        }
    }

    let subject = error
        .get("subject")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();

    // Canvas rescues a failed save and still answers `logged: true`.
    if subject.is_empty() {
        return Ok((StatusCode::OK, Json(json!({ "logged": true, "id": null }))).into_response());
    }

    let text = |key: &str| -> Option<String> {
        error
            .get(key)
            .and_then(Value::as_str)
            .map(|value| value.to_string())
    };

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO error_reports \
         (user_id, account_id, subject, url, email, comments, description, backtrace, \
          user_agent, http_env) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(
        crate::optional_user(&headers, &state)
            .await
            .map(|user| user.id),
    )
    .bind(root_account_id(&state.db).await?)
    .bind(subject)
    .bind(text("url"))
    .bind(text("email"))
    .bind(text("comments"))
    .bind(text("description"))
    .bind(text("backtrace"))
    .bind(text("user_agent"))
    .bind(error.get("http_env").cloned())
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::OK, Json(json!({ "logged": true, "id": id }))).into_response())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the error report router.
pub fn router() -> Router<AppState> {
    Router::new().route("/error_reports", post(create_error_report))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_without_a_subject_is_not_stored() {
        let error = Map::new();
        assert!(error.get("subject").is_none());
    }
}
