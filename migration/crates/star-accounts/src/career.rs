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
//! This is a direct port of the Canvas behavior:
//!
//! - `CanvasCareer::ExperienceResolver.career_affiliated_institution?` is
//!   `root_account.settings[:horizon_account_ids].present?`, so the flag comes
//!   from the settings of the root account.
//! - `CareerExperienceController#experience_summary` renders the resolver's
//!   `resolve` (the current app) and `available_apps`; the resolver is ported
//!   in full below, including the enrollment types, the enrollment roles, the
//!   account users and the career user experience.
//! - `CanvasCareer::UserPreferenceManager` keeps the preferred experience and
//!   the preferred role. Canvas stores them in the session, the port keeps
//!   them in the `user_preferences` document. The defaults are the Canvas
//!   ones: the experience defaults to `academic` and the role defaults to
//!   `learning_provider`.
//! - `CanvasCareer::Config` reads the launch urls of the two career apps;
//!   Canvas keeps them in the `canvas_career.yml` dynamic setting, the port
//!   keeps them in `settings[:canvas_career_config]` of the root account.
//! - `switch_experience` only accepts `career` and `academic`, `switch_role`
//!   only `learner` and `learning_provider`; anything else is the Canvas 400
//!   `{"error": "invalid_experience"}` / `{"error": "invalid_role"}`.
//! - the user experience routes need a career affiliated root account, so they
//!   answer 404 without one.

use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use std::collections::HashMap;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `CanvasCareer::Constants::Experience.all`.
const EXPERIENCES: [&str; 2] = ["career", "academic"];

/// Canvas `CanvasCareer::Constants::Role.all`.
const ROLES: [&str; 2] = ["learner", "learning_provider"];

/// Canvas `CanvasCareer::Constants::App::ACADEMIC`.
const APP_ACADEMIC: &str = "academic";

/// Canvas `CanvasCareer::Constants::App::CAREER_LEARNER`.
const APP_CAREER_LEARNER: &str = "career_learner";

/// Canvas `CanvasCareer::Constants::App::CAREER_LEARNING_PROVIDER`.
const APP_CAREER_LEARNING_PROVIDER: &str = "career_learning_provider";

/// Canvas `ExperienceResolver::LEARNER_ENROLLMENT_TYPES` — the enrollment
/// roles of a learner.
const LEARNER_ROLES: [&str; 1] = ["student"];

/// Canvas `ExperienceResolver::LEARNING_PROVIDER_ENROLLMENT_TYPES` — the
/// enrollment roles of a learning provider.
const LEARNING_PROVIDER_ROLES: [&str; 3] = ["teacher", "ta", "designer"];

/// The user preference that holds the preferred experience
/// (Canvas `UserPreferenceManager::EXPERIENCE_PREFERENCE_SESSION_KEY`).
const EXPERIENCE_KEY: &str = "career_experience";

/// The user preference that holds the preferred role
/// (Canvas `UserPreferenceManager::ROLE_PREFERENCE_SESSION_KEY`).
const ROLE_KEY: &str = "career_role";

// ---------------------------------------------------------------------------
// The resolver — Canvas `CanvasCareer::ExperienceResolver`
// ---------------------------------------------------------------------------

/// Canvas `CanvasCareer::ExperienceResolver` in its contextless form, which is
/// the one the API uses.
struct ExperienceResolver {
    /// `career_at_root_account?`
    career_at_root: bool,
    /// `has_career_associations?`
    has_career_associations: bool,
    /// `has_academic_associations?`
    has_academic_associations: bool,
    /// `has_career_learner_roles?`
    has_career_learner_roles: bool,
    /// `has_career_learning_provider_roles?`
    has_career_learning_provider_roles: bool,
    /// `prefers_academic?`
    prefers_academic: bool,
    /// `prefers_career?`
    prefers_career: bool,
    /// `prefers_learner?`
    prefers_learner: bool,
    /// `prefers_learning_provider?`
    prefers_learning_provider: bool,
    /// `config.learner_app_launch_url.present?`
    learner_app_launch_url: bool,
    /// `config.learning_provider_app_launch_url.present?`
    learning_provider_app_launch_url: bool,
}

impl ExperienceResolver {
    /// Canvas `ExperienceResolver#resolve`.
    fn resolve(&self) -> &'static str {
        if self.resolve_academic() {
            return APP_ACADEMIC;
        }
        if self.resolve_learning_provider() {
            return APP_CAREER_LEARNING_PROVIDER;
        }
        if self.resolve_learner() {
            return APP_CAREER_LEARNER;
        }
        // No enrollments but career at the root account defaults to the
        // career learner app.
        if self.career_at_root {
            return APP_CAREER_LEARNER;
        }
        APP_ACADEMIC
    }

    /// Canvas `ExperienceResolver#resolve_academic?`.
    fn resolve_academic(&self) -> bool {
        self.has_academic_associations && (self.prefers_academic || !self.has_career_associations)
    }

    /// Canvas `ExperienceResolver#resolve_learning_provider?`.
    fn resolve_learning_provider(&self) -> bool {
        self.learning_provider_app_launch_url
            && self.has_career_associations
            && (self.prefers_career || !self.has_academic_associations)
            && self.has_career_learning_provider_roles
            && (self.prefers_learning_provider || !self.has_career_learner_roles)
    }

    /// Canvas `ExperienceResolver#resolve_learner?`.
    fn resolve_learner(&self) -> bool {
        self.learner_app_launch_url
            && self.has_career_associations
            && (self.prefers_career || !self.has_academic_associations)
            && self.has_career_learner_roles
            && (self.prefers_learner || !self.has_career_learning_provider_roles)
    }

    /// Canvas `ExperienceResolver#available_apps`.
    fn available_apps(&self) -> Vec<&'static str> {
        let mut apps = Vec::new();
        if self.has_academic_associations {
            apps.push(APP_ACADEMIC);
        }
        if self.has_career_learning_provider_roles {
            apps.push(APP_CAREER_LEARNING_PROVIDER);
        }
        if self.has_career_learner_roles {
            apps.push(APP_CAREER_LEARNER);
        }
        apps
    }
}

/// The career account ids of the root account
/// (`root_account.settings[:horizon_account_ids]`).
fn career_account_ids(settings: &Value) -> Vec<i64> {
    match settings.get("horizon_account_ids") {
        Some(Value::Array(items)) => items.iter().filter_map(|item| item.as_i64()).collect(),
        _ => Vec::new(),
    }
}

/// Canvas `Account#horizon_account?`: the setting of the account and the
/// `horizon_course_setting` feature of the account.
async fn horizon_account(
    pool: &PgPool,
    account_id: i64,
    settings: &Value,
) -> Result<bool, ApiError> {
    let value = settings
        .get("horizon_account")
        .and_then(|value| value.get("value"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !value {
        return Ok(false);
    }
    feature_enabled(pool, account_id, "horizon_course_setting").await
}

/// Canvas `Account#feature_enabled?`.
async fn feature_enabled(pool: &PgPool, account_id: i64, feature: &str) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM feature_flags \
         WHERE context_type = 'Account' AND context_id = $1 AND feature = $2 \
           AND state = 'enabled' LIMIT 1",
    )
    .bind(account_id)
    .bind(feature)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `ExperienceResolver#enrollment_types`: the horizon flags of the
/// courses the user is enrolled in.
async fn enrollment_types(pool: &PgPool, user_id: i64) -> Result<Vec<bool>, ApiError> {
    let rows: Vec<(bool,)> = sqlx::query_as(
        "SELECT DISTINCT c.horizon_course FROM courses c \
         JOIN enrollments e ON e.course_id = c.id \
         WHERE e.user_id = $1 AND e.workflow_state = 'active' \
           AND c.workflow_state <> 'deleted'",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(flag,)| flag).collect())
}

/// Canvas `ExperienceResolver#career_enrollment_roles`: the roles of the user
/// in the horizon courses.
async fn career_enrollment_roles(pool: &PgPool, user_id: i64) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT e.role FROM enrollments e \
         JOIN courses c ON c.id = e.course_id \
         WHERE e.user_id = $1 AND e.workflow_state = 'active' \
           AND c.workflow_state <> 'deleted' AND c.horizon_course = true",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(role,)| role).collect())
}

/// Canvas `ExperienceResolver#account_user_account_ids`: the accounts the user
/// is an active account user of.
async fn account_user_account_ids(pool: &PgPool, user_id: i64) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT account_id FROM account_users \
         WHERE user_id = $1 AND workflow_state = 'active'",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Canvas `Account.account_chain_ids_for_multiple_accounts`: for every
/// account of the list the ids of its chain.
async fn account_chain_map(
    pool: &PgPool,
    account_ids: &[i64],
) -> Result<HashMap<i64, Vec<i64>>, ApiError> {
    let mut chains: HashMap<i64, Vec<i64>> = HashMap::new();
    for account_id in account_ids {
        let rows: Vec<(i64,)> = sqlx::query_as(
            r#"
            WITH RECURSIVE chain AS (
                SELECT id, parent_account_id FROM accounts WHERE id = $1
                UNION
                SELECT a.id, a.parent_account_id FROM accounts a JOIN chain c ON a.id = c.parent_account_id
            )
            SELECT id FROM chain
            "#,
        )
        .bind(account_id)
        .fetch_all(pool)
        .await?;
        chains.insert(*account_id, rows.into_iter().map(|(id,)| id).collect());
    }
    Ok(chains)
}

/// Canvas `ExperienceResolver#user_experience_exists?`.
async fn user_experience_exists(
    pool: &PgPool,
    user_id: i64,
    root_account_id: i64,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM canvas_career_user_experiences \
         WHERE user_id = $1 AND root_account_id = $2 AND workflow_state = 'active' LIMIT 1",
    )
    .bind(user_id)
    .bind(root_account_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
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

/// Build the resolver of a user.
async fn resolver(pool: &PgPool, user_id: i64) -> Result<ExperienceResolver, ApiError> {
    let root: Option<(i64, Value)> = sqlx::query_as(
        "SELECT id, settings FROM accounts \
         WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    let Some((root_account_id, settings)) = root else {
        return Ok(ExperienceResolver {
            career_at_root: false,
            has_career_associations: false,
            has_academic_associations: true,
            has_career_learner_roles: false,
            has_career_learning_provider_roles: false,
            prefers_academic: true,
            prefers_career: false,
            prefers_learner: false,
            prefers_learning_provider: true,
            learner_app_launch_url: false,
            learning_provider_app_launch_url: false,
        });
    };

    let career_ids = career_account_ids(&settings);
    let career_unaffiliated = career_ids.is_empty();
    let career_at_root = horizon_account(pool, root_account_id, &settings).await?;

    // The launch urls of the two career apps.
    let config = settings
        .get("canvas_career_config")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));
    let learner_app_launch_url = config
        .get("learner_launch_url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.trim().is_empty());
    let learning_provider_app_launch_url = config
        .get("learning_provider_launch_url")
        .and_then(Value::as_str)
        .is_some_and(|url| !url.trim().is_empty());

    // The preferences of the user.
    let experience = read_preference(pool, user_id, EXPERIENCE_KEY).await?;
    let role = read_preference(pool, user_id, ROLE_KEY).await?;
    let prefers_academic = experience.as_deref() != Some("career");
    let prefers_career = experience.as_deref() == Some("career");
    let prefers_learner = role.as_deref() == Some("learner");
    let prefers_learning_provider = role.as_deref() != Some("learner");

    // The associations of the user.
    let enrollment_types = enrollment_types(pool, user_id).await?;
    let enrollment_roles = career_enrollment_roles(pool, user_id).await?;
    let account_ids = account_user_account_ids(pool, user_id).await?;
    let chains = account_chain_map(pool, &account_ids).await?;
    let experience_exists = user_experience_exists(pool, user_id, root_account_id).await?;

    // Canvas: an account of the user is a career account when its chain holds
    // one of the career account ids, and an academic account otherwise.
    let has_career_account_users = chains
        .values()
        .any(|chain| chain.iter().any(|id| career_ids.contains(id)));
    let has_academic_account_users = if account_ids.is_empty() {
        false
    } else if career_ids.is_empty() {
        true
    } else {
        chains
            .values()
            .any(|chain| !chain.iter().any(|id| career_ids.contains(id)))
    };

    let has_career_associations = !career_unaffiliated
        && (career_at_root
            || enrollment_types.contains(&true)
            || has_career_account_users
            || experience_exists);

    let has_academic_associations = career_unaffiliated
        || (!career_at_root && (enrollment_types.contains(&false) || has_academic_account_users));

    let has_career_learner_roles = !career_unaffiliated
        && (enrollment_roles
            .iter()
            .any(|role| LEARNER_ROLES.contains(&role.as_str()))
            || experience_exists);

    let has_career_learning_provider_roles = !career_unaffiliated
        && (enrollment_roles
            .iter()
            .any(|role| LEARNING_PROVIDER_ROLES.contains(&role.as_str()))
            || has_career_account_users);

    Ok(ExperienceResolver {
        career_at_root,
        has_career_associations,
        has_academic_associations,
        has_career_learner_roles,
        has_career_learning_provider_roles,
        prefers_academic,
        prefers_career,
        prefers_learner,
        prefers_learning_provider,
        learner_app_launch_url,
        learning_provider_app_launch_url,
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /career/enabled — Canvas `enabled`.
async fn career_enabled(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let row: Option<(Value,)> = sqlx::query_as(
        "SELECT settings FROM accounts \
         WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;
    let settings = row.map(|(settings,)| settings).unwrap_or(Value::Null);

    // Canvas: career_affiliated_institution?(root_account)
    let affiliated = !career_account_ids(&settings).is_empty();
    Ok(Json(json!({ "enabled": affiliated })))
}

/// GET /career/experience_summary — Canvas `experience_summary`.
async fn career_experience_summary(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let resolver = resolver(&state.db, auth.id).await?;
    Ok(Json(json!({
        "current_app": resolver.resolve(),
        "available_apps": resolver.available_apps(),
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
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts \
         WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;
    let Some((root_account_id,)) = row else {
        return Err(ApiError::NotFound);
    };

    // Canvas: require_feature_flag -> 404 without a career affiliated account.
    let settings: Option<(Value,)> = sqlx::query_as("SELECT settings FROM accounts WHERE id = $1")
        .bind(root_account_id)
        .fetch_optional(&state.db)
        .await?;
    let settings = settings.map(|(settings,)| settings).unwrap_or(Value::Null);
    if career_account_ids(&settings).is_empty() {
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
    .bind(root_account_id)
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
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts \
         WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;
    let Some((root_account_id,)) = row else {
        return Err(ApiError::NotFound);
    };

    let settings: Option<(Value,)> = sqlx::query_as("SELECT settings FROM accounts WHERE id = $1")
        .bind(root_account_id)
        .fetch_optional(&state.db)
        .await?;
    let settings = settings.map(|(settings,)| settings).unwrap_or(Value::Null);
    if career_account_ids(&settings).is_empty() {
        return Err(ApiError::NotFound);
    }

    let workflow_state: Option<String> = sqlx::query_scalar(
        "UPDATE canvas_career_user_experiences \
         SET workflow_state = 'deleted', updated_at = now() \
         WHERE user_id = $1 AND root_account_id = $2 AND workflow_state = 'active' \
         RETURNING workflow_state",
    )
    .bind(auth.id)
    .bind(root_account_id)
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

    fn resolver() -> ExperienceResolver {
        ExperienceResolver {
            career_at_root: false,
            has_career_associations: false,
            has_academic_associations: true,
            has_career_learner_roles: false,
            has_career_learning_provider_roles: false,
            prefers_academic: true,
            prefers_career: false,
            prefers_learner: false,
            prefers_learning_provider: true,
            learner_app_launch_url: true,
            learning_provider_app_launch_url: true,
        }
    }

    #[test]
    fn the_career_flag_comes_from_the_horizon_account_ids() {
        assert!(career_account_ids(&json!({})).is_empty());
        assert!(career_account_ids(&json!({ "horizon_account_ids": [] })).is_empty());
        assert!(career_account_ids(&json!({ "horizon_account_ids": null })).is_empty());
        assert_eq!(
            career_account_ids(&json!({ "horizon_account_ids": [3, 4] })),
            vec![3, 4]
        );
    }

    #[test]
    fn the_accepted_experiences_and_roles_match_canvas() {
        assert_eq!(EXPERIENCES, ["career", "academic"]);
        assert_eq!(ROLES, ["learner", "learning_provider"]);
    }

    #[test]
    fn an_academic_user_resolves_to_the_academic_app() {
        let resolver = resolver();
        assert_eq!(resolver.resolve(), APP_ACADEMIC);
        assert_eq!(resolver.available_apps(), vec![APP_ACADEMIC]);
    }

    #[test]
    fn a_learner_with_a_career_enrollment_resolves_to_the_learner_app() {
        let mut resolver = resolver();
        resolver.has_academic_associations = false;
        resolver.has_career_associations = true;
        resolver.has_career_learner_roles = true;
        resolver.prefers_academic = false;
        resolver.prefers_career = true;
        assert_eq!(resolver.resolve(), APP_CAREER_LEARNER);
        assert_eq!(resolver.available_apps(), vec![APP_CAREER_LEARNER]);
    }

    #[test]
    fn a_learning_provider_resolves_to_the_provider_app() {
        let mut resolver = resolver();
        resolver.has_academic_associations = false;
        resolver.has_career_associations = true;
        resolver.has_career_learning_provider_roles = true;
        resolver.prefers_academic = false;
        resolver.prefers_career = true;
        assert_eq!(resolver.resolve(), APP_CAREER_LEARNING_PROVIDER);
    }

    #[test]
    fn a_career_root_account_defaults_to_the_learner_app() {
        let mut resolver = resolver();
        resolver.has_academic_associations = false;
        resolver.has_career_associations = true;
        resolver.career_at_root = true;
        resolver.prefers_academic = false;
        resolver.prefers_career = true;
        assert_eq!(resolver.resolve(), APP_CAREER_LEARNER);
    }
}
