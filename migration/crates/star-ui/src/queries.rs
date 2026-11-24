//! Thin data loaders for the shell handlers.
//!
//! Each loader either calls a pub loader extracted from a domain crate
//! (`star_courses::load_course_tabs`,
//! `star_pages::load_front_page`, `star_modules::load_modules`) or runs a
//! minimal, shell-specific query. No business logic is duplicated: the goal
//! (spec §2.2) is that the shell renders exactly what the REST API serves.

use serde_json::Value;
use sqlx::PgPool;

use star_common::ApiError;

use crate::env::{EnvPreferences, EnvSettings};

/// Load a user's display name. `None` when the user no longer exists (the
/// session extractor already rejects such users; this is a second line).
pub async fn load_user_name(db: &PgPool, user_id: i64) -> Result<Option<String>, ApiError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|(name,)| name))
}

/// Load one key from the `user_preferences` JSONB document (migration 0123).
///
/// Same read shape as star-users' `get_user_preference` helper (JSONB `->`
/// with a null default), kept local because the star-users helper is
/// handler-private. Returns `Value::Null` when unset.
async fn user_preference(db: &PgPool, user_id: i64, key: &str) -> Result<Value, ApiError> {
    let row: Option<(Value,)> = sqlx::query_as(
        "SELECT COALESCE(preferences -> $2::text, 'null'::jsonb) \
         FROM user_preferences WHERE user_id = $1",
    )
    .bind(user_id)
    .bind(key)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(v,)| v).unwrap_or(Value::Null))
}

/// Load `ENV.PREFERENCES` + `use_high_contrast` for a user.
///
/// Phase-1 defaults per spec §7.1 (no dedicated preference columns exist):
/// `dashboard_view: "cards"`, `custom_colors: {}`, both booleans false.
/// Values found in `user_preferences` (Canvas preference keys) win.
pub async fn load_preferences(
    db: &PgPool,
    user_id: i64,
) -> Result<(EnvPreferences, bool), ApiError> {
    let mut preferences = EnvPreferences::default();

    match user_preference(db, user_id, "dashboard_view").await? {
        Value::String(view) if view == "activity" => {
            preferences.dashboard_view = view;
        }
        Value::String(view) if view == "cards" => {
            preferences.dashboard_view = view;
        }
        _ => {}
    }

    if let Value::Object(colors) = user_preference(db, user_id, "custom_colors").await? {
        preferences.custom_colors = Value::Object(colors);
    }

    if let Value::Bool(flag) = user_preference(db, user_id, "hide_dashcard_color_overlays").await? {
        preferences.hide_dashcard_color_overlays = flag;
    }

    let use_high_contrast = matches!(
        user_preference(db, user_id, "use_high_contrast").await?,
        Value::Bool(true)
    );

    Ok((preferences, use_high_contrast))
}

/// Load `ENV.SETTINGS` (phase 1: only `collapse_global_nav`).
pub async fn load_settings(db: &PgPool, user_id: i64) -> Result<EnvSettings, ApiError> {
    let collapse = matches!(
        user_preference(db, user_id, "collapse_global_nav").await?,
        Value::Bool(true)
    );
    Ok(EnvSettings {
        collapse_global_nav: collapse,
    })
}
