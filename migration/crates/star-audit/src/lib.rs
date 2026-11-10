//! star-audit — Canvas grade-change and course audit logs
//! (`grade_change_audit_api`, `course_audit_api`).
//! Scaffolded for Phase 4; routes are added by the grade-change-audit agent.

use axum::Router;
use star_common::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
}