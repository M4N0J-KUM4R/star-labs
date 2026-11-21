//! star-collaborations — course/group collaborations and collaborators (Canvas
//! CollaborationsApiController) — read + delete API surface.
//!
//! Scaffold crate (issue #138): the router is wired into
//! `migration/Cargo.toml`, `star-http/src/main.rs`, the workflow shard matrix
//! and `tools/ci/shard_coverage.py` — feature workers add their routes in
//! `collaborations_api` without needing to touch any of those files.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use axum::Router;
use star_common::AppState;

/// Domain router for star-collaborations.
pub fn router() -> Router<AppState> {
    Router::new().merge(collaborations_api::router())
}

mod collaborations_api;
