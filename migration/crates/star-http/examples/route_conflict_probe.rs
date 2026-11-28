//! Find which crate's router collides with the rest.
//
//! axum 0.8 panics when two routes conflict, and the panic names only the
//! *path*, not the crate. This probe answers the question directly: it merges
//! the API routers in order and reports which merge fails, then — if that is
//! not enough — probes each crate on its own to find the two that collide.
//!
//! ```sh
//! cargo run --bin route_conflict_probe
//! ```
//!
//! See `docs/knowledge/traps/axum-merge-semantics.md` and
//! `docs/knowledge/traps/double-merge-star-content.md`.

use axum::Router;
use star_common::AppState;

macro_rules! try_merge {
    ($acc:expr, $name:expr, $r:expr) => {{
        let acc = $acc;
        let merged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| acc.merge($r)));
        match merged {
            Ok(r) => r,
            Err(_) => {
                println!("CONFLICT while merging {}", $name);
                std::process::exit(1);
            }
        }
    }};
}

fn main() {
    // Keep the default hook so the panic message (which names the path) is
    // printed before the probe's own line.
    let mut r: Router<AppState> = Router::new();
    r = try_merge!(r, "star_auth", star_auth::router());
    r = try_merge!(r, "star_users", star_users::router());
    r = try_merge!(r, "star_courses", star_courses::router());
    r = try_merge!(r, "star_assignments", star_assignments::router());
    r = try_merge!(r, "star_content", star_content::router());
    r = try_merge!(r, "star_files", star_files::router());
    r = try_merge!(r, "star_enrollments", star_enrollments::router());
    r = try_merge!(r, "star_dashboard", star_dashboard::router());
    r = try_merge!(r, "star_quizzes", star_quizzes::router());
    r = try_merge!(
        r,
        "star_assignment_groups",
        star_assignment_groups::router()
    );
    r = try_merge!(r, "star_accounts", star_accounts::router());
    r = try_merge!(r, "star_calendar", star_calendar::router());
    r = try_merge!(r, "star_conversations", star_conversations::router());
    r = try_merge!(r, "star_discussions", star_discussions::router());
    r = try_merge!(
        r,
        "star_grading_standards",
        star_grading_standards::router()
    );
    r = try_merge!(r, "star_groups", star_groups::router());
    r = try_merge!(r, "star_notifications", star_notifications::router());
    r = try_merge!(r, "star_outcomes", star_outcomes::router());
    r = try_merge!(r, "star_rubrics", star_rubrics::router());
    r = try_merge!(
        r,
        "star_submission_comments",
        star_submission_comments::router()
    );
    r = try_merge!(r, "star_jobs", star_jobs::router());
    r = try_merge!(r, "star_lti", star_lti::router());
    r = try_merge!(r, "star_permissions", star_permissions::router());
    r = try_merge!(r, "star_features", star_features::router());
    r = try_merge!(r, "star_learning_dates", star_learning_dates::router());
    r = try_merge!(r, "star_audit", star_audit::router());
    r = try_merge!(r, "star_media", star_media::router());
    r = try_merge!(r, "star_account_reports", star_account_reports::router());
    r = try_merge!(r, "star_collaborations", star_collaborations::router());
    r = try_merge!(r, "star_conferences", star_conferences::router());
    r = try_merge!(r, "star_eportfolios", star_eportfolios::router());
    r = try_merge!(r, "star_polling", star_polling::router());
    r = try_merge!(r, "star_search", star_search::router());
    r = try_merge!(r, "star_sis", star_sis::router());
    // star_pages / star_modules / star_announcements arrive through
    // star_content::router() — merging them again would double-register
    // every route in those crates.
    r = try_merge!(r, "star_graphql", star_graphql::router());

    let _ = r;
    println!("no conflict in the API router");
    println!("now try the application router: crates/star-http/tests/router_conflicts.rs");
}
