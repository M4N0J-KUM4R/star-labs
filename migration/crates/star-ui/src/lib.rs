//! star-ui — the server-rendered UI shell for star-lms (spec §2).
//!
//! Merged at the TOP LEVEL of star-http's router (sibling of `/health`, NOT
//! under `/api/v1`): `/`, `/login` + `/login/canvas`, `/logout`, `/dashboard`,
//! `/courses/{id}`, `/courses/{id}/{tab}` and `/static/{hash}/{file}`, plus
//! the discussions + inbox journey routes (#72):
//! `/courses/{id}/discussion_topics[...]` and `/conversations[...]`, plus the
//! assignments + gradebook journey routes (issue #71):
//! `/courses/{id}/assignments[...]`, `/courses/{id}/grades`,
//! `/courses/{id}/gradebook[.csv]`, plus the SpeedGrader grading workflow
//! (issue #108): `/courses/{id}/gradebook/speed_grader` and Canvas'
//! `POST /courses/{id}/gradebook/update_submission`.
//!
//! Specific journey routes are registered BEFORE the generic
//! `/courses/{id}/{tab}` so the "assignments"/"grades"/"discussion_topics" tab
//! segments hit the journey pages, not the generic tab landing.
//!
//! Auth is the `star_session` cookie (HttpOnly, SameSite=Lax) verified by
//! the exact same path as the API's Bearer `AuthUser`; shell POSTs are CSRF
//! double-submit protected and honour Rails' hidden `_method` field the way
//! Rack::MethodOverride does (`method_override`); `/api/v1` is untouched.

pub mod assets;
pub mod embed;
pub mod env;
pub mod handlers;
pub mod method_override;
pub mod queries;
pub mod session;
pub mod session_bridge;
pub mod shell;

use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;

use star_common::AppState;

/// The shell's page table. Serve it with [`app`], which adds the `_method`
/// override; star-http and the integration tests all go through `app`.
///
/// Route-order note: axum 0.8 matches STATIC segments before path parameters
/// at the same depth, so `/courses/{id}/discussion_topics` (and the quiz
/// routes)
/// win over `/courses/{id}/{tab}` regardless of registration order; the
/// specific routes are still registered first to keep that property local
/// and visible.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(handlers::login::root))
        // Canvas splits the login surface in two: the page is
        // `get "login" => "login#new"` (config/routes.rb:928, app/views/login/
        // new.html.erb) and the credential POST is the auth-method route
        // `post "login/canvas" => "login/canvas#create"` (:935,
        // app/controllers/login/canvas_controller.rb:53, which is the one that
        // verifies the authenticity token and resets the session id). Canvas
        // answers no POST at /login itself, so neither do we.
        .route("/login", get(handlers::login::login_form))
        .route("/login/canvas", post(handlers::login::login_submit))
        // `delete "logout" => "login#destroy"` (config/routes.rb:930,
        // login_controller.rb:84) is the session-ending verb; Canvas' other
        // logout row is `get "logout" => "login#logout_landing"` (:931), a
        // "please log out" page this port does not have. Rails' `button_to
        // …, method: :delete` reaches the verb through `_method`, which is how
        // the nav form below submits.
        .route("/logout", delete(handlers::login::logout))
        // Canvas' signup is two routes, not one. The page is
        // `get "register" => "users#new"` (canvas config/routes.rb:1009) with
        // `register_from_website` (:1010) as a second name for the same page;
        // the form then submits to `POST /users`, from
        // `resources :users, except: [:destroy, :index]` (:1016) —
        // UsersController#create (users_controller.rb:1729), which is
        // `create_user`, the same action the deprecated
        // `create_self_registered_user` (:1771) delegates to. Canvas has no
        // POST on /register itself, so this table registers none.
        .route("/register", get(handlers::auth::register_form))
        .route("/register_from_website", get(handlers::auth::register_form))
        .route("/users", post(handlers::auth::register_submit))
        // Both of these are `match … :via => [:get, :post]` in Canvas
        // (config/routes.rb:76 for forgot_password), which is why parity.py
        // shows their closest verb as `MATCH`: two verbs on one row, exactly
        // the pair registered here. Nothing in routes.rb serves a
        // `/reset_password` path — Canvas' token link is
        // `get|post "pseudonyms/:pseudonym_id/change_password/:nonce"` (:77-78),
        // keyed on a Pseudonym plus a CommunicationChannel confirmation code.
        // This port issues its own single-use token instead
        // (`password_reset_tokens`, migration 0222), which
        // `forgot_password_submit` puts in the link it prints (auth.rs:364) and
        // `reset_password.html` posts back to, so the token-in-the-path page
        // stays ours and the verbs stay Canvas' `match` pair.
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
        // the generic `/courses/{id}/{tab}` catch-all below. The paths are
        // Canvas' own: `concern :discussions` draws
        // `resources :discussion_topics, only: %i[index new show edit destroy]`
        // (config/routes.rb:156-165) inside the course scope, and the tabs API
        // emits that same html_url (`DEFAULT_COURSE_TABS` ("discussions",
        // "Discussions", "/discussion_topics")). Canvas has no `/discussions`
        // spelling at all, so the alias this port carried is gone.
        //
        // The index and show pages are real HTML: discussion_topics#index ends
        // in `js_bundle :discussion_topics_index` + `render html: "", layout:
        // true` (app/controllers/discussion_topics_controller.rb:531-534),
        // #new delegates to #edit (:578-583) which ends the same way
        // (:765-768), and #show posts through `js_bundle
        // :discussion_topics_post` (:970-977).
        //
        // The two POSTs sit on Canvas paths but are not Canvas web rows:
        // `resources … only:` never draws create, and replies live in the API
        // scope (`post "…/discussion_topics/:topic_id/entries" =>
        // discussions_api#add_entry`, config/routes.rb:1658). Canvas' own
        // editors are React and send those writes as JSON with an
        // `X-CSRF-Token` header; a server-rendered form can only post
        // urlencoded, so these two are the form targets this page needs on the
        // URLs Canvas' controllers answer the same writes at.
        .route(
            "/courses/{id}/discussion_topics",
            get(handlers::discussions::discussions_index)
                .post(handlers::discussions::discussion_create),
        )
        .route(
            "/courses/{id}/discussion_topics/new",
            get(handlers::discussions::discussion_new),
        )
        .route(
            "/courses/{id}/discussion_topics/{topic_id}",
            get(handlers::discussions::discussion_show),
        )
        .route(
            "/courses/{id}/discussion_topics/{topic_id}/entries",
            post(handlers::discussions::discussion_reply),
        )
        // Inbox journey (#72).
        .route(
            "/conversations",
            get(handlers::conversations::conversations_index)
                .post(handlers::conversations::conversation_create),
        )
        // Canvas has no compose URL: `resources :conversations, only: %i[index
        // show update create destroy]` (config/routes.rb:61) plus the scope
        // rows above it, and app/views/conversations holds only
        // index_new.html.erb — the React inbox opens compose as a dialog in
        // that one page. Our inbox is server-rendered, so compose is a page of
        // its own, linked from conversations_index.html's Compose control.
        .route(
            "/conversations/compose",
            get(handlers::conversations::conversation_compose),
        )
        // `post "conversations/mark_all_as_read" => "conversations#mark_all_as_read"`
        // (config/routes.rb:59) — Canvas' spelling, not "mark_all_read".
        .route(
            "/conversations/mark_all_as_read",
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
        // Canvas: `resource :profile, only: [:show, :update]` (config/routes.rb:1045)
        // → GET /profile. Served as the Svelte shell (see `shell`), the first lane on
        // this branch to do so; the tabs API already emits this html_url.
        .route("/profile", get(handlers::profile::profile))
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
            get(handlers::quiz::quiz_show)
                .patch(handlers::quiz::quiz_update)
                .put(handlers::quiz::quiz_update),
        )
        // The verbs above are Canvas' `resources :quizzes` member set
        // (config/routes.rb:447): GET is the show page, PATCH/PUT is the
        // editor save (`quizzes#update`, quizzes_controller.rb:493 — gated by
        // `authorized_action(@quiz, @current_user, :update)`, HTML branch
        // flashes then redirects to the show url at :612-613). The browser
        // form reaches PATCH through Rails' hidden `_method` field, which
        // `method_override` rewrites, so no POST verb is registered here.
        //
        // `/edit` is the editor page itself: `quizzes#edit` (:359) is gated by
        // the same :update right and ends in `render :new` (:431), whose
        // template app/views/quizzes/quizzes/new.html.erb exists — so Canvas
        // really does serve it, and `quiz_create` redirects to it (:352-355).
        // Our page is the server-rendered equivalent of the React mount that
        // template boots (`js_bundle :quizzes`, :429).
        .route(
            "/courses/{id}/quizzes/{qid}/edit",
            get(handlers::quiz::quiz_edit),
        )
        // Canvas' take row is `match "take" => "quizzes/quizzes#show",
        // :take => "1", :via => [:get, :post]` (config/routes.rb:475) — the
        // `MATCH` parity.py reports is that one row carrying two verbs, and
        // GET+POST is exactly what Canvas allows there (the POST is the
        // "start a new attempt" submit of the same page). The page is real
        // HTML: app/views/quizzes/quizzes/take_quiz.html.erb exists and
        // quizzes_controller.rb:289,:317 branch on `params[:take]`.
        .route(
            "/courses/{id}/quizzes/{qid}/take",
            get(handlers::quiz::quiz_take).post(handlers::quiz::quiz_take_start),
        )
        .route(
            "/courses/{id}/quizzes/{qid}/submissions/{sid}",
            get(handlers::quiz::quiz_results),
        )
        // The take page's "Submit Quiz" form. Canvas posts it to the
        // collection, not to a member `/submit` tail:
        // `form_tag @quiz_presenter.form_action(...)`
        // (app/views/quizzes/quizzes/take_quiz.html.erb:66) resolves to
        // `course_quiz_quiz_submissions_path`
        // (app/presenters/quizzes/take_quiz_presenter.rb:167-168), i.e. the
        // `resources :quiz_submissions, path: :submissions` create row under
        // `resources :quizzes` (config/routes.rb:447, :456), and
        // QuizSubmissionsController#create finds the attempt by the *current
        // user* rather than by an id in the path
        // (app/controllers/quizzes/quiz_submissions_controller.rb:60-63). This
        // port does the same lookup, so nothing here invents a route.
        .route(
            "/courses/{id}/quizzes/{qid}/submissions",
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
        // Canvas' student submit is the nested submissions create row:
        // `resources :assignments do … resources :submissions do`
        // (config/routes.rb:270, :301), and SubmissionsController#create ends
        // its HTML branch with `redirect_to(course_assignment_url(...))`
        // (app/controllers/submissions_controller.rb:232 and the redirects in
        // that action), so the form target is the collection path with no
        // `/submit` tail.
        .route(
            "/courses/{id}/assignments/{aid}/submissions",
            post(handlers::assignments::assignment_submit),
        )
        // Grades + gradebook journey (issue #71) — BEFORE the generic tab route.
        .route("/courses/{id}/grades", get(handlers::grades::grades))
        .route("/courses/{id}/gradebook", get(handlers::grades::gradebook))
        // Canvas' CSV export is an async job, not a download URL:
        // `resource :gradebook_csv, only: [:create]` (config/routes.rb:253)
        // hands the request to GradebookCsvsController#create, which queues
        // `gradebook_to_csv_in_background` and `render json:` the Progress
        // (app/controllers/gradebook_csvs_controller.rb:24-46) for the React
        // gradebook to poll (gradebooks_controller.rb:591 passes that URL as
        // `export_gradebook_csv_url`). Producing the bytes needs an attachment
        // store this workspace does not have — the same limit that keeps the
        // files pages read-only — so the export stays a direct download, on the
        // `.csv` web idiom Canvas itself uses for a synchronous CSV:
        // `get "observer_pairing_codes.csv"` → `send_data res, type: "text/csv"`
        // (config/routes.rb:205, app/controllers/courses_controller.rb:1975,
        // :2000). The link is gradebook.html's `#gradebook_csv_link`.
        .route(
            "/courses/{id}/gradebook.csv",
            get(handlers::grades::gradebook_csv),
        )
        // Settings + people journey (issue #73) — BEFORE the generic tab
        // route. The people page is the tabs API "people" html_url
        // (`/courses/{id}/users`, Canvas' `get "users" => "context#roster"`,
        // config/routes.rb:148); the write forms below are the teacher-only
        // mutations.
        //
        // The settings page is `get :settings` (config/routes.rb:228) and
        // Canvas saves it through `put "courses/:course_id/settings"`
        // (:1234 → CoursesController#update_settings); there is no POST on
        // that path anywhere in routes.rb. The form here is server-rendered,
        // so it posts to the member path with Rails' hidden `_method=put` and
        // lands on the verb Canvas answers — the same shape the assignment and
        // quiz editors above use.
        .route(
            "/courses/{id}/settings",
            get(handlers::settings::course_settings).put(handlers::settings::course_settings_save),
        )
        .route(
            "/courses/{id}/users",
            get(handlers::settings::course_people),
        )
        // Canvas has no add-people *page* URL: the roster page's "Start a new
        // enrollment" is a React modal inside `context#roster`
        // (app/views/context/roster.html.erb), which is why nothing in
        // canvas-src references a `/users/add` path. Our roster form is a
        // page, so this GET is ours; its submit is not — it goes to Canvas'
        // own add-users-to-the-course web row below.
        .route(
            "/courses/{id}/users/add",
            get(handlers::settings::course_people_add),
        )
        // `post :enroll_users` (config/routes.rb:231) is Canvas' web route for
        // enrolling people into a course — CoursesController#enroll_users
        // (app/controllers/courses_controller.rb:2870) reads the role, the
        // section and `params[:user_list]` and creates the enrollments,
        // answering JSON to the JS that calls it. Our form posts the same
        // write here and gets the PRG redirect the shell uses everywhere.
        .route(
            "/courses/{id}/enroll_users",
            post(handlers::settings::course_people_add_submit),
        )
        // Conclude and reactivate are Canvas' enrollment-state rows:
        // `delete "conclude_user/:id" => "courses#conclude_user"` and
        // `post "unconclude_user/:id" => "courses#unconclude_user"`
        // (config/routes.rb:215-216). Canvas' `:id` is the *enrollment* id —
        // both actions do `@context.enrollments.find(params[:id])`
        // (app/controllers/courses_controller.rb:2808, :2824) — so this port
        // carries the enrollment id in that slot too and resolves the user
        // from it, and the conclude form submits Rails'
        // `method: :delete` shape through `method_override`.
        .route(
            "/courses/{id}/conclude_user/{enrollment_id}",
            delete(handlers::settings::course_people_conclude),
        )
        .route(
            "/courses/{id}/unconclude_user/{enrollment_id}",
            post(handlers::settings::course_people_reactivate),
        )
        // SpeedGrader grading workflow (issue #108) — BEFORE the generic tab
        // route, sibling of the gradebook list view (#71).
        //
        // Canvas' page is the `collection { get :speed_grader }` on
        // `resource :gradebook` (config/routes.rb:234, :242);
        // GradebooksController#speed_grader (:1120) renders `:speed_grader`
        // (:1360), so the URL has an underscore and no trailing `/grade`
        // segment. The grade save is the sibling `post :update_submission`
        // (:245), whose HTML branch redirects to the gradebook
        // (app/controllers/gradebooks_controller.rb:897, :1010).
        .route(
            "/courses/{id}/gradebook/speed_grader",
            get(handlers::speedgrader::speedgrader),
        )
        .route(
            "/courses/{id}/gradebook/update_submission",
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
        // Content-addressed asset path, and deliberately not a Canvas URL:
        // every Askama page still links `app.css` here (`assets::app_css_url()`
        // is the `css_url` field of ~20 page renders, and `serve_static`
        // answers it from the embedded bytes with `immutable` caching), while
        // the shell's own bundle is served off `/fallback` under `/_app` and
        // `/canvas`. Deleting this row would strip the stylesheet from every
        // server-rendered page at once.
        .route("/static/{hash}/{file}", get(assets::serve_static))
        // Everything the front end ships that is not a page: the Vite bundle under
        // /_app, the Canvas sheets under /canvas, and the fonts and images those
        // sheets reference by root-relative path. A fallback rather than a route
        // because these are assets, not pages — parity.py counts `.route()` calls as
        // the app's route table, and `/fonts/**` has no business in it. Unmatched
        // paths still 404 (`assets::serve_embedded` answers a bare 404, byte-identical
        // to axum's default fallback).
        .fallback(assets::serve_embedded)
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

/// The shell, with `fallback` answering the paths the shell does not own.
///
/// The shell is the application router's fallback service, so the paths it
/// does not register are the ones the front door has to answer: without a
/// fallback of its own the shell would swallow them.
pub fn app_with_fallback<H, T>(state: AppState, fallback: H) -> Router
where
    H: axum::handler::Handler<T, ()>,
    T: 'static,
{
    Router::new()
        .fallback_service(router().with_state(state).fallback(fallback))
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
