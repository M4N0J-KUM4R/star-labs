//! Canvas LTI 1.3 registration and deployment endpoints.

use axum::Router;
use star_common::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}
