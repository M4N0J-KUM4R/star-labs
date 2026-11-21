//! LTI 2 OAuth2 authorization server — the port of Canvas
//! `app/controllers/lti/ims/authorization_controller.rb`.
//!
//! Routes (Canvas `config/routes.rb`, `ApiRouteSet.draw(self, "/api/lti")`):
//! - `POST /api/lti/courses/:course_id/authorize`
//! - `POST /api/lti/accounts/:account_id/authorize`
//!
//! Canvas validates the `assertion` JWT with the tool proxy's split secret
//! or the developer key (the key's `api_key` doubles as the HMAC secret —
//! `Lti::OAuth2::AuthorizationValidator#jwt_secret`); the port accepts the
//! developer-key path and signs the access token with the platform secret.
//! All failures render `{"error": "invalid_grant"}` with 400, exactly like
//! Canvas' `rescue_from`.

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::{ApiError, AppState};
use std::collections::HashMap;

/// Canvas `JWT_GRANT_TYPE`.
const JWT_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// Canvas `AUTHORIZATION_CODE_GRANT_TYPE`.
const AUTHORIZATION_CODE_GRANT_TYPE: &str = "authorization_code";

/// Canvas `Lti::OAuth2::AccessToken::ISS`.
const ACCESS_TOKEN_ISS: &str = "Canvas";

pub(crate) fn advantage_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/courses/{course_id}/authorize",
            post(authorize_course),
        )
        .route(
            "/api/lti/accounts/{account_id}/authorize",
            post(authorize_account),
        )
}

/// `POST /api/lti/courses/:course_id/authorize` — Canvas
/// `lti/ims/authorization#authorize`.
async fn authorize_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    headers: HeaderMap,
    Form(params): Form<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    authorize(state, headers, "courses", course_id, params).await
}

/// `POST /api/lti/accounts/:account_id/authorize` — same action.
async fn authorize_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    headers: HeaderMap,
    Form(params): Form<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    authorize(state, headers, "accounts", account_id, params).await
}

fn invalid_grant() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "invalid_grant" })),
    )
        .into_response()
}

async fn authorize(
    state: AppState,
    headers: HeaderMap,
    context_kind: &str,
    context_id: i64,
    params: HashMap<String, String>,
) -> Result<Response, ApiError> {
    // Canvas `before_action :require_context` — unknown context → 404.
    let table = if context_kind == "accounts" {
        "accounts"
    } else {
        "courses"
    };
    let exists: Option<(i64,)> = sqlx::query_as(&format!("SELECT id FROM {table} WHERE id = $1"))
        .bind(context_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Canvas: `raise InvalidGrant unless GRANT_TYPES.include?(grant_type)`
    // and `raise InvalidGrant if params[:assertion].blank?`.
    let grant_type = params.get("grant_type").map(String::as_str);
    if grant_type != Some(JWT_GRANT_TYPE) && grant_type != Some(AUTHORIZATION_CODE_GRANT_TYPE) {
        return Ok(invalid_grant());
    }
    let assertion = params.get("assertion").map(String::as_str).unwrap_or("");
    if assertion.is_empty() {
        return Ok(invalid_grant());
    }

    // Unverified decode: the `sub` claim identifies the tool proxy or the
    // developer key (`Lti::OAuth2::AuthorizationValidator#unverified_jwt`).
    let unverified = decode_unverified(assertion);
    let sub = unverified
        .as_ref()
        .and_then(|claims| claims.get("sub"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // `jwt_secret`: the developer key's `api_key` doubles as the HMAC
    // secret (`AuthorizationValidator#jwt_secret`). No resolvable key →
    // `SecretNotFound` → 400 invalid_grant.
    let secret = match &sub {
        sub if !sub.is_empty() => find_developer_key_secret(&state.db, sub).await?,
        _ => None,
    };
    let Some(secret) = secret else {
        return Ok(invalid_grant());
    };

    // Signature verification.
    let claims = jsonwebtoken::decode::<Value>(
        assertion,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &jsonwebtoken::Validation::default(),
    )
    .map(|data| data.claims)
    .map_err(|_| ApiError::Unauthorized)
    .ok();
    let Some(claims) = claims else {
        return Ok(invalid_grant());
    };

    // `Canvas::Security::JwtValidator` (REQUIRED: sub, aud, exp, iat, jti;
    // aud must equal the authorization URL; exp must not have passed).
    let host = request_host(&headers);
    let authorization_url = format!("http://{host}/api/lti/{context_kind}/{context_id}/authorize");
    let aud_ok = claims
        .get("aud")
        .map(|aud| match aud {
            Value::String(s) => s == &authorization_url,
            Value::Array(items) => items
                .iter()
                .any(|item| item.as_str() == Some(authorization_url.as_str())),
            _ => false,
        })
        .unwrap_or(false);
    let has = |key: &str| claims.get(key).is_some();
    let exp_ok = claims
        .get("exp")
        .and_then(|v| v.as_i64())
        .map(|exp| exp > chrono::Utc::now().timestamp())
        .unwrap_or(false);
    let iat_ok = claims
        .get("iat")
        .and_then(|v| v.as_i64())
        .map(|iat| {
            let now = chrono::Utc::now().timestamp();
            iat <= now + 60 && iat >= now - 300
        })
        .unwrap_or(false);
    if !has("sub") || !has("jti") || !has("iat") || !aud_ok || !exp_ok || !iat_ok {
        return Ok(invalid_grant());
    }

    // Canvas: `access_token: Lti::OAuth2::AccessToken.create_jwt(aud:, sub:, reg_key:)`
    // — iss "Canvas", aud is the host list, `reg_key` is `code || sub`.
    let reg_key = params
        .get("code")
        .cloned()
        .filter(|code| !code.is_empty())
        .unwrap_or_else(|| sub.clone());
    let now = chrono::Utc::now().timestamp();
    let access_token_claims = json!({
        "iss": ACCESS_TOKEN_ISS,
        "aud": [host],
        "sub": sub,
        "reg_key": reg_key,
        "shard_id": 0,
        "iat": now,
        "exp": now + 3600,
    });
    let access_token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &access_token_claims,
        &jsonwebtoken::EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .map_err(|e| ApiError::Db(e.to_string()))?;

    // Canvas: `{ access_token:, token_type: "bearer", expires_in: 1.hour.to_s }`.
    Ok((
        StatusCode::OK,
        Json(json!({
            "access_token": access_token,
            "token_type": "bearer",
            "expires_in": "3600",
        })),
    )
        .into_response())
}

/// Decode a JWT without verifying it (to read the `iss`/`sub` claims).
fn decode_unverified(token: &str) -> Option<Value> {
    let mut validation = jsonwebtoken::Validation::default();
    validation.validate_aud = false;
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.required_spec_claims.clear();
    validation.insecure_disable_signature_validation = true;
    jsonwebtoken::decode::<Value>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(b""),
        &validation,
    )
    .ok()
    .map(|data| data.claims)
}

/// Canvas `DeveloperKey.find_cached(sub)` — the `api_key` doubles as the
/// signing secret (`jwt_secret`).
async fn find_developer_key_secret(pool: &PgPool, sub: &str) -> Result<Option<String>, ApiError> {
    let numeric = sub.parse::<i64>().ok();
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT api_key FROM developer_keys WHERE api_key = $1 \
         OR ($2::bigint IS NOT NULL AND id = $2) LIMIT 1",
    )
    .bind(sub)
    .bind(numeric)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(api_key,)| api_key))
}

/// The request host (for the `aud` checks Canvas derives from the request).
pub(crate) fn request_host(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost:3000")
        .to_string()
}
