//! star-auth — login, register, logout, and auth middleware for star-lms.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    body::Bytes,
    extract::{FromRef, FromRequest, Path, Query, Request, State},
    http::{
        header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST, LOCATION, WWW_AUTHENTICATE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use axum_core::extract::FromRequestParts;
use chrono::{DateTime, Datelike, Duration, Utc};
use http::request::Parts;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use star_common::{ApiError, AppState, Claims};

// ---------------------------------------------------------------------------
// Login attempt throttling (issue #106 security pass)
// ---------------------------------------------------------------------------

/// How many consecutive failed logins lock an account — Canvas
/// `DEFAULT_LOGIN_ATTEMPTS` (`lib/canvas/security/password_policy.rb:28`).
pub const MAX_FAILED_LOGINS: u32 = 10;

/// How long a lock lasts — Canvas keeps Redis counters with a 30-minute TTL
/// (`login_registry.rb`), so the workspace equivalent releases the lock on
/// the same schedule instead of waiting for an administrator.
const LOCKOUT_WINDOW_SECS: i64 = 30 * 60;

/// Per-account consecutive failed login tracker (issue #106).
///
/// Canvas throttles logins through `Canvas::Security::LoginRegistry` with a
/// Redis hash per pseudonym (`lib/canvas/security/login_registry.rb`):
/// after `maximum_login_attempts` (default 10) consecutive failures the
/// next attempt answers `:too_many_attempts` and is refused BEFORE the
/// password is checked; a successful login clears the counter; the Redis
/// key expires so stale locks release themselves. star-lms has no Redis,
/// and the deployment is a single star-http binary, so the same semantics
/// are kept in process memory: a `Mutex<HashMap>` keyed by lowercase email
/// with a lock deadline. A restart clears the counters, which only makes
/// the limit more permissive, never less.
#[derive(Default)]
pub struct LoginRateLimiter {
    inner: Mutex<HashMap<String, FailedAttempts>>,
}

#[derive(Clone, Copy)]
struct FailedAttempts {
    count: u32,
    /// When the current lock (if any) expires; `None` while only counting.
    locked_until: Option<DateTime<Utc>>,
}

impl LoginRateLimiter {
    /// Whether an attempt for this email is currently allowed.
    ///
    /// A lock whose deadline has passed releases itself (Canvas: Redis TTL),
    /// so the caller only sees `false` while the 30-minute window is open.
    pub fn is_locked(&self, email: &str) -> bool {
        let key = email.trim().to_lowercase();
        let Ok(inner) = self.inner.lock() else {
            return false; // poisoned lock: fail open, never wedge logins
        };
        match inner.get(&key) {
            Some(attempts) => match attempts.locked_until {
                Some(until) if until > Utc::now() => true,
                Some(_) => false, // expired lock — the next recordFailure resets it
                None => false,
            },
            None => false,
        }
    }

    /// Record a failed attempt; returns whether the account is now locked.
    pub fn record_failure(&self, email: &str) -> bool {
        let key = email.trim().to_lowercase();
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        let entry = inner.entry(key).or_insert(FailedAttempts {
            count: 0,
            locked_until: None,
        });
        if entry.locked_until.is_some_and(|until| until <= Utc::now()) {
            // The previous lock expired — start a fresh window.
            entry.count = 0;
            entry.locked_until = None;
        }
        entry.count += 1;
        if entry.count >= MAX_FAILED_LOGINS {
            entry.locked_until = Some(Utc::now() + Duration::seconds(LOCKOUT_WINDOW_SECS));
            return true;
        }
        false
    }

    /// Record a successful login: the counter clears (Canvas
    /// `successful_login!`).
    pub fn record_success(&self, email: &str) {
        let key = email.trim().to_lowercase();
        if let Ok(mut inner) = self.inner.lock() {
            inner.remove(&key);
        }
    }

    /// Drop counters whose lock expired (housekeeping so the map cannot
    /// grow without bound across unique emails).
    pub fn prune_expired(&self) {
        let now = Utc::now();
        if let Ok(mut inner) = self.inner.lock() {
            inner.retain(|_, attempts| {
                !matches!(attempts.locked_until, Some(until) if until <= now) && attempts.count > 0
            });
        }
    }
}

/// The workspace-global limiter shared by the API login route and the UI
/// shell's login form — both must count against the same budget or an
/// attacker just switches surfaces.
pub fn login_rate_limiter() -> &'static LoginRateLimiter {
    static LIMITER: std::sync::OnceLock<LoginRateLimiter> = std::sync::OnceLock::new();
    LIMITER.get_or_init(LoginRateLimiter::default)
}

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

/// Create an HS256 JWT with arbitrary claims (service tokens, scoped
/// preview tokens). The payload's `exp` (if present) is respected.
pub fn create_custom_token(claims: &serde_json::Value, secret: &str) -> Result<String, String> {
    encode(
        &Header::default(),
        claims,
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

mod progress_jwts;
mod small_controllers;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(progress_jwts::progress_jwt_routes())
        .merge(small_controllers::small_controllers_routes())
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/me", get(me))
        .route("/auth/logout", post(logout))
        // Canvas Developer Keys API. star-http nests every domain router
        // under /api/v1, so these resolve as /api/v1/developer_keys.
        .route(
            "/developer_keys",
            get(list_developer_keys).post(create_developer_key),
        )
        .route(
            "/developer_keys/{id}",
            get(show_developer_key)
                .put(update_developer_key)
                .delete(destroy_developer_key),
        )
        // Canvas personal access tokens (tokens_controller). Canvas has no
        // flat /tokens route: the API lives under
        // /api/v1/users/:user_id/tokens (config/routes.rb:1432-1437), where
        // :user_id may be `self` or a numeric id, plus the separate
        // user_generated_tokens listing (routes.rb:1433).
        .route(
            "/users/{user_id}/user_generated_tokens",
            get(list_user_generated_tokens),
        )
        .route("/users/{user_id}/tokens", post(create_user_token))
        .route(
            "/users/{user_id}/tokens/{id}",
            get(show_user_token)
                .put(update_user_token)
                .delete(destroy_user_token),
        )
        // Canvas inst_access_tokens#create (config/routes.rb:2777-2778,
        // ApiRouteSet::V1 -> POST /api/v1/inst_access_tokens).
        .route("/inst_access_tokens", post(create_inst_access_token))
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

/// Canvas login/oauth2 family. These routes live OUTSIDE /api/v1
/// (config/routes.rb:3087-3094), so star-http merges this router at the top
/// level next to star-ui and the LTI Advantage router — exactly like
/// `star_lti::lti_advantage_router`.
pub fn login_router() -> Router<AppState> {
    Router::new()
        // GET /login/oauth2/auth (oauth2_provider#auth, routes.rb:3087)
        .route("/login/oauth2/auth", get(oauth2_auth))
        // POST + DELETE /login/oauth2/token (oauth2_provider#token/#destroy,
        // routes.rb:3088 + 3093)
        .route(
            "/login/oauth2/token",
            post(oauth2_token).delete(oauth2_token_destroy),
        )
        // GET /login/oauth2/jwks (security#jwks, routes.rb:3094)
        .route("/login/oauth2/jwks", get(oauth2_jwks))
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
    // Refuse locked accounts before touching the database or the password
    // hash — Canvas `LoginRegistry.audit_login` returns :too_many_attempts
    // up front (login_registry.rb:68).
    let limiter = login_rate_limiter();
    if limiter.is_locked(&req.email) {
        return Err(ApiError::Forbidden);
    }

    // Fetch user by email.
    let row: Option<(i64, String, String)> =
        sqlx::query_as("SELECT id, password_hash, role FROM users WHERE email = $1")
            .bind(&req.email)
            .fetch_optional(&app.db)
            .await
            .map_err(ApiError::from)?;

    let Some((user_id, password_hash, role)) = row else {
        // Unknown email: count the failure too, so the route cannot be used
        // to enumerate accounts by lockout timing. Same generic 401.
        limiter.record_failure(&req.email);
        return Err(ApiError::Unauthorized);
    };

    // Verify password.
    if !verify_password(&req.password, &password_hash) {
        limiter.record_failure(&req.email);
        return Err(ApiError::Unauthorized);
    }
    // Success clears the counter (Canvas successful_login!).
    limiter.record_success(&req.email);

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

/// GET /developer_keys/{id} — show a single non-deleted key (admin only).
/// Canvas itself has no GET /developer_keys/:id route (the UI filters the
/// account listing); this completes the crate's CRUD trio with the same
/// Canvas-shaped JSON as the list/create/destroy actions.
async fn show_developer_key(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let key = fetch_developer_key(&app.db, id).await?;

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)
         FROM access_tokens
         WHERE developer_key_id = $1 AND workflow_state = 'active'",
    )
    .bind(id)
    .fetch_one(&app.db)
    .await?;

    Ok(Json(developer_key_json(&key, count)))
}

/// PUT /developer_keys/{id} — update a key (admin only). Canvas'
/// `DeveloperKeysController#update` assigns the submitted developer_key
/// params over the existing key, so omitted fields keep their stored values.
/// The client secret is never rotated by an update.
async fn update_developer_key(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    // 404 for unknown or already-deleted keys.
    let existing = fetch_developer_key(&app.db, id).await?;

    let raw = body.get("developer_key").cloned().unwrap_or(body);
    let params: DeveloperKeyParams = serde_json::from_value(raw)
        .map_err(|e| ApiError::BadRequest(format!("invalid developer_key: {e}")))?;

    // Canvas' deprecated single redirect_uri is folded into redirect_uris on
    // create; on update the array form wins when both are sent.
    let mut redirect_uris = params
        .redirect_uris
        .map(StringList::into_vec)
        .unwrap_or_else(|| existing.redirect_uris.clone());
    if let Some(single) = params
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .filter(|single| !single.is_empty())
        .filter(|single| !redirect_uris.iter().any(|uri| uri.as_str() == *single))
    {
        redirect_uris.push(single.to_string());
    }
    let scopes = params
        .scopes
        .map(StringList::into_vec)
        .unwrap_or_else(|| existing.scopes.clone());

    sqlx::query(
        r#"
        UPDATE developer_keys
        SET name = $2, email = $3, icon_url = $4, notes = $5,
            redirect_uris = $6, scopes = $7, require_scopes = $8,
            allow_includes = $9, visible = $10, vendor_code = $11,
            auto_expire_tokens = $12, client_credentials_audience = $13,
            updated_at = now()
        WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(id)
    .bind(params.name.or(existing.name))
    .bind(params.email.or(existing.email))
    .bind(params.icon_url.or(existing.icon_url))
    .bind(params.notes.or(existing.notes))
    .bind(redirect_uris)
    .bind(scopes)
    .bind(params.require_scopes.unwrap_or(existing.require_scopes))
    .bind(params.allow_includes.unwrap_or(existing.allow_includes))
    .bind(params.visible.unwrap_or(existing.visible))
    .bind(params.vendor_code.or(existing.vendor_code))
    .bind(
        params
            .auto_expire_tokens
            .unwrap_or(existing.auto_expire_tokens),
    )
    .bind(
        params
            .client_credentials_audience
            .or(existing.client_credentials_audience),
    )
    .execute(&app.db)
    .await?;

    let key = fetch_developer_key(&app.db, id).await?;

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)
         FROM access_tokens
         WHERE developer_key_id = $1 AND workflow_state = 'active'",
    )
    .bind(id)
    .fetch_one(&app.db)
    .await?;

    Ok(Json(developer_key_json(&key, count)))
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

/// Body of POST/PUT .../tokens. Canvas wraps the fields in `token`; the
/// flat form is accepted too, like the developer key handlers.
#[derive(Debug, Default, Deserialize)]
struct TokenParams {
    purpose: Option<String>,
    /// Canvas API name; stored in `permanent_expires_at`.
    expires_at: Option<String>,
    /// Accepted as an alias for `expires_at`.
    permanent_expires_at: Option<String>,
    scopes: Option<StringList>,
    /// PUT only: regenerate the actual token value (tokens#update).
    #[serde(default)]
    regenerate: Option<bool>,
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

/// Resolve the `{user_id}` path segment the way Canvas resolves API user
/// ids: `self` maps to the authenticated user, anything else must be a
/// numeric id (non-numeric values 404). Nonexistent users 404 in the
/// handlers that need the user row to exist.
fn resolve_target_user(auth: &AuthUser, raw: &str) -> Result<i64, ApiError> {
    if raw == "self" {
        return Ok(auth.id);
    }
    raw.parse::<i64>().map_err(|_| ApiError::NotFound)
}

async fn ensure_user_exists(pool: &PgPool, user_id: i64) -> Result<(), ApiError> {
    // SELECT 1 is INT4 in Postgres; cast to BIGINT for the i64 scalar.
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1::BIGINT FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    if exists.is_some() {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// GET /users/{user_id}/user_generated_tokens — Canvas
/// `TokensController#user_generated_tokens` (routes.rb:1433). Lists only the
/// manually created ("user generated") tokens; OAuth-issued tokens never
/// appear. `view_user_generated_access_tokens` maps to owner-or-admin and
/// fails with 401 otherwise, exactly like `render_unauthorized_action`.
async fn list_user_generated_tokens(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
    Query(pag): Query<TokenPagination>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let target = resolve_target_user(&auth, &raw_user_id)?;
    if target != auth.id && require_admin(&auth).is_err() {
        return Err(ApiError::Unauthorized);
    }

    let (per_page, offset) = pag.resolve();
    let sql = format!(
        "{PERSONAL_TOKEN_SELECT}
         WHERE user_id = $1 AND developer_key_id IS NULL
           AND workflow_state <> 'deleted'
         ORDER BY created_at ASC, id ASC
         LIMIT $2 OFFSET $3"
    );
    let tokens: Vec<PersonalAccessToken> = sqlx::query_as(&sql)
        .bind(target)
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

/// POST /users/{user_id}/tokens — Canvas `TokensController#create`
/// (routes.rb:1435). `token[purpose]` is required; tokens created for another
/// user (admin path) start `pending`; the full token value is returned
/// exactly once, in `visible_token`.
async fn create_user_token(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Response, ApiError> {
    let target = resolve_target_user(&auth, &raw_user_id)?;
    ensure_user_exists(&app.db, target).await?;
    if target != auth.id && require_admin(&auth).is_err() {
        return Err(ApiError::Unauthorized);
    }

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
    // Canvas: creating a token for someone else marks it pending until the
    // user activates it (tokens#create, `unless @context == logged_in_user`).
    let workflow_state = if target != auth.id {
        "pending"
    } else {
        "active"
    };

    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO access_tokens
            (user_id, developer_key_id, token, token_hint, purpose, scopes,
             permanent_expires_at, remember_access, workflow_state)
        VALUES ($1, NULL, $2, $3, $4, $5, $6, FALSE, $7)
        RETURNING id
        "#,
    )
    .bind(target)
    .bind(&token)
    .bind(&hint)
    .bind(purpose)
    .bind(scopes)
    .bind(expires_at)
    .bind(workflow_state)
    .fetch_one(&app.db)
    .await?;

    let row = fetch_personal_token_by_id(&app.db, target, id).await?;
    Ok((
        StatusCode::OK,
        Json(personal_token_json(&row, Some(token.as_str()))),
    )
        .into_response())
}

/// GET /users/{user_id}/tokens/{id} — Canvas `TokensController#show`
/// (routes.rb:1434). The id may be the database id or the 5-character
/// `token_hint`.
async fn show_user_token(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((raw_user_id, id_or_hint)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let target = resolve_target_user(&auth, &raw_user_id)?;
    let token = fetch_personal_token(&app.db, target, &id_or_hint).await?;
    // Canvas read right: owner, or admin viewing a user-generated token
    // (`view_user_generated_access_tokens`); everyone else gets 401.
    if target != auth.id && require_admin(&auth).is_err() {
        return Err(ApiError::Unauthorized);
    }
    Ok(Json(personal_token_json(&token, None)))
}

/// PUT /users/{user_id}/tokens/{id} — Canvas `TokensController#update`
/// (routes.rb:1436). Updates purpose/expires_at/scopes and optionally
/// regenerates the token value; regenerating an expired token requires a
/// new expiration date.
async fn update_user_token(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((raw_user_id, id_or_hint)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<Response, ApiError> {
    let target = resolve_target_user(&auth, &raw_user_id)?;
    let token = fetch_personal_token(&app.db, target, &id_or_hint).await?;
    // Canvas update right: owner or admin (`create_access_tokens`); a
    // non-owner without the right gets 404 (RecordNotFound), an owner that
    // somehow lost the right would get 401 — in star owners always keep it,
    // so the 401 branch is unreachable.
    if target != auth.id && require_admin(&auth).is_err() {
        return Err(ApiError::NotFound);
    }

    let raw = body.get("token").cloned().unwrap_or(body);
    let params: TokenParams = serde_json::from_value(raw)
        .map_err(|e| ApiError::BadRequest(format!("invalid token: {e}")))?;

    let expires_at = match parse_token_expires_at(
        params
            .expires_at
            .as_deref()
            .or(params.permanent_expires_at.as_deref()),
    ) {
        Ok(expires_at) => expires_at,
        Err(message) => return Ok(token_error(message)),
    };

    // Canvas: `regenerate` only applies to manually created tokens, and an
    // expired token cannot be regenerated without a new expiration date.
    let expired = token.permanent_expires_at.is_some_and(|at| at < Utc::now());
    let regenerate = params.regenerate.unwrap_or(false);
    if regenerate && expired && expires_at.is_none() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "errors": { "message": "cannot regenerate an expired token without a new expiration date" }
            })),
        )
            .into_response());
    }

    let new_token = if regenerate {
        Some(generate_opaque_token(32))
    } else {
        None
    };

    // Apply the provided fields; absent fields keep their current values
    // (Canvas `update(token_params)` semantics).
    let purpose = params.purpose;
    let scopes = params.scopes.map(StringList::into_vec);
    sqlx::query(
        r#"
        UPDATE access_tokens
        SET purpose = COALESCE($2, purpose),
            scopes = COALESCE($3, scopes),
            permanent_expires_at = COALESCE($4, permanent_expires_at),
            token = COALESCE($5, token),
            token_hint = COALESCE($6, token_hint),
            workflow_state = CASE WHEN $7 THEN 'pending' ELSE workflow_state END,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(token.id)
    .bind(purpose)
    .bind(scopes)
    .bind(expires_at)
    .bind(new_token.as_deref())
    .bind(new_token.as_deref().map(token_hint))
    .bind(target != auth.id)
    .execute(&app.db)
    .await?;

    let row = fetch_personal_token_by_id(&app.db, target, token.id).await?;
    // The full token value is only revealed when it was just regenerated.
    Ok((
        StatusCode::OK,
        Json(personal_token_json(&row, new_token.as_deref())),
    )
        .into_response())
}

/// DELETE /users/{user_id}/tokens/{id} — Canvas `TokensController#destroy`
/// (routes.rb:1437). Soft-deletes (workflow_state -> "deleted") and renders
/// the token JSON, like Canvas.
async fn destroy_user_token(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((raw_user_id, id_or_hint)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let target = resolve_target_user(&auth, &raw_user_id)?;
    let mut token = fetch_personal_token(&app.db, target, &id_or_hint).await?;
    // Canvas destroy right: owner or admin (`delete_access_tokens`);
    // non-owners without the right get 404.
    if target != auth.id && require_admin(&auth).is_err() {
        return Err(ApiError::NotFound);
    }

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
// OAuth2 provider endpoints (/login/oauth2/*, canvas oauth2_provider_controller)
// ---------------------------------------------------------------------------

/// Access token lifetime, in seconds. Canvas client-credentials tokens also
/// default to one hour.
const OAUTH_TOKEN_TTL_SECS: i64 = 3600;

/// Authorization code lifetime — Canvas stores codes in Redis with a
/// 10 minute TTL (`Canvas::OAuth::Token.generate_code_for`).
const OAUTH2_CODE_TTL_SECS: i64 = 600;

/// Canvas's out-of-band redirect URI marker (`Canvas::OAuth::Provider::OAUTH2_OOB_URI`).
const OAUTH2_OOB_URI: &str = "urn:ietf:wg:oauth:2.0:oob";

/// OAuth2 token request fields (Canvas accepts form-encoded parameters; JSON
/// bodies are accepted too for convenience).
#[derive(Debug, Default, Deserialize)]
struct OAuthTokenRequest {
    grant_type: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
    scope: Option<String>,
    code: Option<String>,
    code_verifier: Option<String>,
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
                "code" => params.code = Some(value),
                "code_verifier" => params.code_verifier = Some(value),
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

/// Canvas `oauth2_provider#oauth_error` sets `WWW-Authenticate: Canvas OAuth
/// 2.0` on 401 responses (oauth2_provider_controller.rb:244).
fn oauth_unauthorized(error: &'static str, description: &'static str) -> Response {
    let mut response = oauth_error(StatusCode::UNAUTHORIZED, error, description);
    response.headers_mut().insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static("Canvas OAuth 2.0"),
    );
    response
}

/// Like `oauth_error` but with a dynamic description (invalid_scope lists the
/// missing scopes, like Canvas `InvalidScopeError`).
fn oauth_error_dynamic(status: StatusCode, error: &'static str, description: String) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": error,
            "error_description": description,
        })),
    )
        .into_response()
}

/// A pending authorization code — the analog of Canvas's Redis entry written
/// by `Canvas::OAuth::Token.generate_code_for`: single-use, 10 minute TTL,
/// bound to the user, client, scopes, and optional PKCE challenge.
struct OAuth2Code {
    user_id: i64,
    client_id: i64,
    scopes: Vec<String>,
    purpose: Option<String>,
    code_challenge: Option<String>,
    expires_at: DateTime<Utc>,
}

/// In-process store for authorization codes. Canvas keeps codes in Redis
/// (ephemeral, single-use); star-http is a single process, so a process-local
/// map gives the same lifetime semantics without new infrastructure.
fn oauth2_codes() -> &'static Mutex<HashMap<String, OAuth2Code>> {
    static CODES: OnceLock<Mutex<HashMap<String, OAuth2Code>>> = OnceLock::new();
    CODES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Store a fresh code (128 hex characters, like `SecureRandom.hex(64)`) and
/// prune expired entries while the lock is held.
fn store_oauth2_code(code: OAuth2Code) -> String {
    let value = generate_opaque_token(64);
    let mut codes = oauth2_codes().lock().expect("oauth2 code store lock");
    codes.retain(|_, stored| stored.expires_at > Utc::now());
    codes.insert(value.clone(), code);
    value
}

/// Consume a code: single-use, so it is removed whether or not it is still
/// valid (`Canvas::OAuth::Token.expire_code` on success; TTL otherwise).
fn take_oauth2_code(code: &str) -> Option<OAuth2Code> {
    let mut codes = oauth2_codes().lock().expect("oauth2 code store lock");
    match codes.remove(code) {
        Some(stored) if stored.expires_at > Utc::now() => Some(stored),
        _ => None,
    }
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Decode standard base64 (with or without padding). Only used for HTTP
/// Basic client authentication; a compact implementation avoids a new
/// dependency for two dozen lines of alphabet math.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let mut table = [255u8; 256];
    for (i, &byte) in BASE64_ALPHABET.iter().enumerate() {
        table[byte as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        if byte == b'=' || byte == b'\n' || byte == b'\r' {
            continue;
        }
        let value = table[byte as usize];
        if value == 255 {
            return None;
        }
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// Unpadded base64url encoding, as used by JWK `n`/`e` values and PKCE S256
/// challenges (`Base64.urlsafe_encode64(..., padding: false)`).
fn base64url_encode(bytes: &[u8]) -> String {
    const URL_ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut buffer = [0u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let value = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
        out.push(URL_ALPHABET[(value >> 18) as usize & 63] as char);
        out.push(URL_ALPHABET[(value >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(URL_ALPHABET[(value >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(URL_ALPHABET[value as usize & 63] as char);
        }
    }
    out
}

/// Percent-encode a query component (`URI.encode_www_form_component`).
fn percent_encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `Canvas::OAuth::Provider.final_redirect`: append the (compact) params to
/// the client redirect_uri with `?` or `&` and answer 302. Out-of-band
/// requests are redirected back to the authorization endpoint itself
/// (`controller.oauth2_auth_url(opts)`), exactly like Canvas.
fn oauth2_redirect(uri: &str, params: &[(&str, String)]) -> Response {
    let target = if uri == OAUTH2_OOB_URI {
        "/login/oauth2/auth"
    } else {
        uri
    };
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", percent_encode_query(value)))
        .collect::<Vec<_>>()
        .join("&");
    let separator = if target.contains('?') { '&' } else { '?' };
    (
        StatusCode::FOUND,
        [(LOCATION, format!("{target}{separator}{query}"))],
    )
        .into_response()
}

/// Redirect back to the client redirect_uri with `error`/`error_description`
/// (and `state`, when the request carried one). Pairs are emitted in the
/// alphabetical order Rails `Hash#to_query` produces (error <
/// error_description < state).
fn oauth2_redirect_error(
    redirect_uri: &str,
    state: Option<&str>,
    error: &str,
    description: &str,
) -> Response {
    let mut params = vec![
        ("error", error.to_string()),
        ("error_description", description.to_string()),
    ];
    if let Some(state) = state {
        params.push(("state", state.to_string()));
    }
    oauth2_redirect(redirect_uri, &params)
}

/// Rails `to_sentence` (en locale) for the missing-scopes message.
fn to_sentence(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first, last] => format!("{first} and {last}"),
        many => {
            // split_last returns (last, rest).
            let (last, all_but_last) = many.split_last().expect("nonempty");
            format!("{}, and {}", all_but_last.join(", "), last)
        }
    }
}

/// Resolve the optional "logged in" user from a Bearer session token — the
/// star analog of Canvas's browser session (`logged_in_user`). Only the
/// signature and expiry are verified.
fn bearer_session_user(app: &AppState, headers: &HeaderMap) -> Option<i64> {
    let header = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let token = header.strip_prefix("Bearer ")?;
    verify_token(token, &app.jwt_secret).map(|claims| claims.sub)
}

/// HTTP Basic credentials (client_id, client_secret), accepted by Canvas
/// `oauth2_provider#token` alongside form parameters
/// (oauth2_provider_controller.rb:179-181).
fn basic_auth_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let header = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let encoded = header.strip_prefix("Basic ")?;
    let decoded = base64_decode(encoded)?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, pass) = text.split_once(':')?;
    Some((user.to_string(), pass.to_string()))
}

/// Host part of an absolute URI (scheme://host[:port]/path...).
fn uri_host(uri: &str) -> Option<String> {
    uri.split_once("://")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .map(str::to_string)
        .filter(|host| !host.is_empty())
}

/// `Canvas::OAuth::Provider#has_valid_redirect?`: the out-of-band URI, or a
/// URI whose host matches one of the key's registered redirect URIs
/// (`DeveloperKey#redirect_domain_matches?`).
fn redirect_uri_allowed(key: &DeveloperKey, uri: &str) -> bool {
    if uri == OAUTH2_OOB_URI {
        return true;
    }
    let Some(host) = uri_host(uri) else {
        return false;
    };
    key.redirect_uris
        .iter()
        .any(|allowed| uri_host(allowed).as_deref() == Some(host.as_str()))
}

/// `Canvas::OAuth::Token#as_json`: the issued token, its type, the user
/// object, the region, the refresh token only when freshly generated, and
/// `expires_in` only when the developer key auto-expires tokens.
async fn oauth_user_token_json(
    app: &AppState,
    key: &DeveloperKey,
    access_token: &str,
    refresh_token: Option<&str>,
    user_id: i64,
) -> Response {
    let user: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&app.db)
        .await
        .ok()
        .flatten();
    let name = user.map(|(name,)| name).unwrap_or_default();
    // star users have no shard-prefixed global ids; the numeric id is the
    // stable public identifier.
    let global_id = user_id.to_string();
    let mut body = serde_json::json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "user": {
            "id": user_id,
            "name": name,
            "global_id": global_id,
            "effective_locale": "en",
            "fake_student": false,
        },
        "canvas_region": "unknown",
    });
    if let Some(refresh_token) = refresh_token {
        body["refresh_token"] = serde_json::Value::String(refresh_token.to_string());
    }
    if key.auto_expire_tokens {
        body["expires_in"] = serde_json::Value::from(OAUTH_TOKEN_TTL_SECS);
    }
    (StatusCode::OK, Json(body)).into_response()
}

/// Query parameters of GET /login/oauth2/auth
/// (`oauth2_provider_controller#auth`).
#[derive(Debug, Default, Deserialize)]
struct OAuthAuthParams {
    client_id: Option<String>,
    redirect_uri: Option<String>,
    response_type: Option<String>,
    scope: Option<String>,
    scopes: Option<String>,
    purpose: Option<String>,
    state: Option<String>,
    prompt: Option<String>,
    force_login: Option<String>,
    canvas_login: Option<String>,
    authentication_provider: Option<String>,
    unique_id: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
}

/// The PKCE challenge to persist, mirroring
/// `Canvas::OAuth::PKCE.use_pkce_in_authorization?`: both parameters present
/// and the method must be S256 (the only method Canvas supports).
fn pkce_challenge(params: &OAuthAuthParams) -> Option<String> {
    let challenge = params.code_challenge.as_deref()?.trim();
    let method = params.code_challenge_method.as_deref()?;
    if challenge.is_empty() || method != "S256" {
        return None;
    }
    Some(challenge.to_string())
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

/// POST /login/oauth2/token — Canvas `oauth2_provider#token`
/// (oauth2_provider_controller.rb:178-214). Client credentials come from
/// the form/JSON body or HTTP Basic auth; `grant_type` defaults to
/// authorization_code when only a `code` is supplied
/// (oauth2_provider_controller.rb:248-252).
async fn oauth2_token(
    State(app): State<AppState>,
    headers: HeaderMap,
    params: OAuthTokenRequest,
) -> Response {
    let (basic_user, basic_pass) = basic_auth_credentials(&headers)
        .map(|(user, pass)| (Some(user), Some(pass)))
        .unwrap_or((None, None));
    let client_id = params
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or(basic_user);
    let client_secret = params
        .client_secret
        .as_deref()
        .map(str::trim)
        .filter(|secret| !secret.is_empty())
        .map(str::to_string)
        .or(basic_pass)
        .unwrap_or_default();
    let grant_type = params.grant_type.clone().or_else(|| {
        params
            .code
            .as_deref()
            .map(|_| "authorization_code".to_string())
    });

    match grant_type.as_deref() {
        Some("authorization_code") => {
            grant_authorization_code(&app, &client_id, &client_secret, &params).await
        }
        Some("refresh_token") => {
            grant_refresh_token(&app, &client_id, &client_secret, &params).await
        }
        Some("client_credentials") => {
            grant_client_credentials(&app, &client_id, &client_secret, &params, &headers).await
        }
        _ => oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "The grant_type you requested is not currently supported",
        ),
    }
}

/// `GrantTypes::BaseType#validate_client_id_and_secret`: unknown client
/// 401 "unknown client", wrong secret 401 "invalid client" (both with the
/// WWW-Authenticate header, like Canvas's `oauth_error` rescue). Small error
/// type — http::Response is too large for a Result variant
/// (clippy::result_large_err).
enum OAuthClientError {
    UnknownClient,
    InvalidSecret,
    Lookup(ApiError),
}

fn oauth_client_error_response(error: OAuthClientError) -> Response {
    match error {
        OAuthClientError::UnknownClient => oauth_unauthorized("invalid_client", "unknown client"),
        OAuthClientError::InvalidSecret => oauth_unauthorized("invalid_client", "invalid client"),
        OAuthClientError::Lookup(err) => internal_error("oauth token: lookup developer key", err),
    }
}

async fn validate_oauth_client(
    app: &AppState,
    client_id: &Option<String>,
    client_secret: &str,
) -> Result<DeveloperKey, OAuthClientError> {
    let Some(client_id) = client_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Err(OAuthClientError::UnknownClient);
    };
    match find_active_developer_key(&app.db, client_id).await {
        Ok(Some(key)) => {
            if secrets_match(client_secret, &key.api_key) {
                Ok(key)
            } else {
                Err(OAuthClientError::InvalidSecret)
            }
        }
        Ok(None) => Err(OAuthClientError::UnknownClient),
        Err(err) => Err(OAuthClientError::Lookup(err)),
    }
}

/// `grant_type=authorization_code` (GrantTypes::AuthorizationCode): the code
/// must exist, belong to this client, and — when the exchange carries a
/// `code_verifier` — match the stored PKCE S256 challenge. Codes are
/// single-use and expire after 10 minutes.
async fn grant_authorization_code(
    app: &AppState,
    client_id: &Option<String>,
    client_secret: &str,
    params: &OAuthTokenRequest,
) -> Response {
    let key = match validate_oauth_client(app, client_id, client_secret).await {
        Ok(key) => key,
        Err(error) => return oauth_client_error_response(error),
    };

    let Some(code_value) = params
        .code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
    else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "You must provide the code parameter when using the authorization_code grant type",
        );
    };

    let Some(code) = take_oauth2_code(code_value) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "authorization_code not found",
        );
    };
    if code.client_id != key.id {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "incorrect client");
    }

    // PKCE (GrantTypes::AuthorizationCodeWithPKCE): when a verifier is sent,
    // base64url(SHA-256(verifier)) must equal the stored challenge.
    if let Some(verifier) = params
        .code_verifier
        .as_deref()
        .map(str::trim)
        .filter(|verifier| !verifier.is_empty())
    {
        let matches = code.code_challenge.as_deref().is_some_and(|challenge| {
            let digest = Sha256::digest(verifier.as_bytes());
            base64url_encode(&digest) == challenge
        });
        if !matches {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "The provided authorization grant is invalid, expired, revoked, does not match the redirection URI used in the authorization request, or was issued to another client.",
            );
        }
    }

    let access_token = generate_opaque_token(32);
    let refresh_token = generate_opaque_token(32);
    let expires_at = key
        .auto_expire_tokens
        .then(|| Utc::now() + Duration::seconds(OAUTH_TOKEN_TTL_SECS));
    if let Err(err) = sqlx::query(
        r#"
        INSERT INTO access_tokens
            (user_id, developer_key_id, token, token_hint, refresh_token,
             scopes, purpose, expires_at, remember_access, workflow_state)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, FALSE, 'active')
        "#,
    )
    .bind(code.user_id)
    .bind(key.id)
    .bind(&access_token)
    .bind(token_hint(&access_token))
    .bind(&refresh_token)
    .bind(&code.scopes)
    .bind(&code.purpose)
    .bind(expires_at)
    .execute(&app.db)
    .await
    {
        return internal_error("oauth token: issue authorization_code token", err);
    }

    oauth_user_token_json(app, &key, &access_token, Some(&refresh_token), code.user_id).await
}

/// `grant_type=refresh_token` (GrantTypes::RefreshToken): the refresh token
/// must be usable (active, not pending, not past `permanent_expires_at`) and
/// belong to this client. Confidential clients keep their refresh token —
/// only the access token is regenerated (Canvas rotates refresh tokens for
/// public clients only, and star has no public clients).
async fn grant_refresh_token(
    app: &AppState,
    client_id: &Option<String>,
    client_secret: &str,
    params: &OAuthTokenRequest,
) -> Response {
    let key = match validate_oauth_client(app, client_id, client_secret).await {
        Ok(key) => key,
        Err(error) => return oauth_client_error_response(error),
    };

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

    // Canvas AccessToken.authenticate_refresh_token -> usable?
    let row: Option<(i64, Option<i64>, Option<i64>)> = match sqlx::query_as(
        "SELECT id, developer_key_id, user_id
         FROM access_tokens
         WHERE refresh_token = $1 AND workflow_state = 'active'
           AND (permanent_expires_at IS NULL OR permanent_expires_at > now())",
    )
    .bind(refresh_token)
    .fetch_optional(&app.db)
    .await
    {
        Ok(row) => row,
        Err(err) => return internal_error("oauth token: lookup refresh token", err),
    };

    let Some((token_id, developer_key_id, user_id)) = row else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "refresh_token not found",
        );
    };
    if developer_key_id != Some(key.id) {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", "incorrect client");
    }
    // Service (client-credentials) tokens carry no user in Canvas and are
    // not refreshable; a user-less row is treated as unknown.
    let Some(user_id) = user_id else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "refresh_token not found",
        );
    };

    let access_token = generate_opaque_token(32);
    if let Err(err) = sqlx::query(
        r#"
        UPDATE access_tokens
        SET token = $2,
            token_hint = $3,
            expires_at = CASE WHEN $4 THEN now() + make_interval(secs => $5)
                              ELSE expires_at END,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(token_id)
    .bind(&access_token)
    .bind(token_hint(&access_token))
    .bind(key.auto_expire_tokens)
    .bind(OAUTH_TOKEN_TTL_SECS)
    .execute(&app.db)
    .await
    {
        return internal_error("oauth token: regenerate access token", err);
    }

    // Canvas does not echo the refresh token back for confidential clients
    // (`plaintext_refresh_token` is only populated on rotation).
    oauth_user_token_json(app, &key, &access_token, None, user_id).await
}

/// `grant_type=client_credentials` (GrantTypes::ClientCredentials with the
/// symmetric provider): only keys with an "external" audience may use it,
/// the requested scopes must be a subset of the key's scopes, and the
/// response carries a signed JWT rather than a stored opaque token
/// (ClientCredentialsProvider#generate_token).
async fn grant_client_credentials(
    app: &AppState,
    client_id: &Option<String>,
    client_secret: &str,
    params: &OAuthTokenRequest,
    headers: &HeaderMap,
) -> Response {
    let key = match validate_oauth_client(app, client_id, client_secret).await {
        Ok(key) => key,
        Err(error) => return oauth_client_error_response(error),
    };

    // Canvas SymmetricClientCredentialsProvider#assertion_method_permitted?
    if key.client_credentials_audience.as_deref() != Some("external") {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "assertion method not supported for this grant_type",
        );
    }

    let scopes = requested_scopes(params, &key);
    // Canvas Provider#valid_scopes? — the requested scopes must be present
    // and each must be on the developer key.
    let missing: Vec<String> = scopes
        .iter()
        .filter(|scope| !key.scopes.iter().any(|allowed| allowed == *scope))
        .cloned()
        .collect();
    if scopes.is_empty() || !missing.is_empty() {
        return oauth_error_dynamic(StatusCode::BAD_REQUEST, "invalid_scope", missing.join(" "));
    }

    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    let now = Utc::now();
    let claims = serde_json::json!({
        // Canvas: Canvas::Security.config["lti_iss"]; star's fixed issuer.
        "iss": "star-lms",
        "sub": key.id.to_string(),
        // Canvas: oauth2_token_url(host:, protocol:) — the token endpoint
        // itself is the audience.
        "aud": format!("http://{host}/login/oauth2/token"),
        "iat": now.timestamp(),
        "exp": now.timestamp() + OAUTH_TOKEN_TTL_SECS,
        "jti": generate_opaque_token(16),
        "scopes": scopes.join(" "),
    });
    let token = match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(app.jwt_secret.as_bytes()),
    ) {
        Ok(token) => token,
        Err(err) => return internal_error("oauth token: sign client_credentials token", err),
    };

    // Canvas ClientCredentialsProvider#generate_token: access_token,
    // token_type, expires_in, scope.
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "access_token": token,
            "token_type": "Bearer",
            "expires_in": OAUTH_TOKEN_TTL_SECS,
            "scope": scopes.join(" "),
        })),
    )
        .into_response()
}

/// DELETE /login/oauth2/token — Canvas `oauth2_provider#destroy`
/// (oauth2_provider_controller.rb:216-232). Revokes the OAuth access token
/// the request is authenticated with; anything else (session JWTs, unknown
/// strings) gets Canvas's 400 message.
async fn oauth2_token_destroy(State(app): State<AppState>, headers: HeaderMap) -> Response {
    let bearer = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string);

    let revoked: Option<i64> = match bearer.as_deref() {
        Some(token) => {
            match sqlx::query_scalar::<_, i64>(
                "SELECT id FROM access_tokens
                 WHERE token = $1 AND workflow_state = 'active'
                   AND (permanent_expires_at IS NULL OR permanent_expires_at > now())",
            )
            .bind(token)
            .fetch_optional(&app.db)
            .await
            {
                Ok(found) => found,
                Err(err) => return internal_error("oauth destroy: lookup token", err),
            }
        }
        None => None,
    };

    let Some(token_id) = revoked else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "can't delete OAuth access token when not using an OAuth access token"
            })),
        )
            .into_response();
    };

    if let Err(err) = sqlx::query(
        "UPDATE access_tokens SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(token_id)
    .execute(&app.db)
    .await
    {
        return internal_error("oauth destroy: revoke token", err);
    }

    (StatusCode::OK, Json(serde_json::json!({}))).into_response()
}

/// GET /login/oauth2/auth — Canvas `oauth2_provider#auth`
/// (oauth2_provider_controller.rb:31-117). Validates the client, redirect,
/// scopes, account authorization, and response_type in Canvas's order — the
/// first two render JSON errors, the later ones redirect back with `error`
/// params — then issues a single-use authorization code bound to the
/// signed-in user.
///
/// Canvas continues into an HTML confirm/accept/deny consent flow for keys
/// that are not pre-authorized; that UI is star-ui shell scope. star
/// short-circuits the way Canvas does for trusted keys
/// (`Provider.confirmation_redirect` -> `final_redirect`): a signed-in user
/// receives the code immediately.
async fn oauth2_auth(
    State(app): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<OAuthAuthParams>,
) -> Response {
    // Canvas reads `scope` or `scopes` (oauth2_provider_controller.rb:39).
    let scopes: Vec<String> = params
        .scope
        .clone()
        .or_else(|| params.scopes.clone())
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();

    let key = match params
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        None => return oauth_unauthorized("invalid_client", "unknown client"),
        Some(client_id) => match find_active_developer_key(&app.db, client_id).await {
            Ok(Some(key)) => key,
            Ok(None) => return oauth_unauthorized("invalid_client", "unknown client"),
            Err(err) => return internal_error("oauth auth: lookup developer key", err),
        },
    };

    let redirect_uri = params.redirect_uri.clone().unwrap_or_default();
    if !redirect_uri_allowed(&key, &redirect_uri) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "redirect_uri does not match client settings",
        );
    }

    // Canvas checks require_scopes before the account check, the
    // response_type check, and the prompt handling.
    if key.require_scopes {
        let missing: Vec<String> = scopes
            .iter()
            .filter(|scope| !key.scopes.iter().any(|allowed| allowed == *scope))
            .cloned()
            .collect();
        if scopes.is_empty() || !missing.is_empty() {
            return oauth2_redirect_error(
                &redirect_uri,
                params.state.as_deref(),
                "invalid_scope",
                &format!(
                    "A requested scope is invalid, unknown, malformed, or exceeds the scope granted by the resource owner. The following scopes were requested, but not granted: {}",
                    to_sentence(&missing)
                ),
            );
        }
    }

    // Canvas `authorized_for_account?`; star's developer keys carry the
    // `visible` flag instead of account bindings.
    if !key.visible {
        return oauth2_redirect_error(
            &redirect_uri,
            params.state.as_deref(),
            "unauthorized_client",
            "Client does not have access to the specified Canvas account.",
        );
    }

    if params.response_type.as_deref() != Some("code") {
        return oauth2_redirect_error(
            &redirect_uri,
            params.state.as_deref(),
            "unsupported_response_type",
            "Only response_type=code is permitted",
        );
    }

    let session_user = bearer_session_user(&app, &headers);
    match params.prompt.as_deref() {
        None => {}
        Some("none") => {
            return match session_user {
                None => oauth2_redirect_error(
                    &redirect_uri,
                    params.state.as_deref(),
                    "login_required",
                    "prompt=none but there is no current session",
                ),
                // Canvas authorized_token? is only true for trusted keys or
                // reusable (force_token_reuse / userinfo) tokens — star has
                // neither, so interaction is always required.
                Some(_) => oauth2_redirect_error(
                    &redirect_uri,
                    params.state.as_deref(),
                    "interaction_required",
                    "prompt=none but a token cannot be granted without user interaction",
                ),
            };
        }
        // prompt=login forces the login screen even with a session
        // (oauth2_provider_controller.rb:99-100).
        Some("login") => {}
        Some(_) => {
            return oauth2_redirect_error(
                &redirect_uri,
                params.state.as_deref(),
                "unsupported_prompt_type",
                "prompt must be \"none\" (or omitted)",
            );
        }
    }

    // Not signed in — or force-login requested: Canvas redirects to its
    // login URL with the rememberable parameters
    // (oauth2_provider_controller.rb:108-116); star-ui serves /login.
    let force_login = params.prompt.as_deref() == Some("login")
        || params
            .force_login
            .as_deref()
            .is_some_and(|value| value == "1" || value == "true");
    let effective_session = if force_login { None } else { session_user };
    let Some(user_id) = effective_session else {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(canvas_login) = params.canvas_login.as_deref() {
            query.push(("canvas_login", canvas_login.to_string()));
        }
        if force_login {
            query.push(("force_login", "1".to_string()));
        }
        if let Some(provider) = params.authentication_provider.as_deref() {
            query.push(("authentication_provider", provider.to_string()));
        }
        if let Some(unique_id) = params.unique_id.as_deref() {
            query.push(("pseudonym_session%5Bunique_id%5D", unique_id.to_string()));
        }
        // Rails `to_query` sorts the pairs alphabetically.
        query.sort_by_key(|(name, _)| *name);
        let login = if query.is_empty() {
            "/login".to_string()
        } else {
            format!(
                "/login?{}",
                query
                    .iter()
                    .map(|(key, value)| format!("{key}={}", percent_encode_query(value)))
                    .collect::<Vec<_>>()
                    .join("&")
            )
        };
        return (StatusCode::FOUND, [(LOCATION, login)]).into_response();
    };

    let code = store_oauth2_code(OAuth2Code {
        user_id,
        client_id: key.id,
        scopes,
        purpose: params.purpose.clone(),
        code_challenge: pkce_challenge(&params),
        expires_at: Utc::now() + Duration::seconds(OAUTH2_CODE_TTL_SECS),
    });
    let mut redirect_params = vec![("code", code)];
    if let Some(state) = params.state.as_deref() {
        redirect_params.push(("state", state.to_string()));
    }
    oauth2_redirect(&redirect_uri, &redirect_params)
}

// ---------------------------------------------------------------------------
// JWKS (security#jwks) + InstAccess tokens
// ---------------------------------------------------------------------------

/// Cache-Control max-age for the JWKS response (Canvas sets the key
/// storage's `max_cache_age` header).
const JWKS_MAX_CACHE_AGE_SECS: u32 = 3600;

/// The instance's OAuth signing key: Canvas stores the "oauth2-asymmetric"
/// keypair in its security key storage (`Canvas::OAuth::KeyStorage`) and
/// publishes only the public half through /login/oauth2/jwks. star loads a
/// PKCS#8 PEM from `STAR_OAUTH_SIGNING_KEY` when the deployment provides
/// one, and otherwise generates a fresh 2048-bit keypair on first use —
/// key material is NEVER committed to the repository (the secret scan
/// would rightly flag it).
struct OAuthSigningKey {
    kid: String,
    /// JWK `n` (base64url, unpadded).
    n: String,
    /// JWK `e` (base64url, unpadded).
    e: String,
    /// (year, month) encoded in the kid, for the rotation_check report.
    created: (i32, u32),
    encoding: jsonwebtoken::EncodingKey,
}

fn oauth_signing_key() -> Result<&'static OAuthSigningKey, String> {
    static KEY: OnceLock<Result<OAuthSigningKey, String>> = OnceLock::new();
    KEY.get_or_init(|| {
        use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey};
        use rsa::traits::PublicKeyParts;

        let private = match std::env::var("STAR_OAUTH_SIGNING_KEY") {
            Ok(pem) if !pem.trim().is_empty() => rsa::RsaPrivateKey::from_pkcs8_pem(pem.trim())
                .map_err(|err| {
                    format!("STAR_OAUTH_SIGNING_KEY is not a valid PKCS#8 PEM: {err}")
                })?,
            _ => {
                // No deployment key: generate a per-process instance key.
                // Only the public half is ever served; the private half
                // lives in memory for the life of the process.
                use argon2::password_hash::rand_core::OsRng;
                rsa::RsaPrivateKey::new(&mut OsRng, 2048)
                    .map_err(|err| format!("failed to generate OAuth signing key: {err}"))?
            }
        };

        let now = Utc::now();
        let kid = format!(
            "star-oauth-{:04}{:02}{:02}",
            now.year(),
            now.month(),
            now.day()
        );
        let created = (now.year(), now.month());
        let public = rsa::RsaPublicKey::from(&private);
        let pem = private
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .map_err(|err| format!("failed to serialize OAuth signing key: {err}"))?;
        let encoding = jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|err| format!("failed to load OAuth signing key: {err}"))?;

        Ok(OAuthSigningKey {
            kid,
            // Canvas JWKs use unpadded base64url for n and e
            // (canvas_security gem -> JSON::JWK).
            n: base64url_encode(&public.n().to_bytes_be()),
            e: base64url_encode(&public.e().to_bytes_be()),
            created,
            encoding,
        })
    })
    .as_ref()
    .map_err(std::clone::Clone::clone)
}

#[derive(Debug, Default, Deserialize)]
struct JwksQuery {
    rotation_check: Option<String>,
}

/// GET /login/oauth2/jwks — Canvas `SecurityController#jwks`
/// (security_controller.rb:75-95, route routes.rb:3094): the public keyset
/// in JWKS shape with a Cache-Control header, or — with `rotation_check` — a
/// report of whether each key's kid-encoded date is from the current month.
/// The platform JWKS as plain JSON (shared by the OAuth2 and LTI security
/// endpoints).
pub(crate) async fn oauth2_jwks_json() -> Json<serde_json::Value> {
    match oauth_signing_key() {
        Ok(key) => Json(serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "alg": "RS256",
                "use": "sig",
                "kid": key.kid,
                "n": key.n,
                "e": key.e,
            }]
        })),
        Err(_) => Json(serde_json::json!({ "keys": [] })),
    }
}

async fn oauth2_jwks(Query(params): Query<JwksQuery>) -> Response {
    if params.rotation_check.is_some() {
        let today = Utc::now();
        let key = oauth_signing_key();
        let this_month = match key {
            Ok(key) => (today.year(), today.month()) == key.created,
            Err(_) => false,
        };
        let report = format!(
            "today is day {} and key 0 is {}from this month",
            today.day(),
            if this_month { "" } else { "not " }
        );
        return Json(serde_json::json!([report])).into_response();
    }

    let Ok(key) = oauth_signing_key() else {
        return internal_error(
            "oauth jwks: signing key unavailable",
            "signing key generation failed",
        );
    };
    let mut response = Json(serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "alg": "RS256",
            "use": "sig",
            "kid": key.kid,
            "n": key.n,
            "e": key.e,
        }]
    }))
    .into_response();
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_str(&format!("max-age={JWKS_MAX_CACHE_AGE_SECS}"))
            .expect("static cache-control header"),
    );
    response
}

/// POST /api/v1/inst_access_tokens — Canvas
/// `InstAccessTokensController#create` (inst_access_tokens_controller.rb:58-73):
/// a short-lived (1 hour) signed JWT for the authenticated user, returned as
/// `{"token": ...}` with 201. Canvas encrypts its tokens for the API gateway
/// (JWE); star returns the RS256-signed JWT directly — the gateway is
/// deployment infrastructure outside REST parity.
async fn create_inst_access_token(
    auth: AuthUser,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    let now = Utc::now();
    // Claim mapping: Canvas user_uuid/user_global_id -> star's numeric user
    // id (star users have no UUID column); region is unknown in star.
    let claims = serde_json::json!({
        "sub": auth.id.to_string(),
        "user_id": auth.id,
        "canvas_domain": host,
        "region": "unknown",
        "iat": now.timestamp(),
        "exp": (now + Duration::seconds(OAUTH_TOKEN_TTL_SECS)).timestamp(),
    });
    let key = oauth_signing_key().map_err(|err| ApiError::Db(err.clone()))?;
    let header = jsonwebtoken::Header {
        alg: jsonwebtoken::Algorithm::RS256,
        kid: Some(key.kid.clone()),
        ..Default::default()
    };
    let token = encode(&header, &claims, &key.encoding)
        .map_err(|err| ApiError::Db(format!("inst access token: {err}")))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "token": token })),
    )
        .into_response())
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

type SsoSettingsRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<bool>,
);

/// GET /accounts/{account_id}/sso_settings — return current SSO settings.
async fn show_sso_settings(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&auth)?;

    let row: Option<SsoSettingsRow> = sqlx::query_as(
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
        let mut bind_idx = 1;

        if new_login_handle_name.is_some() {
            parts.push(format!("login_handle_name = ${}", bind_idx));
            bind_idx += 1;
        }
        if new_change_password_url.is_some() {
            parts.push(format!("change_password_url = ${}", bind_idx));
            bind_idx += 1;
        }
        if new_auth_discovery_url.is_some() {
            parts.push(format!("auth_discovery_url = ${}", bind_idx));
            bind_idx += 1;
        }
        if new_unknown_user_url.is_some() {
            parts.push(format!("unknown_user_url = ${}", bind_idx));
            bind_idx += 1;
        }
        if new_login_help_url.is_some() {
            parts.push(format!("login_help_url = ${}", bind_idx));
            bind_idx += 1;
        }
        if new_discovery_page_active.is_some() {
            parts.push(format!("discovery_page_active = ${}", bind_idx));
            bind_idx += 1;
        }

        sql.push_str(&parts.join(", "));
        // The account_id placeholder must come after every SET
        // placeholder, and be bound last, or every value shifts one
        // column over.
        sql.push_str(&format!(" WHERE id = ${}", bind_idx));

        let mut q = sqlx::query(&sql);
        // Bind the Option (not its flattened value): a key sent with an
        // explicit null must bind NULL to un-set the column, matching
        // Canvas's update_sso_settings semantics.
        if let Some(v) = new_login_handle_name {
            q = q.bind(v);
        }
        if let Some(v) = new_change_password_url {
            q = q.bind(v);
        }
        if let Some(v) = new_auth_discovery_url {
            q = q.bind(v);
        }
        if let Some(v) = new_unknown_user_url {
            q = q.bind(v);
        }
        if let Some(v) = new_login_help_url {
            q = q.bind(v);
        }
        if let Some(v) = new_discovery_page_active {
            q = q.bind(v);
        }
        q = q.bind(account_id);

        q.execute(&app.db).await.map_err(ApiError::from)?;
    }

    // Return updated settings.
    let updated: Option<SsoSettingsRow> = sqlx::query_as(
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
