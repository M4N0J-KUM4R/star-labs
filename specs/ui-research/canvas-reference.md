# Canvas LMS UI Reference for star-lms (server-rendered Rust port)

Source of truth: sparse checkout of `instructure/canvas-lms` @ `1c9f0bb` (2026-04-30, master) at `/home/z/my-project/repos/canvas-lms`. Rust port being cross-checked: `/home/z/star-labs-ui` (axum crates merged under `/api/v1` by `star-http/src/main.rs::build_api_router`; `star-graphql` at `/api/graphql`).

This document condenses the server-rendered structure of each Canvas page group that maps to a star-lms issue. Canvas is JS-heavy (React over `window.ENV`); wherever a Canvas view is just a JS mount point, we document what data the ENV/REST layer carries so the askama port can render it server-side instead.

---

## 1. Shell / Layout Foundation (Rust #43)

### Routes & files
- Layout: `app/views/layouts/application.html.erb` (all `courses#...` HTML pages render into it), `layouts/_head.html.erb`, `layouts/_foot.html.erb`, plus `bare`, `mobile_auth`, `mobile_embed` for special pages (login uses `bare`).
- Global nav: `app/views/shared/_new_nav_header.html.erb`.
- Flash: `app/views/shared/_flash_notices.html.erb` + `shared/_static_notices`.
- Section tabs: helper `app/helpers/section_tab_helper.rb`; tab data from `Course#tabs_available` (`app/models/course.rb`).
- Breadcrumbs: `add_crumb(text, url)` calls in controllers; rendered via `render_crumbs` in the layout.

### application.html.erb structure (condensed)
```
<body class="<%= body_classes.uniq.join(" ") %>">
  <div role="alert" class="screenreader-only">  <!-- masquerade banner if @real_current_user != @current_user -->
  <%= render 'shared/static_notices' %>
  <%= render 'shared/flash_notices' %>          <!-- #flash_message_holder, #flash_screenreader_holder -->
  <div id="application" class="ic-app">
    <%= render 'shared/new_nav_header' %>       <!-- unless @headers == false -->
    <div id="instructure_ajax_error_box">...</div>
    <div id="wrapper" class="ic-Layout-wrapper">
      <% if crumbs.length > 1 %>
        <div class="ic-app-nav-toggle-and-crumbs no-print">
          <button id="courseMenuToggle" class="Button Button--link ic-app-course-nav-toggle">  <!-- if @show_left_side -->
          <div class="ic-app-crumbs"><%= render_crumbs %></div>
          <div class="right-of-crumbs">... student-view button, mount points ...</div>
        </div>
      <% end %>
      <div id="main" class="ic-Layout-columns">
        <div id="left-side" class="ic-app-course-menu ic-sticky-on list-view"
             style="display: <%= @collapse_course_menu ? "none" : "block" %>">
          <div id="sticky-container" class="ic-sticky-frame">
            <%= section_tabs %>                  <!-- or (yield :left_side) override -->
          </div>
        </div>
        <div id="not_right_side" class="ic-app-main-content">
          <div id="content-wrapper" class="ic-Layout-contentWrapper">
            <div id="content" class="ic-Layout-contentMain" role="main"><%= yield %></div>
          </div>
          <div id="right-side-wrapper" class="ic-app-main-content__secondary">
            <aside id="right-side"><%= right_side %></aside>
          </div>
        </div>
      </div>
      <%= render 'shared/canvas_footer' %>       <!-- if @show_footer -->
    </div>
  </div> <!-- /#application -->
  <%= render 'layouts/foot' %>
</body>
```

Key controller-set variables: `@show_left_side` (auto-true when `section_tabs.length > 0`), `@collapse_course_menu`, `@headers = false` (hides nav), `@padless`, `@content_only`, `@fixed_warnings`.

### Body class conventions (from `body_classes` in ApplicationController + layout)
- `no-headers` (@headers == false), `with-left-side`, `course-menu-expanded`, `with-right-side` (only when right-side content non-empty), `padless-content`, `full-width`, `ic-no-flex-layout`
- `primary-nav-expanded` / `primary-nav-transitions` (global nav collapse)
- `is-masquerading-or-student-view`
- `context-<asset_string>` e.g. `context_course_123`
- `get_active_tab` value appended (e.g. `home`, `syllabus`, `assignments`) — set via `set_active_tab "syllabus"` in controllers
- login pages add `full-width ic-Login-Body`; gradebook adds `gradebook full-width`

### _head.html.erb (window.ENV / INST)
```
<html dir="ltr|rtl" lang="en">
<head>
  <meta charset="utf-8"> <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="robots" content="noindex,nofollow" />   <!-- unless @allow_robot_indexing -->
  <title><%= @page_title || (yield :page_title) || "Canvas LMS" %></title>
  <%= stylesheet_link_tag(active_brand_config_url('css')) %>
  <script> INST = {...}; ENV = {...}; REMOTES = {...}; </script>   <!-- js_env accumulates per-controller -->
  ...
</head>
```
`js_env(hash)` in controllers merges into `ENV`; `flash_notices` are pushed into `ENV.notices` (`[{type: error|warning|info|success, content, icon}]`) by the layout — flash is rendered by JS from ENV, not server HTML, except `#flash_message_holder` / `#flash_screenreader_holder` empty divs. The Rust port should render flash HTML server-side into these holders instead.

### Global nav (_new_nav_header.html.erb)
```
<header id="mobile-header" class="no-print"> ... hamburger ... </header>
<nav id="mobileContextNavContainer"></nav>
<header id="header" class="ic-app-header no-print" aria-label="Global Header">
  <a href="#content" id="skip_navigation_link">Skip To Content</a>
  <div role="region" class="ic-app-header__main-navigation" aria-label="Global Navigation">
    <div class="ic-app-header__logomark-container"><a href="<dashboard_url>" class="ic-app-header__logomark"></a></div>
    <ul id="menu" class="ic-app-header__menu-list">
      <li> Account    #global_nav_profile_link  -> /profile/settings   (or Login when logged out)
      <li> Admin      #global_nav_accounts_link  -> /accounts          (if adminable_accounts)
      <li> Dashboard  #global_nav_dashboard_link -> /  (dashboard_url)
      <li> Courses    #global_nav_courses_link   -> /courses
      <li> Groups     #global_nav_groups_link    -> /groups            (if current_active_groups)
      <li> Calendar   #global_nav_calendar_link  -> /calendar
      <li> Inbox      #global_nav_conversations_link -> /conversations (badge span.menu-item__badge)
      <li> Help       #global_nav_help_link (tray, JS)
    </ul>
  </div>
  <div class="ic-app-header__secondary-navigation"> #primaryNavToggle </div>
  <div id="global_nav_tray_container"></div>
</header>
```
Active link gets `ic-app-header__menu-list-item--active` based on `active_path?`.

### Section tabs (course left nav)
`section_tabs` helper renders, inside `#left-side`:
```
<nav role="navigation" aria-label="Courses Navigation Menu">
  <ul id="section-tabs">
    <li class="section"><a id="home-link" class="home [active]" href="/courses/:id">Home</a></li>
    <li class="section"><a id="announcements-link" class="announcements" href="/courses/:id/announcements">Announcements</a></li>
    ...
  </ul>
</nav>
```
- Tabs come from `AvailableSectionTabs.new(@context, @current_user, @domain_root_account, session).to_a` -> `context.tabs_available(...)` filtered by permissions + hidden/unused, cached 1h.
- Course tab constants (`app/models/course.rb` ~line 3512): `TAB_HOME=0, TAB_SYLLABUS=1, TAB_PAGES=2, TAB_ASSIGNMENTS=3, TAB_QUIZZES=4, TAB_GRADES=5, TAB_PEOPLE=6, TAB_GROUPS=7, TAB_DISCUSSIONS=8, TAB_MODULES=10, TAB_FILES=11, TAB_CONFERENCES=12, TAB_SETTINGS=13, TAB_ANNOUNCEMENTS=14, TAB_OUTCOMES=15, TAB_COLLABORATIONS=16/17, TAB_RUBRICS=18`.
- Default tab hrefs: `course_path` (Home), `course_announcements_path`, `course_assignments_path`, `course_discussion_topics_path`, `course_grades_path`, `course_users_path` (People), `course_wiki_path` (Pages), `course_files_path`, `syllabus_course_assignments_path` (Syllabus — note: `/courses/:id/assignments/syllabus`), `course_outcomes_path`, `course_rubrics_path`, `course_quizzes_path`, `course_context_modules_path` (Modules — path `/courses/:id/modules`), `course_conferences_path`, `course_collaborations_path`, `course_settings_path`.
- Hidden/unused tabs render with `class="section section-hidden"`, icon `icon-off`, `data-tooltip` title "Disabled. Not visible to students" / "No content. Not visible to students".
- `a_id` convention: `"<label-with-dashes>-link"` (e.g. `announcements-link`); external tool tabs use `"<tool asset_string>-link"`.

### Breadcrumbs
Controllers call `add_crumb(text, url)`; course show adds `add_crumb(@context.nickname_for(@current_user), course_url, id: "crumb_course_123")`. Layout renders `render_crumbs` only when `crumbs.length > 1` inside `.ic-app-crumbs`. Sub-pages append their own crumb (e.g. Assignments: `add_crumb(t("#crumbs.assignments", "Assignments"), context_assignments_url)`).

### REST endpoints the shell relies on (Rust status)
| Endpoint | Purpose | Rust status |
|---|---|---|
| `GET /api/v1/courses/:id/tabs` | section-tab list (order/visibility) | EXISTS — `star-courses` `/courses/{courseId}/tabs` |
| `GET /api/v1/courses/:id/permissions` | permission checks for nav | EXISTS — `star-courses` `/courses/{courseId}/permissions` |
| `GET /api/v1/users/self` (profile for Account tray) | current user | EXISTS — `star-users` `/users/self` |
| `GET /api/v1/conversations/unread_count` | inbox badge | EXISTS — `star-conversations` `/conversations/unread_count` |
| `GET /api/v1/accounts` | Admin nav visibility | EXISTS — `star-accounts` `/accounts` |
| `GET /api/v1/users/self/todo_item_count` | badge counts | EXISTS — `star-users` `/users/self/todo_item_count` |

---

## 2. Login / Auth (Rust #70)

### Routes (config/routes.rb)
- `GET /login` => `login#new`; `GET /login/session_token`; `DELETE /logout` => `login#destroy`; `GET /logout` => `login#logout_landing`
- `GET /login/canvas` => `login/canvas#new` (as `canvas_login`); `POST /login/canvas` => `login/canvas#create`
- `GET /login/canvas/forgot-password` => `login/canvas#new`; register landing: `GET /login/canvas/register[/student|/parent|/teacher]` => `login/canvas#new`
- `GET /login/ldap`, `POST /login/ldap`; `GET /login/cas[/:id]`, `GET|POST /login/saml[/:id]` (SSO — out of scope for port, but structure documented)
- `GET|POST /forgot_password` => `pseudonyms#forgot_password`
- `GET /register` => `users#new` (also `/register_from_website`)
- `POST /api/v1/accounts/:account_id/self_registration` => `users#create_self_registered_user`
- `GET|POST /register/:nonce` => `communication_channels#confirm` (registration confirmation email link)
- `GET /pseudonyms/:id/register/:nonce` (deprecated alias)

### Login form — `app/views/login/canvas/_new_login_content.html.erb`
Rendered by `login/canvas#new` -> `render "login/canvas/new_login", layout: "bare"`. Class chain: `.ic-Login > .ic-Login__container > .ic-Login__content > .ic-Login__innerContent`.
```
<div class="ic-Login">
  <div class="ic-Login__container"><div class="ic-Login__content"><div class="ic-Login__innerContent">
    <div class="ic-Login-header"> header_logo + login_banner + [Browse courses link] </div>
    <div class="ic-Login__body">
      <%= form_for :pseudonym_session, url: {action: :create}, html: {id: "login_form"} do |f| %>
        <input type="hidden" name="redirect_to_ssl" value="1"/>
        <div class="ic-Form-control ic-Form-control--login">
          <%= f.label :unique_id, login_handle_name, class: "ic-Label" %>
          <%= f.text_field :unique_id, class: "ic-Input text", value: unique_id, autofocus: true %>
        </div>
        <div class="ic-Form-control ic-Form-control--login">
          <%= f.label :password, "Password", class: "ic-Label" %>
          <%= f.password_field :password, class: "ic-Input text", role: "textbox" %>
        </div>
        <div class="ic-Login__actions">
          <% unless session_timeout_enabled? %>
            <%= f.check_box :remember_me, checked: session[:used_remember_me_token] %> "Stay signed in"
          <% end %>
          forgot_password_link partial (link toggles to forgot-password form)
          <%= submit_tag "Log In", class: "Button Button--login" %>
        </div>
        sso_buttons partial (aacs_with_buttons)
      <% end %>
      <form id="forgot_password_form" style="display:none" action="/forgot_password">
        <input type="text" name="pseudonym_session[unique_id_forgot]" id="pseudonym_session_unique_id_forgot">
        "Request Password" submit; "Back to Login" link
      </form>
    </div>
    <footer id="footer" class="ic-Login-footer"> help_link, footer links, Instructure logo </footer>
```
Notes:
- `login_handle_name` = `@domain_root_account.login_handle_name_with_inference` (default "Email" / "Login").
- `unique_id` prefilled from `params[:login_hint]` or `params.dig(:pseudonym_session, :unique_id)`.
- The two forms are both in the page; JS toggles visibility. In a no-JS port, render them as separate pages/anchors.

### LoginController behavior (`app/controllers/login_controller.rb`)
- `new`: if `@current_user` and no `force_login`/`confirm`/`expected_user_id`/remember-me token -> `redirect_to dashboard_url`. Stores `expected_user_id`, `confirm`, `enrollment` into session. Honors `params[:needs_cookies] == "1"` -> renders `shared/unauthorized` 401. If multiple auth providers, redirects to discovery/specific provider.
- `destroy` (DELETE /logout): clears session (`logout_current_user_for_idp` may redirect to IdP), else `redirect_to login_url`.
- `logout_landing` (GET /logout): if logged in -> `render :logout_confirm`; if `flash[:logged_out]` -> logout_landing page; else redirect to login.
- `login/canvas#create` (`Login::CanvasController`): on bad credentials -> `render_new_login(:bad_request)` re-rendering `login/canvas/new_login` with error flash. On success logs in pseudonym, redirects (registration flow may continue).
- `logout_confirm.html.erb`: same `.ic-Login` chrome, "Are you sure you want to log out of Canvas?" + `form_tag logout_path, method: :delete` with a Confirm submit and a JS Cancel (`onclick="window.location.href='/'"` — port should use a plain link).

### Password reset
- `pseudonyms#forgot_password` (GET+POST `/forgot_password`): takes `params[:pseudonym_session][:unique_id_forgot]` (HTML) or `params[:email]` (API). Finds communication channels / pseudonyms by email; calls `cc.forgot_password!` which mails a nonce link. Always flashes the same message (anti-enumeration): flash notice "Confirmation email sent to %{email}, make sure to check your spam box" then `redirect_to canvas_login_url`.
- `pseudonyms#confirm_change_password`: `GET /pseudonyms/:pseudonym_id/change_password/:nonce` (see change_password routes). Loads pseudonym + cc by `confirmation_code == nonce`; managed-password or invalid -> flash error + redirect to login; expired code (`confirmation_code_expires_at`) -> error "The link you used has expired...". View `pseudonyms/confirm_change_password.html.erb` is a React mount (`#confirm_change_password_mount_point` "Loading..."); the port should render a real form: fields `pseudonym[password]`, `pseudonym[password_confirmation]`, hidden nonce + pseudonym id, posting to `change_password`.
- `pseudonyms#change_password` (POST): validates nonce not expired, sets password, confirms cc, logs user in.

### Registration — `app/views/users/new.html.erb` (users#new, layout "bare")
- `#registration_header` (Instructure logo + "Login" link to /login), `#registration_content > #registration_options`: "Sign up now, it's free!" with `#signup_teacher` ("I'm a Teacher" — `data-template="teacherDialog"`), `#signup_student` ("I'm a Student"), parent signup link. The actual dialogs are JS (`js_bundle :registration`); server only renders landing page. Self-registration POST: `POST /api/v1/accounts/:account_id/self_registration` (users#create_self_registered_user -> create_user with pseudonym). Email confirmation: `/register/:nonce` -> `communication_channels#confirm`.
- `users#new` redirects to root if already logged in; sets `ENV.ACCOUNT` and `ENV.PASSWORD_POLICY` (min length, complexity) via js_env.

### Session expiry / login gating (lib/authentication_methods.rb)
- `before_action :require_user` on ApplicationController (except `rescue_action_dispatch_exception`).
- `require_user`: if `@current_user && @current_pseudonym` -> true; else `redirect_to_login`.
- `redirect_to_login`: HTML -> `store_location` (saves `request.fullpath` into `session[:return_to]`, cleaned to same-host path), flash warning "You must be logged in to access this page" (unless path == "/"), `redirect_to login_url`. JSON -> 401 JSON; other formats -> plain "Unauthenticated" 401.
- After login, controllers use `redirect_back_or_default(default)` = `session.delete(:return_to) || default` (note: Canvas uses session `:return_to`, not a `?next=` query param).
- `require_password_session`: for sensitive ops when `session[:used_remember_me_token]` -> flash warning "For security purposes, please enter your password to continue", store_location, redirect to login.
- `check_pending_otp` forces OTP completion (`login/otp`) if `session[:pending_otp]`.
- Masquerade: `@real_current_user != @current_user` adds screenreader alert "You are currently acting as %{user_name}" and body class `is-masquerading-or-student-view`.

### Rust cross-check
| Endpoint | Rust status |
|---|---|
| `POST /api/v1/auth/register` (self registration; Canvas equivalent `POST /accounts/:id/self_registration` + `POST /users`) | EXISTS (custom) — `star-auth` `/auth/register` |
| `POST /api/v1/auth/login` | EXISTS (custom) — `star-auth` `/auth/login` |
| `GET /api/v1/auth/me` | EXISTS (custom) — `star-auth` `/auth/me` |
| `POST /api/v1/auth/logout` | EXISTS (custom) — `star-auth` `/auth/logout` (Canvas is `DELETE /logout` HTML) |
| `POST /api/v1/users/reset_password` (Canvas: `POST /forgot_password`) | MISSING — no `/forgot_password`-style endpoint in star crates; `star-auth` has password-reset-like routes only if present (grep shows only register/login/me/logout + oauth2 + sessions mgmt). Verify before building UI. |
| `GET /api/v1/pseudonyms` / password policy | MISSING |

---

## 3. Dashboard + Course Home (Rust #44)

### Dashboard
Route: `GET "" => "users#user_dashboard"` (root; also legacy `GET /dashboard` => dashboard_redirect), `users#user_dashboard` -> `app/views/users/user_dashboard.html.erb`. Related: `GET /dashboard-sidebar` => `users#dashboard_sidebar` (partial HTML, layout false); `GET /api/v1/dashboard/dashboard_cards` => `users#dashboard_cards` (JSON); `PUT /dashboard/view` (activity|cards|planner); `GET /api/v1/users/self/todo`; `GET /api/v1/planner/items`.

`user_dashboard` action computes: observed users (observer feature), k5/widget dashboard flags, and pushes big `ENV.PREFERENCES { dashboard_view, custom_colors, ... }`. The HTML view:
```
<div id="dashboard" class="ic-dashboard-app">
  <h1 class="screenreader-only">Dashboard</h1>
  <%= render 'shared/dashboard_invitation', collection: @pending_invitations %>
  <%= render 'shared/dashboard_messages' %>
  <div id="dashboard_header_container" class="ic-Dashboard-header" data-props="...">  <!-- JS: view toggle, color overlay toggle -->
  <div id="dashboard-planner" class="StudentPlanner__Container" style="display:none"></div>
  <div id="dashboard-activity" class="ic-Dashboard-Activity" style="display:none"></div>  <!-- recent activity (stream items) -->
  <%= render 'shared/dashboard_card' %>
</div>
```
`shared/_dashboard_card.html.erb` is only SVG placeholders (`#DashboardCard_Container > .ic-DashboardCard__box > .ic-DashboardCard` skeletons); real cards are React, fed by `GET /api/v1/dashboard/dashboard_cards` which returns `map_courses_for_menu(@current_user.menu_courses, tabs: DASHBOARD_CARD_TABS)` where `DASHBOARD_CARD_TABS = [TAB_DISCUSSIONS, TAB_ASSIGNMENTS, TAB_ANNOUNCEMENTS, TAB_FILES]`.

Card JSON fields (CourseForMenuPresenter#to_h — `app/presenters/course_for_menu_presenter.rb`):
`id, href (course_path with invitation), shortName (nickname_for user), originalName, longName ("name - short_name"), courseCode, term (enrollment_term name unless default), subtitle ("enrolled as"/"invited as" + role label), enrollmentState, enrollmentType, observee, assetString, isFavorited, published, image, color, position (user dashboard_positions), links[] (subset of tabs with label+path), canManage, canChangeCoursePublishState, defaultView, pagesUrl, frontPageTitle`.

To-do sidebar / upcoming events (`users#dashboard_sidebar` -> `users/dashboard_sidebar.html.erb`, fetched via XHR `/dashboard-sidebar`):
```
[brandable .ic-sidebar-logo]
<% if planner_enabled? && student %> <div class="Sidebar__TodoListContainer"></div> (JS planner)
<% if teacher/legacy %> courses/_to_do_list (h2.todo-list-header "To Do"; ul.right-side-list.to-do-list;
   li.todo > a.item[href=assignment gradebook_path] with .todo-badge count; needs-grading + needs-submitting)
<%= shared/_event_list object=@upcoming_events, title "Coming Up", period one_week, show_context %>
<% if @show_recent_feedback %> shared/_event_list @recent_feedback "Recent Feedback" two_weeks <% end %>
<button id="start_new_course" class="element_toggler" aria-controls="new_course_form">Start a New Course</button>
<a href="<%= grades_path %>" class="Button button-sidebar-wide">View Grades</a>
```
Data: `cached_upcoming_events(user)` = `user.upcoming_events(context_codes: ...)` (assignments + calendar events), `cached_submissions` (user's submissions for those assignments), `@recent_feedback = user.recent_feedback`. To-do items come from `ToDoListPresenter` (needs-grading counts for teachers; missing submissions for students; mirrors `/users/self/todo` API).

`shared/_new_course_form.html.erb` (toggled by #start_new_course): `form_for Course.new, id: "new_course_form"` with `course[name]` text (placeholder "Course Name"), `course[license]` select, checkboxes (mock/invite) — posts to courses#create.

### Course home — courses#show
Route: `GET /courses/:id` => `courses#show` (HTML branch in CoursesController#show after API branch). Template `app/views/courses/show.html.erb`.

Controller flow (condensed):
- membership/invitation checks (`check_for_xlist`, `check_enrollment`, `claim_course` on verification links), unpublished-course gating (`@unauthorized_message = "This course has not been published by the instructor yet."`).
- If user can only `read_as_admin` -> `redirect_to course_settings_path`.
- `add_crumb(@context.nickname_for(@current_user), url_for(@context))`, `set_badge_counts_for`, `@show_left_side = true`.
- `default_view = @context.default_view || @context.default_home_page` (values: `feed|wiki|modules|syllabus|assignments`); `@course_home_view` forced to `feed` when `params[:view] == "feed"`.
- `js_env(COURSE: { id, long_name, pages_url, is_student, is_instructor, is_published, front_page_title, default_view })`.
- Per-view setup: `wiki` -> `@wiki = context.wiki; @page = @wiki.front_page; @padless = true`; `assignments` -> `@upcoming_assignments`; `modules` -> `load_modules`; `syllabus` -> `set_active_tab "syllabus"`, `@groups = assignment_groups`, `@syllabus_body = syllabus_user_content`; `feed` (default else) -> `@stream_items = @current_user.cached_recent_stream_items`, current/scheduled conferences.
- `@recent_feedback` when current user is a student (`@show_recent_feedback`).

View:
```
<% provide :right_side %> courses/_course_show_secondary + per-view sidebar
<% if @pending_enrollment %> shared/_dashboard_invitation <% end %>
<div id="announcements_on_home_page"></div>   <!-- if show_announcements_on_home_page? -->
<div id="course_home_content">
  <% if @course_home_view == 'wiki' %>      <div id="wiki_page_show"></div>            <!-- React wiki page -->
  <% elsif @course_home_view == 'modules' %> context_modules/_content_next (course_home: true)
  <% elsif @course_home_view == 'assignments' %>  (empty; the index partial is injected via JS env)
  <% elsif @course_home_view == 'syllabus' %>    assignments/_syllabus_content (course_home: true)
  <% else %> shared/_dashboard_messages + shared/_recent_activity
<% end %>
<div class="public-license"> ... CC license ... </div>
<div id="wizard_box"></div>
```

Right sidebar `courses/_course_show_secondary.html.erb` (`.ic-Action-header` actions):
- `#course_status` + `#course_publish_button` (if manage_courses_publish and unpublished/unpublishable)
- `#import_existing_content_btn` -> content_migrations URL
- `#choose_home_page` (JS dialog), `#view_course_stream_btn` -> `?view=feed`, `#course_setup_checklist_btn` (course wizard), `#course_check_accessibility_btn`, "New Announcement" link -> `new_context_discussion_topic_path(is_announcement: true)`
- Self-enrollment join/drop links; "View Course Calendar" (students); `#view_course_notifications_btn` -> `?view=notifications`
- `courses/_to_do_list` (contexts: course), `courses/_group_list`
- Plus "Coming Up" `shared/_event_list` for non-students (see assignments sidebar below).

### Rust cross-check
| Endpoint | Rust status |
|---|---|
| `GET /api/v1/dashboard/dashboard_cards` | EXISTS — `star-dashboard` `/dashboard/dashboard_cards` |
| `GET /dashboard-sidebar` (HTML partial) | MISSING (UI route; port should server-render) |
| `GET /api/v1/users/self/todo` | EXISTS — `star-dashboard` `/users/self/todo` |
| `GET /api/v1/users/self/upcoming_events` | EXISTS — `star-dashboard` `/users/self/upcoming_events` |
| `GET /api/v1/planner/items` | MISSING — planner items API not implemented (planner_notes exist in `star-calendar`) |
| `GET /api/v1/users/self/missing_submissions` | MISSING — not found in crate routers |
| `GET /api/v1/courses/:id` (course show data, default_view) | EXISTS — `star-courses` `/courses/{id}` |
| `GET /api/v1/courses/:id/activity_stream` | EXISTS — `star-courses` `/courses/{courseId}/activity_stream` |
| `GET /api/v1/courses/:id/enrollments` (invitation handling) | EXISTS — `star-courses` `/courses/{id}/enrollments` |
| `GET /api/v1/users/self/favorites/courses` (card ordering/favorites) | EXISTS — `star-dashboard` `/users/self/favorites/courses` |
| `GET /api/v1/courses/:id/front_page` (wiki default_view) | EXISTS — `star-pages` `/courses/{courseId}/front_page` |
| `GET /api/v1/courses/:id/modules` (modules default_view) | EXISTS — `star-modules` `/courses/{courseId}/modules` |
| `GET /api/v1/courses/:id/assignment_groups` (syllabus summary) | EXISTS — `star-assignment-groups` |
| `GET /api/v1/courses/:id/folders/root` (files tab data) | EXISTS — `star-files` |

---

## 4. Assignments + Gradebook (Rust #71)

### Assignments index
Route: `GET /courses/:course_id/assignments` => `assignments#index`. Template: `app/views/assignments/new_index.html.erb` — a pure React mount (`js_bundle :assignment_index`, body class `with_item_groups`, `@padless = true`, only `<div id="hidden"></div>`). The React app loads via `GET /api/v1/courses/:id/assignments` (+ `assignment_groups`, `permissions`, section list).

Controller sets `ENV`: `WEIGHT_FINAL_GRADES, HAS_ASSIGNMENTS, POST_TO_SIS_DEFAULT, SIS_NAME, MAX_NAME_LENGTH..., QUIZ_LTI_ENABLED, DUE_DATE_REQUIRED_FOR_ACCOUNT, SHOW_SPEED_GRADER_LINK, grading_scheme, points_based, scaling_factor` plus `set_js_assignment_data` (assignment list JSON) and `set_section_list_js_env`. `@context.require_assignment_group` ensures a default group exists.

Server-rendered port should render the classic structure: group headings with weights, rows per assignment (name, due date, points, submission types, publish state), links to show/edit/speedgrader.

### Assignment show
Route: `GET /courses/:course_id/assignments/:id` => `assignments#show`. Template `assignments/show.html.erb` + `_assignment_sidebar`, `_assignment_details`, `_student_assignment_overview`, `_submit_assignment`.

Controller: finds assignment, `AssignmentOverrideApplicator.assignment_overridden_for(@current_user)`, computes `@locked = assignment.locked_for?(current_user)` (module/availability locks), `@current_user_submission` (when `read_own_submission` + `read_grades`). Deleted assignment -> flash + redirect to index. Not visible to section -> flash error + redirect. `params[:submitted]` -> flash.now "Assignment successfully submitted."

View structure:
```
<% provide :right_side, "assignment_sidebar" %>
<div id="assignment_show" class="assignment content_underline_links">
  <% if admin %>
    <div class='assignment-title'> h1.title + buttons:
      #assignment_publish_button (btn published/unpublished),
      .assign-to-link (Assign To), .edit_assignment_link -> edit path,
      assignment_submission_button (SpeedGrader / A2),
      #moderated_grading_button, al-options menu (#toolbar-1: SpeedGrader link, Send To..., Copy To...)
  <% else %> <!-- student -->
    <div class='assignment-title'> h1 + mark_as_done + submission button <% end %>
  <% if @locked %> lock_explanation <% end %>
  <div class="description user_content [teacher-version]"> assignment.description (user_content) </div>
  <% if admin %> form-horizontal display-only: Points / Submitting (readable_submission_types) / File Types /
     Allowed Attempts / Turnitin / VeriCite
  <table class="ic-Table assignment_dates">  <!-- due/available dates incl. overrides -->
</div>
```
`_assignment_details.html.erb` renders `ul#assignment_student_details`: Due (Multiple Due Dates / datetime / No due date), Points, Available/Until dates (`shared/_available_dates`).

Student sidebar `_assignment_sidebar` / `_student_assignment_overview` shows submission status, grade, attempt selector, and `_submit_assignment` (file upload / text entry / URL entry forms posting to `POST /courses/:id/assignments/:aid/submissions`).

### Gradebook (teacher grid)
Route: `GET /courses/:course_id/gradebook` => `gradebooks#show` (`GradebooksController`). Permission gate: `[:manage_grades, :view_all_grades]`. Template `app/views/gradebooks/gradebook.html.erb` — SlickGrid React app (`#gradebook-toolbar`, `#gradebook_app`, `#gradebook_grid`, `@collapse_course_menu = true`, body `gradebook full-width`).

Data the view/env relies on (`set_default_gradebook_env`, condensed):
- Columns: student name columns + one column per assignment (via `GET /api/v1/courses/:id/assignments` incl. `assignment_group`), custom columns (`/api/v1/courses/:id/custom_gradebook_columns`), total column.
- Rows: enrollments `enrollments_url` = custom course enrollments API with `per_page`; `prefetch_xhr(user_ids_course_gradebook_url)` = `GET /courses/:id/gradebook/user_ids`.
- Submissions per assignment: `/api/v1/courses/:course_id/assignments/:assignment_id/submissions` (paginated).
- Other env: grading periods, sections (visible to user), late_policy, grading schemes, group weighting, post_manually (post policy), speed_grader_enabled, export CSV (`course_gradebook_csv_url`), teacher notes custom column.
- `format.json` variant of `speed_grader` etc. exist but grid is JS-only; **Canvas gradebook is unusable without JS**. Port: server-rendered `<table>` of students x assignments with submission scores + forms posting to update endpoints; reuse the Individual View concept.

### Student grades page
Route: `GET /courses/:course_id/grades` => `gradebooks#grade_summary` (see `courses/:id/grades` route under courses scope; GradesController alias). Teachers viewing their own grades get redirected to gradebook (`@presenter.user_needs_redirection?`). `params[:id]` may be student id for teachers/observers. Template `gradebooks/grade_summary.html.erb`.

Presenter: `GradeSummaryPresenter` / `GradingPeriodGradeSummaryPresenter` (`app/presenters/grade_summary_presenter.rb`) — `assignment_presenters` (assignment + submission + group), `groups` (assignment groups), `student_name`, `editable?` (What-If), grading period selection.

View structure:
```
<div id="grade-summary-content">
  right-side: #student-grades-right-content
    .student_assignment.final_grade (Total: .grade + #final_letter_grade_text)
    #student-grades-whatif (Show Saved "What-If" Scores), #student-grades-revert (Revert to Actual Score)
    #student-grades-show-all (Show All Details button)
    #assignments-not-weighted -> courses/_sidebar_weighting (group weights) or _sidebar_periods_weighting
    #only_consider_graded_assignments checkbox "Calculate based only on graded assignments"
  #print-grades-container .ic-Action-header (h1 "Grades for %{student}" + #print-grades-button window.print)
  #GradeSummarySelectMenuGroup (grading period select, JS)
  <table id="grades_summary" class="ic-Table ic-Table--hover-row [editable]">
    thead: Name | Due | Submitted | Status | Score | (asset processors) | Details |
    <% presenter.assignment_presenters.each %> <tr class="..." id="submission_<id>" data-muted=... >
      th.title (assignment link -> submission page; .context group name)
      td.due, td.submitted
      td.status (.submission-late-pill / missing / extended / custom pill)
      td.assignment_score > .score_holder
         .tooltip > .grade (student_score_display_for submission; hidden icon when grade not posted)
           .original_points/.original_score/.what_if_score/.student_entered_score/.submission_status/
           .assignment_group_id/.assignment_id/.group_weight/.rules  (hidden spans for JS What-If math)
      td.details (toggle links: scoring details, rubric, submission comments)
  </table>
  (+ "Assignments" / "Learning Mastery" navpills when student_outcome_gradebook enabled)
```

### Rust cross-check
| Endpoint | Rust status |
|---|---|
| `GET /api/v1/courses/:id/assignments` | EXISTS — `star-assignments` `/courses/{courseId}/assignments` |
| `GET /api/v1/assignments/:id` | EXISTS — `/assignments/{id}` |
| `GET /api/v1/courses/:id/assignment_groups` | EXISTS — `star-assignment-groups` |
| `POST/GET /api/v1/courses/:id/assignments/:aid/submissions` | EXISTS — `star-assignments` `/assignments/{id}/submissions`, `/assignments/{id}/submissions/me` |
| `PUT /api/v1/courses/:id/assignments/:aid/submissions/:uid` (grade) | EXISTS (variant) — `star-assignments` `/submissions/{id}/grade`; per-user update-by-ids routes exist in the long tail |
| `GET /courses/:id/gradebook/user_ids` | MISSING (UI route; server port should compute user list server-side) |
| `GET /api/v1/courses/:id/enrollments` (gradebook rows) | EXISTS — `star-courses` |
| `GET /api/v1/courses/:id/custom_gradebook_columns` | MISSING |
| `GET /api/v1/courses/:id/final_grade` | EXISTS — `star-grading-standards` `/courses/{courseId}/final_grade` |
| `GET /api/v1/courses/:id/gradebook_csv` (export) | MISSING (star-assignments has `/courses/{courseId}/gradebook` JSON) |
| `GET /api/v1/submissions/:id/comments` | EXISTS — `star-submission-comments` `/submissions/{submissionId}/comments` |
| `GET /api/v1/courses/:id/grading_standards` | EXISTS — `star-grading-standards` |
| `GET /api/v1/sections/:id` + course sections | EXISTS — `star-courses` |

---

## 5. Discussions + Inbox (Rust #72)

### Discussion topics index
Route: `GET /courses/:course_id/discussion_topics` => `discussion_topics#index` (announcements variant: `?only_announcements=true`). The HTML branch renders an empty page with `js_bundle :discussion_topics_index`, `css_bundle :discussions_index`, `render html: "", layout: true` — full React. Data via `GET /api/v1/courses/:id/discussion_topics` (+ `entries`, `participants`, `permissions`).

Controller scoping (server side, reusable for a server-rendered port): `active_discussion_topics.only_discussion_topics` scoped to user/sections; order by `position_legacy` or `order_by=recent_activity|title`; `filter_by=unread`; `scope=pinned,unpinned,locked,unlocked`; `search_term`. Teacher env includes pinned/locked states, `permissions` (create/update/delete), `DIRECT_SHARE_ENABLED`, discussion settings.

Classic server-rendered topic row (from legacy structure, retained in API JSON): title, author, `unread_count`, `discussion_subentries_count` (replies), `last_reply_at`, pinned/locked/closed icons, publish state, link to show.

### Topic detail (show)
Route: `GET /courses/:course_id/discussion_topics/:id` => `discussion_topics#show`. Also announcements `GET /courses/:course_id/announcements/:id` => `announcements#show` which just redirects to the discussion topic URL. Current HTML branch: `render html: "", layout: true` with `js_bundle :discussion_topics_post` (React "discussions redesign"), env `DISCUSSION_TOPIC: hash` (topic JSON incl. entries), `discussion_topic_id`, permissions, `manual_mark_as_read`, `PEER_REVIEWS_URL`, `INSIGHTS_URL`.

Legacy server-rendered partials still in tree and a good blueprint for askama:
- `discussion_topics/_entry.html.erb` — topic/entry header: `.discussion-topic` style container with header (author link `.user_name`, `.post_date time_ago_date[data-timestamp]`, delete link) + `.content .message.user_content` body.
- `discussion_topics/_sub_entry.html.erb` — reply entry: `div#entry_{id}.communication_sub_message.[read|unread].can_be_marked_as_read[data-mark-read-url=api mark_read url]`, header (`.user_name`, `.delete_entry_link`, `.post_date`), `.message.user_content`, hidden `.message_html` textarea, and for blank entries a server-rendered reply form:
```
<%= form_for :discussion_entry, url: context_discussion_entries_url, html: {class: "add_sub_message_form"} do |f| %>
  <%= f.hidden_field :discussion_topic_id %>
  <%= f.hidden_field :parent_id, class: "parent_id" %>
  <%= f.text_area :message, class: "rich_text" %>
  <button type="submit" class="btn btn-small submit_button">Post Message</button>
  <button type="button" class="btn button-secondary cancel_button">Cancel</button>
<% end %>
```
POST target: `POST /courses/:id/discussion_topics/:tid/entries` (reply: `POST .../entries/:eid/replies`). Read state: `PUT/DELETE .../entries/:eid/read`; topic read: `PUT .../read_all` (all topics) or `.../topics/:tid/read`; subscribe: `.../subscribed`.
Topic show controller behavior: marks topic read on view (unless locked), adds crumbs (topic title), computes `@locked`, `DiscussionTopicPresenter` (dates, peer reviews, rubric presence).

### Conversations inbox
Route: `GET /conversations` => `conversations#index` (scope variants `/conversations/unread|starred|sent|archived` redirect with `redirect_scope`). Template `app/views/conversations/index_new.html.erb` (Conversations 2 — Backbone/React). Body class `ic-no-flex-layout full-width`, `@show_left_side = false`.

Server-rendered structure:
```
<div class="conversations">
  <header class="panel">
    <div class="panel__primary">
      <div class="filters">
        <select class="course-filter" id="course-filter">  <!-- filter by course, JS-filled -->
        <div id="conversation_filter" class="conversations__filter"></div>
      </div>
    </div>
    <div class="panel__secondary">
      <div class="actions" id="submission-comment-actions"> #submission-reply-btn (disabled) </div>
      <div class="actions" id="conversation-actions">
        #compose-btn (icon-compose, "Compose a new message")
        #reply-btn, #reply-all-btn, #archive-btn, #delete-btn (all disabled until selection)
        #admin-btn al-trigger -> #admin-menu (mark-unread/read, forward, star-toggle)
      </div>
      #sending-message badge
      <div role="search" class="search form-search"> #search-autocomplete (ac-input-box, ac-token-list,
        input#compose-message-recipients, recipient-finder #search-btn) </div>
    </div>
  </header>
  <div class="messaging-wrapper">
    <div class="message-list-scroller"><div class="message-list"></div></div>   <!-- left pane: conversation list (JS) -->
    <div class="message-detail conversations__message-detail"></div>           <!-- right pane: messages (JS) -->
  </div>
</div>
```
REST endpoints used by the JS: `GET /api/v1/conversations?scope=...&filter[]=course_...&include_all_conversation_ids`, `GET /api/v1/conversations/:id` (with messages + participants), `POST /api/v1/conversations` (compose: recipients, subject, body, group_conversation, context_code), `POST /api/v1/conversations/:id/add_message`, `PUT /api/v1/conversations` (batch update: archive/star/read), `GET /api/v1/conversations/find_recipients` (search#recipients), `POST /api/v1/conversations/mark_all_as_read`, unread count badge.

For the no-JS port: render the two panes server-side — left list of conversations (participant_names, subject, last message snippet, unread/star flags, `?scope=` filter links + course filter as a GET form), right pane the selected conversation's message thread with a reply form posting to `add_message`, and a compose page with recipients multi-select (populated from find_recipients), subject, body, course context select.

### Rust cross-check
| Endpoint | Rust status |
|---|---|
| `GET /api/v1/courses/:id/discussion_topics` | EXISTS — `star-discussions` |
| `POST /api/v1/courses/:id/discussion_topics` | EXISTS |
| `GET /api/v1/courses/:id/discussion_topics/:tid` | EXISTS |
| `POST .../discussion_topics/:tid/entries` and `/entries/:eid/replies` | EXISTS |
| `GET .../entries` / entry CRUD | EXISTS |
| `PUT/DELETE .../entries/:eid/read`, topic `/read`, `/read_all` | EXISTS |
| `PUT/DELETE .../subscribed` | EXISTS |
| `GET /api/v1/courses/:id/discussion_topics/:tid/view` (threaded view) | MISSING — check spec; not in router |
| `GET /api/v1/conversations` | EXISTS — `star-conversations` |
| `GET /api/v1/conversations/:id` | EXISTS |
| `POST /api/v1/conversations` | EXISTS |
| `POST /api/v1/conversations/:id/add_message` | EXISTS |
| `PUT /api/v1/conversations` (batch) / `/:id` | EXISTS |
| `POST /api/v1/conversations/mark_all_as_read` | EXISTS |
| `GET /api/v1/conversations/find_recipients` | EXISTS — `star-conversations` `/conversations/find_recipients` |
| `GET /api/v1/conversations/unread_count` | EXISTS |

---

## 6. Course Settings + People/Admin (Rust #73)

### Course settings
Route: `GET /courses/:id/settings` => `courses#settings` (`context_details_url`). Permission: `read_as_admin`. Template `app/views/courses/settings.html.erb`, right side `courses/_settings_sidebar`. Tabs are declared server-side then rendered by a React `SettingsTabs` mount:
```
tabs = [ {id: "details", title: "Course Details", contentMount: "tab-details-mount"},
         {id: "sections", title: "Sections", contentMount: "tab-sections-mount"} ]
+ navigation (if can update), tools/apps (read_as_admin), grade-publishing, alerts,
  features ("Feature Options"), integrations, quiz_settings, criteria
<div id="course_settings_tabs_mount" data-component="SettingsTabs" data-props="{tabs: ...}">
<div id="course_details_tabs" style="display:none">  <!-- server-rendered fallback content -->
```
The `#tab-details-mount` region contains the real form (`form_for :course` -> `PUT /courses/:id`), condensed field list (all `course[...]`):
- `name` (text), `course_code` (text), `friendly_name` (text), `sis_source_id` (text), `account_id` (select, sub-account move), `enrollment_term_id` (select), `time_zone` (select), `default_due_time` (select), `locale` (select), `storage_quota_mb` (text), `turnitin_comments` (textarea)
- `filter_speed_grader_by_student_group` (checkbox), `course_grading_standard_enabled` + `grading_standard_id` (hidden), `license` (select), `usage_rights_required`, `course_visibility` (select) + `custom_course_visibility`, `indexed`, `course_format` (select), `conditional_release`, `organize_epub_by_content_type`, `enable_offline_web_export`, `homeroom_course`, `enable_course_paces`
- `public_description` (textarea), `self_enrollment` (+ `open_enrollment`), `show_announcements_on_home_page` + `home_page_announcement_limit` (select 1..15), `allow_student_forum_attachments`, `allow_student_discussion_topics`, `allow_student_discussion_editing`, `allow_student_organized_groups`, `hide_final_grades`, `default_student_gradebook_view`, `hide_distribution_graphs`, `hide_sections_on_course_users_page`, `lock_all_announcements`
- Course status block: `#course-status` span "Course is Published/Unpublished" + publish controls in `_settings_sidebar` (`#course_publish_button`).
Other tabs content (Sections list w/ add-section form; Navigation drag list of section-tabs with hidden/external checkboxes; Apps; Feature Options flags; Grade Syncing SIS; Alerts) are mostly JS, but the tab list + visibility conditions are server-computed — replicate in askama.

### People / roster
Route: `GET /courses/:id/users` (TAB_PEOPLE href `course_users_path`) => `context#roster` (ContextController, template `app/views/context/roster.html.erb`). Course roster renders React (`js_bundle :roster`, `ENV.canManageCourse`), with a group/legacy server-rendered fallback for Group contexts:
```
<div class="ic-Action-header">
  <div class="ic-Action-header__Primary"><h1 class="screenreader-only">People</h1></div>
  <div class="ic-Action-header__Secondary"> context/_roster_right_side </div>
</div>
```
Legacy `.roster-container` columns list users as:
```
<li class="user-list__item" id="user_{id}">
  avatar, <a class="user_name" href="/courses/:cid/users/:uid"> name (pronouns) </a>
  .short_name, .email, .course_section (from @enrollments_hash)
```
`_roster_right_side` "More Options" dropdown (#people-options): View Prior Enrollments (`course_prior_users_path`), Student Interactions Report link, View Registered Services, plus the React "Add People" button (`#addPeople` mount — add by login list; posts enrollments). Roster user detail page: `GET /courses/:cid/users/:uid` => `context#roster_user` (enrollment list w/ sections, last activity, group memberships).

### Account admin
- `GET /accounts` => `accounts#index` — `<div id="context_list"></div>` (React list of adminable accounts).
- `GET /accounts/:id` => `accounts#show` -> `course_user_search` action: `js_bundle :account_course_user_search`, crumbs, big React course/user search page (courses + people tabs, filters, SIS imports). Server-side: requires `read_course_list` OR `read_roster`, else redirect to account settings.
- `GET /accounts/:id/settings` => account settings page: same SettingsTabs pattern (`tab-settings-mount`, `tab-quotas-mount`, `tab-users-mount` (Admins), `tab-announcements-mount`, `tab-reports-mount`, `tab-tools-mount`, `tab-features-mount`, ...). The `accounts/settings.html.erb` contains the real `form_for :account` (name, SIS, quotas `default_user_storage_quota_mb`/`default_group_storage_quota_mb`, etc.).
- Sub-account admin index `accounts/index.html.erb` minimal; the real "Admin" landing is the course_user_search page.

### Rust cross-check
| Endpoint | Rust status |
|---|---|
| `GET /api/v1/courses/:id` + `PUT /api/v1/courses/:id` (settings save) | EXISTS — `star-courses` |
| `GET /api/v1/courses/:id/settings` | EXISTS — `star-courses` `/courses/{courseId}/settings` |
| `GET /api/v1/courses/:id/sections` (+ create/update) | EXISTS — `star-courses` |
| `GET /api/v1/courses/:id/users` (roster) | EXISTS — `star-courses` `/courses/{courseId}/users` |
| `GET /api/v1/courses/:id/search_users` | EXISTS |
| `POST /api/v1/courses/:id/enrollments` (add people) | EXISTS — `star-courses` + `star-enrollments` |
| `DELETE /api/v1/enrollments/:id` | EXISTS — `star-enrollments` |
| `GET /api/v1/accounts` | EXISTS — `star-accounts` |
| `GET /api/v1/accounts/:id` / `PUT` | EXISTS |
| `GET /api/v1/accounts/:id/courses` / `users` | EXISTS |
| `GET /api/v1/accounts/:id/roles` | EXISTS (star-accounts long tail) |
| `GET /api/v1/courses/:id/features` (Feature Options tab) | EXISTS — `star-features` |
| `GET /api/v1/courses/:id/tab_configuration` (Navigation tab reorder) | MISSING — only GET tabs exists |

---

## 7. SpeedGrader (Rust #108)

Route: `GET /courses/:course_id/gradebook/speed_grader?assignment_id=&student_id=` => `gradebooks#speed_grader`. Gates: `@context.allows_speed_grader?` else flash "SpeedGrader is disabled for this course" + redirect to gradebook; permission `[:manage_grades, :view_all_grades]`; unpublished assignment -> flash "SpeedGrader is enabled only for published content." + redirect to assignment.

Template: `app/views/gradebooks/speed_grader.html.erb` (headers off: `@headers = false`, `@show_left_side = false`, `@outer_frame = true`, body `ic-no-flex-layout full-width`). DOM regions:

```
<div id="classic_speedgrader_container">
  <nav id="gradebook_header_container">
    <form class="speed_grader_nav_form" action=".../gradebook/update_submission">  <!-- grade navigation form -->
    <div id="gradebook_header" class="subheadContent">
      #speedgrader-icons (gradebook link #speed_grader_gradebook_link, post-grades menu, settings)
      .assignmentDetails > a#assignment_url > h2.assignmentDetails__Title (assignment title)
        p.assignmentDetails__Info (due date - course link #context_title)
      .statsMetric (#x_of_x_graded "Graded", #average_score "Average", #x_of_x_students_frd)
      .studentSelection: #prev-student-button, #avatar > #avatar_image, #combo_box_container (student select), #next-student-button
    </div>
  </nav>
  [notice figures: #enrollment_inactive_notice, #enrollment_concluded_notice, #closed_gp_notice]
  <section id="full_width_container">
    <div id="left_side"><div id="left_side_inner">
      alerts: #no_annotation_warning, #comment_saved, #comment_submitted, #reassignment_complete
      #submissions_container:
        #this_student_does_not_have_a_submission (h3 "This student does not have a submission...")
        #this_student_has_a_submission (+ anonymous note)
        #iframe_holder (submission preview iframe)
        #ams_grading_container
    </div></div>
    #speed_grader_rubric_assessment_tray_wrapper
    <div id="right_side"><div class=right_side_content><div id="rightside_inner" class="full_height">
      #submission_details .content_box
        #multiple_submissions (attempt dropdown), #speed_grader_edit_status_mount_point
        #submission_not_newest_notice, #submission_attachment_viewed_at_container, #submission_word_count
        #submission_files_container > #submission_files_list
          #submission_file_hidden .submission-file (template row):
            .turnitin_score_container, a.display_name (preview url .../submissions/:id?download=:attId&inline=1),
            delete icon href /api/v1/files/{{attachmentId}}?replace=1, a.submission-file-download (download url)
      (2nd .content_box) Grading:
        #rubric_full > #rubric_holder (shared/_rubric partial, save_rubric_button, update_rubric_assessment_url
           -> POST context_rubric_association_rubric_assessments_url)
        #grading: h2 "Assessment", #grade_container (grading_box_extended + #submit_same_score link),
          #grading_details_mount_point
          #rubric_summary_holder: #rubric_assessments_select (Select Rubric), #rubric_summary_container
          #speed_grader_assessment_audit_button_mount_point, #enhanced-rubric-assessment-container
      (3rd .content_box) Assignment Comments:
        #discussion > #comments > #comment_blank .comment (template: avatar, .draft-marker DRAFT, .comment body,
          .edit_comment_link, .submit_comment_button, .delete_comment_link, .play_comment_link (media),
          .comment_attachments, .comment_citation .author_name/.posted_at)
        <form id="add_a_comment">
          #speed_grader_comment_textarea_mount_point (RCE)
          #group_comment_wrapper: radios submission[group_comment] ("Send comment to this student only" /
            "Send comment to the whole group")
          #comment_attachments + .comment_attachment_input (input type=file name="attachments[-1][uploaded_data]")
          #comment_submit_button (Submit)
        </form>
    </div></div>
  </section>
</div>
```

Data per student (from `SpeedGrader::Assignment#json` and env):
- `context.students` — student list with id, name, sortable_name, anonymous_id, avatar, enrollments, inactive/concluded flags; `rep_for_student` (peer review).
- `submissions` — per student: submission history (versions), attachments, score, grade, workflow_state, entered_score, submission_type, submitted_at, late, excused; `rubric_assessments` grouped by user.
- rubric JSON (criteria, points), `update_rubric_assessment_url`, `update_submission_grade_url` (POST `/courses/:id/gradebook/update_submission`), comments post as `submission[comment]` (+ `submission[group_comment]`) to update_submission.
- quiz submissions get `quiz_history_url` link.
Posting grades: the nav form posts to `PUT /courses/:course_id/gradebook/update_submission` with `submission[:user_id], [:student_id], [:graded_by], [:provisional], [:final], [:score], [:grade], [:comment], [:group_comment], attachments`. JSON variant `GET /courses/:cid/gradebook/speed_grader.json?assignment_id=` returns the full student+submission payload.

**No-JS equivalent:** one page per student (query param `student_id`), prev/next as plain links, submission preview as direct file link/iframe, grade box as a form to `update_submission`, rubric table with per-criterion score inputs posting to rubric assessments URL, comments as a form + rendered list.

### Rust cross-check
| Endpoint | Rust status |
|---|---|
| `GET /api/v1/courses/:id/enrollments` (student list) | EXISTS — `star-courses` |
| `GET /api/v1/courses/:id/assignments/:aid/submissions` (all students) | EXISTS — `star-assignments` `/assignments/{id}/submissions` |
| `GET /api/v1/courses/:id/assignments/:aid/submissions/:uid` | EXISTS (variants) — per-user long-tail routes |
| `PUT /api/v1/courses/:id/gradebook/update_submission` (HTML form post) | MISSING — Rust has `PUT /submissions/{id}/grade` instead; UI port should post there or add alias |
| `POST /api/v1/courses/:id/rubric_associations/:rid/rubric_assessments` | EXISTS — `star-rubrics` `/rubrics/{rubricId}/assessments` (path differs from Canvas `/courses/:id/rubric_associations/...`; check exact route shape) |
| `GET /api/v1/courses/:id/rubrics` / rubric show | EXISTS — `star-rubrics` |
| `GET/POST /api/v1/submissions/:id/comments` | EXISTS — `star-submission-comments` |
| `GET /api/v1/courses/:id/sections` (section filter) | EXISTS |
| SpeedGrader JSON (`speed_grader.json`) | MISSING — build server-side from above endpoints |

---

## 8. Quiz-taking (Rust #109)

Routes (courses scope, `resources :quizzes, controller: "quizzes/quizzes"`):
- `GET /courses/:course_id/quizzes` => index; `GET /courses/:course_id/quizzes/:id` => show; edit/new; `GET .../quizzes/:id/history` => history (results per attempt); `GET .../quizzes/:id/statistics`; `GET .../quizzes/:id/read_only`; `GET .../quizzes/:id/managed_quiz_data`; `GET .../quizzes/:id/submission_versions`.
- Quiz submissions: `resources :quiz_submissions, controller: "quizzes/quiz_submissions", path: :submissions` -> `GET/POST /courses/:cid/quizzes/:qid/submissions` (start), `PUT|POST /courses/:cid/quizzes/:qid/submissions/backup` (save answers during taking), member `record_answer`, plus events log.
- API: `GET/POST /api/v1/courses/:cid/quizzes/:qid/submissions`, `POST /api/v1/quiz_submissions/:id/answer` (single answer), `/complete` (finish), `/time` (timer status).

### Quiz show (start page) — `quizzes/quizzes/show.html.erb`
Controller gates: deleted quiz -> flash + redirect to index; not visible to user -> error; locked handling `@locked/@locked_reason`; sets `@submission` (current user's), `@question_count`, preview flag. Sidebar (`provide :right_side`) has teacher actions: Quiz/Survey Statistics (if submitted_student_count > 0), Moderate This Quiz (`context_quiz_moderate_url`), See Full Quiz (`read_only`), download file-upload submissions, SpeedGrader link; student side has take/resume buttons:
```
<div class="take_quiz_button">  <!-- show.html.erb ~line 374-383 -->
  <%= link_to_take_or_retake_poll('class' => "btn btn-primary", 'id' => "take_quiz_link") %>
```
`#take_quiz_link` -> `GET/POST /courses/:cid/quizzes/:qid/take` (quiz_submissions#create then redirect to take_quiz page). Preview button (`#preview_quiz_button`, `preview=1`) for teachers. Teacher variant `_quiz_show_teacher.html.erb`; student `_quiz_show_student.html.erb` renders `_quiz_details` + instructions:
```
<ul id="quiz_student_details">  <!-- quizzes/quizzes/_quiz_details.html.erb -->
  Due (Multiple Due Dates / datetime / No due date), Points, Questions (count),
  Available/Until dates (shared/_available_dates),
  Time Limit ("Minute(s)" incl. extra_time), Allowed Attempts (extra_attempts + base, or Unlimited),
  "Requires Respondus LockDown Browser" note
```

### Taking the quiz — `quizzes/quizzes/take_quiz.html.erb`
```
<header class="quiz-header">
  h1 quiz title; preview alert; "Started" datetime; #quiz-instructions .user_content
  screenreader note: timed quiz, SHIFT+ALT+T to check remaining time
</header>
<%= form_tag @quiz_presenter.form_action(session, @current_user), id: "submit_quiz_form", method: "post" do %>
  <div id="questions" class="assessing">
    <input type="hidden" name="attempt" value="<%= @submission.attempt %>"/>
    <input type="hidden" name="validation_token" value="..."/>
    <input type="hidden" name="preview" value="1"/>              <!-- if preview -->
    <div id="quiz_urls" style="display:none">
      a.backup_quiz_submission_url -> .../submissions/backup?user_id=&preview=1
      span.started_at / .now / .end_at / .end_at_without_time_limit / .due_at / .time_limit /
        .timer_autosubmit_disabled / .time_left                <!-- ISO8601; JS timer source -->
    </div>
    <%= render partial: "display_question", collection: @quiz_presenter.current_questions, locals: {assessing: true} %>
    [one_question_at_a_time: #last_question_id hidden + Previous/Next submit buttons with data-action form actions]
  </div>
  <div id="times_up_dialog" style="display:none"> Time's Up! countdown + submit </div>
  <div class=form-actions>
    <span id="last_saved_indicator">Not saved</span>
    <button type="submit" class="btn submit_button quiz_submit" id="submit_quiz_button"
            data-action="<submit form action>">Submit Quiz</button>
  </div>
<% end %>
```
Timer semantics visible: `end_at` (with time limit), `end_at_without_time_limit`, autosubmit toggle, `time_left` — the JS computes remaining time from server timestamps; a no-JS port can render remaining time statically and treat the form POST as the only submission path.

Question rendering — `_display_question.html.erb` + `_form_question.html.erb`/`_single_answer.html.erb`/`_multi_answer.html.erb`:
```
<div class="quiz_sortable question_holder" id="question_template">
  <div class="display_question question <question_type> [marked|correct|incorrect|unanswered|partial_credit]"
       id="question_<id>">
    header: question name, points ("pts"), [mark-for-review toggle when assessing]
    .text .question_text (user_content)
    answers:
      radio:      <input type="radio" name="question_<id>" value="<answer_id>" class="question_input">
      checkbox:   hidden 0 + <input type="checkbox" name="question_<id>_answer_<answer_id>" value="1">
      text_box:   <input type="text" name="question_<id>" class="question_input" autocomplete=off>
      textarea:   essay -> rich text area (RCE) name="question_<id>"
      numerical:  input.question_input with precision classes
      matching:   label (left) + <select name="question_<id>_answer_<answer_id>"> of matches
      file:       file input (attachment_ids)
```
Question types (`app/helpers/quizzes_helper.rb` `answer_type` registry + `_form_question.html.erb` select): `multiple_choice_question` (radio), `true_false_question` (radio), `short_answer_question` (text), `fill_in_multiple_blanks_question`, `multiple_answers_question` (checkbox), `multiple_dropdowns_question` (select), `matching_question`, `numerical_question`, `calculated_question`, `missing_word_question` (select), `essay_question` (textarea), `file_upload_question`, `text_only_question` (no input).

### Results — `_quiz_submission.html.erb` / `_quiz_submission_results.html.erb` (rendered on quiz show after submission, and `history` page per version)
- `render_result_protection`: one_time_results / hide_results=until_after_last_attempt messages when results not visible.
- Score block: "Score for this attempt" `score_out_of_points_possible(@submission.score, points_possible)`; pending_review -> "Some questions not yet graded" link to history.
- If results visible: iterates `@submission.questions` + `submission_data` answers; each question shows correct/incorrect/partial/unanswered classes, user answers marked (`*` selected, `+` correct weight), plus correct answers when `show_correct_answers?`.
- Attempt/version switching: `submission_versions` view; `history` page shows per-attempt list.

### Rust cross-check
| Endpoint | Rust status |
|---|---|
| `GET /api/v1/courses/:id/quizzes` | EXISTS — `star-quizzes` |
| `GET /api/v1/courses/:id/quizzes/:qid` | EXISTS |
| `POST /api/v1/courses/:id/quizzes/:qid/submissions` (start attempt) | EXISTS — `start_course_quiz_submission` |
| `GET /api/v1/courses/:id/quizzes/:qid/submissions` (list) | EXISTS |
| `GET /api/v1/quizzes/:id/submissions/me` | EXISTS — `/quizzes/{id}/submissions/me` |
| `GET /api/v1/quiz_submissions/:id` (single submission w/ attempt, validation_token, end_at) | EXISTS (nested variants `/courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}`) |
| `POST /api/v1/quiz_submissions/:id/answer` | MISSING — no `/quiz_submissions/{id}/answer` route (backup/record_answer HTML routes also missing) |
| `POST /api/v1/quiz_submissions/:id/complete` (submit) | MISSING — verify; not in router |
| `GET /api/v1/quiz_submissions/:id/time` | MISSING |
| `GET /api/v1/courses/:id/quizzes/:qid/questions` (question payloads) | EXISTS — `/courses/{courseId}/quizzes/{quizId}/questions` |
| `GET /courses/:id/quizzes/:qid/history` (HTML results) | MISSING (UI route; server-render) |

---

## 9. Files Browser (Rust #110)

Routes: `GET /courses/:course_id/files` => `files#index` (`react_files` method — renders empty layout page with `js_bundle :files` or `:files_v2`, body classes `full-width padless-content`, env `FILES_CONTEXTS` (root folders + rights per context), `COURSE_ID`). `GET /courses/:cid/files/:id` => `files#show` — template `app/views/files/show.html.erb` (real server-rendered page).

File show page structure:
```
<h2><%= @attachment.display_name %></h2>
<div> <%= link_to "Download #{name}", download_url, download: "true" %> (<%= @attachment.readable_size %>) </div>
<!-- preview branching -->
flash: <object class="embedded_swf">                  (shockwave)
inline non-canvadoc: <iframe id="file_content" sandbox="..." src="safe_domain_file_url">
image: <%= link_to image_tag(download_url), download_url %>
audio/video: <div id="media_preview" ...>
other (PDF/doc): <div id="doc_preview" ...>           (canvadoc preview, JS)
locked: folder-locked message or lock_explanation
+ shared/_sequence_footer (module next/previous)
```
Download URL: `context_file_download_url` -> `/courses/:cid/files/:id/download?download_frd=1&verifier=...`.

Files index (React) data endpoints: `GET /api/v1/folders/:id` (folder with `files_url`, `folders_url`), `GET /api/v1/folders/:id/files` (paginated file rows: display_name, size, content_type, updated_at, hidden, locked, usage_rights fields, url), `GET /api/v1/folders/:id/folders` (subfolders), `GET /api/v1/courses/:id/folders/root`, upload: `POST /api/v1/courses/:id/files` (preflight -> upload URL -> success), quota: `GET /api/v1/courses/:id/files/quota` -> `{quota, quota_used}` (rendered as a progress bar dialog; the port should render a static bar + text).

DOM hooks the React files app expects (port equivalents as plain HTML):
- Folder tree in left pane; breadcrumb of folder names; file list table rows (name, size, date, publisher, usage rights indicator).
- Upload: "Upload" button opens dialog with drag-drop area — port as `<form enctype="multipart/form-data">` with `<input type="file" multiple>` posting to the preflight endpoint.
- Usage rights dialog: fields `usage_rights[use_justification]` (select: own_copyright, used_by_permission, public_domain, used_in_education_containers fair_use, creative_commons), `usage_rights[license]` (select of licenses), `usage_rights[legal_copyright]` (text), file_ids[]; PUT `/api/v1/courses/:id/usage_rights`, DELETE to remove. License list: `GET /api/v1/courses/:id/content_licenses`.
- Quota errors: API returns 400/403 on exceeding quota; UI surfaces "Quota exceeded for this folder..." message (port: render flash error).

### Rust cross-check
| Endpoint | Rust status |
|---|---|
| `GET /api/v1/folders/:id` | EXISTS — `star-files` |
| `GET /api/v1/folders/:id/files` / `/folders` | EXISTS |
| `POST /api/v1/folders` / `POST /api/v1/folders/:id/folders` | EXISTS |
| `GET /api/v1/courses/:id/folders/root` | EXISTS |
| `GET /api/v1/courses/:id/folders_and_files` (v2 flat tree) | EXISTS |
| `GET /api/v1/courses/:id/files` | EXISTS |
| `POST /api/v1/courses/:id/files` (upload preflight) | EXISTS |
| `GET /api/v1/files/:id` | EXISTS |
| `PUT /api/v1/files/:id` (rename/move/hidden) | EXISTS |
| `DELETE /api/v1/files/:id` | EXISTS |
| `GET /api/v1/courses/:id/files/quota` | EXISTS — `/courses/{courseId}/files/quota` |
| `PUT/DELETE /api/v1/courses/:id/usage_rights` | EXISTS (course scope; groups/users delete + course put) |
| `GET /api/v1/courses/:id/content_licenses` | EXISTS — `/courses/{courseId}/content_licenses` |
| `GET /courses/:id/files/:id/download` (HTML file download) | MISSING as HTML route — API file url exists; port must add download route |

---

## 10. Announcements + Syllabus + Calendar (Rust #111)

### Announcements index
Route: `GET /courses/:course_id/announcements` => `announcements#index` (HTML). View `announcements/index.html.erb` is now essentially empty (page title + feed code) — the list is React (`js_bundle :announcements`, `css_bundle :announcements_index`). Controller `load_announcements` sets permissions env (`create, moderate, manage_course_content_edit/delete`), `atom_feed_url`, `ANNOUNCEMENTS_LOCKED`. Atom/RSS feed links emitted when feed_key available.

Data: `GET /api/v1/announcements?context_codes[]=course_...&start_date&end_date&active_only` (AnnouncementsApiController) or `GET /api/v1/courses/:id/discussion_topics?only_announcements=true`. Row fields: title, posted_at, author, unread (per user), message HTML, pinned.

### Announcement / topic detail
`GET /courses/:course_id/announcements/:id` => `announcements#show` -> redirect to `/courses/:cid/discussion_topics/:tid` (topic show, section 5). Announcement rendering = discussion topic show with announcement semantics (posted_at, delayed_post, lock, rss).

### Syllabus
Route: `GET /courses/:course_id/assignments/syllabus` => `assignments#syllabus` (collection route under assignments resource; `syllabus_course_assignments_path`). Course home can also render it (`course_home_view == 'syllabus'`). Template `assignments/syllabus.html.erb` -> `_syllabus_content.html.erb` (this one IS server-rendered):

```
<div class="ic-Action-header">
  <h1> Course Syllabus (or course nickname when course_home) </h1>
  [Jump to Today link] [edit_syllabus_link button "Edit" (manage_course_content_edit)]
</div>
<div id="course_syllabus" class="user_content" data-resource-type="syllabus.body">
  <%= @syllabus_body %>                       <!-- syllabus_user_content -->
</div>
<div id="course_syllabus_details"> help text (when empty + can edit) </div>
<form id="edit_course_syllabus_form" method="put" action="<context_url>">
  <label> Syllabus Description </label>
  f.text_area :syllabus_body (RCE)
  submit "Update Syllabus"
</form>
<table id="syllabus" class="ic-Table">        <!-- "Course Summary" table (JS date sorting) -->
  <caption> Course Summary </caption>
  thead: Due Date | Title | Details (points)
  rows from @groups -> assignments (due_at, title link, points, submission types)
</table>
```
Controller: `@groups = @context.assignment_groups.active.order(:position)`, `@syllabus_body = syllabus_user_content`; permission `[:read, :read_syllabus]` + `tab_enabled?(TAB_SYLLABUS)`; crumb "Syllabus"; `ENV.CONTEXT_ACTION_SOURCE = :syllabus`.
Right side `_syllabus_right_side`: "Add Assignment" (manage), weights info (`_sidebar_weighting`).

### Calendar
Route: `GET /calendar` (also `/calendar2`) => `calendars#show`. Template `app/views/calendars/show.html.erb`. Body class `full-width no-page-block`; crumbs cleared. Server-rendered chrome + right sidebar; the grid itself is React (`js_bundle :calendar`, `#calendar-app`, `#calendar-drag-and-drop-container`):

```
<h1 class="screenreader-only">Calendar</h1>
right-side:
  <div id="minical"></div>                          <!-- mini month calendar (JS) -->
  <div id="select-course-component"></div>          <!-- "Create New Event" / course picker -->
  .rs-section: #calendar-toggle-button (element_toggler) -> #calendar-list-holder
      (context checkboxes: my calendars / course calendars / account calendars)
  #select-other-calendars-component, #other-calendars-list-holder (account calendars)
  #undated-events-section: #undated-events-button -> #undated-events list
  #calendar-feed: #calendar-feed-button -> #calendar_feed_box dialog
      (#calendar-feed-url-input value=<feed_url>.ics + link)
main:
  <div id="calendar_header"></div>                  <!-- month/week/agenda buttons + prev/next (JS) -->
  <div id="calendar-app" role="tabpanel"></div>     <!-- month/week grid -->
  <div id="calendar-drag-and-drop-container"></div>
```
Controller `calendars#show`: `get_all_pertinent_contexts(include_groups, include_accounts, favorites_first)`, `@manage_contexts` (context asset_strings with `manage_calendar`), `@feed_url = feeds_calendar_url(feed_code)`, `@selected_contexts` from user preference or `?include_contexts=`. ENV `CALENDAR.CONTEXTS` (context JSON: name, asset_string, can_create_events color), `MANAGE_CONTEXTS`, `SELECTED_CONTEXTS`, `ACTIVE_EVENT`, `VIEW_START`, `SHOW_SCHEDULER`.

Event data: `GET /api/v1/calendar_events?context_codes[]=&start_date=&end_date=&type=assignment|event` (assignments carry due_at; events start_at/end_at; fields: title, description, location_name, all_day, important_dates). Event rendering partial `calendars/_event.html.erb` (legacy server template showing the shape):
```
<div class="calendar_event [assignment] group_course_123 draggable" id="event_{assignment|calendar_event}_{id}">
  <div class="calendar_event_text">
    <span class="due_text">due</span> (assignments)
    <a class="title" href="assignment or event URL"> title </a>
```
Month view = 7-column grid of days, each cell lists events (assignment icon + title, colored by context); week view = 7 columns by time rows. Create event form fields (from API + dialog): title, date, start/end time (or all_day), location, calendar (context select), details textarea — `POST /api/v1/calendar_events` with `calendar_event[context_code], [title], [start_at], [end_at], [description], [location_name]`.

### Rust cross-check
| Endpoint | Rust status |
|---|---|
| `GET /api/v1/announcements` (multi-context) | EXISTS — `star-announcements` `/courses/{courseId}/announcements` (course-scoped; check multi-context variant) |
| `GET /api/v1/courses/:id/announcements` | EXISTS |
| `POST/PUT/DELETE /api/v1/announcements/:id` | EXISTS |
| `GET /api/v1/announcements/:id/show_async` | MISSING |
| `GET/PUT /api/v1/courses/:id` (syllabus_body via course update) | EXISTS — `star-courses` (syllabus_body is a course attribute; no dedicated `/syllabus` endpoint) |
| `GET /api/v1/courses/:id/assignment_groups` + `assignments` (Course Summary table) | EXISTS |
| `GET /api/v1/calendar_events` (user scope) | EXISTS — `star-calendar` `/calendar_events` |
| `GET /api/v1/courses/:id/calendar_events` | EXISTS |
| `POST /api/v1/calendar_events` | EXISTS (course + own) |
| `GET/PUT/DELETE /api/v1/calendar_events/:id` | EXISTS |
| `GET /api/v1/calendar_events/visible_contexts` | EXISTS |
| `POST /api/v1/calendar_events/save_selected_contexts` | EXISTS |
| `GET /calendars/:feed_code` (iCal feed) | MISSING — feeds are HTML/ics routes |
| `GET /api/v1/planner_notes` | EXISTS — `star-calendar` |
| `GET /api/v1/planner/items` (dashboard planner) | MISSING |

---

## 11. Porting recommendations per Rust issue

### #43 Shell/layout
Essential for parity:
- Overall DOM skeleton: `#application.ic-app > #header.ic-app-header + #wrapper.ic-Layout-wrapper > crumbs bar + #main.ic-Layout-columns(#left-side + #not_right_side(#content + #right-side))`. Keep ids/classes as close as possible (CSS parity, tests).
- Global nav items in order: Account(or Login), Admin, Dashboard, Courses, Groups, Calendar, Inbox, Help — with the documented link ids.
- Section tabs from `GET /api/v1/courses/:id/tabs`, `#section-tabs` ul with `<label>-link` anchors, active class from current tab, hidden/unused indicators.
- Breadcrumbs (`crumbs.length > 1` gate), body class conventions (`with-left-side`, `with-right-side`, `full-width`, `context-*`, active tab).
- Flash notices rendered server-side (Canvas defers to `ENV.notices` + JS; port should output the actual notice markup into `#flash_message_holder`).
Skippable first pass: global nav collapse/toggle JS, mobile header, help tray, react portals (#global_nav_tray_container etc.), wizard box, keyboard navigation dialog, microfrontends.

### #44 Dashboard + course home
Essential: dashboard cards grid (`.ic-DashboardCard` with shortName, courseCode, term, subtitle, links from tabs, color/image, favorite star as form), to-do sidebar (legacy `courses/_to_do_list` structure with `.todo-badge` counts), Coming Up / Recent Feedback event lists, "Start a New Course" form, View Grades link; course home default_view branching (wiki front page / modules list / assignments index / syllabus / feed stream) rendered server-side, course home right sidebar (publish state, import, new announcement, join course, notifications).
Skippable: planner (JS-only; replace with server to-do), dashboard view toggle (cards/activity/planner), drag-reorder of cards (dashboard_positions API exists if wanted later), k5/horizon variants, observer picker, announcements-on-home React widget (render top N announcements server-side instead), confetti/eportfolio widgets.
Note: `/dashboard-sidebar` and `/users/self/missing_submissions` are MISSING in Rust — implement sidebar data server-side or add the endpoint.

### #70 Auth
Essential: login page (`.ic-Login` markup, `#login_form` with `pseudonym_session[unique_id]` + `pseudonym_session[password]`, remember_me checkbox, forgot-password as a separate GET page rather than JS toggle), logout confirm page + `DELETE /logout`, `session[:return_to]` store/restore on 401 (not `?next=`), flash "You must be logged in to access this page", registration landing + self-registration form, forgot-password POST always flashing the same neutral message, change-password form with nonce validation + expiry.
Skippable: SSO providers (SAML/CAS/LDAP/GitHub/etc. — keep a provider hook), OneTrust/consent scripts, mobile login layout, OTP/MFA flow (unless required), discovery URL redirect.
Rust gap: password reset endpoint missing — `star-auth` needs `POST /forgot_password`-equivalent + nonce confirmation page.

### #71 Assignments/gradebook
Essential: assignments index server-rendered by assignment groups (name, due, points, types, publish state); assignment show with details header (Due/Points/Available), description `user_content`, teacher buttons (publish, edit, speedgrader), student submission forms (text/url/file posting to submissions API); student grades page — the `#grades_summary` table is the single most complete server-rendered artifact, port it directly (Name/Due/Submitted/Status/Score + total sidebar, group weights, grading period select as GET form).
Skippable: gradebook grid (JS-only; server table of students x submissions with inline grade forms posting to `PUT /submissions/{id}/grade` — note Canvas's HTML post target `update_submission` is MISSING in Rust), custom gradebook columns, What-If scores (client-side math in Canvas; can be a later progressive-enhancement), CSV export, moderation/provisional grades, post policies UI.
Missing in Rust: `/courses/:id/gradebook/user_ids`, custom_gradebook_columns, gradebook_csv.

### #72 Discussions/inbox
Essential: discussions index table (title, replies count, unread, last reply, pinned/locked) with `?order_by=&filter_by=&scope=` query support server-side; topic show as threaded entries (`_sub_entry.html.erb` blueprint: `.communication_sub_message` with reply form `POST .../entries` and `.../entries/:id/replies`); mark-read/unread and subscribe as small forms; inbox: two-pane conversations layout server-rendered (list pane with scope filters as links, detail pane with messages + `add_message` form), compose form (recipients select fed by find_recipients, subject, body, course context).
Skippable: rich text editor (plain textarea; RCE is a later enhancement), media comments, submission-comment inbox section, rating entries, deep threading UI beyond nested lists, search autocomplete (plain GET form).
All needed REST endpoints EXIST in star-discussions/star-conversations.

### #73 Settings/admin
Essential: settings page as real tabs (Details form with the `course[...]` field list above posting to `PUT /courses/:id`; Sections list + add form; Navigation tab with move up/down/hidden checkboxes posting to tab update — note tab_configuration update endpoint MISSING in Rust; Feature Options flags), people page as roster table (name, role/section from enrollments, last activity) + add-people form (login list + role select posting to enrollments API), prior users page; account admin: accounts list, account course/user search as server tables with filters, account settings form.
Skippable: drag-and-drop navigation reorder (use up/down buttons), external apps (LTI) tabs, grade syncing, alerts, integrations, quiz settings, criteria, React add-people wizard (multi-step w/ validation), masquerade.

### #108 SpeedGrader
Essential: URL shape `/courses/:id/gradebook/speed_grader?assignment_id=&student_id=`; page regions: header (assignment title/due, student prev/next links + select as GET form), submission viewer (file link + iframe/inline preview, attempt list as links `?student_id=&submission_id=`), grading box form (score posting to `PUT /submissions/{id}/grade` — Canvas's `update_submission` aggregate post is MISSING in Rust, so post per-submission), rubric table with per-criterion score inputs + comments posting to `star-rubrics` assessments route, comments list + add-comment form (with group_comment radio).
Data needed per student: submission JSON (history, attachments, score, state), rubric + existing assessments, comments (star-submission-comments), enrollment flags (inactive/concluded).
Skippable: annotated document viewer (DocViewer), media comments, comment library, grade-by-question, moderation, anonymous mode UI complexities, keyboard shortcuts, statistics header (Graded/Average) can be computed server-side.

### #109 Quizzes
Essential: quiz show start page (`ul#quiz_student_details` facts + Take Quiz button -> POST start submission -> redirect to taking page); taking page: one `<form>` with hidden `attempt`/`validation_token`, question partials per type (radio/checkbox/text/textarea/select/file inputs with `question_<id>` names), per-question points, Submit Quiz button; timer as server-rendered remaining time (end_at stored on submission) with plain "time is up" behavior on submit; results page: score for attempt, per-question correct/incorrect rendering (reuse display_question with user_answer), attempt history links.
Critical Rust gap: quiz answer recording/submission endpoints are missing (`POST /quiz_submissions/:id/answer|complete|time`, backup) — the UI port needs at minimum a bulk "submit answers" route (the classic HTML form posts all `question_<id>` params to the submission URL) and a server-side complete/grade step.
Skippable: one-question-at-a-time (render all questions; OQAT can be progressive), RCE in essay (textarea), lockdown browser, IP filtering UI, quiz statistics, regrade tooling, flag-for-review, matching shuffle, New Quizzes (LTI).

### #110 Files
Essential: files index server-rendered: left folder tree (nested ul from `/folders` API incl. `folders_and_files` flat variant), breadcrumb, file table rows (name link, size, updated, hidden/locked icons), upload form (multipart, posts to create_file preflight then upload URL), "New Folder" form, quota bar from `/files/quota`; file show page (port `files/show.html.erb` nearly verbatim: download link, image/audio/video/iframe preview branching); usage rights form (use_justification select, license select from content_licenses, legal_copyright text) posting to `PUT /courses/:id/usage_rights`; quota error flash on 400.
Skippable: drag-and-drop upload, inline previews via canvadoc (link to download instead), file preview thumbnails, multi-select bulk operations (single-row actions first), usage-rights batch over selections, media tracking.
All REST endpoints EXIST in star-files except a plain HTML download route (add `/courses/:id/files/:fid/download` in the UI layer).

### #111 Announcements/syllabus/calendar
Essential: announcements index as server table (title, posted_at, author, unread marker) with pagination + RSS/atom link; announcement detail = discussion topic show (see #72) with announcement header (posted date, lock state); syllabus page — port `_syllabus_content.html.erb` directly (it is server-rendered in Canvas: `#course_syllabus` body, hidden edit form, `#syllabus` Course Summary table from assignment groups) + edit form posting syllabus_body to course update; calendar: month grid (7 cols x 5-6 rows, event chips `.calendar_event` colored per context, assignment vs event distinction, link to assignment/event), week view (time rows), context list sidebar as checkboxes (GET form with `include_contexts[]`), create-event form (title/date/start/end/location/calendar/details) posting to `POST /calendar_events`, undated events list, calendar feed link (render `.ics` URL; feed route missing in Rust — acceptable to defer).
Skippable: mini-calendar, drag-drop reschedule, scheduler/appointment groups UI, account calendar management, agenda view (later), important_dates styling, JS timezone switching.

---

## Appendix: quick endpoint -> Rust crate map (verified by grepping `migration/crates/*/src/lib.rs` routers)

| Canvas REST | Rust crate | Status |
|---|---|---|
| `/courses`, `/courses/:id`, `/courses/:id/settings`, `/courses/:id/tabs`, `/courses/:id/permissions`, `/courses/:id/users`, `/courses/:id/search_users`, `/courses/:id/sections`, `/courses/:id/enrollments`, `/courses/:id/activity_stream` | star-courses | EXISTS |
| `/dashboard/dashboard_cards`, `/users/self/todo`, `/users/self/upcoming_events`, `/users/self/favorites/courses`, `/release_notes` | star-dashboard | EXISTS |
| `/auth/register|login|me|logout` (custom), `/login/oauth2/token` | star-auth | EXISTS (custom shape, not Canvas-compatible paths) |
| `/courses/:id/assignments`, `/assignments/:id`, `/assignments/:id/submissions[...]`, `/submissions/:id/grade`, `/courses/:id/gradebook` | star-assignments | EXISTS |
| `/courses/:id/assignment_groups[...]`, group rules, group assignments | star-assignment-groups | EXISTS |
| `/conversations[...]`, `/conversations/find_recipients`, `/conversations/mark_all_as_read`, unread_count | star-conversations | EXISTS |
| `/courses/:id/discussion_topics[...]`, entries, replies, read/subscribe, `/read_all` | star-discussions | EXISTS |
| `/courses/:id/quizzes[...]`, quiz submissions (list/start/me/single), questions, groups, reports | star-quizzes | EXISTS |
| quiz answer/complete/time recording | — | MISSING |
| `/folders[...]`, `/courses/:id/files[...]`, quota, usage_rights, content_licenses, by_path | star-files | EXISTS |
| `/courses/:id/announcements[...]`, `/announcements/:id` | star-announcements | EXISTS |
| `/calendar_events[...]`, visible_contexts, save_selected_contexts, `/planner_notes[...]` | star-calendar | EXISTS |
| `/planner/items` | — | MISSING |
| `/users/self/missing_submissions` | — | MISSING |
| `/accounts[...]`, `/accounts/:id/courses|users`, help_links, permissions | star-accounts | EXISTS |
| `/courses/:id/rubrics`, `/rubrics/:id/assessments` | star-rubrics | EXISTS |
| `/courses/:id/grading_standards`, `/courses/:id/final_grade`, grading_periods | star-grading-standards | EXISTS |
| `/submissions/:id/comments` | star-submission-comments | EXISTS |
| `/courses/:id/modules[...]`, `/courses/:id/modules/:mid/items` | star-modules | EXISTS |
| `/courses/:id/pages[...]`, `/courses/:id/front_page` | star-pages (via star-content) | EXISTS |
| `/courses/:id/features` (feature flags) | star-features | EXISTS |
| `/courses/:id/custom_gradebook_columns` | — | MISSING |
| `/courses/:id/gradebook/user_ids`, `/courses/:id/gradebook_csv` | — | MISSING |
| `/courses/:id/tab_configuration` (PUT reorder) | — | MISSING |
| HTML-only routes (`/dashboard-sidebar`, `/courses/:id/grades`, `/courses/:id/gradebook/update_submission`, `/forgot_password`, quizzes take/backup/history, file download, calendar feeds) | — | MISSING (must live in the star-lms UI server, not the API crates) |
