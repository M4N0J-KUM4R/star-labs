//! learn_platform — Canvas `LearnPlatformController` parity.
//!
//! Routes:
//! - GET /accounts/{accountId}/learn_platform/products
//! - GET /accounts/{accountId}/learn_platform/products_categories
//! - GET /accounts/{accountId}/learn_platform/products/{id}
//! - GET /accounts/{accountId}/learn_platform/filters
//! - GET /accounts/{accountId}/learn_platform/organizations/{organizationSalesforceId}/products
//! - GET /accounts/{accountId}/learn_platform/custom_filters
//!
//! Canvas proxies these routes to the Learn Platform service
//! (`LearnPlatform::Api`). The service needs the `learnplatform` plugin with a
//! base url and the JWT settings; without them `LearnPlatform::Api#products`
//! and its siblings answer an empty body, and the controller renders it with
//! a 200 (`return {} unless valid_learnplatform?`).
//!
//! The routes are public in Canvas (`skip_before_action :require_user`), and
//! the port ships without the Learn Platform plugin, so every route answers
//! the empty body of the unconfigured service — exactly what Canvas answers
//! when the plugin is not enabled.

use axum::{
    extract::Path,
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use star_common::ApiError;

/// The empty answer of the unconfigured Learn Platform service.
fn empty_response() -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(json!({})))
}

/// GET /accounts/{accountId}/learn_platform/products — Canvas `index`.
async fn list_learn_platform_products(
    Path(_account_id): Path<i64>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    Ok(empty_response())
}

/// GET /accounts/{accountId}/learn_platform/products_categories — Canvas
/// `index_by_category`.
async fn list_learn_platform_product_categories(
    Path(_account_id): Path<i64>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    Ok(empty_response())
}

/// GET /accounts/{accountId}/learn_platform/products/{id} — Canvas `show`.
async fn show_learn_platform_product(
    Path((_account_id, _id)): Path<(i64, i64)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    Ok(empty_response())
}

/// GET /accounts/{accountId}/learn_platform/filters — Canvas `filters`.
async fn list_learn_platform_filters(
    Path(_account_id): Path<i64>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    Ok(empty_response())
}

/// GET /accounts/{accountId}/learn_platform/organizations/{organizationSalesforceId}/products
/// — Canvas `index_by_organization`.
async fn list_learn_platform_organization_products(
    Path((_account_id, _organization_id)): Path<(i64, i64)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    Ok(empty_response())
}

/// GET /accounts/{accountId}/learn_platform/custom_filters — Canvas
/// `custom_filters`.
async fn list_learn_platform_custom_filters(
    Path(_account_id): Path<i64>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    Ok(empty_response())
}

/// Build the learn platform router.
pub fn router() -> Router<star_common::AppState> {
    Router::new()
        .route(
            "/accounts/{accountId}/learn_platform/products",
            get(list_learn_platform_products),
        )
        .route(
            "/accounts/{accountId}/learn_platform/products_categories",
            get(list_learn_platform_product_categories),
        )
        .route(
            "/accounts/{accountId}/learn_platform/products/{id}",
            get(show_learn_platform_product),
        )
        .route(
            "/accounts/{accountId}/learn_platform/filters",
            get(list_learn_platform_filters),
        )
        .route(
            "/accounts/{accountId}/learn_platform/organizations/{organizationSalesforceId}/products",
            get(list_learn_platform_organization_products),
        )
        .route(
            "/accounts/{accountId}/learn_platform/custom_filters",
            get(list_learn_platform_custom_filters),
        )
}
