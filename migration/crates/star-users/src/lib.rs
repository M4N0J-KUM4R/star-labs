//! star-users — user CRUD + profile handlers.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, put},
    Json, Router,
};
use serde::Deserialize;
use sqlx::PgPool;
use star_auth::{hash_password, AuthUser};
use star_common::{ApiError, AppState};

// --------------------------------------------------------------------------
// Domain types
// --------------------------------------------------------------------------

/// User record matching Canvas API v1 shape.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct UserResponse {
    pub id: i64,
    pub name: String,
    pub sortable_name: String,
    pub email: String,
    pub role: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Request to update a user.
#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
}

/// Pagination query parameters.
#[derive(Debug, Deserialize)]
pub struct Pagination {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// Paginated list response.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PaginatedUsers {
    pub data: Vec<UserResponse>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// Profile record matching the Canvas API v1 `Profile` model.
///
/// Identity fields come from `users`; the editable fields live in
/// `user_profiles`. `short_name` falls back to `name` and `primary_email`
/// falls back to `users.email`, mirroring Canvas.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct ProfileResponse {
    pub id: i64,
    pub name: String,
    pub short_name: String,
    pub sortable_name: String,
    pub title: Option<String>,
    pub bio: Option<String>,
    pub primary_email: String,
    pub avatar_url: Option<String>,
    pub time_zone: String,
    pub locale: String,
}

/// Request to update the caller's profile. Every field is optional; omitted
/// fields keep their current value, matching Canvas' partial update behavior.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateProfileRequest {
    pub title: Option<String>,
    pub bio: Option<String>,
    pub primary_email: Option<String>,
    pub avatar_url: Option<String>,
    pub time_zone: Option<String>,
    pub locale: Option<String>,
}

// --------------------------------------------------------------------------
// Permission helpers
// --------------------------------------------------------------------------

/// Look up a user's role from the DB.
async fn get_user_role(pool: &PgPool, user_id: i64) -> Result<Option<String>, ApiError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT role FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(row.map(|(r,)| r))
}

fn is_admin(role: &str) -> bool {
    role == "admin"
}

fn is_self(auth_id: i64, user_id: i64) -> bool {
    auth_id == user_id
}

fn pagination_values(page: Option<i64>, per_page: Option<i64>) -> (i64, i64, i64) {
    let page = page.unwrap_or(1).max(1);
    let per_page = per_page.unwrap_or(50).clamp(1, 100);
    // These values come directly from query parameters. Saturation keeps a
    // request such as `?page=9223372036854775807` from overflowing and
    // panicking the handler before PostgreSQL can return an empty page.
    let offset = page.saturating_sub(1).saturating_mul(per_page);
    (page, per_page, offset)
}

/// Canvas' limits for the profile text fields (`maximum_string_length` and
/// `maximum_text_length`).
const MAX_PROFILE_TITLE_LEN: usize = 255;
const MAX_PROFILE_BIO_LEN: usize = 65_535;

fn validate_profile_update(payload: &UpdateProfileRequest) -> Result<(), ApiError> {
    if payload
        .title
        .as_ref()
        .is_some_and(|title| title.chars().count() > MAX_PROFILE_TITLE_LEN)
    {
        return Err(ApiError::BadRequest(format!(
            "title is too long (maximum is {MAX_PROFILE_TITLE_LEN} characters)"
        )));
    }
    if payload
        .bio
        .as_ref()
        .is_some_and(|bio| bio.chars().count() > MAX_PROFILE_BIO_LEN)
    {
        return Err(ApiError::BadRequest(format!(
            "bio is too long (maximum is {MAX_PROFILE_BIO_LEN} characters)"
        )));
    }
    Ok(())
}

/// Load the Canvas-shaped profile for a user. Works whether or not a
/// `user_profiles` row exists yet: missing rows yield Canvas defaults.
async fn load_profile(pool: &PgPool, user_id: i64) -> Result<ProfileResponse, ApiError> {
    let profile: Option<ProfileResponse> = sqlx::query_as(
        r#"
        SELECT u.id,
               u.name,
               u.name AS short_name,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               p.title,
               p.bio,
               COALESCE(NULLIF(p.primary_email, ''), u.email) AS primary_email,
               p.avatar_url,
               COALESCE(p.time_zone, 'UTC') AS time_zone,
               COALESCE(p.locale, 'en') AS locale
        FROM users u
        LEFT JOIN user_profiles p ON p.user_id = u.id
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;

    profile.ok_or(ApiError::NotFound)
}

/// Create or partially update the profile row, then return the merged result.
/// `None` means "leave unchanged"; empty strings intentionally clear a field.
async fn upsert_profile(
    pool: &PgPool,
    user_id: i64,
    payload: &UpdateProfileRequest,
) -> Result<ProfileResponse, ApiError> {
    let profile: Option<ProfileResponse> = sqlx::query_as(
        r#"
        WITH upsert AS (
            INSERT INTO user_profiles
                (user_id, title, bio, primary_email, avatar_url, time_zone, locale)
            VALUES ($1, $2, $3, $4, $5, COALESCE($6, 'UTC'), COALESCE($7, 'en'))
            ON CONFLICT (user_id) DO UPDATE
            SET title         = COALESCE($2, user_profiles.title),
                bio           = COALESCE($3, user_profiles.bio),
                primary_email = COALESCE($4, user_profiles.primary_email),
                avatar_url    = COALESCE($5, user_profiles.avatar_url),
                time_zone     = COALESCE($6, user_profiles.time_zone),
                locale        = COALESCE($7, user_profiles.locale),
                updated_at    = now()
            RETURNING user_id, title, bio, primary_email, avatar_url, time_zone, locale
        )
        SELECT u.id,
               u.name,
               u.name AS short_name,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               upsert.title,
               upsert.bio,
               COALESCE(NULLIF(upsert.primary_email, ''), u.email) AS primary_email,
               upsert.avatar_url,
               upsert.time_zone,
               upsert.locale
        FROM users u
        JOIN upsert ON upsert.user_id = u.id
        "#,
    )
    .bind(user_id)
    .bind(&payload.title)
    .bind(&payload.bio)
    .bind(&payload.primary_email)
    .bind(&payload.avatar_url)
    .bind(&payload.time_zone)
    .bind(&payload.locale)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;

    profile.ok_or(ApiError::NotFound)
}

// --------------------------------------------------------------------------
// Router
// --------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/self", get(get_self))
        .route(
            "/users/self/profile",
            get(get_self_profile).put(update_self_profile),
        )
        .route("/users/{id}", get(get_user))
        .route("/users/{id}", put(update_user))
        .route("/users/{id}", delete(delete_user))
        .route("/users/{id}/profile", get(get_user_profile))
}

// --------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------

/// GET /users — list all users (admin only), paginated.
async fn list_users(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<PaginatedUsers>, ApiError> {
    let role = get_user_role(&state.db, auth.id).await?;
    let role = role.unwrap_or_else(|| "student".to_string());
    if !is_admin(&role) {
        return Err(ApiError::Forbidden);
    }

    let (page, per_page, offset) = pagination_values(pag.page, pag.per_page);

    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?;

    let users: Vec<UserResponse> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, email, role, created_at
        FROM users
        ORDER BY created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(PaginatedUsers {
        data: users,
        page,
        per_page,
        total,
    }))
}

/// GET /users/self — get the current authenticated user (alias for /auth/me behavior).
/// Queries the users table directly so sortable_name and the canonical
/// UserResponse shape (matching Canvas API v1) are returned.
async fn get_self(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<UserResponse>, ApiError> {
    let user: Option<UserResponse> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, email, role, created_at
        FROM users WHERE id = $1
        "#,
    )
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    user.ok_or(ApiError::NotFound).map(Json)
}

/// GET /users/self/profile — the caller's Canvas-shaped profile.
async fn get_self_profile(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<ProfileResponse>, ApiError> {
    load_profile(&state.db, auth.id).await.map(Json)
}

/// GET /users/{id}/profile — another user's profile (self or admin only).
async fn get_user_profile(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
) -> Result<Json<ProfileResponse>, ApiError> {
    let role = get_user_role(&state.db, auth.id).await?;
    let role = role.unwrap_or_else(|| "student".to_string());
    if !is_self(auth.id, user_id) && !is_admin(&role) {
        return Err(ApiError::Forbidden);
    }

    load_profile(&state.db, user_id).await.map(Json)
}

/// PUT /users/self/profile — create or partially update the caller's profile.
async fn update_self_profile(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<UpdateProfileRequest>,
) -> Result<Json<ProfileResponse>, ApiError> {
    validate_profile_update(&payload)?;
    upsert_profile(&state.db, auth.id, &payload).await.map(Json)
}

/// GET /users/{id} — get a single user (self or admin only).
async fn get_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
) -> Result<Json<UserResponse>, ApiError> {
    let role = get_user_role(&state.db, auth.id).await?;
    let role = role.unwrap_or_else(|| "student".to_string());
    if !is_self(auth.id, user_id) && !is_admin(&role) {
        return Err(ApiError::Forbidden);
    }

    let user: Option<UserResponse> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, email, role, created_at
        FROM users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    user.ok_or(ApiError::NotFound).map(Json)
}

/// PUT /users/{id} — update a user (self or admin).
/// Re-hashes password with argon2 if password field is present.
/// Checks email uniqueness before update.
async fn update_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
    Json(payload): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());
    if !is_self(auth.id, user_id) && !is_admin(&auth_role) {
        return Err(ApiError::Forbidden);
    }

    // Verify the user exists.
    let existing: Option<UserResponse> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, email, role, created_at
        FROM users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    let existing = existing.ok_or(ApiError::NotFound)?;

    // If email is being changed, check uniqueness.
    let email = payload.email.clone().unwrap_or(existing.email.clone());
    if payload.email.is_some() && email != existing.email {
        let taken: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM users WHERE email = $1 AND id != $2")
                .bind(&email)
                .bind(user_id)
                .fetch_optional(&state.db)
                .await
                .map_err(ApiError::from)?;

        if taken.is_some() {
            return Err(ApiError::Conflict("email already in use".into()));
        }
    }

    let name = payload.name.clone().unwrap_or(existing.name);

    // Build the update query — handle password_hash separately.
    let updated: UserResponse = if let Some(ref pw) = payload.password {
        let hash = hash_password(pw).map_err(ApiError::BadRequest)?;
        sqlx::query_as(
            r#"
            UPDATE users
            SET name = $2, email = $3, password_hash = $4, updated_at = now()
            WHERE id = $1
            RETURNING id, name, sortable_name, email, role, created_at
            "#,
        )
        .bind(user_id)
        .bind(&name)
        .bind(&email)
        .bind(&hash)
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?
    } else {
        sqlx::query_as(
            r#"
            UPDATE users
            SET name = $2, email = $3, updated_at = now()
            WHERE id = $1
            RETURNING id, name, sortable_name, email, role, created_at
            "#,
        )
        .bind(user_id)
        .bind(&name)
        .bind(&email)
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?
    };

    Ok(Json(updated))
}

/// DELETE /users/{id} — delete a user (admin only).
async fn delete_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let role = get_user_role(&state.db, auth.id).await?;
    let role = role.unwrap_or_else(|| "student".to_string());
    if !is_admin(&role) {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(ApiError::from)?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::pagination_values;

    #[test]
    fn pagination_clamps_limits_and_handles_extreme_page_numbers() {
        assert_eq!(pagination_values(None, None), (1, 50, 0));
        assert_eq!(pagination_values(Some(0), Some(500)), (1, 100, 0));
        assert_eq!(
            pagination_values(Some(i64::MAX), Some(100)),
            (i64::MAX, 100, i64::MAX)
        );
    }
}
