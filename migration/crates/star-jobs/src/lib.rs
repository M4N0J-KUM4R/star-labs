//! Canvas-compatible background job/progress endpoints.

use axum::Router;
use star_common::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}
