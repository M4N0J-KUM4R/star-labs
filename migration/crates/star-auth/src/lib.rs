//! star-auth — login, register, logout, and auth middleware for star-lms.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    body::Bytes,
    extract::{FromRef, FromRequest, Path, Query, Request, State},
    http::{
        header::{AUTHORIZATION, CONTENT_TYPE},
        StatusCode,
    },
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use axum_core::extract::FromRequestParts;
use chrono::{DateTime, Duration, Utc};
use http::request::Parts;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use star_common::{ApiError, AppState, Claims};

// ---------------------------------------------------------------------------
// Authenticated user (middleware-style extractor)
// ---------------------------------------------------------------------------

/// Authenticated user extracted from the Authorization: Bearer *** header.
///
/// The JWT is decoded and verified against `AppState.jwt_secret`; the user's
/// current role is then loaded from PostgreSQL by the token's `sub` claim.
/// The role claim inside the token is never trusted: it may be stale (roles
/// can change after login) and a signed token is not an authorization
/// record. Users that no longer exist are rejected.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: i64,
    pub role: String,
}

impl<S> FromRequestParts<S> for AuthUser
where
    AppState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);

        // Extract Authorization header.
        let auth_header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(ApiError::Unauthorized)?;

        // Expect "Bearer <token>".
        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(ApiError::Unauthorized)?;

        // Verify the signed token against the shared secret from app state.
        // No environment fallback: the secret is injected once at startup.
        let claims = decode::<Claims>(
            token,
            &DecodingKey::from_secret(app.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| ApiError::Unauthorized)?
        .claims;

        // Load the user's current role from the database. The token's role
        // claim is advisory only and never used for authorization; users
        // deleted since the token was minted are rejected.
        let (role, last_logged_out): (String, Option<chrono::DateTime<chrono::Utc>>) =
            sqlx::query_as("SELECT role, last_logged_out FROM users WHERE id = $1")
                .bind(claims.sub)
                .fetch_optional(&app.db)
                .await
                .map_err(ApiError::from)?
                .ok_or(ApiError::Unauthorized)?;

        // Tokens minted before the last logout are no longer valid. The
        // token lifetime is fixed by create_token, so issued-at is derived
        // from exp without adding another claim.
        let issued_at = claims.exp as i64 - TOKEN_EXPIRY_SECS as i64;
        if last_logged_out.is_some_and(|last| issued_at < last.timestamp()) {
            return Err(ApiError::Unauthorized);
        }

        Ok(AuthUser {
            id: claims.sub,
            role,
        })
    }
}

// ---------------------------------------------------------------------------
// Password helpers
// ---------------------------------------------------------------------------

/// Hash a password using argon2. Returns the PHC-formatted hash string.
pub fn hash_password(plain: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| format!("password hashing failed: {e}"))
}

/// Verify a plain-text password against an argon2 hash.
pub fn verify_password(plain: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok()
}

// ---------------------------------------------------------------------------
// JWT helpers
// ---------------------------------------------------------------------------

const TOKEN_EXPIRY_SECS: usize = 7 * 24 * 60 * 60; // 7 days

/// Create a JWT session token for the given user ID. Uses HS256.
pub fn create_token(user_id: i64, role: &str, secret: &str) -> Result<String, String> {
    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::seconds(TOKEN_EXPIRY_SECS as i64))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        sub: user_id,
        role: role.to_string(),
        exp,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| format!("failed to create token: {e}"))
}

/// Verify a JWT and return the claims if valid.
pub fn verify_token(token: &str, secret: &str) -> Option<Claims> {
    let validation = Validation::default();
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .ok()
    .map(|data| data.claims)
}

// ---------------------------------------------------------------------------
// Router + handlers
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/me", get(me))
        .route("/auth/logout", post(logout))
        // Canvas Developer Keys API + OAuth2 token endpoint. star-http nests
        // every domain router under /api/v1, so these resolve as
        // /api/v1/developer_keys and /api/v1/login/oauth2/token.
        .route(
            "/developer_keys",
            get(list_developer_keys).post(create_developer_key),
        )
        .route("/developer_keys/{id}", delete(destroy_developer_key))
        // Canvas personal access tokens (manual "User-Generated" tokens).
        .route(
            "/tokens",
            get(list_personal_tokens).post(create_personal_token),
        )
        .route(
            "/tokens/{id}",
            get(show_personal_token).delete(destroy_personal_token),
        )
        .route("/login/oauth2/token", post(oauth2_token))
        // Canvas account authentication providers + SSO settings.
        .route(
            "/accounts/{account_id}/authentication_providers",
            get(list_authentication_providers).post(create_authentication_provider),
        )
        .route(
            "/accounts/{account_id}/authentication_providers/{id}",
            get(show_authentication_provider)
                .put(update_authentication_provider)
                .delete(destroy_authentication_provider),
        )
        .route(
            "/accounts/{account_id}/sso_settings",
            get(show_sso_settings).put(update_sso_settings),
        )
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RegisterRequest {
    name: String,
    email: String,
    password: String,
    role: Option<String>,
}

/// Public because `fetch_user_by_id` returns it to other crates (star-dashboard).
#[derive(Serialize, sqlx::FromRow)]
pub struct UserResponse {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sortable_name: Option<String>,
    pub email: String,
    pub role: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
    user: UserResponse,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Public registration policy
// ---------------------------------------------------------------------------

/// The only role that public, unauthenticated registration may grant.
const PUBLIC_REGISTRATION_ROLE: &str = "student";

/// Resolve the role for a public `/auth/register` request.
///
/// Registration is unauthenticated, so the caller must never be able to grant
/// themselves global privileges: any requested role other than the safe
/// default is rejected (not silently downgraded), and an omitted role
/// defaults to `student`. Elevated roles may only be granted by an explicit,
/// authenticated admin action.
fn resolve_public_registration_role(requested: Option<&str>) -> Result<String, String> {
    match requested {
        None => Ok(PUBLIC_REGISTRATION_ROLE.to_string()),
        Some(role) if role == PUBLIC_REGISTRATION_ROLE => Ok(role.to_string()),
        Some(role) => Err(format!(
            "role '{role}' cannot be self-assigned at registration; \
             only '{PUBLIC_REGISTRATION_ROLE}' is allowed"
        )),
    }
}

async fn register(
    State(app): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate email: must contain '@'.
    if !req.email.contains('@') {
        return Err(ApiError::Validation("email must contain '@'".into()));
    }

    // Validate password: must be at least 8 characters.
    if req.password.len() < 8 {
        return Err(ApiError::Validation(
            "password must be at least 8 characters".into(),
        ));
    }

    // Public, unauthenticated registration must never grant global elevated
    // roles (teacher/admin): reject anything that is not the safe student
    // role rather than silently downgrading it.
    let role =
        resolve_public_registration_role(req.role.as_deref()).map_err(ApiError::Validation)?;

    // Hash password.
    let password_hash = hash_password(&req.password).map_err(ApiError::BadRequest)?;

    // Insert into DB. sortable_name is computed in SQL: "Last, First"
    let user: UserResponse = sqlx::query_as(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES (
            $1,
            TRIM(SPLIT_PART($1, ' ', -1) || ', ' || LEFT($1, LENGTH($1) - LENGTH(SPLIT_PART($1, ' ', -1)) - 1)),
            $2,
            $3,
            $4
        )
        RETURNING id, name, sortable_name, email, role, created_at
        "#,
    )
    .bind(&req.name)
    .bind(&req.email)
    .bind(&password_hash)
    .bind(&role)
    .fetch_one(&app.db)
    .await
    .map_err(|e| {
        // postgres unique violation code = 23505
        if let sqlx::Error::Database(ref db) = e {
            if db.code().map(|c| c == "23505").unwrap_or(false) {
                return ApiError::Conflict("email already in use".into());
            }
        }
        ApiError::from(e)
    })?;

    Ok((StatusCode::CREATED, Json(user)))
}

async fn login(
    State(app): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    // Fetch user by email.
    let row: Option<(i64, String, String)> =
        sqlx::query_as("SELECT id, password_hash, role FROM users WHERE email = $1")
            .bind(&req.email)
            .fetch_optional(&app.db)
            .await
            .map_err(ApiError::from)?;

    let (user_id, password_hash, role) = row.ok_or(ApiError::Unauthorized)?;

    // Verify password.
    if !verify_password(&req.password, &password_hash) {
        return Err(ApiError::Unauthorized);
    }

    // Load full user details for response.
    let user: UserResponse = sqlx::query_as(
        "SELECT id, name, sortable_name, email, role, created_at FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(&app.db)
    .await
    .map_err(ApiError::from)?;

    // Mint JWT with role.
    let token = create_token(user_id, &role, &app.jwt_secret).map_err(ApiError::BadRequest)?;

    Ok(Json(LoginResponse { token, user }))
}

async fn me(
    State(app): State<AppState>,
    auth_user: AuthUser,
) -> Result<Json<UserResponse>, ApiError> {
    let user = fetch_user_by_id(&app.db, auth_user.id).await?;
    Ok(Json(user))
}

async fn logout(_auth_user: AuthUser) -> Result<impl IntoResponse, ApiError> {
    // JWT is stateless; logout is handled client-side (discard token).
    // We still require authentication so the route is protected.
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Pagination helpers
// ---------------------------------------------------------------------------

/// Fetch a single user by ID from the database (used by auth handlers).
pub async fn fetch_user_by_id(db: &PgPool, id: i64) -> Result<UserResponse, ApiError> {
    sqlx::query_as(
        "SELECT id, name, sortable_name, email, role, created_at FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApiError::Db(e.to_string()))?
    .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Developer keys (Canvas API v1 — /api/v1/developer_keys)
// ---------------------------------------------------------------------------

/// Canvas developer key (OAuth2 client registration) as stored in
/// `developer_keys`. `api_key` is the client secret.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeveloperKey {
    pub id: i64,
    pub name: Option<String>,
    pub email: Option<String>,
    pub icon_url: Option<String>,
    pub notes: Option<String>,
    pub workflow_state: String,
    pub api_key: String,
    pub redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    pub require_scopes: bool,
    pub allow_includes: bool,
    pub visible: bool,
    pub vendor_code: Option<String>,
    pub auto_expire_tokens: bool,
    pub client_credentials_audience: Option<String>,
    pub user_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Name of the creating user, joined from `users` in DEVELOPER_KEY_SELECT.
    pub user_name: Option<String>,
}

/// Shared projection, joined with the creating user's name for `user_name`.
const DEVELOPER_KEY_SELECT: &str = r#"
    SELECT k.id, k.name, k.email, k.icon_url, k.notes, k.workflow_state,
           k.api_key, k.redirect_uris, k.scopes, k.require_scopes,
           k.allow_includes, k.visible, k.vendor_code, k.auto_expire_tokens,
           k.client_credentials_audience, k.user_id, k.created_at, k.updated_at,
           u.name AS user_name
    FROM developer_keys k
    LEFT JOIN users u ON u.id = k.user_id
"#;

/// Canvas-shaped developer key JSON. `redirect_uris` is serialized as a
/// newline-joined string, exactly like `Api::V1::DeveloperKey` in Canvas.
fn developer_key_json(key: &DeveloperKey, access_token_count: i64) -> serde_json::Value {
    serde_json::json!({
        "id": key.id,
        "name": key.name,
        "created_at": key.created_at,
        "updated_at": key.updated_at,
        "workflow_state": key.workflow_state,
        "email": key.email,
        "icon_url": key.icon_url,
        "notes": key.notes,
        "vendor_code": key.vendor_code,
        "visible": key.visible,
        "scopes": key.scopes,
        "require_scopes": key.require_scopes,
        "allow_includes": key.allow_includes,
        "auto_expire_tokens": key.auto_expire_tokens,
        "client_credentials_audience": key.client_credentials_audience,
        "user_id": key.user_id,
        "user_name": key.user_name,
        "api_key": key.api_key,
        "redirect_uri": key.redirect_uris.first(),
        "redirect_uris": key.redirect_uris.join("\n"),
        "access_token_count": access_token_count,
        "last_used_at": null,
        "is_lti_key": false,
        "is_lti_registration": false,
        "account_name": null,
    })
}

/// Accepts either a JSON array or a newline/whitespace separated string for
/// array-valued developer key fields (Canvas emits `redirect_uris` joined by
/// newlines and accepts the array form on input).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum StringList {
    Many(Vec<String>),
    One(String),
}

impl StringList {
    fn into_vec(self) -> Vec<String> {
        match self {
            StringList::Many(values) => values,
            StringList::One(value) => value
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>(),
        }
    }
}

/// Body of `POST /developer_keys` (`developer_key` wrapper or flat fields).
#[derive(Debug, Default, Deserialize)]
struct DeveloperKeyParams {
    name: Option<String>,
    email: Option<String>,
    icon_url: Option<String>,
    notes: Option<String>,
    /// Deprecated single redirect URI, kept for Canvas parity.
    redirect_uri: Option<String>,
    redirect_uris: Option<StringList>,
    scopes: Option<StringList>,
    require_scopes: Option<bool>,
    allow_includes: Option<bool>,
    visible: Option<bool>,
    vendor_code: Option<String>,
    auto_expire_tokens: Option<bool>,
    client_credentials_audience: Option<String>,
}

/// Admin-only management of developer keys, mirroring Canvas
/// `require_manage_developer_keys`.
fn require_admin(auth: &AuthUser) -> Result<(), ApiError> {
    if auth.role == "admin" {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

async fn fetch_developer_key(pool: &PgPool, id: i64) -> Result<DeveloperKey, ApiError> {
    let sql = format!("{DEVELOPER_KEY_SELECT} WHERE k.id = $1 AND k.workflow_state <> 'deleted'");
    sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)
}

/// GET /developer_keys — list non-deleted developer keys (admin only).
async fn list_developer_keys(
    State(app): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let sql =
        format!("{DEVELOPER_KEY_SELECT} WHERE k.workflow_state <> 'deleted' ORDER BY k.id DESC");
    let keys: Vec<DeveloperKey> = sqlx::query_as(&sql).fetch_all(&app.db).await?;

    let counts: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT developer_key_id, COUNT(*)
         FROM access_tokens
         WHERE developer_key_id IS NOT NULL AND workflow_state = 'active'
         GROUP BY developer_key_id",
    )
    .fetch_all(&app.db)
    .await?;

    let body = keys
        .iter()
        .map(|key| {
            let count = counts
                .iter()
                .find(|(key_id, _)| *key_id == key.id)
                .map(|(_, count)| *count)
                .unwrap_or(0);
            developer_key_json(key, count)
        })
        .collect::<Vec<_>>();

    Ok(Json(serde_json::Value::Array(body)))
}

/// POST /developer_keys — create a key and its client secret (admin only).
async fn create_developer_key(
    State(app): State<AppState>,
    auth: AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let raw = body.get("developer_key").cloned().unwrap_or(body);
    let params: DeveloperKeyParams = serde_json::from_value(raw)
        .map_err(|e| ApiError::BadRequest(format!("invalid developer_key: {e}")))?;

    let mut redirect_uris = params
        .redirect_uris
        .map(StringList::into_vec)
        .unwrap_or_default();
    if let Some(single) = params
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .filter(|single| !single.is_empty())
        .filter(|single| !redirect_uris.iter().any(|uri| uri.as_str() == *single))
    {
        redirect_uris.push(single.to_string());
    }
    let scopes = params.scopes.map(StringList::into_vec).unwrap_or_default();
    let api_key = generate_opaque_token(32);

    let (id,): (i64,) = sqlx::query_as(
        r#"
        INSERT INTO developer_keys
            (name, email, icon_url, notes, api_key, redirect_uris, scopes,
             require_scopes, allow_includes, visible, vendor_code,
             auto_expire_tokens, client_credentials_audience, user_id,
             workflow_state)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                'active')
        RETURNING id
        "#,
    )
    .bind(params.name)
    .bind(params.email)
    .bind(params.icon_url)
    .bind(params.notes)
    .bind(api_key)
    .bind(redirect_uris)
    .bind(scopes)
    .bind(params.require_scopes.unwrap_or(false))
    .bind(params.allow_includes.unwrap_or(false))
    .bind(params.visible.unwrap_or(true))
    .bind(params.vendor_code)
    .bind(params.auto_expire_tokens.unwrap_or(false))
    .bind(params.client_credentials_audience)
    .bind(auth.id)
    .fetch_one(&app.db)
    .await?;

    let key = fetch_developer_key(&app.db, id).await?;
    Ok(Json(developer_key_json(&key, 0)))
}

/// DELETE /developer_keys/{id} — soft delete a key and revoke its tokens.
async fn destroy_developer_key(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    // 404 for unknown or already-deleted keys.
    fetch_developer_key(&app.db, id).await?;

    sqlx::query(
        "UPDATE developer_keys SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&app.db)
    .await?;
    sqlx::query(
        "UPDATE access_tokens SET workflow_state = 'deleted', updated_at = now()
         WHERE developer_key_id = $1",
    )
    .bind(id)
    .execute(&app.db)
    .await?;

    let sql = format!("{DEVELOPER_KEY_SELECT} WHERE k.id = $1");
    let key: DeveloperKey = sqlx::query_as(&sql).bind(id).fetch_one(&app.db).await?;
    Ok(Json(developer_key_json(&key, 0)))
}

// ---------------------------------------------------------------------------
// Personal access tokens (Canvas API v1 — /api/v1/tokens)
// ---------------------------------------------------------------------------

/// Canvas uses the first 5 characters of a token as its public hint.
const TOKEN_HINT_LENGTH: usize = 5;

/// GET /tokens defaults to 10 results per page, maximum 100 (Canvas
/// `TokensController#user_generated_tokens`).
const TOKENS_DEFAULT_PER_PAGE: i64 = 10;
const TOKENS_MAX_PER_PAGE: i64 = 100;

/// Pagination params for GET /tokens.
#[derive(Debug, Deserialize)]
struct TokenPagination {
    page: Option<i64>,
    per_page: Option<i64>,
}

impl TokenPagination {
    fn resolve(&self) -> (i64, i64) {
        let page = self.page.unwrap_or(1).max(1);
        let per_page = self
            .per_page
            .unwrap_or(TOKENS_DEFAULT_PER_PAGE)
            .clamp(1, TOKENS_MAX_PER_PAGE);
        (per_page, (page - 1) * per_page)
    }
}

/// Body of POST /tokens. Canvas wraps the fields in `token`; the flat form is
/// accepted too, like the developer key handlers.
#[derive(Debug, Default, Deserialize)]
struct TokenParams {
    purpose: Option<String>,
    /// Canvas API name; stored in `permanent_expires_at`.
    expires_at: Option<String>,
    /// Accepted as an alias for `expires_at`.
    permanent_expires_at: Option<String>,
    scopes: Option<StringList>,
}

/// A manually created personal access token (`access_tokens` row with a NULL
/// `developer_key_id`), shaped like Canvas `Token`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct PersonalAccessToken {
    id: i64,
    user_id: Option<i64>,
    purpose: Option<String>,
    scopes: Vec<String>,
    workflow_state: String,
    token: String,
    token_hint: Option<String>,
    remember_access: bool,
    real_user_id: Option<i64>,
    permanent_expires_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const PERSONAL_TOKEN_SELECT: &str = r#"
    SELECT id, user_id, purpose, scopes, workflow_state, token, token_hint,
           remember_access, real_user_id, permanent_expires_at, last_used_at,
           created_at, updated_at
    FROM access_tokens
"#;

/// First `TOKEN_HINT_LENGTH` characters of a token (Canvas `token_hint`).
fn token_hint(token: &str) -> String {
    token.chars().take(TOKEN_HINT_LENGTH).collect()
}

/// Parse Canvas `token[expires_at]`: an ISO8601/RFC3339 timestamp, with a
/// date-only value accepted as midnight UTC (Canvas `Time.zone.parse`).
fn parse_token_expires_at(raw: Option<&str>) -> Result<Option<DateTime<Utc>>, &'static str> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return Ok(Some(parsed.with_timezone(&Utc)));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let midnight = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
        return Ok(Some(midnight.and_utc()));
    }
    Err("Invalid expiration date format")
}

/// Canvas-shaped `Token` JSON (`Api::V1::Token#token_json`). The full token is
/// only exposed on creation; afterwards `visible_token` is the masked hint.
fn personal_token_json(token: &PersonalAccessToken, full_token: Option<&str>) -> serde_json::Value {
    let hint = token
        .token_hint
        .clone()
        .unwrap_or_else(|| token_hint(&token.token));
    let visible_token = match full_token {
        Some(full) => full.to_string(),
        None => format!("{hint}..."),
    };
    let expired = token
        .permanent_expires_at
        .map(|at| at < Utc::now())
        .unwrap_or(false);

    serde_json::json!({
        "id": token.id,
        "created_at": token.created_at,
        "expires_at": token.permanent_expires_at,
        "last_used_at": token.last_used_at,
        "purpose": token.purpose,
        "real_user_id": token.real_user_id,
        "remember_access": token.remember_access,
        "scopes": token.scopes,
        "updated_at": token.updated_at,
        "user_id": token.user_id,
        "workflow_state": token.workflow_state,
        // Manually created tokens carry Canvas's default ("User-Generated")
        // developer key name.
        "app_name": "User-Generated",
        "visible_token": visible_token,
        "can_manually_regenerate": !expired,
    })
}

/// Canvas `render_error` shape: `[{"message": "..."}]` with status 400.
fn token_error(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!([{ "message": message }])),
    )
        .into_response()
}

async fn fetch_personal_token_by_id(
    pool: &PgPool,
    user_id: i64,
    id: i64,
) -> Result<PersonalAccessToken, ApiError> {
    let sql = format!(
        "{PERSONAL_TOKEN_SELECT}
         WHERE user_id = $1 AND id = $2 AND developer_key_id IS NULL
           AND workflow_state <> 'deleted'"
    );
    sqlx::query_as(&sql)
        .bind(user_id)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)
}

async fn fetch_personal_token_by_hint(
    pool: &PgPool,
    user_id: i64,
    hint: &str,
) -> Result<PersonalAccessToken, ApiError> {
    let sql = format!(
        "{PERSONAL_TOKEN_SELECT}
         WHERE user_id = $1 AND token_hint = $2 AND developer_key_id IS NULL
           AND workflow_state <> 'deleted'
         ORDER BY id DESC
         LIMIT 1"
    );
    sqlx::query_as(&sql)
        .bind(user_id)
        .bind(hint)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)
}

/// Resolve the `{id}` path segment: like Canvas, a 5-character value is a
/// `token_hint`, anything else is the database id.
async fn fetch_personal_token(
    pool: &PgPool,
    user_id: i64,
    id_or_hint: &str,
) -> Result<PersonalAccessToken, ApiError> {
    if id_or_hint.len() == TOKEN_HINT_LENGTH {
        fetch_personal_token_by_hint(pool, user_id, id_or_hint).await
    } else {
        let id: i64 = id_or_hint.parse().map_err(|_| ApiError::NotFound)?;
        fetch_personal_token_by_id(pool, user_id, id).await
    }
}

/// GET /tokens — list the current user's manually created access tokens.
async fn list_personal_tokens(
    State(app): State<AppState>,
    auth: AuthUser,
    Query(pag): Query<TokenPagination>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (per_page, offset) = pag.resolve();
    let sql = format!(
        "{PERSONAL_TOKEN_SELECT}
         WHERE user_id = $1 AND developer_key_id IS NULL
           AND workflow_state <> 'deleted'
         ORDER BY created_at ASC, id ASC
         LIMIT $2 OFFSET $3"
    );
    let tokens: Vec<PersonalAccessToken> = sqlx::query_as(&sql)
        .bind(auth.id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&app.db)
        .await?;

    Ok(Json(serde_json::Value::Array(
        tokens
            .iter()
            .map(|token| personal_token_json(token, None))
            .collect(),
    )))
}

/// POST /tokens — create a personal access token for the current user.
///
/// The full token value is returned exactly once, in `visible_token`.
async fn create_personal_token(
    State(app): State<AppState>,
    auth: AuthUser,
    Json(body): Json<serde_json::Value>,
) -> Result<Response, ApiError> {
    let raw = body.get("token").cloned().unwrap_or(body);
    let params: TokenParams = serde_json::from_value(raw)
        .map_err(|e| ApiError::BadRequest(format!("invalid token: {e}")))?;

    let Some(purpose) = params
        .purpose
        .as_deref()
        .map(str::trim)
        .filter(|purpose| !purpose.is_empty())
    else {
        return Ok(token_error("token[purpose] is missing"));
    };

    let expires_at = match parse_token_expires_at(
        params
            .expires_at
            .as_deref()
            .or(params.permanent_expires_at.as_deref()),
    ) {
        Ok(expires_at) => expires_at,
        Err(message) => return Ok(token_error(message)),
    };

    let scopes = params.scopes.map(StringList::into_vec).unwrap_or_default();
    let token = generate_opaque_token(32);
    let hint = token_hint(&token);

    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO access_tokens
            (user_id, developer_key_id, token, token_hint, purpose, scopes,
             permanent_expires_at, remember_access, workflow_state)
        VALUES ($1, NULL, $2, $3, $4, $5, $6, FALSE, 'active')
        RETURNING id
        "#,
    )
    .bind(auth.id)
    .bind(&token)
    .bind(&hint)
    .bind(purpose)
    .bind(scopes)
    .bind(expires_at)
    .fetch_one(&app.db)
    .await?;

    let row = fetch_personal_token_by_id(&app.db, auth.id, id).await?;
    Ok((
        StatusCode::OK,
        Json(personal_token_json(&row, Some(token.as_str()))),
    )
        .into_response())
}

/// GET /tokens/{id} — show one of the current user's tokens (id or hint).
async fn show_personal_token(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(id_or_hint): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let token = fetch_personal_token(&app.db, auth.id, &id_or_hint).await?;
    Ok(Json(personal_token_json(&token, None)))
}

/// DELETE /tokens/{id} — soft delete one of the current user's tokens.
async fn destroy_personal_token(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(id_or_hint): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut token = fetch_personal_token(&app.db, auth.id, &id_or_hint).await?;

    sqlx::query(
        "UPDATE access_tokens SET workflow_state = 'deleted', updated_at = now()
         WHERE id = $1",
    )
    .bind(token.id)
    .execute(&app.db)
    .await?;

    token.workflow_state = "deleted".to_string();
    token.updated_at = Utc::now();
    Ok(Json(personal_token_json(&token, None)))
}

// ---------------------------------------------------------------------------
// OAuth2 token endpoint (POST /login/oauth2/token)
// ---------------------------------------------------------------------------

/// Access token lifetime, in seconds. Canvas client-credentials tokens also
/// default to one hour.
const OAUTH_TOKEN_TTL_SECS: i64 = 3600;

/// OAuth2 token request fields (Canvas accepts form-encoded parameters; JSON
/// bodies are accepted too for convenience).
#[derive(Debug, Default, Deserialize)]
struct OAuthTokenRequest {
    grant_type: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
    scope: Option<String>,
}

impl OAuthTokenRequest {
    fn from_form(body: &str) -> Self {
        let mut params = Self::default();
        for pair in body.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = percent_decode(key);
            let value = percent_decode(value);
            match key.as_str() {
                "grant_type" => params.grant_type = Some(value),
                "client_id" => params.client_id = Some(value),
                "client_secret" => params.client_secret = Some(value),
                "refresh_token" => params.refresh_token = Some(value),
                "scope" => params.scope = Some(value),
                _ => {}
            }
        }
        params
    }
}

/// Decode `application/x-www-form-urlencoded` escapes (`%XX`, `+`).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl<S> FromRequest<S> for OAuthTokenRequest
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|_| ApiError::BadRequest("invalid request body".into()))?;
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        if content_type.contains("json") {
            serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::BadRequest("invalid JSON body".into()))
        } else {
            Ok(Self::from_form(&String::from_utf8_lossy(&bytes)))
        }
    }
}

/// Generate an opaque token (hex) for `access_token` / `refresh_token`.
/// Uses OsRng from argon2's re-exported rand_core: no extra dependency.
fn generate_opaque_token(byte_len: usize) -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};

    let mut bytes = vec![0u8; byte_len];
    OsRng.fill_bytes(&mut bytes);
    let mut token = String::with_capacity(byte_len * 2);
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    token
}

/// Constant-time-ish comparison for client secrets.
fn secrets_match(provided: &str, expected: &str) -> bool {
    let provided = provided.as_bytes();
    let expected = expected.as_bytes();
    if provided.len() != expected.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in provided.iter().zip(expected.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Canvas OAuth2 error body (`error` / `error_description`).
fn oauth_error(status: StatusCode, error: &'static str, description: &'static str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": error,
            "error_description": description,
        })),
    )
        .into_response()
}

fn internal_error(context: &str, err: impl std::fmt::Display) -> Response {
    eprintln!("{context}: {err}");
    oauth_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "server_error",
        "internal server error",
    )
}

/// Canvas-shaped token response (`Canvas::OAuth::Token#as_json` subset).
fn oauth_token_json(
    access_token: &str,
    refresh_token: &str,
    scopes: &[String],
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": OAUTH_TOKEN_TTL_SECS,
        "refresh_token": refresh_token,
    });
    if !scopes.is_empty() {
        body["scope"] = serde_json::Value::String(scopes.join(" "));
    }
    body
}

async fn find_active_developer_key(
    pool: &PgPool,
    client_id: &str,
) -> Result<Option<DeveloperKey>, ApiError> {
    let Ok(id) = client_id.parse::<i64>() else {
        return Ok(None);
    };
    let sql = format!("{DEVELOPER_KEY_SELECT} WHERE k.id = $1 AND k.workflow_state = 'active'");
    sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)
}

async fn issue_access_token(
    pool: &PgPool,
    developer_key_id: i64,
    user_id: Option<i64>,
    scopes: &[String],
) -> Result<(String, String), ApiError> {
    let access_token = generate_opaque_token(32);
    let refresh_token = generate_opaque_token(32);
    let expires_at = Utc::now() + Duration::seconds(OAUTH_TOKEN_TTL_SECS);

    sqlx::query(
        "INSERT INTO access_tokens
             (user_id, developer_key_id, token, refresh_token, scopes,
              expires_at, workflow_state)
         VALUES ($1, $2, $3, $4, $5, $6, 'active')",
    )
    .bind(user_id)
    .bind(developer_key_id)
    .bind(&access_token)
    .bind(&refresh_token)
    .bind(scopes.to_vec())
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok((access_token, refresh_token))
}

/// Requested scopes: the `scope` parameter (space separated) or the key's
/// configured scopes.
fn requested_scopes(params: &OAuthTokenRequest, key: &DeveloperKey) -> Vec<String> {
    match params
        .scope
        .as_deref()
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
    {
        Some(scope) => scope.split_whitespace().map(str::to_string).collect(),
        None => key.scopes.clone(),
    }
}

/// POST /login/oauth2/token — client_credentials and refresh_token grants.
///
/// Mirrors Canvas `Oauth2ProviderController#token`: client_id is the
/// developer key id, client_secret its api_key, errors are
/// `{"error": ..., "error_description": ...}` bodies.
async fn oauth2_token(State(app): State<AppState>, params: OAuthTokenRequest) -> Response {
    let Some(client_id) = params
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client", "unknown client");
    };

    let key = match find_active_developer_key(&app.db, client_id).await {
        Ok(Some(key)) => key,
        Ok(None) => {
            return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client", "unknown client")
        }
        Err(err) => return internal_error("oauth token: lookup developer key", err),
    };

    let secret = params.client_secret.as_deref().unwrap_or("");
    if !secrets_match(secret, &key.api_key) {
        return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client", "invalid client");
    }

    match params.grant_type.as_deref() {
        Some("client_credentials") => {
            let scopes = requested_scopes(&params, &key);
            match issue_access_token(&app.db, key.id, None, &scopes).await {
                Ok((access_token, refresh_token)) => (
                    StatusCode::OK,
                    Json(oauth_token_json(&access_token, &refresh_token, &scopes)),
                )
                    .into_response(),
                Err(err) => internal_error("oauth token: issue client_credentials", err),
            }
        }
        Some("refresh_token") => {
            let Some(refresh_token) = params
                .refresh_token
                .as_deref()
                .map(str::trim)
                .filter(|token| !token.is_empty())
            else {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "You must provide the refresh_token parameter when using the refresh_token grant type",
                );
            };

            let row: Option<(i64, Option<i64>, Vec<String>)> = match sqlx::query_as(
                "SELECT id, developer_key_id, scopes
                 FROM access_tokens
                 WHERE refresh_token = $1 AND workflow_state = 'active'",
            )
            .bind(refresh_token)
            .fetch_optional(&app.db)
            .await
            {
                Ok(row) => row,
                Err(err) => return internal_error("oauth token: lookup refresh token", err),
            };

            let Some((token_id, developer_key_id, scopes)) = row else {
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "refresh_token not found",
                );
            };
            if developer_key_id != Some(key.id) {
                return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "incorrect client");
            }

            // Rotate both tokens in place (Canvas rotates refresh tokens for
            // public clients after each use).
            let access_token = generate_opaque_token(32);
            let new_refresh_token = generate_opaque_token(32);
            let expires_at = Utc::now() + Duration::seconds(OAUTH_TOKEN_TTL_SECS);

            if let Err(err) = sqlx::query(
                "UPDATE access_tokens
                 SET token = $2, refresh_token = $3, expires_at = $4, updated_at = now()
                 WHERE id = $1",
            )
            .bind(token_id)
            .bind(&access_token)
            .bind(&new_refresh_token)
            .bind(expires_at)
            .execute(&app.db)
            .await
            {
                return internal_error("oauth token: rotate refresh token", err);
            }

            (
                StatusCode::OK,
                Json(oauth_token_json(&access_token, &new_refresh_token, &scopes)),
            )
                .into_response()
        }
        _ => oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "The grant_type you requested is not currently supported",
        ),
    }
}

// ---------------------------------------------------------------------------
// Authentication providers (Canvas API v1 — /accounts/{id}/authentication_providers)
// ---------------------------------------------------------------------------

/// Canvas AuthenticationProvider shape (subset of Api::V1::AuthenticationProvider).
/// auth_type is stored directly; all other settings live in the JSONB `settings` column.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuthenticationProvider {
    pub id: i64,
    pub account_id: i64,
    pub auth_type: String,
    pub position: Option<i32>,
    pub settings: serde_json::Value,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Flat parameters for POST/PUT /authentication_providers.
/// Canvas wraps in `authentication_provider` or accepts flat params.
#[derive(Debug, Default, Deserialize)]
struct AuthProviderParams {
    auth_type: Option<String>,
    position: Option<i32>,
    #[serde(flatten)]
    settings: serde_json::Map<String, serde_json::Value>,
}

const AUTH_PROVIDER_SELECT: &str = r#"
    SELECT id, account_id, auth_type, position, settings,
           workflow_state, created_at, updated_at
    FROM authentication_providers
"#;

/// Canvas-shaped AuthenticationProvider JSON (matches aac_json output).
fn authentication_provider_json(ap: &AuthenticationProvider) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "id": ap.id,
        "account_id": ap.account_id,
        "auth_type": ap.auth_type,
        "position": ap.position,
        "workflow_state": ap.workflow_state,
        "created_at": ap.created_at,
        "updated_at": ap.updated_at,
    });
    // Merge settings fields at the top level.
    if let serde_json::Value::Object(m) = &ap.settings {
        for (k, v) in m {
            obj[k] = v.clone();
        }
    }
    obj
}

/// Fetch a single authentication provider by id, or None if not found / deleted.
async fn fetch_authentication_provider(
    pool: &PgPool,
    id: i64,
) -> Result<AuthenticationProvider, ApiError> {
    sqlx::query_as::<_, AuthenticationProvider>(&format!(
        "{AUTH_PROVIDER_SELECT} WHERE id = $1 AND workflow_state <> 'deleted'"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or(ApiError::NotFound)
}

/// Parse incoming params: accepts `{"authentication_provider": {...}}` wrapper
/// or flat params, matching Canvas's dual input style.
fn parse_auth_provider_params(raw: serde_json::Value) -> Result<AuthProviderParams, ApiError> {
    let params = raw.get("authentication_provider").cloned().unwrap_or(raw);
    serde_json::from_value(params)
        .map_err(|e| ApiError::BadRequest(format!("invalid auth provider params: {e}")))
}

/// GET /accounts/{account_id}/authentication_providers — list active providers.
async fn list_authentication_providers(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let providers: Vec<AuthenticationProvider> =
        sqlx::query_as::<_, AuthenticationProvider>(&format!(
            "{AUTH_PROVIDER_SELECT}
         WHERE account_id = $1 AND workflow_state <> 'deleted'
         ORDER BY position ASC NULLS LAST, id ASC"
        ))
        .bind(account_id)
        .fetch_all(&app.db)
        .await?;

    Ok(Json(serde_json::json!({
        "authentication_providers": providers
            .iter()
            .map(authentication_provider_json)
            .collect::<Vec<_>>()
    })))
}

/// POST /accounts/{account_id}/authentication_providers — create a provider.
async fn create_authentication_provider(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let params = parse_auth_provider_params(raw)?;
    let auth_type = params
        .auth_type
        .ok_or_else(|| ApiError::Validation("auth_type is required".into()))?;

    // position defaults to append
    let position = params.position.unwrap_or(0);

    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO authentication_providers
            (account_id, auth_type, position, settings, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id
        "#,
    )
    .bind(account_id)
    .bind(&auth_type)
    .bind(position)
    .bind(serde_json::Value::Object(params.settings))
    .fetch_one(&app.db)
    .await?;

    let ap = fetch_authentication_provider(&app.db, id).await?;
    Ok(Json(authentication_provider_json(&ap)))
}

/// GET /accounts/{account_id}/authentication_providers/{id} — get one provider.
async fn show_authentication_provider(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((_account_id, id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;
    let ap = fetch_authentication_provider(&app.db, id).await?;
    Ok(Json(authentication_provider_json(&ap)))
}

/// PUT /accounts/{account_id}/authentication_providers/{id} — update a provider.
async fn update_authentication_provider(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((_account_id, id)): Path<(i64, i64)>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let existing = fetch_authentication_provider(&app.db, id).await?;
    let params = parse_auth_provider_params(raw)?;

    let position = params.position.unwrap_or(existing.position.unwrap_or(0));
    let settings = if params.settings.is_empty() {
        existing.settings.clone()
    } else {
        // Merge: existing settings then overlay new ones
        let mut merged = existing
            .settings
            .clone()
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (k, v) in params.settings {
            merged.insert(k, v);
        }
        serde_json::Value::Object(merged)
    };

    sqlx::query(
        r#"
        UPDATE authentication_providers
        SET position = $2, settings = $3, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(position)
    .bind(&settings)
    .execute(&app.db)
    .await?;

    let ap = fetch_authentication_provider(&app.db, id).await?;
    Ok(Json(authentication_provider_json(&ap)))
}

/// DELETE /accounts/{account_id}/authentication_providers/{id} — soft delete.
async fn destroy_authentication_provider(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((_account_id, id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let ap = fetch_authentication_provider(&app.db, id).await?;

    sqlx::query(
        "UPDATE authentication_providers SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&app.db)
    .await?;

    let mut deleted = ap;
    deleted.workflow_state = "deleted".to_string();
    deleted.updated_at = Utc::now();
    Ok(Json(authentication_provider_json(&deleted)))
}

// ---------------------------------------------------------------------------
// SSO Settings (Canvas API v1 — /accounts/{id}/sso_settings)
// ---------------------------------------------------------------------------

/// SSO settings live in the accounts table (already altered by migration 0100).
#[derive(Serialize)]
struct SsoSettingsResponse {
    login_handle_name: Option<String>,
    change_password_url: Option<String>,
    auth_discovery_url: Option<String>,
    unknown_user_url: Option<String>,
    login_help_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discovery_page_active: Option<bool>,
}

/// GET /accounts/{account_id}/sso_settings — return current SSO settings.
async fn show_sso_settings(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let row: Option<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<bool>,
    )> = sqlx::query_as(
        "SELECT login_handle_name, change_password_url, auth_discovery_url,
                    unknown_user_url, login_help_url, discovery_page_active
             FROM accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_optional(&app.db)
    .await
    .map_err(ApiError::from)?;

    let row = row.ok_or(ApiError::NotFound)?;
    let (
        login_handle_name,
        change_password_url,
        auth_discovery_url,
        unknown_user_url,
        login_help_url,
        discovery_page_active,
    ) = row;

    Ok(Json(serde_json::json!({
        "sso_settings": {
            "login_handle_name": login_handle_name,
            "change_password_url": change_password_url,
            "auth_discovery_url": auth_discovery_url,
            "unknown_user_url": unknown_user_url,
            "login_help_url": login_help_url,
            "discovery_page_active": discovery_page_active,
        }
    })))
}

/// Parse sso_settings wrapper or flat params (Canvas accepts both).
fn parse_sso_settings(raw: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    raw.get("sso_settings")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_else(|| raw.as_object().cloned().unwrap_or_default())
}

/// PUT /accounts/{account_id}/sso_settings — update SSO settings.
async fn update_sso_settings(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    // Verify account exists.
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(&app.db)
        .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }

    let params = parse_sso_settings(raw);

    // Build dynamic UPDATE from provided fields.
    let mut set_clauses: Vec<String> = vec![];
    let mut bind_idx = 1;

    macro_rules! bind_opt {
        ($field:expr, $col:expr) => {
            if let Some(val) = params
                .get(stringify!($field))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
            {
                set_clauses.push(format!("{} = ${}", $col, bind_idx));
                bind_idx += 1;
            }
        };
    }

    // Handle all possible SSO setting fields
    let mut new_login_handle_name: Option<Option<String>> = None;
    let mut new_change_password_url: Option<Option<String>> = None;
    let mut new_auth_discovery_url: Option<Option<String>> = None;
    let mut new_unknown_user_url: Option<Option<String>> = None;
    let mut new_login_help_url: Option<Option<String>> = None;
    let mut new_discovery_page_active: Option<Option<bool>> = None;

    for (key, val) in &params {
        match key.as_str() {
            "login_handle_name" => {
                new_login_handle_name = Some(val.as_str().map(|s| s.to_string()));
            }
            "change_password_url" => {
                new_change_password_url = Some(val.as_str().map(|s| s.to_string()));
            }
            "auth_discovery_url" => {
                new_auth_discovery_url = Some(val.as_str().map(|s| s.to_string()));
            }
            "unknown_user_url" => {
                new_unknown_user_url = Some(val.as_str().map(|s| s.to_string()));
            }
            "login_help_url" => {
                new_login_help_url = Some(val.as_str().map(|s| s.to_string()));
            }
            "discovery_page_active" => {
                new_discovery_page_active = Some(val.as_bool());
            }
            _ => {}
        }
    }

    if new_login_handle_name.is_some()
        || new_change_password_url.is_some()
        || new_auth_discovery_url.is_some()
        || new_unknown_user_url.is_some()
        || new_login_help_url.is_some()
        || new_discovery_page_active.is_some()
    {
        let mut sql = "UPDATE accounts SET ".to_string();
        let mut parts = vec![];

        if let Some(v) = new_login_handle_name {
            parts.push(format!("login_handle_name = ${}", bind_idx));
            bind_idx += 1;
        }
        if let Some(v) = new_change_password_url {
            parts.push(format!("change_password_url = ${}", bind_idx));
            bind_idx += 1;
        }
        if let Some(v) = new_auth_discovery_url {
            parts.push(format!("auth_discovery_url = ${}", bind_idx));
            bind_idx += 1;
        }
        if let Some(v) = new_unknown_user_url {
            parts.push(format!("unknown_user_url = ${}", bind_idx));
            bind_idx += 1;
        }
        if let Some(v) = new_login_help_url {
            parts.push(format!("login_help_url = ${}", bind_idx));
            bind_idx += 1;
        }
        if let Some(v) = new_discovery_page_active {
            parts.push(format!("discovery_page_active = ${}", bind_idx));
            bind_idx += 1;
        }

        sql.push_str(&parts.join(", "));
        sql.push_str(" WHERE id = $1");

        let mut q = sqlx::query(&sql).bind(account_id);
        if let Some(v) = new_login_handle_name.flatten() {
            q = q.bind(v);
        }
        if let Some(v) = new_change_password_url.flatten() {
            q = q.bind(v);
        }
        if let Some(v) = new_auth_discovery_url.flatten() {
            q = q.bind(v);
        }
        if let Some(v) = new_unknown_user_url.flatten() {
            q = q.bind(v);
        }
        if let Some(v) = new_login_help_url.flatten() {
            q = q.bind(v);
        }
        if let Some(v) = new_discovery_page_active {
            q = q.bind(v);
        }

        q.execute(&app.db).await.map_err(ApiError::from)?;
    }

    // Return updated settings.
    let updated: Option<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<bool>,
    )> = sqlx::query_as(
        "SELECT login_handle_name, change_password_url, auth_discovery_url,
                    unknown_user_url, login_help_url, discovery_page_active
             FROM accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_optional(&app.db)
    .await?;

    let (
        login_handle_name,
        change_password_url,
        auth_discovery_url,
        unknown_user_url,
        login_help_url,
        discovery_page_active,
    ) = updated.unwrap_or((None, None, None, None, None, None));

    Ok(Json(serde_json::json!({
        "sso_settings": {
            "login_handle_name": login_handle_name,
            "change_password_url": change_password_url,
            "auth_discovery_url": auth_discovery_url,
            "unknown_user_url": unknown_user_url,
            "login_help_url": login_help_url,
            "discovery_page_active": discovery_page_active,
        }
    })))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_password_hash_roundtrip() {
        let h = hash_password("test123").unwrap();
        assert!(verify_password("test123", &h));
        assert!(!verify_password("wrong", &h));
    }
    #[test]
    fn test_create_and_verify_token() {
        let secret = "test-secret";
        let token = create_token(1, "admin", secret).unwrap();
        let claims = verify_token(&token, secret).unwrap();
        assert_eq!(claims.sub, 1);
        assert_eq!(claims.role, "admin");
    }
    #[test]
    fn test_verify_invalid_token() {
        assert!(verify_token("garbage", "secret").is_none());
    }
    #[test]
    fn test_expired_token() {
        use chrono::Utc;
        use jsonwebtoken::{encode, EncodingKey, Header};
        use star_common::Claims;
        let expired = Claims {
            sub: 1,
            role: "student".to_string(),
            exp: (Utc::now().timestamp() - 3600) as usize,
        };
        let token = encode(
            &Header::default(),
            &expired,
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();
        assert!(verify_token(&token, "secret").is_none());
    }

    #[test]
    fn test_public_registration_cannot_grant_elevated_roles() {
        // Regression: /auth/register is unauthenticated, so an attacker must
        // not be able to self-assign a global elevated role. Every elevated
        // (or unknown) role request must be rejected, and the only role the
        // resolver can ever return is "student".
        for elevated in [
            "admin",
            "teacher",
            "Admin",
            "ADMIN",
            "root",
            "superadmin",
            "",
        ] {
            let resolved = resolve_public_registration_role(Some(elevated));
            assert!(
                resolved.is_err(),
                "role '{elevated}' must not be grantable at public registration"
            );
        }
        // Safe inputs resolve to exactly "student".
        assert_eq!(resolve_public_registration_role(None).unwrap(), "student");
        assert_eq!(
            resolve_public_registration_role(Some("student")).unwrap(),
            "student"
        );
    }

    // -- OAuth2 token endpoint helpers --------------------------------------

    #[test]
    fn test_opaque_token_is_random_hex() {
        let first = generate_opaque_token(32);
        let second = generate_opaque_token(32);
        assert_eq!(first.len(), 64);
        assert_eq!(second.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second, "tokens must be random");
    }

    #[test]
    fn test_percent_decode_form_values() {
        // Plain values pass through, '+' is a space, %XX is decoded.
        assert_eq!(percent_decode("abc"), "abc");
        assert_eq!(percent_decode("a+b%20c"), "a b c");
        assert_eq!(
            percent_decode("url%3AGET%7C%2Fapi%2Fv1%2Fcourses"),
            "url:GET|/api/v1/courses"
        );
        // Malformed escapes are kept verbatim.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn test_oauth_form_request_parsing() {
        let req = OAuthTokenRequest::from_form(
            "grant_type=client_credentials&client_id=12&client_secret=ab%2Bcd\
             &scope=url%3AGET%7C%2Fapi%2Fv1%2Fcourses+url%3APOST%7C%2Fapi%2Fv1%2Fcourses",
        );
        assert_eq!(req.grant_type.as_deref(), Some("client_credentials"));
        assert_eq!(req.client_id.as_deref(), Some("12"));
        assert_eq!(req.client_secret.as_deref(), Some("ab+cd"));
        assert_eq!(
            req.scope.as_deref(),
            Some("url:GET|/api/v1/courses url:POST|/api/v1/courses")
        );
        assert!(req.refresh_token.is_none());
    }

    #[test]
    fn test_secrets_match_is_exact() {
        assert!(secrets_match("abc", "abc"));
        assert!(!secrets_match("abc", "abd"));
        assert!(!secrets_match("abc", "abcd"));
        assert!(!secrets_match("", "x"));
        assert!(secrets_match("", ""));
    }

    // -- Personal access token helpers --------------------------------------

    #[test]
    fn test_personal_token_hint_and_expiration_parsing() {
        // Canvas token_hint is the first 5 characters of the token.
        assert_eq!(token_hint("abcdef0123456789"), "abcde");
        assert_eq!(token_hint("abc"), "abc");

        // RFC3339 timestamps (the documented form).
        let expected = DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            parse_token_expires_at(Some("2030-01-02T03:04:05Z"))
                .unwrap()
                .unwrap(),
            expected
        );
        // Date-only values mean midnight UTC, like Canvas Time.zone.parse.
        let date_only = parse_token_expires_at(Some("2030-01-02")).unwrap().unwrap();
        assert_eq!(date_only.to_rfc3339(), "2030-01-02T00:00:00+00:00");
        // Absent/blank means "no expiration"; garbage is rejected.
        assert_eq!(parse_token_expires_at(None).unwrap(), None);
        assert_eq!(parse_token_expires_at(Some("   ")).unwrap(), None);
        assert!(parse_token_expires_at(Some("next tuesday")).is_err());
    }

    // -- Extractor: verified token + DB-sourced role (needs DATABASE_URL) ----

    async fn extractor_app_state() -> Option<AppState> {
        let database_url = std::env::var("DATABASE_URL").ok()?;
        let db = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await
            .expect("connect to DATABASE_URL");
        star_db::MIGRATOR
            .run(&db)
            .await
            .expect("run star-lms database migrations");
        Some(AppState {
            db,
            jwt_secret: "test-extractor-secret".to_string(),
        })
    }

    fn bearer_parts(token: &str) -> Parts {
        let req = http::Request::builder()
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(())
            .expect("build request");
        req.into_parts().0
    }

    #[tokio::test]
    async fn test_authuser_uses_state_secret_and_db_role() {
        let Some(app) = extractor_app_state().await else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };

        // Throwaway user whose database role is 'student'.
        let uniq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let email = format!("authuser-{uniq}@test.star");
        let (user_id,): (i64,) = sqlx::query_as(
            "INSERT INTO users (name, email, password_hash, role)
             VALUES ($1, $2, $3, 'student') RETURNING id",
        )
        .bind("AuthUser Test")
        .bind(&email)
        .bind("unused-hash")
        .fetch_one(&app.db)
        .await
        .expect("insert test user");

        // 1) The role claim says 'admin' but the DB says 'student': the
        //    database role must win and the claim must be ignored.
        let token = create_token(user_id, "admin", &app.jwt_secret).unwrap();
        let user = AuthUser::from_request_parts(&mut bearer_parts(&token), &app)
            .await
            .expect("valid token for an existing user");
        assert_eq!(user.id, user_id);
        assert_eq!(user.role, "student");

        // 2) A token signed with any other secret must be rejected: the
        //    extractor uses AppState.jwt_secret, never an env fallback.
        let forged = create_token(user_id, "student", "unrelated-secret").unwrap();
        let err = AuthUser::from_request_parts(&mut bearer_parts(&forged), &app)
            .await
            .unwrap_err();
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);

        // 3) A valid token for a user that no longer exists must be rejected.
        let ghost = create_token(i64::MAX, "admin", &app.jwt_secret).unwrap();
        let err = AuthUser::from_request_parts(&mut bearer_parts(&ghost), &app)
            .await
            .unwrap_err();
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);

        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&app.db)
            .await
            .expect("cleanup test user");
    }
}
