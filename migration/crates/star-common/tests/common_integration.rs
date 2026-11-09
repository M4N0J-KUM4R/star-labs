//! Integration tests for star-common.
//!
//! Covers the crate's three public surfaces without involving any domain
//! crate:
//! - ApiError -> HTTP status/body mapping produced by the IntoResponse impl.
//! - Claims serde shape plus JWT encode/decode roundtrip, mirroring how
//!   star-auth mints and verifies session tokens (create_token/verify_token).
//! - AppState construction and Clone against a real Postgres pool.
//!
//! Conventions (mirrors the rest of the workspace):
//! - The DB-backed test is skipped cleanly when DATABASE_URL is not set
//!   (prints SKIP and returns); all other tests are pure and always run.
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-common
//! - The DB-backed test creates no rows, so no run_prefix seeding/teardown
//!   is needed here.

use axum::http::StatusCode;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde_json::json;
use sqlx::PgPool;
use star_common::{ApiError, AppState, Claims};

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Connect and run migrations. Returns None when DATABASE_URL is unset so
/// callers can skip cleanly.
async fn db_pool() -> Option<PgPool> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    Some(pool)
}

/// Expiry one hour in the future.
fn future_exp() -> usize {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600) as usize
}

/// Expiry one hour in the past.
fn past_exp() -> usize {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 3600) as usize
}

/// Mint an HS256 token for the claims, the same way star-auth::create_token
/// does (the only difference being the fixed test secret).
fn mint(claims: &Claims, secret: &str) -> String {
    encode(
        &Header::default(),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode token")
}

/// Verify a token and return its claims, the same way star-auth::verify_token
/// does (default validation: HS256, exp required).
fn verify(token: &str, secret: &str) -> Option<Claims> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .ok()
    .map(|data| data.claims)
}

// ---------------------------------------------------------------------------
// ApiError -> HTTP mapping
// ---------------------------------------------------------------------------

#[test]
fn api_error_status_matches_variant() {
    assert_eq!(ApiError::NotFound.status(), StatusCode::NOT_FOUND);
    assert_eq!(ApiError::Unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(ApiError::Forbidden.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        ApiError::Conflict("email already in use".into()).status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        ApiError::Validation("email must contain '@'".into()).status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        ApiError::BadRequest("malformed body".into()).status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        ApiError::Db("boom".into()).status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[tokio::test]
async fn api_error_into_response_maps_status_and_body() {
    use axum::response::IntoResponse;

    // The IntoResponse impl produces (status, Display string): a plain-text
    // body, not a JSON error envelope.
    let cases = [
        (ApiError::NotFound, StatusCode::NOT_FOUND, "not found"),
        (
            ApiError::Unauthorized,
            StatusCode::UNAUTHORIZED,
            "unauthorized",
        ),
        (ApiError::Forbidden, StatusCode::FORBIDDEN, "forbidden"),
        (
            ApiError::Conflict("email already in use".into()),
            StatusCode::CONFLICT,
            "conflict: email already in use",
        ),
        (
            ApiError::Validation("email must contain '@'".into()),
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation: email must contain '@'",
        ),
        (
            ApiError::BadRequest("malformed body".into()),
            StatusCode::BAD_REQUEST,
            "bad request: malformed body",
        ),
        (
            ApiError::Db("boom".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
            "database error",
        ),
    ];

    for (err, status, body) in cases {
        let label = format!("{err}");
        let resp = err.into_response();
        assert_eq!(resp.status(), status, "status for {label}");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("read response body");
        assert_eq!(
            bytes.as_ref(),
            body.as_bytes(),
            "body for {label}: body is the Display string, not JSON"
        );
    }
}

#[test]
fn api_error_from_sqlx_error_maps_to_db() {
    // Every `?` on a sqlx error in the handlers funnels through this From
    // impl: it must become a 500 Db error.
    let err = ApiError::from(sqlx::Error::RowNotFound);
    assert!(matches!(err, ApiError::Db(_)));
    assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------
// Claims serde + JWT roundtrip
// ---------------------------------------------------------------------------

#[test]
fn claims_json_field_names_are_stable() {
    let claims = Claims {
        sub: 42,
        role: "teacher".to_string(),
        exp: 1_900_000_000,
    };

    let v = serde_json::to_value(&claims).expect("serialize claims");
    let obj = v.as_object().expect("claims serialize to an object");
    assert_eq!(obj.len(), 3, "unexpected extra claim fields: {obj:?}");
    assert_eq!(v["sub"], json!(42));
    assert_eq!(v["role"], json!("teacher"));
    assert_eq!(v["exp"], json!(1_900_000_000));
}

#[test]
fn claims_deserialize_from_json() {
    let c: Claims =
        serde_json::from_str(r#"{"sub":7,"role":"admin","exp":123}"#).expect("deserialize claims");
    assert_eq!(c.sub, 7);
    assert_eq!(c.role, "admin");
    assert_eq!(c.exp, 123);
}

#[test]
fn claims_jwt_roundtrip() {
    let claims = Claims {
        sub: 99,
        role: "student".to_string(),
        exp: future_exp(),
    };

    let token = mint(&claims, "test-jwt-secret");
    let back = verify(&token, "test-jwt-secret").expect("valid token must decode");

    assert_eq!(back.sub, claims.sub);
    assert_eq!(back.role, claims.role);
    assert_eq!(back.exp, claims.exp);
}

#[test]
fn claims_jwt_wrong_secret_rejected() {
    let claims = Claims {
        sub: 1,
        role: "student".to_string(),
        exp: future_exp(),
    };

    let token = mint(&claims, "test-jwt-secret");
    assert!(
        verify(&token, "unrelated-secret").is_none(),
        "a token signed with another secret must not verify"
    );
}

#[test]
fn claims_jwt_expired_token_rejected() {
    let claims = Claims {
        sub: 1,
        role: "student".to_string(),
        exp: past_exp(),
    };

    let token = mint(&claims, "test-jwt-secret");
    assert!(
        verify(&token, "test-jwt-secret").is_none(),
        "an expired token must not verify"
    );
}

#[test]
fn claims_jwt_tampered_payload_rejected() {
    let student_claims = Claims {
        sub: 1,
        role: "student".to_string(),
        exp: future_exp(),
    };
    let admin_claims = Claims {
        sub: 1,
        role: "admin".to_string(),
        exp: future_exp(),
    };

    let student_token = mint(&student_claims, "test-jwt-secret");
    let admin_token = mint(&admin_claims, "test-jwt-secret");

    // Swap the admin payload segment into the student token: the signature
    // no longer matches, so verification must fail.
    let s: Vec<&str> = student_token.split('.').collect();
    let a: Vec<&str> = admin_token.split('.').collect();
    assert_eq!(s.len(), 3, "JWTs have three segments");
    let forged = format!("{}.{}.{}", s[0], a[1], s[2]);
    assert!(
        verify(&forged, "test-jwt-secret").is_none(),
        "a tampered token must not verify"
    );
}

// ---------------------------------------------------------------------------
// AppState (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn app_state_clones_and_holds_working_pool() {
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let state = AppState {
        db: pool,
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let clone = state.clone();
    assert_eq!(clone.jwt_secret, "test-jwt-secret");

    // The original and the clone share a usable pool (handlers receive
    // cloned state on every request).
    for db in [&state.db, &clone.db] {
        let (one,): (i64,) = sqlx::query_as("SELECT 1::BIGINT")
            .fetch_one(db)
            .await
            .expect("trivial query through AppState.db");
        assert_eq!(one, 1);
    }
}
