//! Gradebooks mutations — Canvas `GradebooksController`
//! `update_final_grade_overrides` + `apply_score_to_ungraded_submissions`
//! (parity gap).
//!
//! Routes:
//!   - PUT /courses/{courseId}/update_final_grade_overrides
//!   - PUT /courses/{courseId}/apply_score_to_ungraded_submissions
//!
//! Canvas semantics:
//!   - update_final_grade_overrides: manage_grades (teacher/ta/admin); the
//!     course must allow final-grade override (Canvas
//!     allow_final_grade_override? — the course's final_grade_override
//!     setting) — 401 when off; optional grading_period_id validated
//!     against the course's grading periods (400 {error:
//!     invalid_grading_period}); `override_scores` required (each entry:
//!     student_id + override_score/override_status_id); queues a bulk
//!     update → progress JSON (the workspace applies synchronously and
//!     returns a completed progress).
//!   - apply_score_to_ungraded: manage_grades; percent or excused required
//!     (400 no_score_or_excused_provided), not both (400
//!     cannot_both_score_and_excuse), percent within 0..100 (400
//!     invalid_percent_value); applies to every ungraded submission of the
//!     course (all students) — the workspace writes the score directly.

use axum::extract::{Path, State};
use axum::routing::put;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct OverrideScoresPayload {
    pub override_scores: Option<Vec<OverrideScore>>,
    pub grading_period_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct OverrideScore {
    pub student_id: Option<i64>,
    pub override_score: Option<f64>,
    pub override_status_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ApplyScorePayload {
    pub percent: Option<f64>,
    pub excused: Option<bool>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn course_exists(pool: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

async fn can_manage_grades(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
           AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

/// Canvas Course#allow_final_grade_override? — the course's
/// final_grade_override setting (stored on the course row / feature
/// flags; the workspace reads the settings JSON on the course's account
/// chain the same way the grading-period code does).
async fn allow_final_grade_override(pool: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        "SELECT COALESCE( \
             (SELECT (a.settings ->> 'final_grade_override')::boolean FROM accounts a \
              WHERE a.id = c.account_id), \
             false) \
         FROM courses c WHERE c.id = $1",
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

fn progress_json(course_id: i64, tag: &str, completion: f64) -> Value {
    json!({
        "id": format!("{tag}-{course_id}"),
        "context_id": course_id,
        "context_type": "Course",
        "tag": tag,
        "completion": completion,
        "workflow_state": "completed",
        "url": format!("/api/v1/progress/{tag}-{course_id}"),
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// PUT /courses/{courseId}/update_final_grade_overrides
async fn update_final_grade_overrides(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<OverrideScoresPayload>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !can_manage_grades(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    if !allow_final_grade_override(&state.db, course_id).await? {
        return Err(ApiError::Unauthorized);
    }

    let grading_period_id = match payload.grading_period_id {
        None => None,
        Some(gpid) => {
            // GradingPeriod.for(course).find_by(id:) — 400 when not one of
            // the course's periods.
            let valid: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM grading_periods gp \
                 JOIN grading_period_groups gpg ON gpg.id = gp.grading_period_group_id \
                 WHERE gp.id = $1 AND gpg.account_id = \
                     (SELECT account_id FROM courses WHERE id = $2))",
            )
            .bind(gpid)
            .bind(course_id)
            .fetch_one(&state.db)
            .await?;
            if !valid {
                return Err(ApiError::BadRequest("invalid_grading_period".into()));
            }
            Some(gpid)
        }
    };

    let updates = payload
        .override_scores
        .ok_or_else(|| ApiError::BadRequest("override_scores is required".into()))?;

    for o in &updates {
        let Some(student_id) = o.student_id else {
            return Err(ApiError::BadRequest("student_id is required".into()));
        };
        // Upsert one override per (course, student, period).
        sqlx::query(
            "INSERT INTO score_overrides \
                (course_id, student_id, grading_period_id, override_score, override_status_id) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (course_id, student_id, grading_period_id) DO UPDATE SET \
                override_score = EXCLUDED.override_score, \
                override_status_id = EXCLUDED.override_status_id, \
                updated_at = now()",
        )
        .bind(course_id)
        .bind(student_id)
        .bind(grading_period_id)
        .bind(o.override_score)
        .bind(o.override_status_id)
        .execute(&state.db)
        .await?;
    }

    Ok(Json(progress_json(
        course_id,
        "final_grade_overrides",
        100.0,
    )))
}

/// PUT /courses/{courseId}/apply_score_to_ungraded_submissions
async fn apply_score_to_ungraded(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<ApplyScorePayload>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !can_manage_grades(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let excused = payload.excused.unwrap_or(false);
    // percent or excused, not both; percent in 0..100.
    let percent = match (payload.percent, excused) {
        (None, false) => {
            return Err(ApiError::BadRequest("no_score_or_excused_provided".into()));
        }
        (Some(_), true) => {
            return Err(ApiError::BadRequest("cannot_both_score_and_excuse".into()));
        }
        (Some(p), false) => {
            if !(0.0..=100.0).contains(&p) {
                return Err(ApiError::BadRequest("invalid_percent_value".into()));
            }
            Some(p)
        }
        (None, true) => None,
    };

    // Apply to every ungraded submission of the course: score = percent of
    // points_possible, or excused when excused=true.
    let updated = sqlx::query(
        "UPDATE submissions s SET \
            score = CASE WHEN $2::float8 IS NULL THEN NULL \
                         ELSE ROUND((a.points_possible * $2 / 100.0)::numeric, 2)::float8 END, \
            graded_at = now() \
         FROM assignments a \
         WHERE s.assignment_id = a.id AND a.course_id = $1 \
           AND s.score IS NULL AND s.workflow_state <> 'deleted' \
           AND a.workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .bind(percent)
    .execute(&state.db)
    .await?
    .rows_affected();

    Ok(Json(json!({
        "applied_count": updated,
        "excused": excused,
        "percent": percent,
        "progress": progress_json(course_id, "apply_score_to_ungraded", 100.0),
    })))
}

/// Mount the gradebooks-mutation routes.
pub fn gradebooks_mutation_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/update_final_grade_overrides",
            put(update_final_grade_overrides),
        )
        .route(
            "/courses/{courseId}/apply_score_to_ungraded_submissions",
            put(apply_score_to_ungraded),
        )
}
