//! star-account-reports — account and course reports (Canvas AccountReportsApiController,
//! CourseReportsApiController) — the admin CSV report engine.
//!
//! Routes:
//! - the account reports API lives in `account_reports_api`
//!   (AccountReportsApiController, spec `specs/p5-rest-account-reports.md`);
//! - the course reports API lives in `course_reports_api`
//!   (CourseReportsController: last / create / show, migration 0241).
//!
//! Scaffold crate (issue #138): the router is intentionally EMPTY. Feature
//! workers implement the spec routes here afterwards; they must not need to
//! touch `migration/Cargo.toml`, `star-http/src/main.rs`, the workflow shard
//! matrix or `tools/ci/shard_coverage.py` — this crate is already wired into
//! all of them.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use axum::Router;
use star_common::AppState;

/// Domain router for star-account-reports.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(account_reports_api::router())
        .merge(course_reports_api::router())
}

mod account_reports_api;
mod course_reports_api;
