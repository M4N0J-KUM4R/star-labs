//! Brand configs — Canvas `BrandConfigsApiController` (parity gap).
//!
//! Routes:
//!   - GET /brand_variables                     (show — global)
//!   - GET /accounts/{accountId}/brand_variables (show_context)
//!   - GET /courses/{courseId}/brand_variables   (show_context)
//!
//! Canvas semantics:
//!   - `show` skips require_user (public: CORS `*`), then 302-redirects to
//!     the active brand config's public JSON path (the BrandableCSS default
//!     when the account has no active brand).
//!   - `show_context` still runs `require_user` (Canvas skips it only for
//!     `show`): an anonymous caller answers 401 before the context check,
//!     and a signed-in caller with an unknown account/course answers 404.
//!   - `show_context`: require_context — the account/course must exist
//!     (404); anything else would be a 400, but the route shapes here only
//!     carry Account/Course ids, so existence is the only check.
//!   - The redirect target: `<cdn-host>/dist/brand-css/<token>/<sha>.json`
//!     with no configured active brand → Canvas' public default path.

use axum::extract::{Path, State};
use axum::http::header::LOCATION;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `Canvas::Cdn.config.host` — unset in this workspace, so the path
/// is relative.
const CDN_HOST: &str = "";

/// Canvas BrandableCSS default public json path.
const DEFAULT_BRAND_JSON: &str = "/dist/brand-css/default.json";

/// Resolve the context's active brand config JSON path: the newest active
/// `brand_configs` row linked to the account (via shared_brand_configs),
/// else the default. Returns the token/sha path when a brand exists.
async fn active_brand_json_path(
    pool: &PgPool,
    account_id: Option<i64>,
) -> Result<String, ApiError> {
    let path: Option<String> = match account_id {
        None => None,
        Some(account_id) => {
            sqlx::query_scalar(
                "SELECT 'dist/brand-css/' || sbc.brand_config_md5 || '.json' \
                 FROM shared_brand_configs sbc \
                 WHERE sbc.account_id = $1 \
                 ORDER BY sbc.id DESC LIMIT 1",
            )
            .bind(account_id)
            .fetch_optional(pool)
            .await?
        }
    };
    Ok(match path {
        Some(p) if !p.is_empty() => format!("{CDN_HOST}/{p}"),
        _ => format!("{CDN_HOST}{DEFAULT_BRAND_JSON}"),
    })
}

fn brand_redirect_response(path: String) -> Response {
    // Rails `redirect_to` answers 302 Found; axum's `Redirect` only offers
    // 303/307/308 constructors (no 302 in axum 0.8), so the response is
    // built directly. Canvas also sets Access-Control-Allow-Origin: * for
    // the public CSS/JSON asset.
    let mut resp = (StatusCode::FOUND, [(LOCATION, path)]).into_response();
    resp.headers_mut().insert(
        "Access-Control-Allow-Origin",
        "*".parse().expect("static header value"),
    );
    resp
}

/// GET /brand_variables — global (site-admin account brand), no auth.
async fn show_brand_variables(State(state): State<AppState>) -> Result<Response, ApiError> {
    // The global brand is the first root account's, matching the
    // domain-root-account resolution used across the workspace.
    let root_account: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL \
         AND workflow_state <> 'deleted' ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;
    let path = active_brand_json_path(&state.db, root_account).await?;
    Ok(brand_redirect_response(path))
}

/// GET /accounts/{accountId}/brand_variables
///
/// Canvas `show_context`: `require_user` still applies (only `show` skips
/// it), so an anonymous caller answers 401 before the context lookup.
async fn show_account_brand_variables(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    _auth: AuthUser,
) -> Result<Response, ApiError> {
    // require_context: unknown account → 404.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let path = active_brand_json_path(&state.db, Some(account_id)).await?;
    Ok(brand_redirect_response(path))
}

/// GET /courses/{courseId}/brand_variables
async fn show_course_brand_variables(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    _auth: AuthUser,
) -> Result<Response, ApiError> {
    // require_context + the course's account brand.
    let account_id: Option<i64> = sqlx::query_scalar(
        "SELECT account_id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let account_id = account_id.ok_or(ApiError::NotFound)?;
    let path = active_brand_json_path(&state.db, Some(account_id)).await?;
    Ok(brand_redirect_response(path))
}

/// Only GETs are defined; axum's default method routing answers the
/// wrong-verb cases (405 + Allow) and every unknown path stays the global
/// 404 — a router-local fallback would hijack the merged app's fallback.
pub fn brand_configs_routes() -> Router<AppState> {
    Router::new()
        .route("/brand_variables", get(show_brand_variables))
        .route(
            "/accounts/{accountId}/brand_variables",
            get(show_account_brand_variables),
        )
        .route(
            "/courses/{courseId}/brand_variables",
            get(show_course_brand_variables),
        )
}
