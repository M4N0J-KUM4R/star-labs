//! star-content — content domain crate.
//!
//! Re-exports and merges routers from the compilable content sub-crates:
//! - `star_pages::router()`       — `/courses/{courseId}/pages`, `/pages/{id}`
//! - `star_announcements::router()` — `/courses/{courseId}/announcements`, `/announcements/{id}`
//! - `star_modules::router()`      — `/courses/{courseId}/modules`, `/modules/{id}`
//!
//! and adds the content_exports routes owned by this crate:
//! - `/courses/{courseId}/content_exports`      — list / create
//! - `/courses/{courseId}/content_exports/{id}` — Canvas v1 nested show
//! - `/content_exports/{id}`                    — flat show
//!
//! plus the content_migrations routes (`content_migrations.rs`):
//! - `/courses/{courseId}/content_migrations`            — list / create
//! - `/courses/{courseId}/content_migrations/{id}`       — nested show / update
//! - `/courses/{courseId}/content_migrations/{id}/selective_data` — selective import tree
//! - `/courses/{courseId}/content_migrations/migrators`  — available migrators
//! - `/content_migrations/{id}`                          — flat show
//! - `/content_migrations/migrators`                     — flat migrators
//!
//! and the migration_issues routes (`migration_issues.rs`), one Canvas route
//! block per context (course, account, group, user):
//! - `/{context}/{contextId}/content_migrations/{id}/migration_issues`            — list / create
//! - `/{context}/{contextId}/content_migrations/{id}/migration_issues/{issueId}` — show / update
//!
//! where `{context}` is one of `courses`, `accounts`, `groups`, `users`.
//!
//! The ePub and web zip export routes (`epub_exports.rs`, `web_zip_exports.rs`)
//! and the deprecated course-copy API (`content_imports.rs`) live here as well:
//! - `/epub_exports`                                  — courses with their latest export
//! - `/courses/{courseId}/epub_exports`               — create an ePub export
//! - `/courses/{courseId}/epub_exports/{id}`          — show an ePub export
//! - `/courses/{courseId}/web_zip_exports`            — list web zip exports
//! - `/courses/{courseId}/web_zip_exports/{id}`       — show a web zip export
//! - `/courses/{courseId}/course_copy`                — start a course copy
//! - `/courses/{courseId}/course_copy/{id}`           — the status of a course copy
//!
//! The sub-crates own all handlers and domain logic; this crate only
//! provides a single merged `Router<AppState>` for inclusion in `star-http`.

pub mod block_editor_templates;
pub mod content_exports;
pub mod content_imports;
pub mod content_migrations;
pub mod content_shares;
pub mod epub_exports;
pub mod migration_issues;
pub mod web_zip_exports;

use axum::Router;
use star_common::AppState;

/// Merge content-domain routers into one `Router<AppState>`.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(star_pages::router())
        .merge(star_announcements::router())
        .merge(star_modules::router())
        .merge(content_exports::router())
        .merge(content_migrations::router())
        .merge(content_shares::router())
        .merge(block_editor_templates::router())
        .merge(epub_exports::router())
        .merge(web_zip_exports::router())
        .merge(content_imports::router())
        .merge(migration_issues::router())
}

#[cfg(test)]
mod tests {
    use super::router;

    /// Verify the merged router builds and can be instantiated without panic.
    /// (Router::routes() introspection is not available in axum 0.8.9;
    /// fall back to a build-only verification as specified.)
    #[test]
    fn routes() {
        let _ = router();
    }
}
