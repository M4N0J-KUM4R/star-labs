//! star-users parity gaps (PORT-B) — four Canvas controllers.
//!
//! Controllers implemented here (Canvas name → routes):
//! - `FavoritesController` (groups scope) — favorite groups list/add/remove/
//!   reset over the existing `favorites` (0097, context_type='Group') +
//!   `groups` (0018) tables.
//! - `NotificationPreferencesController` — per-notification update,
//!   update_all, and update-by-category over `communication_channels` +
//!   `notification_preferences` (0045).
//! - `ObserverPairingCodesApiController` — POST /users/:user_id/
//!   observer_pairing_codes (table 0225).
//! - `AvatarsController#profile_pics` — GET /users/:user_id/avatars.
//!
//! The page-view/pseudonym/bookmark/course-nickname/observer-alert-threshold
//! handlers of the same backup batch are already served by `users_api.rs` and
//! `observer_alerts_api.rs` and are NOT re-registered here.
//!
//! Schema stances (existing tables reused, no new tables):
//! - `favorites` keeps Canvas' context pair (context_type/context_id); group
//!   favorites are rows with context_type='Group' (0097 only seeded 'Course').
//! - `observer_pairing_codes` (0225) uses naive `timestamp` columns, so the
//!   expiry is bound as a naive UTC datetime.
//! - avatars: Canvas gravatar tokens are the MD5 of the lowercased email;
//!   this workspace has no md5 crate, so a deterministic FNV-1a 64-bit hex
//!   digest of the email is used as the token (documented stance).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// api_find(User): 404 for an unknown user id.
async fn require_user(pool: &PgPool, user_id: i64) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// Self-or-admin gate (Canvas `:user_id` == caller || site admin).
fn authorized_self_or_admin(user_id: i64, auth: &AuthUser) -> bool {
    auth.id == user_id || auth.role == "admin"
}

// ---------------------------------------------------------------------------
// Favorites (groups scope) — Canvas FavoritesController
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct GroupRow {
    id: i64,
    name: String,
    description: Option<String>,
    join_level: String,
    group_category_id: Option<i64>,
    course_id: Option<i64>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn group_json(g: &GroupRow) -> Value {
    json!({
        "id": g.id,
        "name": g.name,
        "description": g.description,
        "join_level": g.join_level,
        "group_category_id": g.group_category_id,
        "course_id": g.course_id,
        "workflow_state": g.workflow_state,
        "created_at": g.created_at.to_rfc3339(),
        "updated_at": g.updated_at.to_rfc3339(),
    })
}

/// Canvas: favorites_controller#list_favorite_groups —
/// GET /api/v1/users/self/favorites/groups
async fn favorite_groups_index(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    let rows: Vec<GroupRow> = sqlx::query_as(
        "SELECT g.id, g.name, g.description, g.join_level, g.group_category_id, \
                g.course_id, g.workflow_state, g.created_at, g.updated_at \
         FROM favorites f JOIN groups g ON g.id = f.context_id \
         WHERE f.user_id = $1 AND f.context_type = 'Group' \
         ORDER BY g.id",
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.iter().map(group_json).collect()))
}

/// Canvas: favorites_controller#add_favorite_groups —
/// POST /api/v1/users/self/favorites/groups/:id
async fn add_favorite_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    sqlx::query(
        "INSERT INTO favorites (user_id, context_type, context_id) \
         VALUES ($1, 'Group', $2) \
         ON CONFLICT (user_id, context_type, context_id) DO NOTHING",
    )
    .bind(auth.id)
    .bind(group_id)
    .execute(&state.db)
    .await?;
    let row: Option<GroupRow> = sqlx::query_as(
        "SELECT id, name, description, join_level, group_category_id, course_id, \
                workflow_state, created_at, updated_at \
         FROM groups WHERE id = $1",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;
    match row {
        // Inserting a favorite for an unknown group still leaves the
        // favorites row; the API surface mirrors Canvas' 404 for unknown
        // groups, so remove the orphaned row again.
        None => {
            sqlx::query(
                "DELETE FROM favorites WHERE user_id = $1 AND context_type = 'Group' \
                 AND context_id = $2",
            )
            .bind(auth.id)
            .bind(group_id)
            .execute(&state.db)
            .await?;
            Err(ApiError::NotFound)
        }
        Some(g) => Ok(Json(group_json(&g))),
    }
}

/// Canvas: favorites_controller#remove_favorite_groups —
/// DELETE /api/v1/users/self/favorites/groups/:id
async fn remove_favorite_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let result = sqlx::query(
        "DELETE FROM favorites WHERE user_id = $1 AND context_type = 'Group' AND context_id = $2",
    )
    .bind(auth.id)
    .bind(group_id)
    .execute(&state.db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Canvas: favorites_controller#reset_groups_favorites —
/// DELETE /api/v1/users/self/favorites/groups
async fn favorite_groups_reset(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    sqlx::query("DELETE FROM favorites WHERE user_id = $1 AND context_type = 'Group'")
        .bind(auth.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Notification preferences — Canvas NotificationPreferencesController
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct PolicyRow {
    notification: String,
    category: String,
    frequency: String,
}

fn policy_json(p: &PolicyRow) -> Value {
    json!({
        "notification": p.notification,
        "category": p.category,
        "frequency": p.frequency,
    })
}

const FREQUENCIES: [&str; 4] = ["immediately", "daily", "weekly", "never"];

const NOTIFICATION_CATEGORIES: [&str; 12] = [
    "all",
    "course_activities",
    "discussions",
    "due_date",
    "grading",
    "invitations",
    "grouping",
    "admin",
    "student_association_created",
    "conversation",
    "files",
    "other",
];

/// Canvas notification-name → category mapping for preferences upserted for
/// the first time (existing rows keep their stored category).
fn notification_category(notification: &str) -> &'static str {
    match notification {
        "new_announcement" | "announcement_created_by_student" => "course_activities",
        "new_discussion_topic" | "discussion_entry" | "discussion_mention" => "discussions",
        "assignment_due_date_changed" | "assignment_created" | "assignment_updated" => "due_date",
        "submission_grade_change" | "submission_comment" | "grade_change" => "grading",
        "invitation" | "new_course" | "enrollment_invitation" => "invitations",
        "added_to_conversation" | "conversation_message" => "conversation",
        _ => "other",
    }
}

fn validate_frequency(frequency: &str) -> Result<(), ApiError> {
    if FREQUENCIES.contains(&frequency) {
        Ok(())
    } else {
        Err(ApiError::Validation("invalid frequency".to_string()))
    }
}

async fn find_channel_by_path(
    pool: &PgPool,
    user_id: i64,
    path_type: &str,
    path: &str,
) -> Result<Option<i64>, ApiError> {
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM communication_channels \
         WHERE user_id = $1 AND path_type = $2 AND path = $3 \
         ORDER BY id LIMIT 1",
    )
    .bind(user_id)
    .bind(path_type)
    .bind(path)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

async fn channel_policies(pool: &PgPool, channel_id: i64) -> Result<Vec<PolicyRow>, ApiError> {
    let rows: Vec<PolicyRow> = sqlx::query_as(
        "SELECT notification, category, frequency FROM notification_preferences \
         WHERE communication_channel_id = $1 ORDER BY notification",
    )
    .bind(channel_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

async fn upsert_policy(
    pool: &PgPool,
    user_id: i64,
    channel_id: i64,
    notification: &str,
    frequency: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO notification_preferences \
            (user_id, communication_channel_id, notification, category, frequency) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (communication_channel_id, notification) \
         DO UPDATE SET frequency = EXCLUDED.frequency, updated_at = now()",
    )
    .bind(user_id)
    .bind(channel_id)
    .bind(notification)
    .bind(notification_category(notification))
    .bind(frequency)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct FrequencyParams {
    frequency: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FrequencyBody {
    notification_preferences: Option<FrequencyParams>,
    #[serde(flatten)]
    flat: FrequencyParams,
}

fn body_frequency(body: Option<FrequencyBody>) -> Result<String, ApiError> {
    let freq = body
        .and_then(|b| b.notification_preferences.or(Some(b.flat)))
        .and_then(|p| p.frequency)
        .ok_or_else(|| ApiError::Validation("frequency is required".to_string()))?;
    validate_frequency(&freq)?;
    Ok(freq)
}

/// Canvas: notification_preferences_controller#update —
/// PUT /api/v1/users/self/communication_channels/:type/:address/notification_preferences/:notification
async fn update_notification_preference(
    State(state): State<AppState>,
    Path((channel_type, address, notification)): Path<(String, String, String)>,
    auth: AuthUser,
    body: Option<Json<FrequencyBody>>,
) -> Result<Json<Value>, ApiError> {
    let frequency = body_frequency(body.map(|Json(b)| b))?;
    let channel_id = find_channel_by_path(&state.db, auth.id, &channel_type, &address)
        .await?
        .ok_or(ApiError::NotFound)?;
    upsert_policy(&state.db, auth.id, channel_id, &notification, &frequency).await?;
    Ok(Json(json!({
        "notification_preferences": [{
            "notification": notification,
            "category": notification_category(&notification),
            "frequency": frequency,
        }]
    })))
}

#[derive(Debug, Deserialize)]
struct PrefItem {
    notification: Option<String>,
    frequency: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PrefsPayload {
    List(Vec<PrefItem>),
    Single(PrefItem),
}

#[derive(Debug, Deserialize)]
struct UpdateAllBody {
    notification_preferences: Option<PrefsPayload>,
}

/// Canvas: notification_preferences_controller#update_all —
/// PUT /api/v1/users/self/communication_channels/:type/:address/notification_preferences
async fn update_all_notification_preferences(
    State(state): State<AppState>,
    Path((channel_type, address)): Path<(String, String)>,
    auth: AuthUser,
    body: Option<Json<UpdateAllBody>>,
) -> Result<Json<Value>, ApiError> {
    let channel_id = find_channel_by_path(&state.db, auth.id, &channel_type, &address)
        .await?
        .ok_or(ApiError::NotFound)?;
    let items = match body.and_then(|Json(b)| b.notification_preferences) {
        None => {
            return Err(ApiError::Validation(
                "notification_preferences is required".to_string(),
            ))
        }
        Some(PrefsPayload::List(items)) => items,
        Some(PrefsPayload::Single(item)) => vec![item],
    };
    for item in &items {
        if item
            .notification
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none()
        {
            return Err(ApiError::Validation("notification is required".to_string()));
        }
        validate_frequency(&item.frequency)?;
    }
    for item in &items {
        if let Some(notification) = item.notification.as_deref() {
            upsert_policy(
                &state.db,
                auth.id,
                channel_id,
                notification,
                &item.frequency,
            )
            .await?;
        }
    }
    let policies = channel_policies(&state.db, channel_id).await?;
    Ok(Json(json!({
        "notification_preferences": policies.iter().map(policy_json).collect::<Vec<_>>()
    })))
}

/// Canvas: notification_preferences_controller#update_preferences_by_category —
/// PUT /api/v1/users/self/communication_channels/:communication_channel_id/
/// notification_preference_categories/:category
async fn update_preferences_by_category(
    State(state): State<AppState>,
    Path((channel_key, category)): Path<(String, String)>,
    auth: AuthUser,
    body: Option<Json<FrequencyBody>>,
) -> Result<Json<Value>, ApiError> {
    if !NOTIFICATION_CATEGORIES.contains(&category.as_str()) {
        return Err(ApiError::Validation("invalid category".to_string()));
    }
    let frequency = body_frequency(body.map(|Json(b)| b))?;
    let channel_id: i64 = match channel_key.parse() {
        Ok(id) => id,
        Err(_) => return Err(ApiError::NotFound),
    };
    let channel: Option<i64> =
        sqlx::query_scalar("SELECT id FROM communication_channels WHERE id = $1 AND user_id = $2")
            .bind(channel_id)
            .bind(auth.id)
            .fetch_optional(&state.db)
            .await?;
    let channel_id = channel.ok_or(ApiError::NotFound)?;

    if category == "all" {
        sqlx::query(
            "UPDATE notification_preferences SET frequency = $2, updated_at = now() \
             WHERE communication_channel_id = $1",
        )
        .bind(channel_id)
        .bind(&frequency)
        .execute(&state.db)
        .await?;
    } else {
        sqlx::query(
            "UPDATE notification_preferences SET frequency = $3, updated_at = now() \
             WHERE communication_channel_id = $1 AND category = $2",
        )
        .bind(channel_id)
        .bind(&category)
        .bind(&frequency)
        .execute(&state.db)
        .await?;
    }
    let policies = channel_policies(&state.db, channel_id).await?;
    Ok(Json(json!({
        "notification_preferences": policies.iter().map(policy_json).collect::<Vec<_>>()
    })))
}

// ---------------------------------------------------------------------------
// Observer pairing codes — Canvas ObserverPairingCodesApiController
// ---------------------------------------------------------------------------

/// Random 6-character alphanumeric pairing code (Canvas' pairing codes are
/// 6-char alphanumeric). Uses argon2's OsRng — the only RNG in the crate.
fn random_code() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz0123456789";
    let mut bytes = [0u8; 6];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[(*b % ALPHABET.len() as u8) as usize] as char)
        .collect()
}

/// Whether `teacher_id` actively teaches a course `student_id` is actively
/// in (Canvas' teacher-of-student gate for pairing codes).
async fn teacher_of_user(
    pool: &PgPool,
    teacher_id: i64,
    student_id: i64,
) -> Result<bool, ApiError> {
    let teaches: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
            SELECT 1 FROM enrollments te \
            JOIN enrollments se ON se.course_id = te.course_id \
            WHERE te.user_id = $1 AND te.role IN ('teacher', 'ta') \
              AND te.workflow_state = 'active' \
              AND se.user_id = $2 AND se.workflow_state = 'active')",
    )
    .bind(teacher_id)
    .bind(student_id)
    .fetch_one(pool)
    .await?;
    Ok(teaches)
}

/// Canvas: observer_pairing_codes_api#create —
/// POST /api/v1/users/:user_id/observer_pairing_codes
///
/// Gate: the user themselves, a teacher of a course the user is in, or an
/// admin. Codes expire after 7 days like Canvas'.
async fn create_pairing_code(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_user(&state.db, user_id).await?;
    let allowed = auth.id == user_id
        || auth.role == "admin"
        || teacher_of_user(&state.db, auth.id, user_id).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    let code = random_code();
    let expires_at = Utc::now() + chrono::Duration::days(7);
    sqlx::query(
        "INSERT INTO observer_pairing_codes \
            (user_id, code, expires_at, workflow_state, created_at, updated_at) \
         VALUES ($1, $2, $3, 'active', now(), now())",
    )
    .bind(user_id)
    .bind(&code)
    .bind(expires_at.naive_utc())
    .execute(&state.db)
    .await?;
    Ok(Json(json!({
        "pairing_code": {
            "user_id": user_id,
            "code": code,
            "expires_at": expires_at.to_rfc3339(),
            "workflow_state": "active",
        }
    })))
}

// ---------------------------------------------------------------------------
// Avatars — Canvas AvatarsController#profile_pics
// ---------------------------------------------------------------------------

/// Deterministic FNV-1a 64-bit digest, hex-encoded. Canvas gravatar tokens
/// are the MD5 of the lowercased email; this workspace has no md5 crate, so
/// FNV-1a stands in as a stable per-email token (documented stance).
fn fnv1a64_hex(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Canvas: avatars_controller#profile_pics — GET /api/v1/users/:user_id/avatars
///
/// Self or admin (Canvas "view avatars" right). Deterministic stance: one
/// gravatar entry derived from the email, or {"type": "none"} when the user
/// has no email.
async fn profile_pics(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_user(&state.db, user_id).await?;
    if !authorized_self_or_admin(user_id, &auth) {
        return Err(ApiError::Forbidden);
    }
    let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
    let avatars = if email.trim().is_empty() {
        vec![json!({ "type": "none" })]
    } else {
        let token = fnv1a64_hex(email.trim().to_lowercase().as_bytes());
        vec![json!({
            "type": "gravatar",
            "token": token,
            "url": format!("https://secure.gravatar.com/avatar/{token}"),
        })]
    };
    Ok(Json(avatars))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Mount the PORT-B user-gap routes.
pub fn user_gaps_routes() -> Router<AppState> {
    Router::new()
        // FavoritesController, groups scope (4).
        .route(
            "/users/self/favorites/groups",
            get(favorite_groups_index).delete(favorite_groups_reset),
        )
        .route(
            "/users/self/favorites/groups/{groupId}",
            axum::routing::post(add_favorite_group).delete(remove_favorite_group),
        )
        // NotificationPreferencesController (3). The channel segment is a
        // type+address pair for the first two routes and a channel id for
        // the category route; one param name keeps matchit happy.
        .route(
            "/users/self/communication_channels/{channel}/{address}/notification_preferences/{notification}",
            put(update_notification_preference),
        )
        .route(
            "/users/self/communication_channels/{channel}/{address}/notification_preferences",
            put(update_all_notification_preferences),
        )
        .route(
            "/users/self/communication_channels/{channel}/notification_preference_categories/{category}",
            put(update_preferences_by_category),
        )
        // ObserverPairingCodesApiController (1).
        .route(
            "/users/{userId}/observer_pairing_codes",
            axum::routing::post(create_pairing_code),
        )
        // AvatarsController#profile_pics (1).
        .route("/users/{userId}/avatars", get(profile_pics))
}

// ---------------------------------------------------------------------------
// Tests (DB-free; HTTP behavior is pinned in tests/user_gaps_integration.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::user_gaps_routes;

    /// Route registration sanity: the gap router builds cleanly.
    #[test]
    fn routes() {
        let _ = user_gaps_routes();
    }
}
