//! Group wiki page revisions (task P5) — the wiki_pages_api revision routes
//! in a Group context.
//!
//! Routes:
//!   - GET  /groups/{groupId}/pages/{urlOrId}/revisions           (revisions)
//!   - GET  /groups/{groupId}/pages/{urlOrId}/revisions/latest    (show_revision)
//!   - GET  /groups/{groupId}/pages/{urlOrId}/revisions/{revisionId}
//!   - POST /groups/{groupId}/pages/{urlOrId}/revisions/{revisionId} (revert)
//!
//! Canvas serves group wikis through the same pages API, but wiki pages are
//! course-scoped in this schema (`pages` has no group column), so — exactly
//! like the crate's other /groups/{groupId} wiki routes (see `lib.rs` and
//! tests/pages_groups.rs) — these routes run Canvas' context check (404 for
//! an unknown group) and the read/manage gates (group member read, parent
//! course teacher manage, admin both), then decline with 403 because no page
//! can belong to a group context.
//!
//! Revisions themselves are real for course pages: the `page_revisions`
//! table (migration 0040) stores one revision per page create/update and the
//! course routes list/show/revert them. No group-context revision table is
//! added: group page CRUD declines in this schema, so such a table could
//! never be populated.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{ensure_group, ensure_group_manage_access, ensure_group_read_access};

// ---------------------------------------------------------------------------
// Handlers (declined: no group-scoped wiki pages in this schema)
// ---------------------------------------------------------------------------

/// GET /groups/{groupId}/pages/{urlOrId}/revisions — Canvas
/// wiki_pages_api_controller#revisions in a group context (declined).
async fn list_group_page_revisions(
    State(state): State<AppState>,
    Path((group_id, _page_ref)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<Vec<crate::RevisionSummary>>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

/// GET /groups/{groupId}/pages/{urlOrId}/revisions/latest — Canvas
/// wiki_pages_api_controller#show_revision (latest) in a group context
/// (declined).
async fn show_group_latest_revision(
    State(state): State<AppState>,
    Path((group_id, _page_ref)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<crate::RevisionDetail>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

/// GET /groups/{groupId}/pages/{urlOrId}/revisions/{revisionId} — Canvas
/// wiki_pages_api_controller#show_revision in a group context (declined).
async fn show_group_revision(
    State(state): State<AppState>,
    Path((group_id, _page_ref, _revision_ref)): Path<(i64, String, String)>,
    auth: AuthUser,
) -> Result<Json<crate::RevisionDetail>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

/// POST /groups/{groupId}/pages/{urlOrId}/revisions/{revisionId} — Canvas
/// wiki_pages_api_controller#revert in a group context (declined; Canvas
/// requires :read_revisions and :update, i.e. group manage rights).
async fn revert_group_page(
    State(state): State<AppState>,
    Path((group_id, _page_ref, _revision_ref)): Path<(i64, String, String)>,
    auth: AuthUser,
) -> Result<Json<crate::RevisionDetail>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_manage_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// P5 gap routes; merged into the crate router by `lib.rs`.
pub fn page_revisions_gaps_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/groups/{groupId}/pages/{urlOrId}/revisions",
            get(list_group_page_revisions),
        )
        .route(
            "/groups/{groupId}/pages/{urlOrId}/revisions/latest",
            get(show_group_latest_revision),
        )
        .route(
            "/groups/{groupId}/pages/{urlOrId}/revisions/{revisionId}",
            get(show_group_revision).post(revert_group_page),
        )
}
