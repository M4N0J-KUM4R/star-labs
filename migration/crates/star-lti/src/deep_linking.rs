//! LTI 1.3 deep linking content selection — the port of Canvas
//! `app/controllers/lti/ims/deep_linking_controller.rb` and its concerns
//! (`Lti::IMS::Concerns::DeepLinkingServices` / `DeepLinkingModules`).
//!
//! Routes (Canvas `config/routes.rb`, default route set):
//! - `POST /courses/:course_id/deep_linking_response`
//! - `POST /accounts/:account_id/deep_linking_response`
//! - `POST /groups/:group_id/deep_linking_response`
//! - `GET  /deep_linking_cancel`
//!
//! Canvas renders a `js_env` page that postMessages a `deep_link_response`
//! object to the parent frame; the port returns that exact object as JSON
//! (`{ deep_link_response: { ... } }`). The tool's response JWT is verified
//! against the platform secret (Canvas verifies against the tool's public
//! JWK — the port signs everything with the shared HS256 secret, like the
//! rest of the workspace). The `data` JWT is single-use (nonce), exactly
//! like Canvas `Lti::DeepLinkingData`.
//!
//! Out of scope (documented): assignment/line-item creation from content
//! items (`create_update_assignment!`) — items carrying `lineItem` are
//! returned without an assignment and are not added to modules; and the
//! `ActivityAssetProcessor` placements (fall through to the default
//! content-selection branch).

use axum::extract::{Form, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_common::{ApiError, AppState};
use std::collections::HashMap;

use crate::authentication::{find_developer_key, LTI_ISS};
use crate::launch_cache;
use crate::lti_authorization::request_host;
use crate::resource_links::can_manage_course;

/// Canvas `DeepLinkingServices::CLAIM_PREFIX`.
const DL_CLAIM_PREFIX: &str = "https://purl.imsglobal.org/spec/lti-dl/claim/";

/// Canvas `DeepLinkingModules::CREATE_NEW_MODULE_PLACEMENTS`.
const CREATE_NEW_MODULE_PLACEMENTS: [&str; 2] =
    ["course_assignments_menu", "module_index_menu_modal"];

/// Canvas `DeepLinkingModules::ALLOW_LINE_ITEM_PLACEMENTS`.
const ALLOW_LINE_ITEM_PLACEMENTS: [&str; 5] = [
    "course_assignments_menu",
    "module_index_menu_modal",
    "assignment_selection",
    "link_selection",
    "module_menu_modal",
];

/// Canvas `Lti::DeepLinkingData::ERROR_TYPES`.
const DATA_ERRORS: [(&str, &str); 3] = [
    (
        "presence_required",
        "Do not modify the deep_link_return_url in any way.",
    ),
    (
        "invalid_or_malformed",
        "Do not modify the deep_link_return_url in any way.",
    ),
    (
        "already_used",
        "Do not attempt to submit content items multiple times.",
    ),
];

pub(crate) fn advantage_router() -> Router<AppState> {
    Router::new()
        .route("/courses/{id}/deep_linking_response", post(course_response))
        .route(
            "/accounts/{account_id}/deep_linking_response",
            post(account_response),
        )
        .route(
            "/groups/{group_id}/deep_linking_response",
            post(group_response),
        )
        .route("/deep_linking_cancel", get(deep_linking_cancel))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn course_response(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    headers: axum::http::HeaderMap,
    Form(params): Form<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    require_context(&state.db, "courses", course_id).await?;
    deep_linking_response(state, headers, "Course", course_id, params).await
}

async fn account_response(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    headers: axum::http::HeaderMap,
    Form(params): Form<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    require_context(&state.db, "accounts", account_id).await?;
    deep_linking_response(state, headers, "Account", account_id, params).await
}

async fn group_response(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    headers: axum::http::HeaderMap,
    Form(params): Form<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    require_context(&state.db, "groups", group_id).await?;
    deep_linking_response(state, headers, "Group", group_id, params).await
}

// ---------------------------------------------------------------------------
// Context resolution
// ---------------------------------------------------------------------------

/// Canvas `before_action :require_context` — 404 for unknown contexts.
async fn require_context(pool: &PgPool, table: &str, id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(&format!("SELECT id FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    if row.is_none() {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// Which user may submit deep-linking content for this context (Canvas
/// `require_context_update_rights` → `manage_course_content_add` +
/// `manage_course_content_update`): the course teacher / account admin.
async fn can_update_context(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    auth: &star_auth::AuthUser,
) -> Result<bool, ApiError> {
    match context_type {
        "Course" => can_manage_course(pool, auth, context_id).await,
        "Account" => crate::can_manage_account(pool, auth, context_id).await,
        // Group contexts: the parent course's teachers/admins.
        "Group" => {
            let course: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT course_id FROM groups WHERE id = $1")
                    .bind(context_id)
                    .fetch_optional(pool)
                    .await?;
            match course {
                Some((Some(course_id),)) => can_manage_course(pool, auth, course_id).await,
                _ => Ok(auth.role == "admin"),
            }
        }
        _ => Ok(false),
    }
}

/// The account chain searched for tools: a course's accounts, an account's
/// own chain, or a group's parent course's accounts
/// (Canvas `Lti::ToolFinderUtils.contexts_to_search`).
async fn context_account_chain(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
) -> Result<Vec<i64>, ApiError> {
    let mut root: Option<i64> = None;
    match context_type {
        "Account" => root = Some(context_id),
        "Course" => {
            root = sqlx::query_scalar("SELECT account_id FROM courses WHERE id = $1")
                .bind(context_id)
                .fetch_optional(pool)
                .await?
                .flatten();
        }
        "Group" => {
            let course_account: Option<(Option<i64>,)> = sqlx::query_as(
                "SELECT c.account_id FROM groups g JOIN courses c ON c.id = g.course_id \
                 WHERE g.id = $1",
            )
            .bind(context_id)
            .fetch_optional(pool)
            .await?;
            root = course_account.and_then(|(a,)| a);
        }
        _ => {}
    }
    let mut chain = Vec::new();
    let mut current = root;
    let mut depth = 0;
    while let Some(id) = current {
        if depth >= 20 {
            break;
        }
        depth += 1;
        chain.push(id);
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        current = parent.and_then(|(p,)| p);
    }
    Ok(chain)
}

// ---------------------------------------------------------------------------
// JWT validation (Canvas DeepLinkingJwt)
// ---------------------------------------------------------------------------

struct ValidatedJwt {
    /// The verified claims.
    claims: Value,
    /// The developer key id (numeric, for tool lookup).
    developer_key_id: i64,
    /// `developer_key.scopes` for the replace-editor-contents check.
    scopes: Vec<String>,
}

fn jwt_error(errors: Map<String, Value>) -> Response {
    (StatusCode::BAD_REQUEST, Json(Value::Object(errors))).into_response()
}

/// The port of Canvas `DeepLinkingJwt#verified_jwt` — signature, then the
/// standard claims (`Canvas::Security::JwtValidator` with `jti = nonce`,
/// `sub = iss`, `expected_aud = lti_iss`, `require_iss`), then the
/// developer key context check. Only the first failing standard check is
/// reported (Canvas `errors?` short-circuit), while decode and
/// developer-key errors accumulate.
async fn validate_deep_linking_jwt(
    state: &AppState,
    raw: Option<&String>,
) -> Result<Result<ValidatedJwt, Map<String, Value>>, ApiError> {
    let mut errors = Map::new();

    let Some(raw) = raw else {
        errors.insert("jwt".to_string(), json!(["JWT format is invalid"]));
        return Ok(Err(errors));
    };

    // client_id from the unverified iss claim.
    let unverified = decode_unverified(raw);
    let client_id = unverified
        .as_ref()
        .and_then(|c| c.get("iss"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let dev_key = if client_id.is_empty() {
        None
    } else {
        find_developer_key(&state.db, &client_id).await?
    };
    let Some(dev_key) = dev_key else {
        errors.insert("jwt".to_string(), json!(["Client not found"]));
        return Ok(Err(errors));
    };

    // Signature (Canvas: the tool's public JWK; port: platform secret).
    let mut validation = jsonwebtoken::Validation::default();
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    let claims = match jsonwebtoken::decode::<Value>(
        raw,
        &jsonwebtoken::DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &validation,
    ) {
        Ok(data) => data.claims,
        Err(err) => {
            let message = match err.kind() {
                jsonwebtoken::errors::ErrorKind::InvalidAlgorithm => "JWT has unexpected alg",
                _ => "JWT verification failure",
            };
            errors.insert("jwt".to_string(), json!([message]));
            return Ok(Err(errors));
        }
    };

    // Standard claims: jti ← nonce, sub ← iss, iss required, aud = lti_iss.
    let has_nonce = claims.get("nonce").is_some();
    let jti_ok = claims.get("jti").is_some() || has_nonce;
    let mut missing: Vec<&str> = Vec::new();
    for claim in ["sub", "aud", "exp", "iat"] {
        if claims.get(claim).is_none() {
            missing.push(claim);
        }
    }
    if !jti_ok {
        missing.push("jti");
    }
    if claims.get("iss").is_none() {
        missing.push("iss");
    }
    if !missing.is_empty() {
        errors.insert(
            "base".to_string(),
            json!([format!(
                "the following assertions are missing: {}",
                missing.join(",")
            )]),
        );
    } else {
        // Canvas stops at the first failing check (`return if errors?`).
        let aud_ok = match claims.get("aud") {
            Some(Value::String(s)) => s == LTI_ISS,
            Some(Value::Array(items)) => items.iter().any(|i| i.as_str() == Some(LTI_ISS)),
            _ => false,
        };
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
        if !aud_ok {
            errors.insert("base".to_string(), json!(["the 'aud' is invalid"]));
        } else if !exp_ok {
            errors.insert("base".to_string(), json!(["the JWT has expired"]));
        } else if !iat_ok {
            errors.insert("base".to_string(), json!(["the 'iat' must be a number"]));
        }
    }

    // developer_key.usable_in_context? — the port checks the key's state.
    if dev_key.workflow_state != "active" {
        errors.insert(
            "developer_key".to_string(),
            json!(["Developer key inactive in context"]),
        );
    }

    if errors.is_empty() {
        let scopes: Option<(Vec<String>,)> =
            sqlx::query_as("SELECT scopes FROM developer_keys WHERE id = $1")
                .bind(dev_key.id)
                .fetch_optional(&state.db)
                .await?;
        Ok(Ok(ValidatedJwt {
            claims,
            developer_key_id: dev_key.id,
            scopes: scopes.map(|(s,)| s).unwrap_or_default(),
        }))
    } else {
        Ok(Err(errors))
    }
}

fn decode_unverified(token: &str) -> Option<Value> {
    let mut validation = jsonwebtoken::Validation::default();
    validation.validate_aud = false;
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.required_spec_claims.clear();
    validation.insecure_disable_signature_validation();
    jsonwebtoken::decode::<Value>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(b""),
        &validation,
    )
    .ok()
    .map(|data| data.claims)
}

// ---------------------------------------------------------------------------
// data JWT (Canvas Lti::DeepLinkingData)
// ---------------------------------------------------------------------------

fn data_error(kind: usize) -> Value {
    let (kind_name, message) = DATA_ERRORS[kind];
    json!({
        "data_jwt": [{
            "attribute": "?data=JWT",
            "type": kind_name,
            "message": message,
        }]
    })
}

/// Canvas `Lti::DeepLinkingData.from_jwt`: decode (ignoring expiry) and
/// consume the nonce once.
fn validate_data_jwt(state: &AppState, raw: Option<&String>) -> Result<Value, Response> {
    let Some(raw) = raw else {
        return Err((StatusCode::BAD_REQUEST, Json(data_error(0))).into_response());
    };
    let mut validation = jsonwebtoken::Validation::default();
    validation.validate_aud = false;
    validation.validate_exp = false;
    validation.validate_nbf = false;
    validation.required_spec_claims.clear();
    let claims = match jsonwebtoken::decode::<Value>(
        raw,
        &jsonwebtoken::DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &validation,
    ) {
        Ok(data) => data.claims,
        Err(_) => return Err((StatusCode::BAD_REQUEST, Json(data_error(1))).into_response()),
    };
    let nonce = claims.get("nonce").and_then(|v| v.as_str()).unwrap_or("");
    if nonce.is_empty()
        || !launch_cache::check_and_store_nonce(&format!("nonce:deep_linking_response:{nonce}"))
    {
        return Err((StatusCode::BAD_REQUEST, Json(data_error(2))).into_response());
    }
    Ok(claims)
}

// ---------------------------------------------------------------------------
// deep_linking_response
// ---------------------------------------------------------------------------

async fn deep_linking_response(
    state: AppState,
    headers: axum::http::HeaderMap,
    context_type: &str,
    context_id: i64,
    params: HashMap<String, String>,
) -> Result<Response, ApiError> {
    // Canvas before_action order: validate_jwt → validate_return_url_data →
    // require_context_update_rights → require_tool.
    let jwt = match validate_deep_linking_jwt(&state, params.get("JWT")).await? {
        Ok(jwt) => jwt,
        Err(errors) => return Ok(jwt_error(errors)),
    };
    let data = match validate_data_jwt(&state, params.get("data")) {
        Ok(data) => data,
        Err(response) => return Ok(response),
    };

    let placement = data
        .get("placement")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let context_module_id = data.get("context_module_id").and_then(|v| v.as_i64());
    let content_item_id = data.get("content_item_id").and_then(|v| v.as_i64());
    let module_name = jwt
        .claims
        .get("https://canvas.instructure.com/lti/module_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    // Canvas `tool` — the LTI 1.3 tool from the JWT's developer key,
    // available in the context chain.
    let tool =
        find_tool_for_developer_key(&state.db, context_type, context_id, jwt.developer_key_id)
            .await?;

    // Content items (Canvas `content_items` / `lti_resource_links`).
    let mut items: Vec<Value> = jwt
        .claims
        .get(format!("{DL_CLAIM_PREFIX}content_items"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let resource_link_items: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.get("type").and_then(|v| v.as_str()) == Some("ltiResourceLink"))
        .map(|(i, _)| i)
        .collect();

    let for_placement = |name: &str| placement == name;
    let allow_line_items = ALLOW_LINE_ITEM_PLACEMENTS.contains(&placement.as_str());
    let add_assignment = context_type == "Course"
        && allow_line_items
        && items.iter().any(|item| item.get("lineItem").is_some());
    let create_resources = for_placement("assignment_selection")
        || for_placement("submission_type_selection")
        || CREATE_NEW_MODULE_PLACEMENTS.contains(&placement.as_str())
        || context_module_id.is_some()
        || add_assignment;

    // require_context_update_rights / require_tool — only when the placement
    // creates resources.
    if create_resources {
        // The port needs an authenticated user for the permission check.
        let auth = extract_auth_user(&state, &headers).await;
        match auth {
            Some(auth) => {
                if !can_update_context(&state.db, context_type, context_id, &auth).await? {
                    return Err(ApiError::Forbidden);
                }
            }
            None => return Err(ApiError::Unauthorized),
        }
        if tool.is_none() {
            // Canvas render_unauthorized_action → 401.
            return Err(ApiError::Unauthorized);
        }
    }

    // Branch 1 (Canvas): link_selection with a single non-assignment item →
    // create the resource link, do not reload.
    // Branch 2: collaboration — no resource links, tool id passed along.
    // Branch 3: !create_resources? → create resource links, no reload.
    // Branch 4: assignment/submission_type selection → nothing created.
    // Branch 5: module flow → create/find the module, add items, reload.
    let tool_row = tool;
    let host = request_host(&headers);
    let lti_endpoint = match context_type {
        "Account" => format!("http://{host}/accounts/{context_id}/external_tools/retrieve"),
        "Group" => format!("http://{host}/groups/{context_id}/external_tools/retrieve"),
        _ => format!("http://{host}/courses/{context_id}/external_tools/retrieve"),
    };

    let mut reloadpage = false;
    let mut module_created = false;
    let mut extra = Map::new();

    // Canvas branch order (deep_linking_response):
    // 1. link_selection with a single non-assignment item → create the
    //    resource link, do not reload.
    // 2. collaboration — CollaborationController owns link creation.
    // 3. ActivityAssetProcessor placements — items are filtered to the
    //    processor types, nothing is created.
    // 4. !create_resources? (default: editor_button and friends) → create
    //    resource links, no reload.
    // 5. assignment/submission_type selection → nothing created.
    // 6. module flow → create/find the module, add items, reload.
    if for_placement("link_selection") && resource_link_items.len() == 1 && !add_assignment {
        create_resource_links_for_items(
            &state.db,
            context_type,
            context_id,
            tool_row,
            &mut items,
            &resource_link_items,
        )
        .await?;
    } else if for_placement("collaboration") {
        if let Some(tool) = tool_row {
            extra.insert("tool_id".to_string(), json!(tool));
        }
    } else if for_placement("ActivityAssetProcessor")
        || for_placement("ActivityAssetProcessorContribution")
    {
        let wanted = if for_placement("ActivityAssetProcessor") {
            "ltiAssetProcessor"
        } else {
            "ltiAssetProcessorContribution"
        };
        items.retain(|item| item.get("type").and_then(|v| v.as_str()) == Some(wanted));
        if let Some(tool) = tool_row {
            extra.insert("tool_id".to_string(), json!(tool));
        }
    } else if for_placement("assignment_selection") || for_placement("submission_type_selection") {
        // Nothing is created on the new/edit assignment page.
    } else if !create_resources {
        // Default branch: create a resource link per ltiResourceLink item.
        create_resource_links_for_items(
            &state.db,
            context_type,
            context_id,
            tool_row,
            &mut items,
            &resource_link_items,
        )
        .await?;
    } else {
        // Module flow: line-item items would create assignments in Canvas
        // (`create_update_assignment!`, not ported) — they are returned
        // without one and are not added to the module.
        let line_item_items: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.get("lineItem").is_some())
            .map(|(i, _)| i)
            .collect();
        if context_type == "Course"
            && !line_item_items.is_empty()
            && line_item_items.len() == items.len()
        {
            // Canvas: only assignments from the assignments page → no new
            // module, reload the page.
            reloadpage = true;
        } else {
            let module_id = ensure_context_module(
                &state.db,
                context_id,
                module_name.as_deref(),
                CREATE_NEW_MODULE_PLACEMENTS.contains(&placement.as_str()),
                context_module_id,
            )
            .await?;
            module_created = CREATE_NEW_MODULE_PLACEMENTS.contains(&placement.as_str());
            create_resource_links_for_items(
                &state.db,
                context_type,
                context_id,
                tool_row,
                &mut items,
                &resource_link_items,
            )
            .await?;
            add_items_to_module(
                &state.db,
                context_id,
                module_id,
                tool_row,
                &items,
                &resource_link_items,
                &line_item_items,
            )
            .await?;
            reloadpage = true;
        }
    }

    // Canvas `render_content_items` — the js_env `deep_link_response`
    // payload (compacted).
    let mut response = Map::new();
    if !placement.is_empty() {
        response.insert("placement".to_string(), json!(placement));
    }
    response.insert("content_items".to_string(), json!(items));
    if let Some(service_id) = content_item_id {
        response.insert("service_id".to_string(), json!(service_id));
    }
    for (key, claim, prefix) in [
        ("msg", "msg", Some("Message from external tool: ")),
        ("log", "log", None),
        (
            "errormsg",
            "errormsg",
            Some("Error message from external tool: "),
        ),
        ("errorlog", "errorlog", None),
    ] {
        if let Some(value) = jwt
            .claims
            .get(format!("{DL_CLAIM_PREFIX}{claim}"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            let text = match prefix {
                Some(prefix) => format!("{prefix}{value}"),
                None => value.to_string(),
            };
            response.insert(key.to_string(), json!(text));
        }
    }
    response.insert("ltiEndpoint".to_string(), json!(lti_endpoint));
    response.insert("reloadpage".to_string(), json!(reloadpage));
    response.insert("moduleCreated".to_string(), json!(module_created));
    let replace_editor_contents =
        jwt.claims
            .get("https://canvas.instructure.com/lti/replace_editor_contents")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && jwt.scopes.iter().any(|s| {
                s.as_str() == "https://canvas.instructure.com/lti/replace_editor_contents"
            });
    response.insert(
        "replaceEditorContents".to_string(),
        json!(replace_editor_contents),
    );
    for (key, value) in extra {
        response.insert(key, value);
    }

    Ok((
        StatusCode::OK,
        Json(json!({ "deep_link_response": Value::Object(response) })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// deep_linking_cancel
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct CancelQuery {
    placement: Option<String>,
    lti_msg: Option<String>,
    lti_log: Option<String>,
    lti_errormsg: Option<String>,
    lti_errorlog: Option<String>,
}

/// `GET /deep_linking_cancel` — Canvas `deep_linking_cancel`: an empty
/// content selection echo page (js_env payload as JSON in the port).
async fn deep_linking_cancel(Query(query): Query<CancelQuery>) -> Response {
    let mut response = Map::new();
    if let Some(placement) = &query.placement {
        response.insert("placement".to_string(), json!(placement));
    }
    response.insert("content_items".to_string(), json!([]));
    if let Some(msg) = query.lti_msg.as_deref().filter(|s| !s.is_empty()) {
        response.insert(
            "msg".to_string(),
            json!(format!("Message from external tool: {msg}")),
        );
    }
    if let Some(log) = query.lti_log.as_deref().filter(|s| !s.is_empty()) {
        response.insert("log".to_string(), json!(log));
    }
    if let Some(errormsg) = query.lti_errormsg.as_deref().filter(|s| !s.is_empty()) {
        response.insert(
            "errormsg".to_string(),
            json!(format!("Error message from external tool: {errormsg}")),
        );
    }
    if let Some(errorlog) = query.lti_errorlog.as_deref().filter(|s| !s.is_empty()) {
        response.insert("errorlog".to_string(), json!(errorlog));
    }
    response.insert("reloadpage".to_string(), json!(false));
    response.insert("moduleCreated".to_string(), json!(false));
    response.insert("replaceEditorContents".to_string(), json!(false));

    (
        StatusCode::OK,
        Json(json!({ "deep_link_response": Value::Object(response) })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Resource link / module helpers
// ---------------------------------------------------------------------------

/// Canvas `tool` — `ToolFinder.from_context(@context, scope: active tools
/// with developer_key_id == client_id)`. Returns the tool id.
async fn find_tool_for_developer_key(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    developer_key_id: i64,
) -> Result<Option<i64>, ApiError> {
    let account_chain = context_account_chain(pool, context_type, context_id).await?;
    let context_clause = match context_type {
        "Account" => "(context_type = 'Account' AND context_id = ANY($2))".to_string(),
        "Group" => "(context_type = 'Group' AND context_id = $3) \
                    OR (context_type = 'Account' AND context_id = ANY($2))"
            .to_string(),
        _ => "(context_type = 'Course' AND context_id = $3) \
              OR (context_type = 'Account' AND context_id = ANY($2))"
            .to_string(),
    };
    let sql = format!(
        "SELECT id FROM external_tools \
         WHERE workflow_state <> 'deleted' AND developer_key_id = $1 AND ({context_clause}) \
         ORDER BY (context_type = 'Course') DESC, id LIMIT 1"
    );
    let row: Option<(i64,)> = sqlx::query_as(&sql)
        .bind(developer_key_id)
        .bind(&account_chain)
        .bind(context_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(id,)| id))
}

/// Canvas `Lti::ResourceLink.create_with(context, tool, custom, url, title)`
/// per ltiResourceLink content item — nil (no link) when the tool is
/// missing, and the item's `lookup_uuid` is set from the created link.
async fn create_resource_links_for_items(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    tool: Option<i64>,
    items: &mut [Value],
    resource_link_items: &[usize],
) -> Result<(), ApiError> {
    let Some(tool_id) = tool else {
        // Canvas: create_with returns nil → lookup_uuid stays unset (null).
        return Ok(());
    };
    let root_account_id = match context_type {
        "Account" => context_id,
        "Course" => account_chain_root(pool, context_id).await?,
        "Group" => group_root_account(pool, context_id).await?,
        _ => 1,
    };
    for &index in resource_link_items {
        let item = &items[index];
        let custom = filter_custom_value(item.get("custom").cloned());
        let lookup_uuid = uuid::Uuid::new_v4().to_string();
        let sql = "INSERT INTO lti_resource_links \
             (context_external_tool_id, workflow_state, root_account_id, context_id, context_type, \
              custom, lookup_uuid, resource_link_uuid, url, lti_1_1_id, title, created_at, updated_at) \
             VALUES ($1, 'active', $2, $3, $4, $5, $6, $7, $8, NULL, $9, now(), now()) RETURNING id";
        let _: (i64,) = sqlx::query_as(sql)
            .bind(tool_id)
            .bind(root_account_id)
            .bind(context_id)
            .bind(context_type)
            .bind(custom)
            .bind(&lookup_uuid)
            .bind(uuid::Uuid::new_v4().to_string())
            .bind(item.get("url").and_then(|v| v.as_str()))
            .bind(item.get("title").and_then(|v| v.as_str()))
            .fetch_one(pool)
            .await?;
        items[index]["lookup_uuid"] = json!(lookup_uuid);
    }
    Ok(())
}

/// Canvas `Lti::DeepLinkingUtil.validate_custom_params`.
fn filter_custom_value(custom: Option<Value>) -> Value {
    match custom {
        Some(Value::Object(map)) => Value::Object(
            map.into_iter()
                .filter(|(k, v)| {
                    !k.is_empty()
                        && matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_))
                })
                .collect::<Map<String, Value>>(),
        ),
        _ => Value::Null,
    }
}

async fn account_chain_root(pool: &PgPool, course_id: i64) -> Result<i64, ApiError> {
    let account: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    let mut current = account.and_then(|(a,)| a);
    let mut last = current.unwrap_or(1);
    let mut depth = 0;
    while let Some(id) = current {
        if depth >= 20 {
            break;
        }
        depth += 1;
        last = id;
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        current = parent.and_then(|(p,)| p);
    }
    Ok(last)
}

async fn group_root_account(pool: &PgPool, group_id: i64) -> Result<i64, ApiError> {
    let account: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT c.account_id FROM groups g JOIN courses c ON c.id = g.course_id WHERE g.id = $1",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?;
    Ok(account.and_then(|(a,)| a).unwrap_or(1))
}

/// Canvas: create a new module ("New Content From App" or the
/// `module_name` claim, unpublished) or use the one from the return URL.
async fn ensure_context_module(
    pool: &PgPool,
    course_id: i64,
    module_name: Option<&str>,
    create_new: bool,
    context_module_id: Option<i64>,
) -> Result<i64, ApiError> {
    if !create_new {
        let Some(module_id) = context_module_id else {
            // Canvas: find(nil) → RecordNotFound. The placement determined
            // create_resources, so a module id must be present.
            return Err(ApiError::NotFound);
        };
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM context_modules WHERE id = $1 AND context_id = $2 \
                            AND context_type = 'Course' AND workflow_state <> 'deleted'",
        )
        .bind(module_id)
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
        return row.map(|(id,)| id).ok_or(ApiError::NotFound);
    }
    let name = module_name.unwrap_or("New Content From App");
    let position: (Option<i32>,) = sqlx::query_as(
        "SELECT MAX(position) FROM context_modules WHERE context_id = $1 AND context_type = 'Course'",
    )
    .bind(course_id)
    .fetch_one(pool)
    .await?;
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO context_modules (context_id, context_type, name, position, workflow_state, \
         created_at, updated_at, root_account_id) \
         VALUES ($1, 'Course', $2, $3, 'unpublished', now(), now(), \
         (SELECT COALESCE(account_id, 1) FROM courses WHERE id = $1)) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(position.0.map(|p| p + 1).unwrap_or(1))
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Canvas `context_module.add_item(build_module_item(content_item))` for
/// resource-link items (line-item items become assignment items in Canvas;
/// the port skips them — documented).
async fn add_items_to_module(
    pool: &PgPool,
    course_id: i64,
    module_id: i64,
    tool: Option<i64>,
    items: &[Value],
    resource_link_items: &[usize],
    line_item_items: &[usize],
) -> Result<(), ApiError> {
    let Some(tool_id) = tool else {
        return Ok(());
    };
    let position: (Option<i32>,) =
        sqlx::query_as("SELECT MAX(position) FROM content_tags WHERE context_module_id = $1")
            .bind(module_id)
            .fetch_one(pool)
            .await?;
    let mut next_position = position.0.map(|p| p + 1).unwrap_or(1);

    for &index in resource_link_items {
        if line_item_items.contains(&index) {
            continue;
        }
        let item = &items[index];
        // The resource link created for this item.
        let lookup_uuid = item
            .get("lookup_uuid")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let link_id: Option<(i64,)> = if lookup_uuid.is_empty() {
            None
        } else {
            sqlx::query_as(
                "SELECT id FROM lti_resource_links \
                 WHERE context_type = 'Course' AND context_id = $1 AND lookup_uuid = $2",
            )
            .bind(course_id)
            .bind(lookup_uuid)
            .fetch_optional(pool)
            .await?
        };
        let new_tab = item
            .get("window")
            .and_then(|w| w.get("targetName"))
            .and_then(|v| v.as_str())
            == Some("_blank");
        sqlx::query(
            "INSERT INTO content_tags \
             (content_id, content_type, context_id, context_type, title, url, tag_type, \
              context_module_id, position, indent, workflow_state, associated_asset_id, \
              associated_asset_type, new_tab, created_at, updated_at, root_account_id) \
             VALUES ($1, 'ContextExternalTool', $2, 'Course', $3, $4, 'context_module', $5, $6, 0, \
              'unpublished', $7, 'Lti::ResourceLink', $8, now(), now(), \
              (SELECT COALESCE(account_id, 1) FROM courses WHERE id = $2))",
        )
        .bind(tool_id)
        .bind(course_id)
        .bind(item.get("title").and_then(|v| v.as_str()))
        .bind(item.get("url").and_then(|v| v.as_str()))
        .bind(module_id)
        .bind(next_position)
        .bind(link_id.map(|(id,)| id))
        .bind(new_tab)
        .execute(pool)
        .await?;
        next_position += 1;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Auth for the update-rights check
// ---------------------------------------------------------------------------

/// The deep-linking response is posted by the tool, but Canvas still has
/// the user's session (the launch happened in the user's browser); the port
/// accepts the same session bearer token via the `AuthUser` extractor.
async fn extract_auth_user(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Option<star_auth::AuthUser> {
    // The same semantics as the AuthUser extractor: a verified bearer
    // token whose user still exists (with their current role).
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?
        .strip_prefix("Bearer ")?;
    let mut validation = jsonwebtoken::Validation::default();
    validation.validate_aud = false;
    let claims = jsonwebtoken::decode::<star_common::Claims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &validation,
    )
    .ok()?;
    let (role,): (String,) = sqlx::query_as("SELECT role FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_one(&state.db)
        .await
        .ok()?;
    Some(star_auth::AuthUser {
        id: claims.sub,
        role,
    })
}
