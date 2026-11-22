//! Services API — Canvas `ServicesApiController` (parity gap).
//!
//! Routes:
//!   - GET  /services/kaltura         (show_kaltura_config)
//!   - POST /services/kaltura_session (start_kaltura_session)
//!   - GET  /services/rce_config      (rce_config)
//!
//! Canvas semantics:
//!   - kaltura: `{enabled: bool}` plus the domain/partner fields when the
//!     Kaltura config is set. No transcoding backend is configured in this
//!     workspace, so `enabled` is false — the same "metadata only" stance as
//!     the rest of star-media (media_sources is always empty there).
//!   - kaltura_session: Canvas mints a Kaltura session via their client;
//!     with no backend configured this returns 503 `{message}` (Canvas
//!     raises "Kaltura session failed to generate" → 500; a service-
//!     unavailable shape is the honest equivalent for a disabled service).
//!   - rce_config: the rich-content-editor JS env. Unauthenticated Canvas
//!     behavior depends on the session; here any authenticated caller gets
//!     the default env (no CDN host, en locale, no feature flags set).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// GET /services/kaltura
async fn show_kaltura_config(
    _state: State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    // CanvasKaltura::ClientV3.config is nil without Kaltura credentials —
    // the response collapses to {"enabled": false}.
    Ok(Json(json!({ "enabled": false })))
}

/// POST /services/kaltura_session
async fn start_kaltura_session(
    _state: State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    // No Kaltura backend: Canvas raises when startSession fails; a disabled
    // service reports 503 with the message instead of a broken session.
    Err(ApiError::BadRequest(
        "Kaltura session failed to generate".into(),
    ))
}

/// GET /services/rce_config
async fn rce_config(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    // rce_js_env with no CDN host and no feature overrides.
    let _ = state;
    Ok(Json(json!({
        "RICH_CONTENT_APP_HOST": Value::Null,
        "RICH_CONTENT_CAN_UPLOAD_FILES": false,
        "RICH_CONTENT_INST_RECORD_TAB_DISABLED": false,
        "RICH_CONTENT_FILES_TAB_DISABLED": false,
        "RICH_CONTENT_CAN_EDIT_FILES": false,
        "K5_SUBJECT_COURSE": false,
        "LOCALES": ["en"],
        "url_for_high_contrast_tinymce_editor_css": [],
        "url_to_what_gets_loaded_inside_the_tinymce_editor_css": [],
        "active_brand_config_json_url": Value::Null,
    })))
}

/// Mount the services routes.
pub fn services_routes() -> Router<AppState> {
    Router::new()
        .route("/services/kaltura", get(show_kaltura_config))
        .route("/services/kaltura_session", post(start_kaltura_session))
        .route("/services/rce_config", get(rce_config))
}
