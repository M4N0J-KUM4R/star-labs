//! star-collaborations — course/group collaborations and collaborators (Canvas
//! CollaborationsApiController) — read + delete API surface.
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

/// Domain router for star-collaborations.
pub fn router() -> Router<AppState> {
    Router::new().merge(collaborations_api::router())
}

mod collaborations_api;
