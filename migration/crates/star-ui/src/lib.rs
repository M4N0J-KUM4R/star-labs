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
//! double-submit protected and honour Rails' hidden `_method` field the way
//! Rack::MethodOverride does (`method_override`); `/api/v1` is untouched.

pub mod assets;
pub mod env;
pub mod handlers;
pub mod method_override;
pub mod queries;
pub mod session;

use axum::middleware;
use axum::routing::{get, post};
use axum::Router;

use star_common::AppState;

/// The shell's page table. Serve it with [`app`], which adds the `_method`
/// override; star-http and the integration tests all go through `app`.
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
        // Canvas has NO GET on this path. `resources :quizzes, except: :new`
        // (config/routes.rb:447) drops the Rails-generated form page and the
        // line right above it replaces it with `post "quizzes/new" =>
        // quizzes#new` (:446, comment: "use POST instead of GET (not
        // idempotent)"), and QuizzesController#new really does create
        // (`@context.quizzes.build` .. `quiz.save!`, then redirect to the quiz
        // edit url — app/controllers/quizzes/quizzes_controller.rb:343-357).
        // So the index "+ Quiz" affordance is a POST form, not a link.
        .route(
            "/courses/{id}/quizzes/new",
            post(handlers::quiz::quiz_create),
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
        // Canvas' web assignments routes come from `resources :assignments do`
        // (config/routes.rb:270) with no `only:`/`except:`, i.e. the full 7:
        // GET ..../assignments/new and GET ..../assignments/:id/edit are form
        // PAGES, while create is POST ..../assignments and update is
        // PATCH/PUT ..../assignments/:id. An HTML form cannot submit PATCH, so
        // Rails' helpers post to the member path with a hidden `_method`
        // field and Rack::MethodOverride rewrites the verb before routing —
        // that shim is `method_override`, applied by `app` ahead of this
        // table, and it is why no verb here is invented: `/new` and `/edit`
        // answer GET only, and a POST to either is a method mismatch.
        .route(
            "/courses/{id}/assignments",
            get(handlers::assignments::assignments_index)
                .post(handlers::assignments::assignment_create),
        )
        .route(
            "/courses/{id}/assignments/new",
            get(handlers::assignments::assignment_new),
        )
        .route(
            "/courses/{id}/assignments/{aid}",
            get(handlers::assignments::assignment_show)
                .patch(handlers::assignments::assignment_update)
                .put(handlers::assignments::assignment_update),
        )
        .route(
            "/courses/{id}/assignments/{aid}/edit",
            get(handlers::assignments::assignment_edit),
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
        // tab route. Canvas' web routes: `concerns :announcements`
        // (config/routes.rb:266/:702/:796) expands `resources :announcements`
        // (:154-156, no `only:`/`except:`) under the course scope, giving
        // GET/POST /courses/:cours_id/announcements and GET .../:id; the
        // syllabus page is the `collection { get :syllabus }` inside
        // `resources :assignments` (:344-346). Write verbs Canvas reserves for
        // those rows (DELETE .../:id, plus PUT/PATCH) now reach this port
        // through `crate::method_override`, but these server-rendered forms do
        // not offer them, so they are not registered.
        //
        // `GET .../announcements/new` is NOT Canvas. parity.py expands the
        // `/new` and `/edit` routes `resources` draws (see its
        // SERVED_FORM_PAGES) but gates each on the controller action and what
        // it renders, and AnnouncementsController (app/controllers/
        // announcements_controller.rb:21) defines only `index` (:64) and
        // `show` (:105) — app/views/announcements holds only index.html.erb.
        // So a request to Canvas' /courses/:id/announcements/new is a 500, and
        // this row reads `absent` in the extras list for the real reason: it
        // is our page, not a port. Canvas edits an announcement through
        // `discussion_topics#new`/`#edit` with `params[:is_announcement]`
        // (discussion_topics_controller.rb:578/:585); carrying that model over
        // — which would replace this page — is the discussion-topics lane.
        // `POST .../announcements` is the same story one verb over: Rails
        // draws the create route, `AnnouncementsController#create` does not
        // exist, so the scoreboard's `web_exact` there proves routing, not a
        // served page.
        // `/assignments/syllabus` and `/files/quota` did look unmatched only
        // because the extractor drew `collection do` symbols on the member
        // path; both are Canvas-real collection routes and now read web_exact.
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
            "/courses/{id}/assignments/syllabus",
            get(handlers::announcements::syllabus_show),
        )
        // Files journey (issue #110) — BEFORE the generic tab route. Canvas
        // serves the course files list and one file's page at these web paths
        // (the `:files` concern, `resources :files`, config/routes.rb:114-129).
        // Its POST sibling targets `files#create`, which FilesController does
        // not implement — uploads really go through
        // `POST /api/v1/courses/:course_id/files` => `courses#create_file` with
        // a multipart attachment — and `/download` streams the stored bytes.
        // This workspace keeps file metadata with no byte store, so neither
        // could answer the way Canvas does; the views stay read-only and the
        // templates carry no upload or download control.
        .route("/courses/{id}/files", get(handlers::files::files_index))
        .route("/courses/{id}/files/{fid}", get(handlers::files::file_show))
        .route("/courses/{id}/{tab}", get(handlers::course::course_tab))
        // Calendar journey (issue #111) — global page, sibling of /dashboard.
        // Canvas' web calendar is read-only at this path
        // (`get "calendar" => "calendars#show"`, config/routes.rb:1103); event
        // writes live under `resources :calendar_events` in the per-context
        // scopes (`/users/:user_id/calendar_events`, `/courses/:cours_id/...`),
        // not at `/calendar` — so no create or delete is registered here.
        .route("/calendar", get(handlers::calendar::calendar_index))
        .route("/static/{hash}/{file}", get(assets::serve_static))
}

/// The shell as it is actually served: the page table behind Rack's `_method`
/// override.
///
/// The override is NOT a `.layer()` on [`router`]. axum's `Router::layer`
/// forwards to `MethodRouter::layer`, which wraps each per-verb handler and so
/// runs *after* the verb has been selected — a `POST` to a `PATCH`-only member
/// path is answered with 405 before such a layer ever sees the request. Passing
/// the table to `fallback_service` makes it one whole-router service, and a
/// layer over that runs before any routing, which is where
/// `Rack::MethodOverride` sits in Canvas' stack (`config/application.rb`).
pub fn app(state: AppState) -> Router {
    Router::new()
        .fallback_service(router().with_state(state))
        .layer(middleware::from_fn(method_override::method_override))
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
