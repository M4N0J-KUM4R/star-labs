//! star-content — thin aggregator crate.
//!
//! Re-exports and merges routers from the compilable content sub-crates:
//! - `star_pages::router()`       — `/courses/{courseId}/pages`, `/pages/{id}`
//! - `star_announcements::router()` — `/courses/{courseId}/announcements`, `/announcements/{id}`
//! - `star_modules::router()`      — `/courses/{courseId}/modules`, `/modules/{id}`
//!
//! The sub-crates own all handlers and domain logic; this crate only
//! provides a single merged `Router<AppState>` for inclusion in `star-http`.

use axum::Router;
use star_common::AppState;

/// Merge content-domain routers into one `Router<AppState>`.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(star_pages::router())
        .merge(star_announcements::router())
        .merge(star_modules::router())
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
