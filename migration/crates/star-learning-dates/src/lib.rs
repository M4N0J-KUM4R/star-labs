//! star-learning-dates — Canvas learning object dates (`learning_object_dates_api`).
//! Scaffolded for Phase 4; routes are added by the learning-object-dates agent.

use axum::Router;
use star_common::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}
