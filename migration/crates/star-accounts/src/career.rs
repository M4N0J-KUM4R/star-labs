//! career — Canvas `CareerExperienceController` and
//! `CanvasCareer::UserExperiencesController` parity.
//!
//! Routes:
//! - GET    /career/enabled             — is the root account career affiliated
//! - GET    /career/experience_summary  — the experiences of the caller
//! - POST   /career/switch_experience   — store the experience of the caller
//! - POST   /career/switch_role         — store the role of the caller
//! - POST   /career/user_experiences    — create the career experience of the caller
//! - DELETE /career/user_experiences    — delete the career experience of the caller
//!
//! Canvas behavior mirrored here:
//! - `CanvasCareer::ExperienceResolver.career_affiliated_institution?` is
//!   `root_account.settings[:horizon_account_ids].present?`, so the flag comes
//!   from the settings of the root account.
//! - the preferred experience and the preferred role live in the session in
//!   Canvas; the port keeps them in the `user_preferences` document.
//! - `switch_experience` only accepts `career` and `academic`, `switch_role`
//!   only `learner` and `learning_provider`; anything else is the Canvas 400
//!   `{"error": "invalid_experience"}` / `{"error": "invalid_role"}`.
//! - the user experience routes need a career affiliated root account, so they
//!   answer 404 without one.

use axum::{
    extract::State,
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `CanvasCareer::Constants::Experience.all`.
const EXPERIENCES: [&str; 2] = ["career", "academic"];

/// Canvas `CanvasCareer::Constants::Role.all`.
const ROLES: [&str; 2] = ["learner", "learning_provider"];

/// The user preference that holds the preferred experience.
const EXPERIENCE_KEY: &str = "career_experience";

/// The user preference that holds the preferred role.
const ROLE_KEY: &str = "career_role";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The root account of the installation and its settings.
async fn root_account(pool: &PgPool) -> Result<(i64, Value), ApiError> {
    let row: Option<(i64, Option<Value>)> = sqlx::query_as(
        "SELECT id, settings FROM accounts \
         WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    let (id, settings) = row.ok_or(ApiError::NotFound)?;
    Ok((id, settings.unwrap_or(Value::Object(Map::new()))))
}

/// Canvas `ExperienceResolver.career_affiliated_institution?`.
fn career_affiliated(settings: &Value) -> bool {
    match settings.get("horizon_account_ids") {
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

/// Read one user preference (the `user_preferences` JSONB document).
async fn read_preference(
    pool: &PgPool,
    user_id: i64,
    key: &str,
) -> Result<Option<String>, ApiError> {
    let row: Option<(Value,)> = sqlx::query_as(
        "SELECT COALESCE(preferences -> $2::text, 'null'::jsonb) \
         FROM user_preferences WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(key)
    .fetch_optional(pool)
    .await?;

    Ok(row.and_then(|(value,)| {
        value
            .as_str()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }))
}

/// Write one user preference.
async fn write_preference(
    pool: &PgPool,
    user_id: i64,
    key: &str,
    value: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO user_preferences (user_id, preferences)
        VALUES ($1, jsonb_build_object($2::text, to_jsonb($3::text)))
        ON CONFLICT (user_id) DO UPDATE
        SET preferences = user_preferences.preferences
                || jsonb_build_object($2::text, to_jsonb($3::text)),
            updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /career/enabled — Canvas `enabled`.
async fn career_enabled(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let (_, settings) = root_account(&state.db).await?;
    Ok(Json(json!({ "enabled": career_affiliated(&settings) })))
}

/// GET /career/experience_summary — Canvas `experience_summary`.
async fn career_experience_summary(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let (_, settings) = root_account(&state.db).await?;
    let affiliated = career_affiliated(&settings);

    // The academic app is always available; the career learner app needs a
    // career affiliated institution.
    let mut available_apps = vec!["academic".to_string()];
    if affiliated {
        available_apps.push("career_learner".to_string());
    }

    let experience = read_preference(&state.db, auth.id, EXPERIENCE_KEY).await?;
    let current_app = match experience.as_deref() {
        Some("career") => "career_learner",
        Some("academic") => "academic",
        _ if affiliated => "career_learner",
        _ => "academic",
    };

    Ok(Json(json!({
        "current_app": current_app,
        "available_apps": available_apps,
    })))
}

/// POST /career/switch_experience — Canvas `switch_experience`.
async fn career_switch_experience(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let experience = payload
        .get("experience")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();

    if !EXPERIENCES.contains(&experience.as_str()) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_experience" })),
        ));
    }

    write_preference(&state.db, auth.id, EXPERIENCE_KEY, &experience).await?;
    Ok((StatusCode::OK, Json(json!({ "experience": experience }))))
}

/// POST /career/switch_role — Canvas `switch_role`.
async fn career_switch_role(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let role = payload
        .get("role")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();

    if !ROLES.contains(&role.as_str()) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_role" })),
        ));
    }

    write_preference(&state.db, auth.id, ROLE_KEY, &role).await?;
    Ok((StatusCode::OK, Json(json!({ "role": role }))))
}

/// POST /career/user_experiences — Canvas
/// `CanvasCareer::UserExperiencesController#create`.
async fn create_career_user_experience(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (root_id, settings) = root_account(&state.db).await?;
    // Canvas: require_feature_flag -> 404 without a career affiliated account.
    if !career_affiliated(&settings) {
        return Err(ApiError::NotFound);
    }

    let workflow_state: String = sqlx::query_scalar(
        "INSERT INTO canvas_career_user_experiences (user_id, root_account_id, workflow_state) \
         VALUES ($1, $2, 'active') \
         ON CONFLICT (user_id, root_account_id) DO UPDATE \
         SET workflow_state = 'active', updated_at = now() \
         RETURNING workflow_state",
    )
    .bind(auth.id)
    .bind(root_id)
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "workflow_state": workflow_state })),
    ))
}

/// DELETE /career/user_experiences — Canvas
/// `CanvasCareer::UserExperiencesController#destroy`.
async fn destroy_career_user_experience(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (root_id, settings) = root_account(&state.db).await?;
    if !career_affiliated(&settings) {
        return Err(ApiError::NotFound);
    }

    let workflow_state: Option<String> = sqlx::query_scalar(
        "UPDATE canvas_career_user_experiences \
         SET workflow_state = 'deleted', updated_at = now() \
         WHERE user_id = $1 AND root_account_id = $2 AND workflow_state = 'active' \
         RETURNING workflow_state",
    )
    .bind(auth.id)
    .bind(root_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(workflow_state) = workflow_state else {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "user experience not found" })),
        ));
    };

    Ok((
        StatusCode::OK,
        Json(json!({ "workflow_state": workflow_state })),
    ))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the career router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/career/enabled", get(career_enabled))
        .route("/career/experience_summary", get(career_experience_summary))
        .route("/career/switch_experience", post(career_switch_experience))
        .route("/career/switch_role", post(career_switch_role))
        .route(
            "/career/user_experiences",
            post(create_career_user_experience).delete(destroy_career_user_experience),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_career_flag_comes_from_the_horizon_account_ids() {
        assert!(!career_affiliated(&json!({})));
        assert!(!career_affiliated(&json!({ "horizon_account_ids": [] })));
        assert!(!career_affiliated(&json!({ "horizon_account_ids": null })));
        assert!(career_affiliated(&json!({ "horizon_account_ids": [1] })));
    }

    #[test]
    fn the_accepted_experiences_and_roles_match_canvas() {
        assert_eq!(EXPERIENCES, ["career", "academic"]);
        assert_eq!(ROLES, ["learner", "learning_provider"]);
    }
}
