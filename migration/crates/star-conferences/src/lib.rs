//! star-conferences — web conferences and recordings (Canvas ConferencesApiController) over
//! course/group contexts plus the current-user listing and the
//! recording_ready provider webhook.
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

/// Domain router for star-conferences.
pub fn router() -> Router<AppState> {
    Router::new().merge(conferences_api::router())
}

mod conferences_api;
