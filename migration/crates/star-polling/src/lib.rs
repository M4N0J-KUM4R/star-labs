//! star-polling — polls, poll choices, poll sessions and poll submissions (Canvas
//! PollingApiController, PollChoicesApiController, PollSessionsApiController,
//! PollSubmissionsApiController) — the in-class "clicker" feature.
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

mod poll_sessions;

mod polls;

/// Domain router for star-polling.
///
/// Empty on purpose (scaffold train, issue #138). Routes from the spec
/// (`specs/p5-rest-polling.md` §2) are merged here by the feature workers; the
/// `Router::new()` base already gives unmounted paths the standard 404.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(poll_sessions::router())
        .merge(polls::router())
}
