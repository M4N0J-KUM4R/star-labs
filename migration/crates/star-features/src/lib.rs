//! star-features — Canvas feature flags (`feature_flags_api`).
//! Scaffolded for Phase 4; routes are added by the feature-flags agent.

use axum::Router;
use star_common::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}