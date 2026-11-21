//! tokens — Canvas `TokensController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /users/{userId}/user_generated_tokens` — the access tokens of a user
//! - `GET    /users/{userId}/tokens/{id}` — show an access token
//! - `POST   /users/{userId}/tokens` — create an access token
//! - `PUT    /users/{userId}/tokens/{id}` — update an access token
//! - `DELETE /users/{userId}/tokens/{id}` — delete an access token
//!
//! Canvas routes (config/routes.rb, the user API block):
//!   get    "users/:user_id/user_generated_tokens", action: :user_generated_tokens
//!   get    "users/:user_id/tokens/:id", action: :show
//!   post   "users/:user_id/tokens", action: :create
//!   put    "users/:user_id/tokens/:id", action: :update
//!   delete "users/:user_id/tokens/:id", action: :destroy
//!
//! Canvas controller (`app/controllers/tokens_controller.rb`): the
//! `before_action` chain is `require_registered_user` (401), `get_context`
//! (404 when the user does not exist), `find_token` (404) and
//! `require_password_session` (401 when the caller has no password session).
//! The owner of a token may read, update and delete it; the other users may
//! not.
//!
//! The token JSON is the Canvas `Api::V1::Token#token_json` shape: `id`,
//! `created_at`, `expires_at`, `workflow_state`, `remember_access`,
//! `scopes`, `real_user_id`, `token`, `token_hint`, `user_id`, `purpose`,
//! `app_name` and `can_manually_regenerate`. The token value itself is only
//! rendered when the token is created.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// An `access_tokens` row.
#[derive(Debug, sqlx::FromRow)]
struct AccessToken {
    id: i64,
    user_id: Option<i64>,
    developer_key_id: Option<i64>,
    token: String,
    scopes: Vec<String>,
    purpose: Option<String>,
    workflow_state: String,
    permanent_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// The columns of an `access_tokens` row.
const TOKEN_COLUMNS: &str = "id, user_id, developer_key_id, token, scopes, purpose, \
     workflow_state, permanent_expires_at, last_used_at, created_at, updated_at";

/// The body of a create or update request.
#[derive(Debug, Default, Deserialize)]
struct TokenRequest {
    token: Option<TokenFields>,
}

/// The fields of a token in a request.
#[derive(Debug, Default, Deserialize)]
struct TokenFields {
    purpose: Option<String>,
    expires_at: Option<String>,
    regenerate: Option<String>,
    scopes: Option<Vec<String>>,
}

/// The query parameters of the listing.
#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    per_page: Option<String>,
    page: Option<String>,
}

/// Domain router for the Canvas token routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/users/{user_id}/user_generated_tokens",
            get(user_generated_tokens),
        )
        .route("/users/{user_id}/tokens", axum::routing::post(create_token))
        .route(
            "/users/{user_id}/tokens/{id}",
            get(show_token).put(update_token).delete(destroy_token),
        )
}

/// `GET /users/:user_id/user_generated_tokens` — tokens#user_generated_tokens.
async fn user_generated_tokens(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Response, ApiError> {
    require_user(&state.db, user_id).await?;

    // Canvas only lets a user read their own tokens.
    if auth.id != user_id && auth.role != "admin" {
        return Err(ApiError::Unauthorized);
    }

    let per_page = params
        .per_page
        .as_deref()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(10)
        .min(100);
    let page = params
        .page
        .as_deref()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    let offset = (page - 1) * per_page;

    // Canvas lists the tokens the user generated themselves.
    let tokens: Vec<AccessToken> = sqlx::query_as(&format!(
        "SELECT {TOKEN_COLUMNS} FROM access_tokens \
         WHERE user_id = $1 AND developer_key_id IS NULL \
           AND workflow_state <> 'deleted' \
         ORDER BY created_at ASC, id ASC LIMIT $2 OFFSET $3"
    ))
    .bind(user_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let items = tokens
        .iter()
        .map(|token| token_json(token, None))
        .collect::<Vec<Value>>();
    Ok(Json(items).into_response())
}

/// `GET /users/:user_id/tokens/:id` — tokens#show.
async fn show_token(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_user(&state.db, user_id).await?;
    let token = load_token(&state.db, user_id, &id).await?;
    require_owner(&auth, &token)?;
    Ok(Json(token_json(&token, None)).into_response())
}

/// `POST /users/:user_id/tokens` — tokens#create.
async fn create_token(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<TokenRequest>>,
) -> Result<Response, ApiError> {
    require_user(&state.db, user_id).await?;
    if auth.id != user_id && auth.role != "admin" {
        return Err(ApiError::Unauthorized);
    }

    let body = body.map(|Json(body)| body).unwrap_or_default();
    let fields = body.token.unwrap_or_default();
    let Some(purpose) = fields.purpose else {
        return Err(ApiError::BadRequest(
            "token[purpose] is missing".to_string(),
        ));
    };

    // The token value itself is only rendered when the token is created.
    let value = format!("{}:{}", user_id, uuid::Uuid::new_v4());

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO access_tokens (user_id, token, scopes, purpose, workflow_state, \
             permanent_expires_at) \
         VALUES ($1, $2, $3, $4, 'active', $5) RETURNING id",
    )
    .bind(user_id)
    .bind(&value)
    .bind(fields.scopes.unwrap_or_default())
    .bind(&purpose)
    .bind(parse_expiry(fields.expires_at.as_deref()))
    .fetch_one(&state.db)
    .await?;

    let token = load_token_by_id(&state.db, id).await?;
    Ok(Json(token_json(&token, Some(&value))).into_response())
}

/// `PUT /users/:user_id/tokens/:id` — tokens#update.
async fn update_token(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, String)>,
    auth: AuthUser,
    body: Option<Json<TokenRequest>>,
) -> Result<Response, ApiError> {
    require_user(&state.db, user_id).await?;
    let token = load_token(&state.db, user_id, &id).await?;
    require_owner(&auth, &token)?;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    let fields = body.token.unwrap_or_default();

    let scopes = match fields.scopes {
        Some(scopes) => scopes,
        None => token.scopes.clone(),
    };
    let purpose = fields.purpose.clone().or_else(|| token.purpose.clone());
    let expires_at = match fields.expires_at.as_deref() {
        Some(value) => parse_expiry(Some(value)),
        None => token.permanent_expires_at,
    };

    // Canvas regenerates the token value when the request asks for it.
    let regenerate = value_to_boolean(fields.regenerate.as_deref());
    let value = if regenerate {
        Some(format!("{}:{}", user_id, uuid::Uuid::new_v4()))
    } else {
        None
    };

    sqlx::query(
        "UPDATE access_tokens SET scopes = $1, purpose = $2, permanent_expires_at = $3, \
             token = COALESCE($4, token), updated_at = now() WHERE id = $5",
    )
    .bind(&scopes)
    .bind(&purpose)
    .bind(expires_at)
    .bind(&value)
    .bind(token.id)
    .execute(&state.db)
    .await?;

    let token = load_token_by_id(&state.db, token.id).await?;
    Ok(Json(token_json(&token, value.as_deref())).into_response())
}

/// `DELETE /users/:user_id/tokens/:id` — tokens#destroy.
async fn destroy_token(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_user(&state.db, user_id).await?;
    let token = load_token(&state.db, user_id, &id).await?;
    require_owner(&auth, &token)?;

    sqlx::query("DELETE FROM access_tokens WHERE id = $1")
        .bind(token.id)
        .execute(&state.db)
        .await?;

    Ok(Json(token_json(&token, None)).into_response())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The user of a route has to exist — 404.
async fn require_user(db: &PgPool, user_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// The token of a user — by id or by token hint — 404.
///
/// Canvas `find_token`: a hint is a five character string, the token is
/// then looked up by its prefix.
async fn load_token(db: &PgPool, user_id: i64, id: &str) -> Result<AccessToken, ApiError> {
    if is_token_hint(id) {
        let sql = format!(
            "SELECT {TOKEN_COLUMNS} FROM access_tokens \
             WHERE user_id = $1 AND token LIKE $2 LIMIT 1"
        );
        let token: Option<AccessToken> = sqlx::query_as(&sql)
            .bind(user_id)
            .bind(format!("{id}%"))
            .fetch_optional(db)
            .await?;
        return token.ok_or(ApiError::NotFound);
    }
    let sql = format!(
        "SELECT {TOKEN_COLUMNS} FROM access_tokens \
         WHERE user_id = $1 AND id::text = $2 LIMIT 1"
    );
    let token: Option<AccessToken> = sqlx::query_as(&sql)
        .bind(user_id)
        .bind(id)
        .fetch_optional(db)
        .await?;
    token.ok_or(ApiError::NotFound)
}

/// The token of an id.
async fn load_token_by_id(db: &PgPool, id: i64) -> Result<AccessToken, ApiError> {
    let sql = format!("SELECT {TOKEN_COLUMNS} FROM access_tokens WHERE id = $1");
    let token: Option<AccessToken> = sqlx::query_as(&sql).bind(id).fetch_optional(db).await?;
    token.ok_or(ApiError::NotFound)
}

/// Only the owner of a token may change it.
fn require_owner(auth: &AuthUser, token: &AccessToken) -> Result<(), ApiError> {
    if token.user_id == Some(auth.id) || auth.role == "admin" {
        Ok(())
    } else {
        Err(ApiError::Unauthorized)
    }
}

/// The expiration date of a token.
fn parse_expiry(raw: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = raw?;
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

/// Canvas `value_to_boolean`.
fn value_to_boolean(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|value| value.trim().to_lowercase()),
        Some(ref value) if value == "1" || value == "true" || value == "yes" || value == "on"
    )
}

/// Canvas `Api::V1::Token#token_json`.
fn token_json(token: &AccessToken, value: Option<&str>) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("id".to_string(), json!(token.id));
    object.insert(
        "created_at".to_string(),
        json!(token
            .created_at
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
    );
    object.insert(
        "expires_at".to_string(),
        match token.permanent_expires_at {
            Some(expires_at) => {
                json!(expires_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            }
            None => Value::Null,
        },
    );
    object.insert(
        "last_used_at".to_string(),
        match token.last_used_at {
            Some(last_used_at) => {
                json!(last_used_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            }
            None => Value::Null,
        },
    );
    object.insert("purpose".to_string(), json!(token.purpose));
    object.insert("real_user_id".to_string(), Value::Null);
    object.insert("remember_access".to_string(), json!(false));
    object.insert("scopes".to_string(), json!(token.scopes));
    object.insert(
        "updated_at".to_string(),
        json!(token
            .updated_at
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
    );
    object.insert("user_id".to_string(), json!(token.user_id));
    object.insert("workflow_state".to_string(), json!(token.workflow_state));
    // The token value is only rendered when the token is created.
    object.insert(
        "token".to_string(),
        match value {
            Some(value) => json!(value),
            None => Value::Null,
        },
    );
    object.insert("token_hint".to_string(), json!(token_hint(&token.token)));
    object.insert(
        "app_name".to_string(),
        match token.developer_key_id {
            Some(_) => json!("Developer Key"),
            None => Value::Null,
        },
    );
    object.insert("can_manually_regenerate".to_string(), json!(true));
    Value::Object(object)
}

/// Canvas `AccessToken#token_hint` — the first five characters of the token.
fn token_hint(token: &str) -> String {
    token.chars().take(HINT_LENGTH).collect()
}

/// Canvas `AccessToken::HINT_LENGTH`.
const HINT_LENGTH: usize = 5;

/// Canvas `AccessToken.token_hint?` — a hint is a five character string.
fn is_token_hint(id: &str) -> bool {
    id.chars().count() == HINT_LENGTH
}
