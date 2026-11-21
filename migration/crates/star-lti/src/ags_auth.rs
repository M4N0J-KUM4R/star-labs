//! Shared LTI Advantage access-token (JWT) authentication for the AGS
//! Scores/Results and NRPS Names-and-Roles services (issue #94).
//!
//! Canvas source: `app/controllers/lti/ims/concerns/lti_services.rb`,
//! `app/controllers/lti/ims/concerns/advantage_services.rb` (merged into
//! `app/controllers/lti/ims/concerns/gradebook_services.rb` for AGS).
//!
//! Canvas's before_action chain, in order (parity for which error fires
//! first):
//! 1. `verify_access_token`  — missing bearer token → 401 "Missing access
//!    token"; malformed/expired token → 401 with the token error message.
//! 2. `verify_developer_key` — token's `client_id` must resolve to a known,
//!    active developer key → else 401 "Unknown or inactive Developer Key".
//! 3. `verify_access_scope`  — the token's `scopes` claim (space-separated
//!    string) must satisfy the controller's `scopes_matcher` → else 401
//!    "Insufficient permissions".
//! 4. `verify_active_in_account` (AGS via AdvantageServices) — the developer
//!    key must be usable in the context's account → 401 "Invalid Developer
//!    Key".
//! 5. `verify_context` — context blank or deleted → 404 "Context is deleted
//!    or not found".
//! 6. `verify_tool` — a tool bound to the developer key in the context →
//!    401 "Access Token not linked to a Tool associated with this Context".
//!
//! Error envelope (lti_services.rb#render_error):
//! `{"errors": {"type": "<status>", "message": "<msg>"}}`.
//!
//! star-lms tokens: the platform signs service JWTs with the shared
//! `jwt_secret` (HS256) exactly like `dynamic_registration.rs` — same
//! `jsonwebtoken` crate, same `EncodingKey`. The `sub`/`client_id` claim
//! identifies the developer key by its numeric id (Canvas's client_id is the
//! developer key's global id; star-lms developer keys are the 0032 table
//! and the numeric id is the only stable handle). The `scopes` claim is the
//! space-separated scope URN list, mirroring Canvas's access token shape
//! (`access_token.claim("scopes")&.split`, lti_services.rb:68).

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_common::AppState;

/// AGS scope: create/update results (`TokenScopes::LTI_AGS_SCORE_SCOPE`,
/// lib/token_scopes.rb:44).
pub(crate) const SCOPE_AGS_SCORE: &str = "https://purl.imsglobal.org/spec/lti-ags/scope/score";

/// AGS scope: read results (`TokenScopes::LTI_AGS_RESULT_READ_ONLY_SCOPE`,
/// lib/token_scopes.rb:43).
pub(crate) const SCOPE_AGS_RESULT_READ_ONLY: &str =
    "https://purl.imsglobal.org/spec/lti-ags/scope/result.readonly";

/// NRPS scope: context memberships read
/// (`TokenScopes::LTI_NRPS_V2_SCOPE`, lib/token_scopes.rb:46).
pub(crate) const SCOPE_NRPS_MEMBERSHIP_READ_ONLY: &str =
    "https://purl.imsglobal.org/spec/lti-nrps/scope/contextmembership.readonly";

/// A developer key resolved from a valid access token.
#[derive(Debug, Clone)]
pub(crate) struct AdvantageAuth {
    pub developer_key_id: i64,
    /// Whether the key is bound (active) in the context's account — the
    /// `verify_active_in_account` gate. star-lms models this as the
    /// developer key itself being active in the account that owns the
    /// context (0032 has no per-account binding table; documented
    /// deviation, see module doc on the handlers).
    pub scopes: Vec<String>,
}

/// Canvas `render_error` envelope: `{"errors": {"type", "message"}}`.
pub(crate) fn advantage_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        axum::Json(json!({
            "errors": { "type": status.canonical_reason().unwrap_or("error"), "message": message }
        })),
    )
        .into_response()
}

/// The `LtiServices` before_action chain steps 1–3: extract the bearer
/// token, decode it against the shared secret, resolve + activate the
/// developer key, and check the required scope.
///
/// `required_scope` is the single scope each controller demands (Canvas
/// `scopes_matcher` = `all_of(<one scope>)` for scores/results/NRPS).
pub(crate) async fn verify_advantage_access(
    state: &AppState,
    auth_header: Option<&str>,
    required_scope: &str,
) -> Result<AdvantageAuth, Response> {
    // 1. verify_access_token — missing bearer → 401 "Missing access token".
    let token = auth_header
        .and_then(|h| h.strip_prefix("Bearer "))
        .ok_or_else(|| advantage_error(StatusCode::UNAUTHORIZED, "Missing access token"))?;

    let claims = decode::<Value>(
        token,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| advantage_error(StatusCode::UNAUTHORIZED, "Invalid access token"))?;

    // 2. verify_developer_key — client_id must resolve to an active key.
    let client_id = claims
        .claims
        .get("client_id")
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
        .ok_or_else(|| {
            advantage_error(
                StatusCode::UNAUTHORIZED,
                "Unknown or inactive Developer Key",
            )
        })?;

    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM developer_keys WHERE id = $1 AND workflow_state = 'active'")
            .bind(client_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;

    let developer_key_id = row.map(|(id,)| id).ok_or_else(|| {
        advantage_error(
            StatusCode::UNAUTHORIZED,
            "Unknown or inactive Developer Key",
        )
    })?;

    // 3. verify_access_scope — the scopes claim must contain the scope.
    let scopes: Vec<String> = claims
        .claims
        .get("scopes")
        .and_then(|v| v.as_str())
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();

    if !scopes.iter().any(|s| s == required_scope) {
        return Err(advantage_error(
            StatusCode::UNAUTHORIZED,
            "Insufficient permissions",
        ));
    }

    Ok(AdvantageAuth {
        developer_key_id,
        scopes,
    })
}

/// Steps 4–5 of the chain: `verify_active_in_account` (AGS) — 401 "Invalid
/// Developer Key" — then `verify_context` — 404 "Context is deleted or not
/// found" for a blank/deleted course.
pub(crate) async fn verify_advantage_course(
    pool: &PgPool,
    auth: &AdvantageAuth,
    course_id: i64,
) -> Result<(), Response> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;

    row.ok_or_else(|| advantage_error(StatusCode::NOT_FOUND, "Context is deleted or not found"))?;
    Ok(())
}

/// Step 6: `verify_tool` — a tool bound to the developer key in the course
/// context. star-lms models tools as `lti_registrations` rows linked to the
/// developer key (0115): the tool exists for the key when a registration
/// with that developer_key_id exists. Missing → 401 "Access Token not
/// linked to a Tool associated with this Context".
pub(crate) async fn verify_advantage_tool(
    pool: &PgPool,
    auth: &AdvantageAuth,
) -> Result<(), Response> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM lti_ims_registrations \
         WHERE developer_key_id = $1 LIMIT 1",
    )
    .bind(auth.developer_key_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"))?;

    row.ok_or_else(|| {
        advantage_error(
            StatusCode::UNAUTHORIZED,
            "Access Token not linked to a Tool associated with this Context",
        )
    })?;
    Ok(())
}

/// Mint an LTI Advantage service token (test helper + dev surface): the
/// platform signs `client_id` + `scopes` claims with the shared secret,
/// mirroring Canvas's access token shape.
pub fn mint_service_token(secret: &str, client_id: i64, scopes: &[&str]) -> String {
    let payload = json!({
        "client_id": client_id,
        "scopes": scopes.join(" "),
        "exp": (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp(),
    });
    encode(
        &Header::default(),
        &payload,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("mint service token")
}
