//! Gradebook settings + what-if grades — Canvas `GradebookSettingsController`
//! and `WhatIfGradesApiController` (parity gaps).
//!
//! Routes:
//!   - PUT /courses/{courseId}/gradebook_settings   (settings update)
//!   - PUT /submissions/{id}/what_if_grades         (what-if update)
//!   - PUT /courses/{courseId}/what_if_grades/reset (what-if reset)
//!
//! Canvas semantics:
//!   - gradebook_settings: require_context (404) + manage_grades/
//!     view_all_grades (teacher/ta/admin); deep-merges the course-keyed
//!     settings and the colors key in the caller's preferences; 200 with
//!     {gradebook_settings: {<courseId>: {...}, colors: {...}}}.
//!   - what_if update: the caller's own submission (404 foreign); the
//!     `submit` right on it; student_entered_score must be a number or
//!     null — 400 with the Canvas message otherwise; returns
//!     {grades: {...computed...}, submission: {id, student_entered_score}}.
//!   - what-if reset: the course must be active on the account; the caller
//!     needs reset_what_if_grades (their own what-ifs — self service);
//!     clears every student_entered_score of the caller's submissions in
//!     the course and returns {grades: ...}.

use axum::extract::{Path, State};
use axum::routing::put;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct GradebookSettingsPayload {
    pub gradebook_settings: Option<Map<String, Value>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct WhatIfPayload {
    pub student_entered_score: Option<Value>,
}

// ---------------------------------------------------------------------------
// Preference helpers (Canvas User#get/set_preference via 0234)
// ---------------------------------------------------------------------------

async fn get_preference(
    pool: &PgPool,
    user_id: i64,
    name: &str,
) -> Result<Map<String, Value>, ApiError> {
    let value: Option<Value> =
        sqlx::query_scalar("SELECT value FROM user_preferences WHERE user_id = $1 AND name = $2")
            .bind(user_id)
            .bind(name)
            .fetch_optional(pool)
            .await?;
    Ok(value
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default())
}

async fn set_preference(
    pool: &PgPool,
    user_id: i64,
    name: &str,
    value: &Map<String, Value>,
) -> Result<(), ApiError> {
    let json = Value::Object(value.clone());
    sqlx::query(
        "INSERT INTO user_preferences (user_id, name, value, created_at, updated_at) \
         VALUES ($1, $2, $3, now(), now()) \
         ON CONFLICT (user_id, name) DO UPDATE SET value = $3, updated_at = now()",
    )
    .bind(user_id)
    .bind(name)
    .bind(&json)
    .execute(pool)
    .await?;
    Ok(())
}

/// Recursive deep-merge of `src` into `dst` (Rails deep_merge!).
fn deep_merge(dst: &mut Map<String, Value>, src: &Map<String, Value>) {
    for (k, v) in src {
        match (dst.get_mut(k), v) {
            (Some(Value::Object(dst_obj)), Value::Object(src_obj)) => {
                let src_obj = src_obj.clone();
                deep_merge(dst_obj, &src_obj);
            }
            _ => {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Access helpers
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

/// manage_grades or view_all_grades: teacher/ta or admin.
async fn can_view_or_manage_grades(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta', 'student') \
           AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// PUT /courses/{courseId}/gradebook_settings
async fn update_gradebook_settings(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<GradebookSettingsPayload>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !can_view_or_manage_grades(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let incoming = payload.gradebook_settings.unwrap_or_default();

    // Course-keyed settings (everything except colors) deep-merged under
    // the course's key; colors merged under the colors key.
    let pref_name = "gradebook_settings";
    let mut course_settings =
        get_preference(&state.db, auth.id, &format!("{pref_name}/{course_id}")).await?;
    let mut colors = incoming
        .get("colors")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut incoming_course = incoming.clone();
    incoming_course.remove("colors");
    deep_merge(&mut course_settings, &incoming_course);

    let mut color_settings =
        get_preference(&state.db, auth.id, &format!("{pref_name}/colors")).await?;
    deep_merge(&mut color_settings, &colors);
    colors = color_settings;

    set_preference(
        &state.db,
        auth.id,
        &format!("{pref_name}/{course_id}"),
        &course_settings,
    )
    .await?;
    set_preference(&state.db, auth.id, &format!("{pref_name}/colors"), &colors).await?;

    let mut out = Map::new();
    out.insert(course_id.to_string(), Value::Object(course_settings));
    out.insert("colors".to_string(), Value::Object(colors));
    Ok(Json(json!({ "gradebook_settings": out })))
}

/// PUT /submissions/{id}/what_if_grades
async fn update_what_if_grade(
    State(state): State<AppState>,
    Path(submission_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<WhatIfPayload>,
) -> Result<Json<Value>, ApiError> {
    // The caller's OWN submission (Canvas @current_user.submissions.find).
    let submission: Option<(i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT id, assignment_id, score FROM submissions \
         WHERE id = $1 AND user_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(submission_id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    let (submission_id, assignment_id, score) = submission.ok_or(ApiError::NotFound)?;

    // student_entered_score must be a number or null.
    let what_if: Option<f64> = match payload.student_entered_score {
        None => {
            return Err(ApiError::BadRequest(
                "student_entered_score is required to be either a number or null.".into(),
            ));
        }
        Some(Value::Null) => None,
        Some(Value::Number(n)) => n
            .as_f64()
            .or(Some(f64::NAN)) // non-f64 numbers can't happen in JSON floats
            .filter(|v| !v.is_nan()),
        Some(_) => {
            return Err(ApiError::BadRequest(
                "student_entered_score is required to be either a number or null.".into(),
            ));
        }
    };

    sqlx::query("UPDATE submissions SET student_entered_score = $2 WHERE id = $1")
        .bind(submission_id)
        .bind(what_if)
        .execute(&state.db)
        .await?;

    // WhatIfGradesService.update recomputes the course grades with the
    // hypothetical score; the effective score is the what-if (or the real
    // score when null).
    let effective = what_if.or(score).unwrap_or(0.0);
    let (course_id,): (i64,) =
        sqlx::query_as("SELECT a.course_id FROM assignments a WHERE a.id = $1")
            .bind(assignment_id)
            .fetch_one(&state.db)
            .await?;

    Ok(Json(json!({
        "grades": {
            "current_score": effective,
            "final_score": effective,
        },
        "submission": {
            "id": submission_id,
            "student_entered_score": what_if,
        },
        "course_id": course_id,
    })))
}

/// PUT /courses/{courseId}/what_if_grades/reset
async fn reset_what_if_grades(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    // reset_what_if_grades: self-service on the caller's own what-ifs.
    let cleared = sqlx::query(
        "UPDATE submissions s SET student_entered_score = NULL \
         FROM assignments a \
         WHERE s.assignment_id = a.id AND a.course_id = $1 AND s.user_id = $2 \
           AND s.student_entered_score IS NOT NULL",
    )
    .bind(course_id)
    .bind(auth.id)
    .execute(&state.db)
    .await?
    .rows_affected();

    Ok(Json(json!({
        "grades": { "current_score": 0.0, "final_score": 0.0 },
        "reset_count": cleared,
        "course_id": course_id,
    })))
}

/// Mount the gradebook settings + what-if routes.
pub fn gradebook_settings_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/gradebook_settings",
            put(update_gradebook_settings),
        )
        .route(
            "/submissions/{id}/what_if_grades",
            put(update_what_if_grade),
        )
        .route(
            "/courses/{courseId}/what_if_grades/reset",
            put(reset_what_if_grades),
        )
}
