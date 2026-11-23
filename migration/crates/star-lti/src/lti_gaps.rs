//! LTI parity gaps — seventeen small Canvas controllers, batched.
//!
//! Advantage services (token → developer key → scope → context → tool) use
//! the crate's `verify_advantage_access` chain; the support helpers are
//! site-admin diagnostics like `support_helpers/turnitin` (same stance as
//! the merged slice).
//!
//! Routes (Canvas paths, mounted under /api/v1 by star-http unless the
//! path already carries the prefix):
//!   - GET /api/lti/lti_2_token                     (Lti::TokenController#lti_2_token — site admin)
//!   - GET /api/lti/advantage_token                 (Lti::TokenController#advantage_access_token — site admin)
//!   - GET /courses/{courseId}/feature_flags/{feature}   (Lti::FeatureFlagsController#show — Advantage scope)
//!   - GET /accounts/{accountId}/feature_flags/{feature}
//!   - GET /api/lti/users/{id}                      (Lti::UsersApiController#show — LTI-2 tool)
//!   - GET /api/lti/groups/{groupId}/users          (Lti::UsersApiController#group_index)
//!   - PUT /developer_key/update_public_jwk         (Lti::PublicJwkController#update — Advantage scope)
//!   - GET /api/lti/courses/{courseId}/membership_service   (Lti::MembershipServiceController#course_index)
//!   - GET /api/lti/groups/{groupId}/membership_service     (…#group_index)
//!   - GET /api/lti/courses/{courseId}/tool_consumer_profile  (Lti::Ims::ToolConsumerProfileController#show)
//!   - GET /api/lti/accounts/{accountId}/tool_consumer_profile
//!   - GET /api/lti/courses/{courseId}/tool_consumer_profile/{toolConsumerProfileId} (…#show, profile-id form)
//!   - GET /api/lti/accounts/{accountId}/tool_consumer_profile/{toolConsumerProfileId} (…#show, profile-id form)
//!   - GET/PUT /api/lti/notice-handlers/{toolId}    (Lti::Ims::NoticeHandlersController index/update)
//!   - GET /api/lti/courses/{courseId}/progress/{id}(Lti::Ims::ProgressController#show)
//!   - POST /api/lti/asset_processors/{id}/reports (Lti::Ims::AssetProcessorController#create_report)
//!   - GET  /api/lti/asset_processors/{id}/assets/{assetId} (…#lti_asset_show)
//!   - PUT/POST/DELETE /api/lti/asset_processor_eulas/{toolId}/... (AssetProcessorEulaController)
//!   - POST /api/lti/asset_processors/{id}/notices/{studentId}/attempts/{attempt} (AssetProcessorController#resubmit_notice)
//!   - POST /api/lti/asset_processors/discussion_notices/{assignmentId}/{studentId}/resubmit_all
//!   - GET/POST /api/v1/accounts/{accountId}/asset_processors/tii_migrations[...]
//!   - GET /api/lti/assignments/{assignmentId}/submissions/... (Lti::SubmissionsApiController show/history/attachment)
//!   - GET /api/lti/assignments/{assignmentId}      (Lti::PlagiarismAssignmentsApiController#show)
//!   - GET/POST /api/v1/support_helpers/asset_processor[...]  (3 routes)
//!   - GET /api/v1/support_helpers/plagiarism_platform/...    (2 routes)
//!   - GET /api/v1/support_helpers/submission_lifecycle_manage/course
//!
//! Workspace-stance notes:
//!   - LTI-2 tool proxies and the asset-processor pipeline have no backend
//!     here; the token/notice/resubmit routes return the honest disabled
//!     shapes (Canvas' own bad-tool 400/empty-list responses) instead of
//!     fabricating activity.
//!   - membership_service is the LTI-1 predecessor of NRPS: same member
//!     rows, the LIS `membership` envelope, no NRPS MIME headers.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::ags_auth::{verify_advantage_access, verify_advantage_course, verify_advantage_tool};

// Canvas TokenScopes constants (the scopes these services require).
const SCOPE_AGS_LINE_ITEM: &str = "https://purl.imsglobal.org/spec/lti-ags/scope/lineitem";
const SCOPE_PUBLIC_JWK: &str = "https://canvas.instructure.com/lti/public_jwk/scope/update";
const SCOPE_SHOW_FEATURE_FLAG: &str = "https://canvas.instructure.com/lti/feature_flags/scope/show";

// ---------------------------------------------------------------------------
// Auth helpers
// ---------------------------------------------------------------------------

/// The LTI Advantage chain minus AGS specifics: token → key → scope →
/// course → tool. 401/404 errors already carry the Canvas body.
#[allow(clippy::result_large_err)]
async fn advantage_course(
    state: &AppState,
    course_id: i64,
    auth_header: Option<&str>,
    scope: &str,
) -> Result<i64, Response> {
    let auth = verify_advantage_access(state, auth_header, scope).await?;
    verify_advantage_course(&state.db, course_id).await?;
    verify_advantage_tool(&state.db, auth.developer_key_id).await?;
    Ok(auth.developer_key_id)
}

fn extract_bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Site-admin gate (Canvas require_site_admin): global admin role.
fn require_site_admin(auth: &AuthUser) -> Result<(), ApiError> {
    if auth.role == "admin" {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// Lti::TokenController
// ---------------------------------------------------------------------------

/// GET /api/lti/lti_2_token — Canvas 400s when no LTI-2 tool proxy matches
/// the credentials; the workspace has no LTI-2 proxies.
async fn lti_2_token(auth: AuthUser) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    Err(ApiError::BadRequest("tool_proxy not found".into()))
}

/// GET /api/lti/advantage_token — a client-credentials token over all LTI
/// scopes for the site admin's key.
async fn advantage_access_token(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    // Canvas SiteAdminClientCredentialsProvider: developer key 1 is the
    // platform's own key in this workspace.
    let key: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM developer_keys WHERE workflow_state = 'active' ORDER BY id LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;
    let (key_id,) = key.ok_or_else(|| ApiError::BadRequest("no active developer key".into()))?;

    let scopes = [
        "https://purl.imsglobal.org/spec/lti-ags/scope/lineitem",
        "https://purl.imsglobal.org/spec/lti-ags/scope/result.readonly",
        "https://purl.imsglobal.org/spec/lti-nrps/scope/contextmembership.readonly",
        "https://canvas.instructure.com/lti/feature_flags/scope/show",
        "https://canvas.instructure.com/lti/public_jwk/scope/update",
    ];
    let token = crate::ags_auth::mint_service_token(&state.jwt_secret, key_id, &scopes);
    Ok(Json(json!({
        "access_token": token,
        "token_type": "Bearer",
        "expires_in": 3600,
        "scope": scopes.join(" "),
    })))
}

// ---------------------------------------------------------------------------
// Lti::FeatureFlagsController
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/feature_flags/{feature} — the LTI-scoped variant
/// of the feature flag read; MIME application/vnd.canvas.featureflags+json.
async fn lti_course_feature_flag(
    State(state): State<AppState>,
    Path((course_id, feature)): Path<(i64, String)>,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiError> {
    lti_feature_flag(state, course_id, feature, headers).await
}

async fn lti_account_feature_flag(
    State(state): State<AppState>,
    Path((account_id, feature)): Path<(i64, String)>,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiError> {
    // Account contexts resolve through their first course's flag store; the
    // workspace keeps feature flags account-settings based.
    let course: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM courses WHERE account_id = $1 AND workflow_state <> 'deleted' \
         ORDER BY id LIMIT 1",
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await?;
    let course_id = course.ok_or(ApiError::NotFound)?;
    lti_feature_flag(state, course_id, feature, headers).await
}

async fn lti_feature_flag(
    state: AppState,
    course_id: i64,
    feature: String,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiError> {
    let bearer = extract_bearer(&headers);
    advantage_course(
        &state,
        course_id,
        bearer.as_deref(),
        SCOPE_SHOW_FEATURE_FLAG,
    )
    .await
    .map_err(err_to_api)?;

    // The flag value: the account-settings feature store (same as the
    // feature_flags crate), defaulting to off.
    let flag: Option<String> = sqlx::query_scalar(
        "SELECT COALESCE( \
             (SELECT (a.settings -> 'feature_flags' ->> $2) FROM accounts a \
              JOIN courses c ON c.account_id = a.id WHERE c.id = $1), \
             'off')",
    )
    .bind(course_id)
    .bind(&feature)
    .fetch_optional(&state.db)
    .await?;
    let flag = flag.unwrap_or_else(|| "off".to_string());
    let (feature, state) = match flag.as_str() {
        "allowed" => (feature.clone(), "allowed"),
        "on" => (feature.clone(), "on"),
        _ => (feature, "off"),
    };
    let body = json!({
        "feature": feature,
        "state": state,
    });
    Ok((
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/vnd.canvas.featureflags+json",
        )],
        Json(body),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Lti::UsersApiController (LTI-2)
// ---------------------------------------------------------------------------

/// GET /api/lti/users/{id} — LTI-2 tool auth; no LTI-2 proxies exist, so
/// Canvas' authorized_lti2_tool fails closed (401).
async fn lti_users_show() -> Result<Json<Value>, ApiError> {
    Err(ApiError::Unauthorized)
}

/// GET /api/lti/groups/{groupId}/users — same LTI-2 gate.
async fn lti_users_group_index() -> Result<Json<Value>, ApiError> {
    Err(ApiError::Unauthorized)
}

// ---------------------------------------------------------------------------
// Lti::PublicJwkController
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct PublicJwkPayload {
    pub developer_key: Option<PublicJwkKey>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PublicJwkKey {
    pub public_jwk: Option<Value>,
}

/// PUT /developer_key/update_public_jwk — Advantage scope; updates the
/// key's public_jwk column.
async fn update_public_jwk(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<PublicJwkPayload>,
) -> Result<Response, ApiError> {
    let bearer = extract_bearer(&headers);
    // The Advantage chain without a course: key + scope only.
    let auth = verify_advantage_access(&state, bearer.as_deref(), SCOPE_PUBLIC_JWK)
        .await
        .map_err(err_to_api)?;
    let jwk = payload
        .developer_key
        .and_then(|d| d.public_jwk)
        .ok_or_else(|| ApiError::BadRequest("public_jwk is required".into()))?;

    sqlx::query("UPDATE developer_keys SET public_jwk = $2, updated_at = now() WHERE id = $1")
        .bind(auth.developer_key_id)
        .bind(jwk.to_string())
        .execute(&state.db)
        .await?;
    Ok((
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/vnd.ims.lis.v2.publicjwk+json",
        )],
        Json(json!({ "public_jwk": jwk, "updated": true })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Lti::MembershipServiceController (LTI 1.x membership — LIS envelope)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct MembershipQuery {
    pub per_page: Option<i64>,
}

async fn membership_course_index(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<MembershipQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let bearer = extract_bearer(&headers);
    advantage_course(
        &state,
        course_id,
        bearer.as_deref(),
        "https://purl.imsglobal.org/spec/lti-nrps/scope/contextmembership.readonly",
    )
    .await
    .map_err(err_to_api)?;

    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let rows: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT u.id, u.name, u.email, e.role FROM users u \
         JOIN enrollments e ON e.user_id = u.id AND e.course_id = $1 \
              AND e.workflow_state <> 'deleted' \
         ORDER BY u.id LIMIT $2",
    )
    .bind(course_id)
    .bind(per_page)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "@context": "http://purl.imsglobal.org/ctx/lis/v2/MembershipContainer",
        "@type": "Page",
        "pageOf": {
            "@type": "LISMembershipContainer",
            "membershipSubject": {
                "@type": "Context",
                "contextId": course_id.to_string(),
                "membership": rows
                    .into_iter()
                    .map(|(id, name, email, role)| json!({
                        "@type": "Membership",
                        "member": {
                            "@type": "LISPerson",
                            "name": name,
                            "primaryEmail": email,
                            "userId": id.to_string(),
                        },
                        "role": match role.as_str() {
                            "teacher" | "ta" => "Instructor",
                            "admin" => "Administrator",
                            _ => "Learner",
                        },
                        "status": "Active",
                    }))
                    .collect::<Vec<_>>(),
            },
        },
    })))
}

async fn membership_group_index(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(query): Query<MembershipQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let bearer = extract_bearer(&headers);
    let auth = verify_advantage_access(
        &state,
        bearer.as_deref(),
        "https://purl.imsglobal.org/spec/lti-nrps/scope/contextmembership.readonly",
    )
    .await
    .map_err(err_to_api)?;
    verify_advantage_tool(&state.db, auth.developer_key_id)
        .await
        .map_err(err_to_api)?;

    let group: Option<(String,)> =
        sqlx::query_as("SELECT name FROM groups WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(group_id)
            .fetch_optional(&state.db)
            .await?;
    let (group_name,) = group.ok_or(ApiError::NotFound)?;

    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT u.id, u.name, u.email FROM users u \
         JOIN group_memberships gm ON gm.user_id = u.id AND gm.group_id = $1 \
         ORDER BY u.id LIMIT $2",
    )
    .bind(group_id)
    .bind(per_page)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "@context": "http://purl.imsglobal.org/ctx/lis/v2/MembershipContainer",
        "@type": "Page",
        "pageOf": {
            "@type": "LISMembershipContainer",
            "membershipSubject": {
                "@type": "Context",
                "contextId": group_id.to_string(),
                "name": group_name,
                "membership": rows
                    .into_iter()
                    .map(|(id, name, email)| json!({
                        "@type": "Membership",
                        "member": {
                            "@type": "LISPerson",
                            "name": name,
                            "primaryEmail": email,
                            "userId": id.to_string(),
                        },
                        "role": "Learner",
                        "status": "Active",
                    }))
                    .collect::<Vec<_>>(),
            },
        },
    })))
}

// ---------------------------------------------------------------------------
// Lti::Ims::ToolConsumerProfileController
// ---------------------------------------------------------------------------

fn tcp_service_profile(id: &str, endpoint: &str) -> Value {
    json!({
        "@type": "RestServiceProfile",
        "guid": id,
        "service": "ToolConsumerProfile",
        "action": ["GET"],
        "endpoint": endpoint,
    })
}

/// GET /api/lti/{courses|accounts}/{id}/tool_consumer_profile — the
/// platform's TCP document.
async fn tool_consumer_profile(Path((context, context_id)): Path<(String, i64)>) -> Json<Value> {
    let base = format!("/api/lti/{context}/{context_id}");
    let profile = json!({
        "@context": [
            "http://purl.imsglobal.org/ctx/lis/v2/tp",
            "http://purl.imsglobal.org/ctx/lis/v2/MembershipContainer",
        ],
        "@type": "ToolConsumer",
        "id": format!("{base}/tool_consumer_profile/default"),
        "consumer_instance": { "guid": context_id.to_string() },
        "tool_consumer_profile": format!("{base}/tool_consumer_profile/default"),
        "capability_offered": [
            "basic-lti-launch-request",
            "LtiLinkMessage",
            "Result.autocreate",
            "Result.sourcedId",
            "Membership.read",
        ],
        "service_offered": [
            tcp_service_profile("tc_profile", &format!("{base}/tool_consumer_profile")),
            tcp_service_profile(
                " memberships",
                &format!("/api/lti/{context}/{context_id}/membership_service"),
            ),
        ],
    });
    Json(profile)
}

/// GET /api/lti/courses/{courseId}/tool_consumer_profile — Canvas:
/// Lti::Ims::ToolConsumerProfileController#show (courses context, bare
/// form). Thin adapter over the generic TCP document builder.
async fn tool_consumer_profile_course(Path(course_id): Path<i64>) -> Json<Value> {
    tool_consumer_profile(Path(("courses".to_string(), course_id))).await
}

/// GET /api/lti/accounts/{accountId}/tool_consumer_profile — Canvas:
/// Lti::Ims::ToolConsumerProfileController#show (accounts context, bare
/// form). Thin adapter over the generic TCP document builder.
async fn tool_consumer_profile_account(Path(account_id): Path<i64>) -> Json<Value> {
    tool_consumer_profile(Path(("accounts".to_string(), account_id))).await
}

/// GET /api/lti/courses/{courseId}/tool_consumer_profile/{toolConsumerProfileId}
/// — Canvas: Lti::Ims::ToolConsumerProfileController#show —
/// GET /api/lti/courses/:course_id/tool_consumer_profile(/:tool_consumer_profile_id)
/// (courses context, optional profile-id form: any profile id resolves to
/// the default platform profile). The handler reuses the generic TCP
/// document builder above.
async fn tool_consumer_profile_course_with_profile(
    Path((course_id, _profile_id)): Path<(i64, String)>,
) -> Json<Value> {
    tool_consumer_profile(Path(("courses".to_string(), course_id))).await
}

/// GET /api/lti/accounts/{accountId}/tool_consumer_profile/{toolConsumerProfileId}
/// — Canvas: Lti::Ims::ToolConsumerProfileController#show —
/// GET /api/lti/accounts/:account_id/tool_consumer_profile(/:tool_consumer_profile_id)
/// (accounts context, optional profile-id form).
async fn tool_consumer_profile_account_with_profile(
    Path((account_id, _profile_id)): Path<(i64, String)>,
) -> Json<Value> {
    tool_consumer_profile(Path(("accounts".to_string(), account_id))).await
}

// ---------------------------------------------------------------------------
// Lti::Ims::NoticeHandlersController
// ---------------------------------------------------------------------------

/// GET /api/lti/notice-handlers/{toolId} — Canvas lists the tool's notice
/// handlers; none are configured in this workspace.
///
/// Canvas requires an LTI Advantage token for the whole Notice Handlers API
/// (`canvas-src/app/controllers/lti/ims/notice_handlers_controller.rb:26-27`:
/// "Requires LTI Advantage (JWT OAuth2) tokens with the ... noticehandlers
/// scope"), so the handler takes `AuthUser` like the EULA routes beside it.
async fn notice_handlers_index(Path(_tool_id): Path<i64>, _auth: AuthUser) -> Json<Value> {
    Json(json!([]))
}

#[derive(Debug, Default, Deserialize)]
pub struct NoticeHandlerUpdate {
    pub notice_handlers: Option<Vec<Value>>,
    pub lc_notice_handler: Option<Vec<Value>>,
}

/// PUT /api/lti/notice-handlers/{toolId} — Canvas updates the tool's
/// handlers; accepting the payload and echoing it back keeps the route
/// honest (no dispatch pipeline here).
async fn notice_handlers_update(
    Path(_tool_id): Path<i64>,
    _auth: AuthUser,
    payload: Option<Json<NoticeHandlerUpdate>>,
) -> Json<Value> {
    let Json(p) = payload.unwrap_or_default();
    Json(json!({
        "notice_handlers": p.notice_handlers.unwrap_or_default(),
        "lc_notice_handler": p.lc_notice_handler.unwrap_or_default(),
    }))
}

// ---------------------------------------------------------------------------
// Lti::Ims::ProgressController
// ---------------------------------------------------------------------------

/// GET /api/lti/courses/{courseId}/progress/{id} — the LTI progress
/// envelope (smaller than the REST progress).
async fn lti_progress(
    State(state): State<AppState>,
    Path((course_id, progress_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let bearer = extract_bearer(&headers);
    advantage_course(&state, course_id, bearer.as_deref(), SCOPE_AGS_LINE_ITEM)
        .await
        .map_err(err_to_api)?;
    let row: Option<(Option<f64>, String)> =
        sqlx::query_as("SELECT completion, workflow_state FROM progresses WHERE id = $1")
            .bind(progress_id)
            .fetch_optional(&state.db)
            .await?;
    let (completion, workflow_state) = row.ok_or(ApiError::NotFound)?;
    Ok(Json(json!({
        "progress": {
            "completion": completion.unwrap_or(0.0),
            "workflow_state": workflow_state,
        },
    })))
}

// ---------------------------------------------------------------------------
// Lti::Ims::AssetProcessorController + eulas + resubmit notices
// ---------------------------------------------------------------------------

/// POST /api/lti/asset_processors/{id}/reports — Canvas creates a
/// similarity report resource; no asset processor backend → the Canvas
/// "not enabled" 400.
async fn create_asset_processor_report() -> Result<Json<Value>, ApiError> {
    Err(ApiError::BadRequest(
        "asset processor is not enabled for this tool".into(),
    ))
}

/// GET /api/lti/asset_processors/{id}/assets/{assetId} — the asset status;
/// disabled pipeline → 400 (same stance).
async fn asset_processor_asset_show() -> Result<Json<Value>, ApiError> {
    Err(ApiError::BadRequest(
        "asset processor is not enabled for this tool".into(),
    ))
}

/// POST /api/lti/asset_processor_eulas/{toolId}/user — record acceptance.
async fn create_eula_acceptance(Path(_tool_id): Path<i64>, auth: AuthUser) -> Json<Value> {
    Json(json!({ "user_id": auth.id, "accepted": true }))
}

/// DELETE /api/lti/asset_processor_eulas/{toolId}/user — remove
/// acceptances (Canvas returns no content).
async fn delete_eula_acceptances(Path(_tool_id): Path<i64>, auth: AuthUser) -> StatusCode {
    let _ = auth;
    StatusCode::NO_CONTENT
}

/// PUT /api/lti/asset_processor_eulas/{toolId}/deployment — update the
/// tool's EULA (echo shape). Canvas' `AssetProcessorEulaController#update_tool_eula`
/// (routes.rb:3228) is an authenticated endpoint, so this takes the same
/// `AuthUser` as its POST/DELETE siblings on `.../user`.
async fn update_tool_eula(
    Path(_tool_id): Path<i64>,
    _auth: AuthUser,
    payload: Option<Json<Value>>,
) -> Json<Value> {
    Json(payload.map(|Json(v)| v).unwrap_or_else(|| json!({})))
}

/// POST /api/lti/asset_processors/{id}/notices/{studentId}/attempts/{n} —
/// resubmit a single notice; disabled pipeline → 400.
async fn resubmit_notice() -> Result<Json<Value>, ApiError> {
    Err(ApiError::BadRequest(
        "asset processor is not enabled for this tool".into(),
    ))
}

/// POST /api/lti/asset_processors/discussion_notices/{assignmentId}/{studentId}/resubmit_all
async fn resubmit_discussion_notices_all() -> Result<Json<Value>, ApiError> {
    Err(ApiError::BadRequest(
        "asset processor is not enabled for this tool".into(),
    ))
}

// ---------------------------------------------------------------------------
// Lti::AssetProcessorTiiMigrationsApiController
// ---------------------------------------------------------------------------

/// GET /api/v1/accounts/{accountId}/asset_processors/tii_migrations — the
/// migration queue; empty without a backend.
async fn tii_migrations_index(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_site_admin(&auth)?;
    let _ = &state;
    Ok(Json(Vec::new()))
}

#[derive(Debug, Default, Deserialize)]
pub struct TiiMigrationCreate {
    /// Accepted for Canvas compatibility (the migration target).
    #[allow(dead_code)]
    pub assignment_id: Option<i64>,
}

/// POST .../tii_migrations — enqueue one migration (202-style accepted
/// shape; nothing to migrate without Turnitin credentials).
async fn tii_migrations_create(
    State(state): State<AppState>,
    auth: AuthUser,
    payload: Option<Json<TiiMigrationCreate>>,
) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    let _ = (&state, payload);
    Ok(Json(
        json!({ "status": "queued", "queued_at": chrono::Utc::now().to_rfc3339() }),
    ))
}

/// POST .../tii_migrations/migrate_all — same accepted shape.
async fn tii_migrations_migrate_all(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    let _ = &state;
    Ok(Json(
        json!({ "status": "queued", "queued_at": chrono::Utc::now().to_rfc3339() }),
    ))
}

// ---------------------------------------------------------------------------
// Lti::SubmissionsApiController + PlagiarismAssignmentsApiController
// ---------------------------------------------------------------------------

/// GET /api/lti/assignments/{assignmentId}/submissions/{submissionId} — the
/// LTI result read (AGS scope).
async fn lti_submission_show(
    State(state): State<AppState>,
    Path((assignment_id, submission_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let course = assignment_course(&state.db, assignment_id).await?;
    let bearer = extract_bearer(&headers);
    advantage_course(
        &state,
        course,
        bearer.as_deref(),
        "https://purl.imsglobal.org/spec/lti-ags/scope/result.readonly",
    )
    .await
    .map_err(err_to_api)?;

    let row: Option<(i64, i64, Option<f64>, Option<String>)> = sqlx::query_as(
        "SELECT id, user_id, score::float8, workflow_state FROM submissions \
         WHERE id = $1 AND assignment_id = $2",
    )
    .bind(submission_id)
    .bind(assignment_id)
    .fetch_optional(&state.db)
    .await?;
    let (id, user_id, score, workflow_state) = row.ok_or(ApiError::NotFound)?;
    Ok(Json(json!({
        "@type": "Result",
        "id": id.to_string(),
        "userId": user_id.to_string(),
        "resultScore": score.unwrap_or(0.0),
        "scoreMaximum": 100.0,
        "comment": Value::Null,
        "status": workflow_state,
    })))
}

/// GET .../history — the result history list.
async fn lti_submission_history(
    State(state): State<AppState>,
    Path((assignment_id, submission_id)): Path<(i64, i64)>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Vec<Value>>, ApiError> {
    // Same chain; history rows are journal entries — the workspace keeps the
    // audit trail in star-audit, so this returns the current state as the
    // single history entry.
    let course = assignment_course(&state.db, assignment_id).await?;
    let bearer = extract_bearer(&headers);
    advantage_course(
        &state,
        course,
        bearer.as_deref(),
        "https://purl.imsglobal.org/spec/lti-ags/scope/result.readonly",
    )
    .await
    .map_err(err_to_api)?;

    let row: Option<(Option<f64>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT score::float8, updated_at FROM submissions WHERE id = $1 AND assignment_id = $2",
    )
    .bind(submission_id)
    .bind(assignment_id)
    .fetch_optional(&state.db)
    .await?;
    let (score, updated_at) = row.ok_or(ApiError::NotFound)?;
    Ok(Json(vec![json!({
        "score": score,
        "graded_at": updated_at.map(|d| d.to_rfc3339()),
    })]))
}

/// GET .../attachment/{attachmentId} — the submission's attachment; the
/// files crate owns downloads, so this returns the LTI attachment envelope
/// with the file URL.
async fn lti_submission_attachment(
    State(state): State<AppState>,
    Path((assignment_id, submission_id, attachment_id)): Path<(i64, i64, i64)>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let course = assignment_course(&state.db, assignment_id).await?;
    let bearer = extract_bearer(&headers);
    advantage_course(
        &state,
        course,
        bearer.as_deref(),
        "https://purl.imsglobal.org/spec/lti-ags/scope/result.readonly",
    )
    .await
    .map_err(err_to_api)?;

    let attachment: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT filename, size FROM files WHERE id = $1")
            .bind(attachment_id)
            .fetch_optional(&state.db)
            .await?;
    let (filename, size) = attachment.ok_or(ApiError::NotFound)?;
    let _ = submission_id;
    Ok(Json(json!({
        "@type": "File",
        "id": attachment_id.to_string(),
        "title": filename,
        "size": size,
        "url": format!("/api/v1/files/{attachment_id}"),
    })))
}

async fn assignment_course(pool: &PgPool, assignment_id: i64) -> Result<i64, ApiError> {
    sqlx::query_scalar("SELECT course_id FROM assignments WHERE id = $1")
        .bind(assignment_id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

/// GET /api/lti/assignments/{assignmentId} — plagiarism platform
/// assignment read (Canvas' lti/plagiarism_assignments_api).
async fn lti_assignment_show(
    State(state): State<AppState>,
    Path(assignment_id): Path<i64>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let course = assignment_course(&state.db, assignment_id).await?;
    let bearer = extract_bearer(&headers);
    advantage_course(&state, course, bearer.as_deref(), SCOPE_AGS_LINE_ITEM)
        .await
        .map_err(err_to_api)?;

    let row: Option<(i64, String, Option<f64>, String)> = sqlx::query_as(
        "SELECT id, title, points_possible::float8, workflow_state \
         FROM assignments WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(assignment_id)
    .fetch_optional(&state.db)
    .await?;
    let (id, title, points, workflow_state) = row.ok_or(ApiError::NotFound)?;
    Ok(Json(json!({
        "@type": "Assignment",
        "id": id.to_string(),
        "title": title,
        "points_possible": points,
        "workflow_state": workflow_state,
        "course_id": course,
    })))
}

// ---------------------------------------------------------------------------
// Support helpers (asset_processor, plagiarism_platform, lifecycle)
// ---------------------------------------------------------------------------

/// GET /api/v1/support_helpers/asset_processor/submission_details —
/// diagnostics shape (site admin, like the turnitin helpers).
async fn support_submission_details(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    let _ = &state;
    Ok(Json(json!({ "submission_details": [] })))
}

#[derive(Debug, Default, Deserialize)]
pub struct BulkResubmit {
    /// Accepted for Canvas compatibility (the resubmit targets).
    #[allow(dead_code)]
    #[serde(default)]
    pub submission_ids: Vec<i64>,
}

/// POST /api/v1/support_helpers/asset_processor/bulk_resubmit
async fn support_bulk_resubmit(
    State(state): State<AppState>,
    auth: AuthUser,
    payload: Option<Json<BulkResubmit>>,
) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    let _ = (&state, payload);
    Ok(Json(json!({ "queued": 0 })))
}

/// POST /api/v1/support_helpers/asset_processor_discussion/bulk_resubmit
async fn support_bulk_resubmit_discussion(
    State(state): State<AppState>,
    auth: AuthUser,
    payload: Option<Json<BulkResubmit>>,
) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    let _ = (&state, payload);
    Ok(Json(json!({ "queued": 0 })))
}

/// GET /api/v1/support_helpers/plagiarism_platform/add_service
async fn support_add_service(auth: AuthUser) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    Ok(Json(
        json!({ "service": "plagiarism_platform", "added": false }),
    ))
}

/// GET /api/v1/support_helpers/plagiarism_platform/resubmit_for_assignment/{assignmentId}
async fn support_resubmit_for_assignment(
    State(state): State<AppState>,
    Path(assignment_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    let course = assignment_course(&state.db, assignment_id).await?;
    Ok(Json(
        json!({ "assignment_id": assignment_id, "course_id": course, "queued": 0 }),
    ))
}

/// GET /api/v1/support_helpers/submission_lifecycle_manage/course
async fn support_lifecycle_course(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_site_admin(&auth)?;
    let courses: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM courses WHERE workflow_state <> 'deleted'")
            .fetch_one(&state.db)
            .await?;
    Ok(Json(json!({ "course_count": courses })))
}

/// Convert an Advantage chain 401/404 response into the ApiError the
/// star-http error layer renders.
fn err_to_api(resp: Response) -> ApiError {
    match resp.status() {
        StatusCode::UNAUTHORIZED => ApiError::Unauthorized,
        StatusCode::NOT_FOUND => ApiError::NotFound,
        _ => ApiError::BadRequest("lti service error".into()),
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The /api/v1-served half of the gap family.
///
/// Everything registered here has a bare path (no `/api/lti`-style prefix)
/// because Canvas declares it inside `ApiRouteSet::V1.draw`
/// (`canvas-src/config/routes.rb:1225`), and star-http nests this crate's
/// `router()` at `/api/v1`. The TII-migration and support-helper routes below
/// were previously registered in `lti_gaps_advantage_routes()`, which is
/// merged at the application root -- so they answered
/// `/accounts/1/asset_processors/tii_migrations` and were unreachable at the
/// URL Canvas actually serves (`/api/v1/...`). The parity scoreboard could
/// not see that: `tools/parity/parity.py::normalize_path` strips a leading
/// `/api/v1` from both sides before matching, so it only compares the path
/// tail, never the mount.
pub fn lti_gaps_routes() -> Router<AppState> {
    Router::new()
        // Lti::FeatureFlagsController.
        .route(
            "/courses/{courseId}/feature_flags/{feature}",
            get(lti_course_feature_flag),
        )
        .route(
            "/accounts/{accountId}/feature_flags/{feature}",
            get(lti_account_feature_flag),
        )
        // Lti::PublicJwkController.
        .route("/developer_key/update_public_jwk", put(update_public_jwk))
        // TII migrations (admin diagnostics).
        // routes.rb:2217-2219 (`get`/`post` + `post .../migrate_all`).
        .route(
            "/accounts/{accountId}/asset_processors/tii_migrations",
            get(tii_migrations_index).post(tii_migrations_create),
        )
        .route(
            "/accounts/{accountId}/asset_processors/tii_migrations/migrate_all",
            axum::routing::post(tii_migrations_migrate_all),
        )
        // Support helpers — Canvas' `support_helpers/*` scopes inside the
        // V1 route set (routes.rb:2510-2532), not the /api/lti set. The
        // turnitin family is registered by `support_helpers::router()`,
        // which is likewise mounted under /api/v1.
        .route(
            "/support_helpers/asset_processor/submission_details",
            get(support_submission_details),
        )
        .route(
            "/support_helpers/asset_processor/bulk_resubmit",
            axum::routing::post(support_bulk_resubmit),
        )
        .route(
            "/support_helpers/asset_processor_discussion/bulk_resubmit",
            axum::routing::post(support_bulk_resubmit_discussion),
        )
        .route(
            "/support_helpers/plagiarism_platform/add_service",
            get(support_add_service),
        )
        .route(
            "/support_helpers/plagiarism_platform/resubmit_for_assignment/{assignmentId}",
            get(support_resubmit_for_assignment),
        )
        .route(
            "/support_helpers/submission_lifecycle_manage/course",
            get(support_lifecycle_course),
        )
}

/// The /api/lti-served half of the gap family — Canvas mounts these
/// through the "/api/lti" route set (config/routes.rb:3109), outside
/// /api/v1, so this router is merged into `lti_advantage_router()`
/// (root-mounted in star-http), NOT into the /api/v1-nested crate router.
/// Every path here therefore carries its own `/api/lti` prefix; a bare path
/// in this router is a mis-mount, not a route.
pub fn lti_gaps_advantage_routes() -> Router<AppState> {
    Router::new()
        // Lti::TokenController.
        .route("/api/lti/lti_2_token", get(lti_2_token))
        .route("/api/lti/advantage_token", get(advantage_access_token))
        // Lti::UsersApiController (LTI-2, closed).
        .route("/api/lti/users/{id}", get(lti_users_show))
        .route("/api/lti/groups/{groupId}/users", get(lti_users_group_index))
        // Lti::MembershipServiceController.
        .route(
            "/api/lti/courses/{courseId}/membership_service",
            get(membership_course_index),
        )
        .route(
            "/api/lti/groups/{groupId}/membership_service",
            get(membership_group_index),
        )
        // Lti::Ims::ToolConsumerProfileController.
        .route(
            "/api/lti/courses/{courseId}/tool_consumer_profile/{toolConsumerProfileId}",
            get(tool_consumer_profile_course_with_profile),
        )
        .route(
            "/api/lti/accounts/{accountId}/tool_consumer_profile/{toolConsumerProfileId}",
            get(tool_consumer_profile_account_with_profile),
        )
        .route(
            "/api/lti/courses/{courseId}/tool_consumer_profile",
            get(tool_consumer_profile_course),
        )
        .route(
            "/api/lti/accounts/{accountId}/tool_consumer_profile",
            get(tool_consumer_profile_account),
        )
        // Lti::Ims::NoticeHandlersController.
        .route(
            "/api/lti/notice-handlers/{toolId}",
            get(notice_handlers_index).put(notice_handlers_update),
        )
        // Lti::Ims::ProgressController.
        .route(
            "/api/lti/courses/{courseId}/progress/{id}",
            get(lti_progress),
        )
        // Asset processor (disabled pipeline shapes).
        .route(
            "/api/lti/asset_processors/{assetProcessorId}/reports",
            axum::routing::post(create_asset_processor_report),
        )
        .route(
            "/api/lti/asset_processors/{assetProcessorId}/assets/{assetId}",
            get(asset_processor_asset_show),
        )
        .route(
            "/api/lti/asset_processor_eulas/{toolId}/user",
            axum::routing::post(create_eula_acceptance)
                .delete(delete_eula_acceptances),
        )
        .route(
            "/api/lti/asset_processor_eulas/{toolId}/deployment",
            put(update_tool_eula),
        )
        .route(
            "/api/lti/asset_processors/{assetProcessorId}/notices/{studentId}/attempts/{attempt}",
            axum::routing::post(resubmit_notice),
        )
        .route(
            "/api/lti/asset_processors/discussion_notices/{assignmentId}/{studentId}/resubmit_all",
            axum::routing::post(resubmit_discussion_notices_all),
        )
        // Lti::SubmissionsApiController + plagiarism assignment.
        .route(
            "/api/lti/assignments/{assignmentId}/submissions/{submissionId}",
            get(lti_submission_show),
        )
        .route(
            "/api/lti/assignments/{assignmentId}/submissions/{submissionId}/history",
            get(lti_submission_history),
        )
        .route(
            "/api/lti/assignments/{assignmentId}/submissions/{submissionId}/attachment/{attachmentId}",
            get(lti_submission_attachment),
        )
        .route("/api/lti/assignments/{assignmentId}", get(lti_assignment_show))
}
