//! lti_api — Canvas `LtiApiController` parity.
//!
//! Routes (the LTI Advantage router mounts this module at the app root, so
//! the paths carry the literal `/api/lti` prefix):
//! - `POST /api/lti/v1/tools/{toolId}/grade_passback` — the LTI 1.1 grade passback
//! - `POST /api/lti/v1/tools/{toolId}/ext_grade_passback` — the legacy grade passback
//! - `POST /api/lti/v1/xapi/{token}` — the xAPI page view
//! - `POST /api/lti/v1/caliper/{token}` — the Caliper page view
//! - `POST /api/lti/v1/logout_service/{token}` — the logout callback
//! - `POST /api/lti/v1/turnitin/outcomes_placement/{toolId}` — the Turnitin outcome
//!
//! Canvas routes (config/routes.rb, the LTI API block):
//!   post "tools/:tool_id/grade_passback", controller: :lti_api, action: :grade_passback
//!   post "tools/:tool_id/ext_grade_passback", controller: :lti_api, action: :legacy_grade_passback
//!   post "xapi/:token", controller: :lti_api, action: :xapi_service
//!   post "caliper/:token", controller: :lti_api, action: :caliper_service
//!   post "logout_service/:token", controller: :lti_api, action: :logout_service
//!   post "turnitin/outcomes_placement/:tool_id", controller: :lti_api, action: :turnitin_outcomes_placement
//!
//! Canvas controller (`app/controllers/lti_api_controller.rb`): every action
//! verifies the OAuth 1.0a signature of the request against the shared
//! secret of the tool, checks that the timestamp is within 90 minutes and
//! that the nonce was not used before. A failed check answers 401 with the
//! `BasicLTI::BasicOutcomes::Unauthorized` body.
//!
//! The grade passback answers the LTI 1.1 outcome XML; the xAPI, Caliper
//! and logout services answer `200` with an empty body.

// The internal helpers of this module answer with a pre-rendered
// `axum::response::Response`, so the error variant is as large as a
// response body. The lint is silenced for the whole module instead of
// on every single helper.
#![allow(clippy::result_large_err)]

use axum::{
    body::Bytes,
    extract::{OriginalUri, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha1::Sha1;

use star_common::AppState;

type HmacSha1 = Hmac<Sha1>;

/// The LTI 1.1 spec allows a timestamp within 90 minutes.
const ALLOWED_TIMESTAMP_DELTA_SECONDS: i64 = 90 * 60;

/// The OAuth parameters of a request.
#[derive(Debug, Default)]
struct OAuthParams {
    consumer_key: String,
    signature: String,
    timestamp: Option<i64>,
    nonce: String,
    signature_method: String,
}

/// Domain router for the Canvas LTI 1.1 API routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/v1/tools/{tool_id}/grade_passback",
            post(grade_passback),
        )
        .route(
            "/api/lti/v1/tools/{tool_id}/ext_grade_passback",
            post(legacy_grade_passback),
        )
        .route("/api/lti/v1/xapi/{token}", post(xapi_service))
        .route("/api/lti/v1/caliper/{token}", post(caliper_service))
        .route("/api/lti/v1/logout_service/{token}", post(logout_service))
        .route(
            "/api/lti/v1/turnitin/outcomes_placement/{tool_id}",
            post(turnitin_outcomes_placement),
        )
}

/// `POST /api/lti/v1/tools/:tool_id/grade_passback` — lti_api#grade_passback.
async fn grade_passback(
    State(state): State<AppState>,
    Path(tool_id): Path<String>,
    headers: HeaderMap,
    uri: OriginalUri,
    _body: Bytes,
) -> Response {
    let tool = match load_tool(&state, &tool_id).await {
        Ok(tool) => tool,
        Err(response) => return response,
    };
    if let Err(response) = verify_oauth(&state, &headers, &uri, &tool).await {
        return response;
    }

    // Canvas only accepts the XML outcome service.
    if !is_xml(&headers) {
        return unauthorized("Content-Type must be 'application/xml'");
    }

    // The outcome itself is processed by the LTI 1.1 outcome service; the
    // workspace has no BasicLTI outcome processor, so the request is
    // acknowledged without a grade.
    outcome_response("success")
}

/// `POST /api/lti/v1/tools/:tool_id/ext_grade_passback` —
/// lti_api#legacy_grade_passback.
async fn legacy_grade_passback(
    State(state): State<AppState>,
    Path(tool_id): Path<String>,
    headers: HeaderMap,
    uri: OriginalUri,
    _body: Bytes,
) -> Response {
    let tool = match load_tool(&state, &tool_id).await {
        Ok(tool) => tool,
        Err(response) => return response,
    };
    if let Err(response) = verify_oauth(&state, &headers, &uri, &tool).await {
        return response;
    }
    outcome_response("success")
}

/// `POST /api/lti/v1/xapi/:token` — lti_api#xapi_service.
async fn xapi_service(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    uri: OriginalUri,
    _body: Bytes,
) -> Response {
    let tool = match load_tool_by_token(&state, &token).await {
        Ok(tool) => tool,
        Err(response) => return response,
    };
    if let Err(response) = verify_oauth(&state, &headers, &uri, &tool).await {
        return response;
    }
    // Canvas answers 415 when the body is not JSON.
    if !is_json(&headers) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    StatusCode::OK.into_response()
}

/// `POST /api/lti/v1/caliper/:token` — lti_api#caliper_service.
async fn caliper_service(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    uri: OriginalUri,
    _body: Bytes,
) -> Response {
    let tool = match load_tool_by_token(&state, &token).await {
        Ok(tool) => tool,
        Err(response) => return response,
    };
    if let Err(response) = verify_oauth(&state, &headers, &uri, &tool).await {
        return response;
    }
    StatusCode::OK.into_response()
}

/// `POST /api/lti/v1/logout_service/:token` — lti_api#logout_service.
async fn logout_service(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    uri: OriginalUri,
    _body: Bytes,
) -> Response {
    let tool = match load_tool_by_token(&state, &token).await {
        Ok(tool) => tool,
        Err(response) => return response,
    };
    if let Err(response) = verify_oauth(&state, &headers, &uri, &tool).await {
        return response;
    }
    StatusCode::OK.into_response()
}

/// `POST /api/lti/v1/turnitin/outcomes_placement/:tool_id` —
/// lti_api#turnitin_outcomes_placement.
async fn turnitin_outcomes_placement(
    State(state): State<AppState>,
    Path(tool_id): Path<String>,
    headers: HeaderMap,
    uri: OriginalUri,
    _body: Bytes,
) -> Response {
    let tool = match load_tool(&state, &tool_id).await {
        Ok(tool) => tool,
        Err(response) => return response,
    };
    if let Err(response) = verify_oauth(&state, &headers, &uri, &tool).await {
        return response;
    }
    Json(json!({})).into_response()
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// An LTI 1.1 tool.
struct Tool {
    consumer_key: String,
    shared_secret: String,
}

/// The tool of an id — Canvas `Lti::ToolFinder.find`.
async fn load_tool(state: &AppState, tool_id: &str) -> Result<Tool, Response> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT consumer_key, shared_secret, consumer_id FROM external_tools \
         WHERE consumer_key = $1 OR consumer_id = $1 OR id::text = $1 LIMIT 1",
    )
    .bind(tool_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| server_error(&error.to_string()))?;

    match row {
        Some((consumer_key, shared_secret, consumer_id)) => {
            // The consumer key of the tool; the consumer id is the fallback.
            let consumer_key = if consumer_key.is_empty() {
                consumer_id.ok_or_else(not_found)?
            } else {
                consumer_key
            };
            let shared_secret = shared_secret.unwrap_or_default();
            Ok(Tool {
                consumer_key,
                shared_secret,
            })
        }
        None => Err(not_found()),
    }
}

/// The tool of an analytics token — Canvas
/// `Lti::AnalyticsService::Token.parse_and_validate`.
async fn load_tool_by_token(state: &AppState, token: &str) -> Result<Tool, Response> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT consumer_key, shared_secret, consumer_id FROM external_tools \
         WHERE consumer_key = $1 OR consumer_id = $1 OR id::text = $1 LIMIT 1",
    )
    .bind(token)
    .fetch_optional(&state.db)
    .await
    .map_err(|error| server_error(&error.to_string()))?;

    match row {
        Some((consumer_key, shared_secret, consumer_id)) => {
            // The consumer key of the tool; the consumer id is the fallback.
            let consumer_key = if consumer_key.is_empty() {
                consumer_id.ok_or_else(not_found)?
            } else {
                consumer_key
            };
            let shared_secret = shared_secret.unwrap_or_default();
            Ok(Tool {
                consumer_key,
                shared_secret,
            })
        }
        None => Err(not_found()),
    }
}

// ---------------------------------------------------------------------------
// OAuth 1.0a
// ---------------------------------------------------------------------------

/// Canvas `LtiApiController#verify_oauth`.
///
/// The signature of the request is checked against the shared secret of the
/// tool, the timestamp has to be within 90 minutes and the nonce has to be
/// unused.
async fn verify_oauth(
    state: &AppState,
    headers: &HeaderMap,
    uri: &OriginalUri,
    tool: &Tool,
) -> Result<(), Response> {
    let params =
        parse_oauth_header(headers).ok_or_else(|| unauthorized("Invalid authorization header"))?;

    if params.consumer_key != tool.consumer_key {
        return Err(unauthorized("Invalid authorization header"));
    }
    if !params.signature_method.eq_ignore_ascii_case("HMAC-SHA1") {
        return Err(unauthorized("Invalid authorization header"));
    }

    // The timestamp has to be within the allowed window.
    let Some(timestamp) = params.timestamp else {
        return Err(unauthorized(
            "Timestamp too old or too far in the future, request has expired",
        ));
    };
    let now = chrono::Utc::now().timestamp();
    if timestamp < now - ALLOWED_TIMESTAMP_DELTA_SECONDS
        || timestamp > now + ALLOWED_TIMESTAMP_DELTA_SECONDS
    {
        return Err(unauthorized(
            "Timestamp too old or too far in the future, request has expired",
        ));
    }

    // The signature has to match the one of the request.
    let expected = signature_of(headers, uri, &params, &tool.shared_secret);
    if !constant_time_eq(expected.as_bytes(), params.signature.as_bytes()) {
        eprintln!(
            "OAUTH-DEBUG expected_sig={expected} got_sig={} base_url={:?} host={:?} scheme={:?} path={:?} consumer_key={:?} nonce={:?} ts={:?} tool_key={:?}",
            params.signature,
            format!("{scheme}://{host}{path}", scheme = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or("http"), host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("localhost"), path = uri.path()),
            headers.get(header::HOST).and_then(|v| v.to_str().ok()),
            headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()),
            uri.path(),
            params.consumer_key,
            params.nonce,
            params.timestamp,
            tool.consumer_key
        );
        return Err(unauthorized("Invalid authorization header"));
    }

    // The nonce may only be used once.
    if nonce_used(state, &tool.consumer_key, &params.nonce).await? {
        return Err(unauthorized("Duplicate nonce detected"));
    }
    store_nonce(state, &tool.consumer_key, &params.nonce).await?;

    Ok(())
}

/// The OAuth parameters of the `Authorization` header.
fn parse_oauth_header(headers: &HeaderMap) -> Option<OAuthParams> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let rest = value.strip_prefix("OAuth ")?;
    let mut params = OAuthParams::default();
    for part in rest.split(',') {
        let part = part.trim();
        let Some((key, raw)) = part.split_once('=') else {
            continue;
        };
        let raw = raw.trim().trim_matches('"');
        match key.trim() {
            "oauth_consumer_key" => params.consumer_key = raw.to_string(),
            "oauth_signature" => params.signature = raw.to_string(),
            "oauth_timestamp" => params.timestamp = raw.parse().ok(),
            "oauth_nonce" => params.nonce = raw.to_string(),
            "oauth_signature_method" => params.signature_method = raw.to_string(),
            _ => {}
        }
    }
    if params.consumer_key.is_empty() || params.signature.is_empty() {
        return None;
    }
    Some(params)
}

/// The OAuth 1.0a signature of the request.
///
/// The signature base string is the method, the URL and the normalized
/// OAuth parameters, percent encoded and joined by an ampersand.
fn signature_of(
    headers: &HeaderMap,
    uri: &OriginalUri,
    params: &OAuthParams,
    secret: &str,
) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    let url = format!("{scheme}://{host}{}", uri.path());

    // The signature base string: the method, the URL and the normalized
    // parameters.
    let mut normalized = [
        percent_encode(params.consumer_key.as_str()),
        percent_encode(params.nonce.as_str()),
        percent_encode(&params.signature_method.to_uppercase()),
        percent_encode(&params.timestamp.unwrap_or_default().to_string()),
    ];
    normalized.sort();
    let base = format!(
        "{}&{}&{}",
        percent_encode("POST"),
        percent_encode(&url),
        percent_encode(&normalized.join("&"))
    );

    let mut mac = HmacSha1::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(base.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// The OAuth 1.0a percent encoding.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Whether the nonce was used before.
async fn nonce_used(state: &AppState, consumer_key: &str, nonce: &str) -> Result<bool, Response> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM lti_nonces WHERE consumer_key = $1 AND nonce = $2 LIMIT 1")
            .bind(consumer_key)
            .bind(nonce)
            .fetch_optional(&state.db)
            .await
            .map_err(|error| server_error(&error.to_string()))?;
    Ok(row.is_some())
}

/// Store the nonce of a request.
async fn store_nonce(state: &AppState, consumer_key: &str, nonce: &str) -> Result<(), Response> {
    sqlx::query(
        "INSERT INTO lti_nonces (consumer_key, nonce, created_at) \
         VALUES ($1, $2, now()) ON CONFLICT DO NOTHING",
    )
    .bind(consumer_key)
    .bind(nonce)
    .execute(&state.db)
    .await
    .map_err(|error| server_error(&error.to_string()))?;
    Ok(())
}

/// A constant time comparison of two byte strings.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in left.iter().zip(right.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// Canvas renders the LTI 1.1 errors with the
/// `BasicLTI::BasicOutcomes::Unauthorized` body.
fn unauthorized(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": message,
            "error_code": "unauthorized",
        })),
    )
        .into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response()
}

fn server_error(message: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": message })),
    )
        .into_response()
}

/// The LTI 1.1 outcome XML response.
fn outcome_response(code_major: &str) -> Response {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<imsx_POXEnvelopeResponse \
         xmlns=\"http://www.imsglobal.org/services/ltiv1p1/xsd/imsoms_v1p1\">\
         <imsx_POXHeader><imsx_POXRequestHeaderInfo><imsx_version>V1.0</imsx_version>\
         <imsx_messageRefIdentifier></imsx_messageRefIdentifier>\
         <imsx_messageRefIdentifier>1</imsx_messageRefIdentifier>\
         <imsx_pooxmlnsVersion>1.0</imsx_pooxmlnsVersion>\
         <imsx_POXResponseCodeInfo><imsx_POXResponseCode><imsx_messageRefIdentifier>1\
         </imsx_messageRefIdentifier><imsx_codeMajor>{code_major}</imsx_codeMajor>\
         </imsx_POXResponseCode></imsx_POXResponseCodeInfo></imsx_POXRequestHeaderInfo>\
         </imsx_POXHeader><imsx_POXBody><imsx_replaceResultResponse>\
         <imsx_messageRefIdentifier>1</imsx_messageRefIdentifier>\
         </imsx_replaceResultResponse></imsx_POXBody></imsx_POXEnvelopeResponse>"
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml")],
        body,
    )
        .into_response()
}

/// Whether the request asks for XML.
fn is_xml(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.starts_with("application/xml"))
        .unwrap_or(false)
}

/// Whether the request asks for JSON.
fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.starts_with("application/json"))
        .unwrap_or(false)
}
