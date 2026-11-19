//! star-account-reports — account and course reports (Canvas AccountReportsApiController,
//! CourseReportsApiController) — the admin CSV report engine.
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
///
/// Empty on purpose (scaffold train, issue #138). Routes from the spec
/// (`specs/p5-rest-account-reports.md` §2) are merged here by the feature workers; the
/// `Router::new()` base already gives unmounted paths the standard 404.
pub fn router() -> Router<AppState> {
    Router::new()
}
