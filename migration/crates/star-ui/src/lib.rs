//! star-ui — the server-rendered UI shell for star-lms (spec §2).
//!
//! Merged at the TOP LEVEL of star-http's router (sibling of `/health`, NOT
//! under `/api/v1`): `/`, `/login`, `/logout`, `/dashboard`,
//! `/courses/{id}`, `/courses/{id}/{tab}` and `/static/{hash}/{file}`, plus
//! the discussions + inbox journey routes (#72):
//! `/courses/{id}/discussions[...]` (alias `/discussion_topics`) and
//! `/conversations[...]`, plus the assignments + gradebook journey routes
//! (issue #71): `/courses/{id}/assignments[...]`, `/courses/{id}/grades`,
//! `/courses/{id}/gradebook[.csv]`, plus the SpeedGrader grading workflow
//! (issue #108): `/courses/{id}/gradebook/speedgrader[.../grade]`.
//!
//! Specific journey routes are registered BEFORE the generic
//! `/courses/{id}/{tab}` so the "assignments"/"grades"/"discussions" tab
//! segments hit the journey pages, not the generic tab landing.
//!
//! Auth is the `star_session` cookie (HttpOnly, SameSite=Lax) verified by
//! the exact same path as the API's Bearer `AuthUser`; shell POSTs are CSRF
//! double-submit protected; `/api/v1` is untouched.

pub mod assets;
pub mod env;
pub mod handlers;
pub mod queries;
pub mod session;

use axum::routing::{get, post};
use axum::Router;

use star_common::AppState;

/// The shell router. Mounted via `.merge(star_ui::router())` at the top level
/// of the app router in star-http.
///
/// Route-order note: axum 0.8 matches STATIC segments before path parameters
/// at the same depth, so `/courses/{id}/discussions` (and the quiz routes)
/// win over `/courses/{id}/{tab}` regardless of registration order; the
/// specific routes are still registered first to keep that property local
/// and visible.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(handlers::login::root))
        .route(
            "/login",
            get(handlers::login::login_form).post(handlers::login::login_submit),
        )
        .route("/logout", post(handlers::login::logout))
        .route(
            "/register",
            get(handlers::auth::register_form).post(handlers::auth::register_submit),
        )
        .route(
            "/forgot_password",
            get(handlers::auth::forgot_password_form).post(handlers::auth::forgot_password_submit),
        )
        .route(
            "/reset_password/{token}",
            get(handlers::auth::reset_password_form).post(handlers::auth::reset_password_submit),
        )
        .route("/dashboard", get(handlers::dashboard::dashboard))
        // Precision Workspace quick-jump command bar.
        .route("/quick-jump", get(handlers::quick_jump::quick_jump))
        // Discussions journey (#72) — the specific tab routes come before
        // the generic `/courses/{id}/{tab}` catch-all below. Both
        // /discussions and /discussion_topics serve the index: the latter is
        // the html_url the tabs API emits
        // (`DEFAULT_COURSE_TABS` ("discussions", "Discussions", "/discussion_topics")).
        .route(
            "/courses/{id}/discussions",
            get(handlers::discussions::discussions_index)
                .post(handlers::discussions::discussion_create),
        )
        .route(
            "/courses/{id}/discussion_topics",
            get(handlers::discussions::discussions_index),
        )
        .route(
            "/courses/{id}/discussions/new",
            get(handlers::discussions::discussion_new),
        )
        .route(
            "/courses/{id}/discussions/{topic_id}",
            get(handlers::discussions::discussion_show),
        )
        .route(
            "/courses/{id}/discussions/{topic_id}/entries",
            post(handlers::discussions::discussion_reply),
        )
        // Inbox journey (#72).
        .route(
            "/conversations",
            get(handlers::conversations::conversations_index)
                .post(handlers::conversations::conversation_create),
        )
        .route(
            "/conversations/compose",
            get(handlers::conversations::conversation_compose),
        )
        .route(
            "/conversations/mark_all_read",
            post(handlers::conversations::conversations_mark_all_read),
        )
        .route(
            "/conversations/{id}",
            get(handlers::conversations::conversation_show),
        )
        .route(
            "/conversations/{id}/add_message",
            post(handlers::conversations::conversation_reply),
        )
        .route("/courses/{id}", get(handlers::course::course_home))
        // Quiz-taking journey (issue #109) — specific before generic.
        .route("/courses/{id}/quizzes", get(handlers::quiz::quiz_list))
        .route(
            "/courses/{id}/quizzes/new",
            get(handlers::quiz::quiz_new).post(handlers::quiz::quiz_create),
        )
        .route(
            "/courses/{id}/quizzes/{qid}",
            get(handlers::quiz::quiz_show),
        )
        .route(
            "/courses/{id}/quizzes/{qid}/take",
            get(handlers::quiz::quiz_take).post(handlers::quiz::quiz_take_start),
        )
        .route(
            "/courses/{id}/quizzes/{qid}/submissions/{sid}",
            get(handlers::quiz::quiz_results),
        )
        .route(
            "/courses/{id}/quizzes/{qid}/submissions/{sid}/submit",
            post(handlers::quiz::quiz_submit),
        )
        // Assignments journey (issue #71) — BEFORE the generic tab route.
        .route(
            "/courses/{id}/assignments",
            get(handlers::assignments::assignments_index),
        )
        .route(
            "/courses/{id}/assignments/new",
            get(handlers::assignments::assignment_new)
                .post(handlers::assignments::assignment_create),
        )
        .route(
            "/courses/{id}/assignments/{aid}",
            get(handlers::assignments::assignment_show),
        )
        .route(
            "/courses/{id}/assignments/{aid}/edit",
            get(handlers::assignments::assignment_edit)
                .post(handlers::assignments::assignment_update),
        )
        .route(
            "/courses/{id}/assignments/{aid}/submit",
            post(handlers::assignments::assignment_submit),
        )
        // Grades + gradebook journey (issue #71) — BEFORE the generic tab route.
        .route("/courses/{id}/grades", get(handlers::grades::grades))
        .route("/courses/{id}/gradebook", get(handlers::grades::gradebook))
        .route(
            "/courses/{id}/gradebook.csv",
            get(handlers::grades::gradebook_csv),
        )
        // Settings + people journey (issue #73) — BEFORE the generic tab
        // route. The people page is the tabs API "people" html_url
        // (`/courses/{id}/users`); add/conclude/reactivate are the
        // teacher-only mutation forms.
        .route(
            "/courses/{id}/settings",
            get(handlers::settings::course_settings).post(handlers::settings::course_settings_save),
        )
        .route(
            "/courses/{id}/users",
            get(handlers::settings::course_people),
        )
        .route(
            "/courses/{id}/users/add",
            get(handlers::settings::course_people_add)
                .post(handlers::settings::course_people_add_submit),
        )
        .route(
            "/courses/{id}/users/{user_id}/conclude",
            post(handlers::settings::course_people_conclude),
        )
        .route(
            "/courses/{id}/users/{user_id}/reactivate",
            post(handlers::settings::course_people_reactivate),
        )
        // SpeedGrader grading workflow (issue #108) — BEFORE the generic tab
        // route, sibling of the gradebook list view (#71).
        .route(
            "/courses/{id}/gradebook/speedgrader",
            get(handlers::speedgrader::speedgrader),
        )
        .route(
            "/courses/{id}/gradebook/speedgrader/grade",
            post(handlers::speedgrader::speedgrader_grade),
        )
        // Announcements + syllabus journey (issue #111) — BEFORE the generic
        // tab route.
        .route(
            "/courses/{id}/announcements",
            get(handlers::announcements::announcements_index)
                .post(handlers::announcements::announcement_create),
        )
        .route(
            "/courses/{id}/announcements/new",
            get(handlers::announcements::announcement_new),
        )
        .route(
            "/courses/{id}/announcements/{aid}",
            get(handlers::announcements::announcement_show),
        )
        .route(
            "/courses/{id}/announcements/{aid}/delete",
            post(handlers::announcements::announcement_delete),
        )
        .route(
            "/courses/{id}/assignments/syllabus",
            get(handlers::announcements::syllabus_show)
                .post(handlers::announcements::syllabus_update),
        )
        // Files journey (issue #110) — BEFORE the generic tab route.
        .route(
            "/courses/{id}/files",
            get(handlers::files::files_index).post(handlers::files::files_create),
        )
        .route(
            "/courses/{id}/files/{fid}/download",
            get(handlers::files::file_download),
        )
        .route("/courses/{id}/files/{fid}", get(handlers::files::file_show))
        .route("/courses/{id}/{tab}", get(handlers::course::course_tab))
        // Calendar journey (issue #111) — global page, sibling of /dashboard.
        .route(
            "/calendar",
            get(handlers::calendar::calendar_index).post(handlers::calendar::calendar_create),
        )
        .route(
            "/calendar/{eid}/delete",
            post(handlers::calendar::calendar_delete),
        )
        .route("/static/{hash}/{file}", get(assets::serve_static))
}

/// Request origin for `ENV.DEEP_LINKING_POST_MESSAGE_ORIGIN`.
///
/// No host header is available without threading the request through every
/// handler, so this mirrors what the tests exercise: an http origin on the
/// local host. When a real deployment needs LTI deep-linking validation, this
/// should read the Host/X-Forwarded-Proto headers.
pub fn request_origin() -> String {
    "http://localhost".to_string()
}
