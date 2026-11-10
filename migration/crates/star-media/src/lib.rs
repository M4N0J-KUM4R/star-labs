//! star-media — Canvas media objects and tracks (`media_objects_api`).
//! Scaffolded for Phase 4; routes are added by the media-objects agent.

use axum::Router;
use star_common::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}