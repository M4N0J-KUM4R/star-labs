//! star-auth — login, register, logout, and auth middleware for star-lms.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{FromRef, State},
    http::{header::AUTHORIZATION, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use axum_core::extract::FromRequestParts;
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
        let (role,): (String,) = sqlx::query_as("SELECT role FROM users WHERE id = $1")
            .bind(claims.sub)
            .fetch_optional(&app.db)
            .await
            .map_err(ApiError::from)?
            .ok_or(ApiError::Unauthorized)?;

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

#[derive(Serialize, sqlx::FromRow)]
struct UserResponse {
    id: i64,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sortable_name: Option<String>,
    email: String,
    role: String,
    created_at: chrono::DateTime<chrono::Utc>,
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
