//! star-features — Canvas feature flags (`feature_flags_controller`).
//!
//! Endpoints:
//! - GET /accounts/{accountId}/features        — list account features (admin)
//! - GET /courses/{courseId}/features          — list course features (enrolled/admin)
//! - GET /users/{userId}/features              — list user features (self/admin)
//! - PUT /accounts/{accountId}/features/{feature} — set account feature state (admin)
//! - PUT /courses/{courseId}/features/{feature}   — set course feature state (teacher/admin)

use axum::{
    extract::{Path, State},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Feature catalog
// ---------------------------------------------------------------------------

/// Feature catalog: (feature, display_name). Absent overrides default to
/// `disabled` for the requested context.
const FEATURES: &[(&str, &str)] = &[
    ("fancy_wickets", "Fancy Wickets"),
    ("automatic_essay_grading", "Automatic Essay Grading"),
    ("differentiation_tags", "Differentiation Tags"),
];

const STATES: &[&str] = &[
    "enabled",
    "disabled",
    "indisputable",
    "pending_delivery",
    "pending_cancellation",
];

fn display_name(feature: &str) -> Option<&'static str> {
    FEATURES
        .iter()
        .find(|(name, _)| *name == feature)
        .map(|(_, display)| *display)
}

/// Canvas accepts `on`/`off` (plus the stored state names) for an update.
fn normalize_state(raw: &str) -> Option<&'static str> {
    match raw {
        "on" => Some("enabled"),
        "off" => Some("disabled"),
        other => STATES.iter().find(|state| **state == other).copied(),
    }
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Canvas Feature shape.
#[derive(Debug, Clone, Serialize)]
pub struct Feature {
    pub feature: String,
    pub display_name: String,
    pub state: String,
    pub locking_account_id: Option<i64>,
    pub transitions: Transitions,
}

#[derive(Debug, Clone, Serialize)]
pub struct Transitions {
    pub allowed: bool,
    pub locked: bool,
}

/// Request body for setting a feature state.
#[derive(Debug, Deserialize)]
pub struct UpdateFeature {
    pub state: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct FlagRow {
    feature: String,
    state: String,
    locking_account_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

async fn enrolled_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

async fn teacher_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

async fn account_exists(pool: &PgPool, account_id: i64) -> Result<bool, ApiError> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(pool)
            .await?
            .is_some(),
    )
}

async fn course_exists(pool: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?
            .is_some(),
    )
}

async fn user_exists(pool: &PgPool, user_id: i64) -> Result<bool, ApiError> {
    Ok(
        sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?
            .is_some(),
    )
}

async fn load_flags(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
) -> Result<Vec<Feature>, ApiError> {
    let rows: Vec<FlagRow> = sqlx::query_as(
        r#"SELECT feature, state, locking_account_id
           FROM feature_flags
           WHERE context_type = $1 AND context_id = $2"#,
    )
    .bind(context_type)
    .bind(context_id)
    .fetch_all(pool)
    .await?;

    let mut overrides: HashMap<String, FlagRow> = rows
        .into_iter()
        .map(|row| (row.feature.clone(), row))
        .collect();

    Ok(FEATURES
        .iter()
        .map(|(feature, display)| {
            let row = overrides.remove(*feature);
            Feature {
                feature: (*feature).to_string(),
                display_name: (*display).to_string(),
                state: row
                    .as_ref()
                    .map(|r| r.state.clone())
                    .unwrap_or_else(|| "disabled".to_string()),
                locking_account_id: row.as_ref().and_then(|r| r.locking_account_id),
                transitions: Transitions {
                    allowed: true,
                    locked: false,
                },
            }
        })
        .collect())
}

async fn set_flag(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    feature: &str,
    state: &str,
) -> Result<Feature, ApiError> {
    let display =
        display_name(feature).ok_or_else(|| ApiError::BadRequest("invalid feature".into()))?;

    let row: FlagRow = sqlx::query_as(
        r#"INSERT INTO feature_flags (context_type, context_id, feature, display_name, state)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (context_type, context_id, feature)
           DO UPDATE SET state = EXCLUDED.state, display_name = EXCLUDED.display_name,
                         updated_at = now()
           RETURNING feature, state, locking_account_id"#,
    )
    .bind(context_type)
    .bind(context_id)
    .bind(feature)
    .bind(display)
    .bind(state)
    .fetch_one(pool)
    .await?;

    Ok(Feature {
        feature: row.feature,
        display_name: display.to_string(),
        state: row.state,
        locking_account_id: row.locking_account_id,
        transitions: Transitions {
            allowed: true,
            locked: false,
        },
    })
}

fn requested_state(body: &UpdateFeature) -> Result<&'static str, ApiError> {
    let raw = body
        .state
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("must specify state".into()))?;
    normalize_state(raw).ok_or_else(|| ApiError::BadRequest("invalid state".into()))
}

// ---------------------------------------------------------------------------
// Account handlers
// ---------------------------------------------------------------------------

async fn list_account_features(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
) -> Result<Json<Vec<Feature>>, ApiError> {
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(load_flags(&state.db, "Account", account_id).await?))
}

async fn update_account_feature(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((account_id, feature)): Path<(i64, String)>,
    Json(body): Json<UpdateFeature>,
) -> Result<Json<Feature>, ApiError> {
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let next = requested_state(&body)?;
    Ok(Json(
        set_flag(&state.db, "Account", account_id, &feature, next).await?,
    ))
}

// ---------------------------------------------------------------------------
// Course handlers
// ---------------------------------------------------------------------------

async fn list_course_features(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
) -> Result<Json<Vec<Feature>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(load_flags(&state.db, "Course", course_id).await?))
}

async fn update_course_feature(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, feature)): Path<(i64, String)>,
    Json(body): Json<UpdateFeature>,
) -> Result<Json<Feature>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let next = requested_state(&body)?;
    Ok(Json(
        set_flag(&state.db, "Course", course_id, &feature, next).await?,
    ))
}

// ---------------------------------------------------------------------------
// User handlers
// ---------------------------------------------------------------------------

async fn list_user_features(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
) -> Result<Json<Vec<Feature>>, ApiError> {
    if !user_exists(&state.db, user_id).await? {
        return Err(ApiError::NotFound);
    }
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(load_flags(&state.db, "User", user_id).await?))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounts/{accountId}/features", get(list_account_features))
        .route(
            "/accounts/{accountId}/features/{feature}",
            put(update_account_feature),
        )
        .route("/courses/{courseId}/features", get(list_course_features))
        .route(
            "/courses/{courseId}/features/{feature}",
            put(update_course_feature),
        )
        .route("/users/{userId}/features", get(list_user_features))
}
