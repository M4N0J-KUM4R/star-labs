//! LTI 1.3 OIDC login initiation and authentication request — the port of
//! Canvas `app/controllers/lti/ims/authentication_controller.rb`.
//!
//! Routes (Canvas `config/routes.rb`, `ApiRouteSet.draw(self, "/api/lti")`):
//! - `GET|POST /api/lti/authorize_redirect` → `authentication#authorize_redirect`
//! - `GET  /api/lti/authorize`              → `authentication#authorize`
//!
//! Canvas renders the authentication response as an auto-submitting HTML form
//! (`lti/ims/authentication/authorize`) whose parameters are either the error
//! object or `{ id_token, state }`; the port returns those exact parameters
//! as JSON. Signed JWTs (`lti_message_hint`, the id token) use the platform
//! HS256 secret — Canvas signs hints with the platform signing key and id
//! tokens with its RSA private JWK; the port uses the shared `jwt_secret`
//! for both, consistent with the rest of the workspace.

use axum::extract::{FromRef, FromRequestParts, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::{ApiError, AppState};
use std::collections::HashMap;

use crate::launch_cache::{self, CachedLaunch};

/// Canvas `Canvas::Security.config["lti_iss"]` (default from
/// `config/security.yml.example`): the platform issuer for LTI 1.3 tokens.
pub(crate) const LTI_ISS: &str = "https://canvas.instructure.com";

/// Canvas `AuthenticationController::REQUIRED_PARAMS`.
const REQUIRED_PARAMS: [&str; 9] = [
    "client_id",
    "login_hint",
    "lti_message_hint",
    "nonce",
    "prompt",
    "redirect_uri",
    "response_mode",
    "response_type",
    "scope",
];

/// Canvas `OPTIONAL_PARAMS`.
const OPTIONAL_PARAMS: [&str; 1] = ["state"];

/// Canvas `SCOPE`.
const OPENID_SCOPE: &str = "openid";

/// Canvas `Lti::PlatformStorage::FORWARDING_TARGET`.
const FORWARDING_TARGET: &str = "post_message_forwarding";

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub(crate) fn advantage_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/authorize_redirect",
            get(authorize_redirect_get).post(authorize_redirect_post),
        )
        .route("/api/lti/authorize", get(authorize))
}

// ---------------------------------------------------------------------------
// Optional auth (Canvas `skip_before_action :require_user`)
// ---------------------------------------------------------------------------

/// The current user when a valid session token is presented, `None`
/// otherwise. Canvas' authentication controller works without a user (the
/// login_required error is part of the contract), so unlike `AuthUser` this
/// extractor never rejects the request.
pub(crate) struct OptionalUser {
    pub id: Option<i64>,
}

impl<S> FromRequestParts<S> for OptionalUser
where
    AppState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);
        let user_id = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .and_then(|token| {
                let mut validation = jsonwebtoken::Validation::default();
                validation.validate_aud = false;
                jsonwebtoken::decode::<star_common::Claims>(
                    token,
                    &jsonwebtoken::DecodingKey::from_secret(app.jwt_secret.as_bytes()),
                    &validation,
                )
                .ok()
                .map(|data| data.claims.sub)
            });
        let id = match user_id {
            Some(id) => {
                // Mirror AuthUser: users deleted since the token was minted
                // do not count as logged in.
                let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&app.db)
                    .await
                    .ok()
                    .flatten();
                exists.map(|(id,)| id)
            }
            None => None,
        };
        Ok(OptionalUser { id })
    }
}

// ---------------------------------------------------------------------------
// authorize_redirect
// ---------------------------------------------------------------------------

/// `GET /api/lti/authorize_redirect` — Canvas `authentication#authorize_redirect`.
async fn authorize_redirect_get(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    authorize_redirect(state, params).await
}

/// `POST /api/lti/authorize_redirect` — same action, form-encoded body.
async fn authorize_redirect_post(
    State(state): State<AppState>,
    axum::Form(params): axum::Form<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    authorize_redirect(state, params).await
}

/// Redirect to the `authorize` action for the domain stored in the
/// `lti_message_hint` JWT (Canvas `authorize_redirect_url`).
async fn authorize_redirect(
    state: AppState,
    params: HashMap<String, String>,
) -> Result<Response, ApiError> {
    let claims = decode_message_hint(&state.jwt_secret, params.get("lti_message_hint"))?;
    let canvas_domain = claims
        .get("canvas_domain")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if canvas_domain.is_empty() {
        // Canvas: `canvas_domain.split(":")` on a hint without the claim
        // raises (500); the port treats it as an invalid hint.
        return Err(ApiError::BadRequest("Invalid lti_message_hint".to_string()));
    }

    // Canvas `lti_1_3_authorization_url(params: oidc_params)` only carries
    // the permitted OIDC parameters.
    let query: Vec<String> = params
        .iter()
        .filter(|(k, _)| {
            REQUIRED_PARAMS.contains(&k.as_str()) || OPTIONAL_PARAMS.contains(&k.as_str())
        })
        .map(|(k, v)| format!("{}={}", k, urlencode(v)))
        .collect();
    let location = format!(
        "http://{canvas_domain}/api/lti/authorize?{}",
        query.join("&")
    );

    Ok((
        StatusCode::FOUND,
        [(axum::http::header::LOCATION, location)],
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// authorize
// ---------------------------------------------------------------------------

/// `GET /api/lti/authorize` — Canvas `authentication#authorize`.
///
/// Canvas validation order (`set_oidc_error!` overwrites, so later errors
/// replace earlier ones):
/// 1. missing OIDC params / wrong scope → error parameters
/// 2. with a session: decode the hint (400 when invalid), load the context
///    (404 when unknown), then the `login_hint` must identify the user →
///    error parameters (overwrites 1)
/// 3. `DeveloperKey.find_cached` → 404 when unknown; inactive key →
///    `unauthorized_client` error parameters (overwrites)
/// 4. without a session: decode the hint (400 when invalid), then the
///    login_required error screen → 401 (the port of the final retry state)
/// 5. the cached launch must exist and not be consumed → error parameters
/// 6. `redirect_uri` must be registered on the developer key → 400
/// 7. success → 200 `{ id_token, state }`
async fn authorize(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    user: OptionalUser,
) -> Result<Response, ApiError> {
    let get = |k: &str| params.get(k).cloned();
    let state_param = get("state");
    let mut oidc_error: Option<(String, String)> = None;
    let mut login_required = false;

    // 1. Canvas `validate_oidc_params!`.
    let mut missing: Vec<&str> = Vec::new();
    for param in REQUIRED_PARAMS {
        if !params.contains_key(param) {
            missing.push(param);
        }
    }
    if !missing.is_empty() {
        oidc_error = Some((
            "invalid_request_object".to_string(),
            format!(
                "The following parameters are missing: {}",
                missing.join(",")
            ),
        ));
    } else if params.get("scope").map(String::as_str) != Some(OPENID_SCOPE) {
        oidc_error = Some((
            "invalid_request_object".to_string(),
            format!("The 'scope' must be '{OPENID_SCOPE}'"),
        ));
    }

    // 2. Canvas `validate_current_user!` — with a session this decodes the
    //    hint (the `context` helper reads `decoded_jwt`) and compares the
    //    opaque login_hint identifier.
    let hint = if let Some(user_id) = user.id {
        let hint = decode_message_hint(&state.jwt_secret, params.get("lti_message_hint"))?;
        let context_type = hint
            .get("context_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Course")
            .to_string();
        let context_id = hint.get("context_id").and_then(|v| v.as_i64()).unwrap_or(0);
        if !context_exists(&state.db, &context_type, context_id).await? {
            return Err(ApiError::NotFound);
        }
        let login_hint = get("login_hint").unwrap_or_default();
        if login_hint != opaque_login_hint(user_id) {
            login_required = true;
            oidc_error = Some((
                "login_required".to_string(),
                "Must have an active user session".to_string(),
            ));
        }
        hint
    } else {
        // The hint is decoded later (Canvas: the cookie screens call
        // `decoded_jwt`), but the branch result is reserved here.
        Value::Null
    };

    // 3. Canvas `validate_client_id!` (`DeveloperKey.find_cached`).
    let client_id = get("client_id").unwrap_or_default();
    let dev_key = find_developer_key(&state.db, &client_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if dev_key.workflow_state != "active" {
        oidc_error = Some((
            "unauthorized_client".to_string(),
            "Client not authorized in requested context".to_string(),
        ));
    }

    // 4. Canvas' missing-cookie screens: without a session the hint is
    //    decoded here and the request ends at the login_required error
    //    screen (401 in the port).
    let hint = if user.id.is_some() {
        hint
    } else {
        let hint = decode_message_hint(&state.jwt_secret, params.get("lti_message_hint"))?;
        let context_type = hint
            .get("context_type")
            .and_then(|v| v.as_str())
            .unwrap_or("Course")
            .to_string();
        let context_id = hint.get("context_id").and_then(|v| v.as_i64()).unwrap_or(0);
        if !context_exists(&state.db, &context_type, context_id).await? {
            return Err(ApiError::NotFound);
        }
        login_required = true;
        oidc_error = Some((
            "login_required".to_string(),
            "Must have an active user session".to_string(),
        ));
        hint
    };
    if login_required && user.id.is_none() {
        return Ok((
            StatusCode::UNAUTHORIZED,
            Json(error_parameters(
                "login_required",
                "Must have an active user session",
                state_param.as_deref(),
            )),
        )
            .into_response());
    }

    let context_type = hint
        .get("context_type")
        .and_then(|v| v.as_str())
        .unwrap_or("Course")
        .to_string();
    let context_id = hint.get("context_id").and_then(|v| v.as_i64()).unwrap_or(0);

    // 5. Canvas `validate_launch_eligibility!` — consume the cached launch.
    let verifier = hint
        .get("verifier")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let mut launch: Option<CachedLaunch> = None;
    if oidc_error.is_none() {
        launch = launch_cache::fetch_and_delete_launch(&context_type, context_id, &verifier);
        if launch.is_none() {
            oidc_error = Some((
                "launch_no_longer_valid".to_string(),
                "The launch has either expired or already been consumed".to_string(),
            ));
        }
    }

    // 6. Canvas validates `redirect_uri` when rendering the form locals —
    //    after eligibility, before the parameters.
    if !redirect_uri_allowed(
        &dev_key.redirect_uris,
        &get("redirect_uri").unwrap_or_default(),
    ) {
        return Err(ApiError::BadRequest("Invalid redirect_uri".to_string()));
    }

    // 7. Render: error parameters or `{ id_token, state }`. (The 401 for a
    // missing session was already returned in step 4.)
    if let Some((error, error_description)) = oidc_error {
        return Ok((
            StatusCode::OK,
            Json(error_parameters(
                &error,
                &error_description,
                state_param.as_deref(),
            )),
        )
            .into_response());
    }

    let launch = launch.expect("launch was checked above");
    let Some(id_token) = generate_id_token(
        &state,
        &launch,
        &params,
        user.id.expect("checked above"),
        &context_type,
        context_id,
    )
    .await?
    else {
        // The launch references a tool or developer key that can no longer
        // be resolved — Canvas caches the claims with the launch, so this is
        // the launch_no_longer_valid path in the port.
        return Ok((
            StatusCode::OK,
            Json(error_parameters(
                "launch_no_longer_valid",
                "The launch has either expired or already been consumed",
                state_param.as_deref(),
            )),
        )
            .into_response());
    };

    let mut parameters = json!({ "id_token": id_token, "state": state_param });
    if hint
        .get("include_storage_target")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        parameters["lti_storage_target"] = json!(FORWARDING_TARGET);
    }
    Ok((StatusCode::OK, Json(parameters)).into_response())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The parameters Canvas embeds in the authorize auto-submit form for a
/// failed validation (`set_oidc_error!` — `state` is always present, null
/// when the request carried none).
fn error_parameters(error: &str, error_description: &str, state: Option<&str>) -> Value {
    json!({
        "error": error,
        "error_description": error_description,
        "state": state,
    })
}

/// Percent-encode a query component (form style: space as `+`).
fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Decode the `lti_message_hint` JWT (platform-signed, HS256). Canvas
/// `decoded_jwt` → `reject! "Invalid lti_message_hint"` (400 plain text).
pub(crate) fn decode_message_hint(secret: &str, hint: Option<&String>) -> Result<Value, ApiError> {
    let Some(hint) = hint else {
        return Err(ApiError::BadRequest("Invalid lti_message_hint".to_string()));
    };
    let mut validation = jsonwebtoken::Validation::default();
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    jsonwebtoken::decode::<Value>(
        hint,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|_| ApiError::BadRequest("Invalid lti_message_hint".to_string()))
}

/// Canvas looks the developer key up by `client_id.to_i` (the numeric
/// developer key id); LTI client ids are also accepted as the `api_key`
/// string. Returns `None` when neither matches (`DeveloperKey.find_cached`
/// raises RecordNotFound → 404).
pub(crate) struct DeveloperKeyRow {
    pub id: i64,
    pub workflow_state: String,
    pub redirect_uris: Vec<String>,
}

pub(crate) async fn find_developer_key(
    pool: &PgPool,
    client_id: &str,
) -> Result<Option<DeveloperKeyRow>, ApiError> {
    let numeric = client_id.parse::<i64>().ok();
    let row: Option<(i64, String, Vec<String>)> = sqlx::query_as(
        "SELECT id, workflow_state, redirect_uris FROM developer_keys \
         WHERE api_key = $1 OR ($2::bigint IS NOT NULL AND id = $2) LIMIT 1",
    )
    .bind(client_id)
    .bind(numeric)
    .fetch_optional(pool)
    .await?;
    Ok(
        row.map(|(id, workflow_state, redirect_uris)| DeveloperKeyRow {
            id,
            workflow_state,
            redirect_uris,
        }),
    )
}

/// Canvas `redirect_uri` validation: the requested URI matches a registered
/// URI exactly (ignoring query strings), or carries every query parameter a
/// registered URI requires.
pub(crate) fn redirect_uri_allowed(registered: &[String], requested: &str) -> bool {
    let (requested_base, requested_query) = match requested.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (requested, None),
    };
    let requested_params = requested_query.map(parse_query_pairs);

    registered.iter().any(|uri| match uri.split_once('?') {
        Some((_, required_query)) => {
            let required = parse_query_pairs(required_query);
            match &requested_params {
                None => required.is_empty(),
                Some(requested) => required
                    .iter()
                    .all(|(k, v)| requested.iter().any(|(rk, rv)| rk == k && rv == v)),
            }
        }
        None => uri == requested_base,
    })
}

/// Minimal `key=value&key=value` parser for the redirect_uri check.
fn parse_query_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

/// The context the hint points at must exist (Canvas `context` →
/// `model.find` → 404).
async fn context_exists(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
) -> Result<bool, ApiError> {
    let sql = match context_type {
        "Account" => "SELECT 1 FROM accounts WHERE id = $1::bigint",
        _ => "SELECT 1 FROM courses WHERE id = $1::bigint",
    };
    let row: Option<(i32,)> = sqlx::query_as(sql)
        .bind(context_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Canvas `Lti::V1p1::Asset.opaque_identifier_for(user, context:)` returns
/// the user's opaque `lti_context_id` UUID. The port derives one
/// deterministically from the user id (no schema change): two FNV-1a hashes
/// of the user id folded into a UUID, stable across requests.
pub fn opaque_login_hint(user_id: i64) -> String {
    fn fnv(input: &str) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in input.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
    let hi = fnv(&format!("lti-user:{user_id}"));
    let lo = fnv(&format!("lti-asset:{user_id}"));
    let combined = ((hi as u128) << 64) | (lo as u128);
    uuid::Uuid::from_u128(combined).to_string()
}

/// Canvas `Lti::Messages::JwtMessage.generate_id_token(cached_launch_with_nonce)`
/// — resolves the client id and deployment id from the tool
/// (`add_security_claims!`: aud/azp = developer key id, deployment_id =
/// tool deployment), applies the OIDC `nonce`, and signs with the platform
/// key (port: the shared HS256 secret). Returns `None` when the tool or its
/// developer key can no longer be resolved.
async fn generate_id_token(
    state: &AppState,
    launch: &CachedLaunch,
    params: &HashMap<String, String>,
    user_id: i64,
    context_type: &str,
    context_id: i64,
) -> Result<Option<String>, ApiError> {
    // Tool → developer key (client id) and LTI deployment.
    let tool: Option<(Option<i64>, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT developer_key_id, deployment_id, lti_registration_id FROM external_tools \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(launch.tool_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((tool_developer_key_id, tool_deployment_id, tool_registration_id)) = tool else {
        return Ok(None);
    };

    let client_id: Option<String> = match tool_developer_key_id {
        Some(key_id) => {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT api_key FROM developer_keys WHERE id = $1")
                    .bind(key_id)
                    .fetch_optional(&state.db)
                    .await?;
            row.map(|(api_key,)| api_key)
        }
        None => None,
    };
    let Some(client_id) = client_id else {
        return Ok(None);
    };

    // Deployment-id resolution: the tool's stored deployment id, else the
    // active deployment of the tool's registration for this context
    // (Canvas `ContextExternalTool#deployment_id`).
    let deployment_id: Option<String> = match tool_deployment_id {
        Some(id) => Some(id),
        None => {
            let row: Option<(String,)> = sqlx::query_as(
                "SELECT deployment_id FROM lti_deployments \
                 WHERE registration_id = $1 AND context_type = $2 AND context_id = $3 \
                 AND workflow_state = 'active' LIMIT 1",
            )
            .bind(tool_registration_id)
            .bind(context_type)
            .bind(context_id)
            .fetch_optional(&state.db)
            .await?;
            row.map(|(id,)| id)
        }
    };

    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({
        "iss": LTI_ISS,
        "aud": client_id,
        "azp": client_id,
        "sub": opaque_login_hint(user_id),
        "nonce": params.get("nonce").cloned().unwrap_or_default(),
        "iat": now,
        "exp": now + 3600,
        "https://purl.imsglobal.org/spec/lti/claim/message_type": launch.message_type,
        "https://purl.imsglobal.org/spec/lti/claim/version": "1.3.0",
        "https://purl.imsglobal.org/spec/lti/claim/target_link_uri": launch.target_link_uri,
    });
    if let Some(deployment_id) = deployment_id {
        claims["https://purl.imsglobal.org/spec/lti/claim/deployment_id"] = json!(deployment_id);
    }
    // Extra message claims (custom, resource_link, deep_linking, context...).
    if let Some(extra) = launch.extra_claims.as_object() {
        for (key, value) in extra {
            claims[key.as_str()] = value.clone();
        }
    }

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .map_err(|e| ApiError::Db(e.to_string()))?;
    Ok(Some(token))
}
