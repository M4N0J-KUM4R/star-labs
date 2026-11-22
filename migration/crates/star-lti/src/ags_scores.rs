//! LTI AGS Scores service — `POST /api/lti/courses/{course_id}/line_items/
//! {line_item_id}/scores` (issue #94).
//!
//! Canvas source: `app/controllers/lti/ims/scores_controller.rb` (+ its
//! `Concerns::GradebookServices` before_actions), route
//! `config/routes.rb:3205-3207`.
//!
//! Canvas validation order (before_action chain, each renders on first
//! failure — this order is parity-critical):
//! 1. access token / developer key / scope chain (`ags_auth.rs`) — 401s.
//! 2. `verify_course_not_concluded` — 422 (omitted: star-lms courses have
//!    no concluded state; `workflow_state` covers deleted only — documented
//!    deviation).
//! 3. `verify_line_item_in_context` — line item's course must match the
//!    path → 404.
//! 4. `verify_user_in_context` — the `userId` must be an enrolled student
//!    → 422 "User not found in course or is not a student".
//! 5. `verify_required_params` — `userId`, `activityProgress`,
//!    `gradingProgress`, `timestamp` required → 400 (Rails `params.require`
//!    renders 400).
//! 6. `verify_assignment_published` — 422 "This assignment is still
//!    unpublished".
//! 7. `verify_valid_timestamp` — unparseable → 400; before the existing
//!    result's `updated_at` → 400.
//! 8. `verify_valid_score_maximum` — missing with scoreGiven → 422;
//!    negative → 422; zero with a non-zero line-item maximum → 422.
//! 9. `verify_valid_score_given` — negative → 422.
//! 10. `verify_valid_submitted_at` — unparseable or > 1 minute in the
//!     future → 400.
//!
//! Success semantics (`update_or_create_result`, scores_controller.rb:451):
//! upserts the `Lti::Result` for (line_item, user) — `scoreGiven` →
//! `result_score`, `scoreMaximum` → `result_maximum`, `comment` saved —
//! and, when the line item has an assignment and `gradingProgress` is
//! `FullyGraded`/`PendingManual` with a score, updates the submission score
//! (Canvas `score_submission`; grade passback). Response MIME
//! `application/vnd.ims.lis.v1.score+json`, body `{ resultUrl, ... }` where
//! `resultUrl` is the fully-qualified result show URL.
//!
//! File-attachment content items (the `.../lti/submission` extension with
//! file upload) are omitted — no file-service plumbing; the response's
//! `content_items` list is always empty, matching Canvas's shape with zero
//! attachments.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_common::AppState;

use crate::ags_auth::{
    advantage_error, verify_advantage_access, verify_advantage_course, verify_advantage_tool,
    SCOPE_AGS_SCORE,
};

/// Canvas MIME type (scores_controller.rb:88).
const SCORE_MIME: &str = "application/vnd.ims.lis.v1.score+json";

/// `Lti::Result::AGS_EXT_SUBMISSION` (app/models/lti/result.rb:29).
const AGS_EXT_SUBMISSION: &str = "https://canvas.instructure.com/lti/submission";

// ---------------------------------------------------------------------------
// Request body
// ---------------------------------------------------------------------------

/// `scores_params` (scores_controller.rb:298-307): required `userId`,
/// `activityProgress`, `gradingProgress`, `timestamp`; optional `scoreGiven`,
/// `scoreMaximum`, `comment`, `submittedAt`, and the
/// `https://canvas.instructure.com/lti/submission` extension.
#[derive(Debug, Deserialize)]
pub struct ScoreRequest {
    #[serde(rename = "userId")]
    user_id: Option<Value>,
    #[serde(rename = "activityProgress")]
    activity_progress: Option<String>,
    #[serde(rename = "gradingProgress")]
    grading_progress: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "scoreGiven")]
    score_given: Option<f64>,
    #[serde(rename = "scoreMaximum")]
    score_maximum: Option<f64>,
    comment: Option<String>,
    #[serde(rename = "submittedAt")]
    submitted_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------

/// The line item with its assignment link, active only.
#[derive(Debug, sqlx::FromRow)]
struct LineItemLookup {
    course_id: i64,
    score_maximum: f64,
    assignment_id: Option<i64>,
}

#[allow(clippy::result_large_err)]
async fn find_line_item(
    pool: &PgPool,
    line_item_id: i64,
) -> Result<Option<LineItemLookup>, Response> {
    let row: Option<LineItemLookup> = sqlx::query_as(
        "SELECT course_id, score_maximum, assignment_id FROM lti_line_items \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(line_item_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;
    Ok(row)
}

/// Resolve the target user like Canvas `user`/`user_id`
/// (gradebook_services.rb:44-58, 99-102): the `userId` may be the lti_id or
/// the Canvas user id; star-lms has no `users.lti_id` column, so the user id
/// (numeric string or number) is the only handle. Returns None for a
/// non-numeric or unknown id.
#[allow(clippy::result_large_err)]
async fn resolve_target_user(pool: &PgPool, user_id: &Value) -> Result<Option<i64>, Response> {
    let id = match user_id {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    };
    let Some(id) = id else {
        return Ok(None);
    };
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1::bigint FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;
    Ok(row.map(|_| id))
}

/// Whether the user has a student enrollment in the course — Canvas
/// `context.user_is_student?` (gradebook_services.rb:81-84).
#[allow(clippy::result_large_err)]
async fn user_is_student_in_course(
    pool: &PgPool,
    course_id: i64,
    user_id: i64,
) -> Result<bool, Response> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM enrollments \
         WHERE course_id = $1 AND user_id = $2 AND role = 'student' \
           AND workflow_state = 'active'",
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;
    Ok(row.is_some())
}

/// Whether the line item's assignment is published
/// (`verify_assignment_published`, scores_controller.rb:391-394). Line items
/// without an assignment pass (star-lms line items may be unattached).
#[allow(clippy::result_large_err)]
async fn assignment_published(pool: &PgPool, assignment_id: Option<i64>) -> Result<bool, Response> {
    let Some(assignment_id) = assignment_id else {
        return Ok(true);
    };
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT workflow_state = 'published' FROM assignments WHERE id = $1")
            .bind(assignment_id)
            .fetch_optional(pool)
            .await
            .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;
    Ok(row.map(|(p,)| p).unwrap_or(true))
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
}

// ---------------------------------------------------------------------------
// Router + handler
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/lti/courses/{course_id}/line_items/{line_item_id}/scores",
        post(create_score),
    )
}

async fn create_score(
    State(state): State<AppState>,
    Path((course_id, line_item_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ScoreRequest>,
) -> Response {
    // 1. LtiServices chain: token → developer key → score scope.
    let auth = match verify_advantage_access(
        &state,
        headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok()),
        SCOPE_AGS_SCORE,
    )
    .await
    {
        Ok(a) => a,
        Err(resp) => return resp,
    };

    // AdvantageServices: context exists (404) + tool bound to the key (401).
    if let Err(resp) = verify_advantage_course(&state.db, course_id).await {
        return resp;
    }
    if let Err(resp) = verify_advantage_tool(&state.db, auth.developer_key_id).await {
        return resp;
    }

    // 3. verify_line_item_in_context — 404 when missing or another course's.
    let line_item = match find_line_item(&state.db, line_item_id).await {
        Ok(Some(li)) if li.course_id == course_id => li,
        Ok(_) => {
            return advantage_error(StatusCode::NOT_FOUND, "not found");
        }
        Err(resp) => return resp,
    };

    // 4. verify_user_in_context — 422 "User not found in course or is not a
    //    student" (fires before the required-params check because
    //    verify_user_in_context precedes verify_required_params in the
    //    before_action list).
    let user_id =
        match resolve_target_user(&state.db, body.user_id.as_ref().unwrap_or(&Value::Null)).await {
            Ok(Some(id)) => match user_is_student_in_course(&state.db, course_id, id).await {
                Ok(true) => id,
                Ok(false) => {
                    return advantage_error(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "User not found in course or is not a student",
                    )
                }
                Err(resp) => return resp,
            },
            Ok(None) => {
                return advantage_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "User not found in course or is not a student",
                )
            }
            Err(resp) => return resp,
        };

    // 5. verify_required_params — Rails params.require renders 400.
    if body.activity_progress.is_none()
        || body.grading_progress.is_none()
        || body.timestamp.is_none()
    {
        return advantage_error(
            StatusCode::BAD_REQUEST,
            "missing required parameter: activityProgress, gradingProgress or timestamp",
        );
    }

    // 6. verify_assignment_published — 422.
    if !assignment_published(&state.db, line_item.assignment_id)
        .await
        .unwrap_or(true)
    {
        return advantage_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "This assignment is still unpublished",
        );
    }

    let timestamp_str = body.timestamp.as_deref().unwrap_or_default();

    // 7. verify_valid_timestamp — 400 on parse failure.
    let Some(timestamp) = parse_timestamp(timestamp_str) else {
        return advantage_error(
            StatusCode::BAD_REQUEST,
            &format!("Provided timestamp of {timestamp_str} not a valid timestamp"),
        );
    };

    // 8. verify_valid_score_maximum — 422s.
    if body.score_given.is_some() {
        match body.score_maximum {
            None => {
                return advantage_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "ScoreMaximum not supplied when ScoreGiven present.",
                )
            }
            Some(max) if max < 0.0 => {
                return advantage_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "ScoreMaximum must be greater than or equal to 0",
                )
            }
            Some(max) if max == 0.0 && line_item.score_maximum != 0.0 => {
                return advantage_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "cannot be zero if line item's maximum is not zero",
                )
            }
            _ => {}
        }
    }

    // 9. verify_valid_score_given — 422.
    if let Some(given) = body.score_given {
        if given < 0.0 {
            return advantage_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "ScoreGiven must be greater than or equal to 0",
            );
        }
    }

    // 10. verify_valid_submitted_at — 400.
    if let Some(submitted) = body.submitted_at.as_deref() {
        match parse_timestamp(submitted) {
            None => {
                return advantage_error(
                    StatusCode::BAD_REQUEST,
                    &format!(
                        "Provided submitted_at timestamp of {submitted} not a valid timestamp"
                    ),
                )
            }
            Some(t) if t > Utc::now() + chrono::Duration::minutes(1) => {
                return advantage_error(
                    StatusCode::BAD_REQUEST,
                    &format!("Provided submitted_at timestamp of {submitted} in the future"),
                )
            }
            _ => {}
        }
    }

    // -- update_or_create_result: upsert the result row ------------------
    let existing: Option<(i64, Option<DateTime<Utc>>)> = match sqlx::query_as(
        // 0224 stores created_at/updated_at as naive `timestamp`; cast on
        // read/write so chrono's Timestamptz mapping (DateTime<Utc>) works.
        "SELECT id, updated_at::timestamptz FROM lti_results \
         WHERE lti_line_item_id = $1 AND user_id = $2 AND workflow_state = 'active'",
    )
    .bind(line_item_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(_) => return advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    };

    if let Some((existing_id, Some(existing_updated))) = existing {
        // verify_valid_timestamp second branch: timestamp before last update → 400.
        if timestamp < existing_updated {
            return advantage_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "Provided timestamp of {timestamp_str} before last updated timestamp of {}",
                    existing_updated.to_rfc3339()
                ),
            );
        }
        let _ = existing_id;
    }

    let result_id: i64 = match existing {
        Some((existing_id, _)) => sqlx::query_as::<_, (i64,)>(
            "UPDATE lti_results \
                 SET result_score = $1, result_maximum = $2, comment = $3, \
                     activity_progress = $4, grading_progress = $5, \
                     updated_at = $6::timestamptz \
                 WHERE id = $7 \
                 RETURNING id",
        )
        .bind(body.score_given)
        .bind(body.score_maximum)
        .bind(&body.comment)
        .bind(body.activity_progress.as_deref())
        .bind(body.grading_progress.as_deref())
        .bind(timestamp)
        .bind(existing_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|(id,)| id),
        None => sqlx::query_as::<_, (i64,)>(
            "INSERT INTO lti_results \
                 (result_score, result_maximum, comment, activity_progress, \
                  grading_progress, lti_line_item_id, user_id, created_at, \
                  updated_at, workflow_state, root_account_id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8::timestamptz, $8::timestamptz, 'active', 1) \
                 RETURNING id",
        )
        .bind(body.score_given)
        .bind(body.score_maximum)
        .bind(&body.comment)
        .bind(body.activity_progress.as_deref())
        .bind(body.grading_progress.as_deref())
        .bind(line_item_id)
        .bind(user_id)
        .bind(timestamp)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .map(|(id,)| id),
    }
    .unwrap_or(0);

    if result_id == 0 {
        return advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error");
    }

    // -- score_submission: grade passback when FullyGraded/PendingManual --
    let grading_progress = body.grading_progress.as_deref().unwrap_or_default();
    let graded = matches!(grading_progress, "FullyGraded" | "PendingManual");
    if let (Some(assignment_id), true, Some(score)) =
        (line_item.assignment_id, graded, body.score_given)
    {
        let _ = sqlx::query(
            "INSERT INTO submissions (assignment_id, user_id, score, graded_at, grader_id, \
             submitted_at, workflow_state) \
             VALUES ($1, $2, $3, $4, -1, $4, 'graded') \
             ON CONFLICT (assignment_id, user_id) \
             DO UPDATE SET score = EXCLUDED.score, graded_at = EXCLUDED.graded_at, \
                           grader_id = EXCLUDED.grader_id, \
                           updated_at = now()",
        )
        .bind(assignment_id)
        .bind(user_id)
        .bind(score)
        .bind(timestamp)
        .execute(&state.db)
        .await;
    }

    // -- response: resultUrl + empty content_items ------------------------
    let result_url =
        format!("/api/lti/courses/{course_id}/line_items/{line_item_id}/results/{result_id}");
    let payload = json!({
        "resultUrl": result_url,
        AGS_EXT_SUBMISSION: { "content_items": [] },
    });

    (
        StatusCode::CREATED,
        [(header::CONTENT_TYPE, SCORE_MIME)],
        axum::Json(payload),
    )
        .into_response()
}
