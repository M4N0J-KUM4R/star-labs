//! Quick-jump command bar (Precision Workspace): `GET /quick-jump?q=…`
//! matches the query against the user's courses and their published
//! assignments by name; a single hit redirects straight to it, otherwise
//! the matches are listed for one more click.

use askama::Template;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};

use star_common::{ApiError, AppState};

use crate::handlers::error::from_api_error;
use crate::session::SessionUser;
use crate::{assets, env::JsEnv, queries};

#[derive(serde::Deserialize, Default)]
pub struct QuickJumpQuery {
    pub q: Option<String>,
}

#[derive(Template)]
#[template(path = "quick_jump.html")]
pub struct QuickJumpPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub unread_badge: i64,
    pub query: String,
    pub courses: Vec<QuickJumpCourse>,
    pub assignments: Vec<QuickJumpAssignment>,
}

pub struct QuickJumpCourse {
    pub href: String,
    pub name: String,
    pub course_code: String,
}

pub struct QuickJumpAssignment {
    pub href: String,
    pub title: String,
    pub course_code: String,
}

impl IntoResponse for QuickJumpPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("quick-jump template render failed: {err}")).into_response()
            }
        }
    }
}

/// GET /quick-jump?q=… — resolve the query or list the matches.
pub async fn quick_jump(
    State(app): State<AppState>,
    user: SessionUser,
    jar: axum_extra::extract::CookieJar,
    Query(q): Query<QuickJumpQuery>,
) -> Response {
    let origin = crate::request_origin();
    let term = q.q.unwrap_or_default().trim().to_string();
    if term.is_empty() {
        return page(&app, &user, jar, origin, String::new(), Vec::new(), Vec::new()).await;
    }
    let pattern = format!("%{}%", term.replace('%', ""));
    let courses: Vec<QuickJumpCourse> =
        match sqlx::query_as::<_, (i64, String, String)>(
            r#"
            SELECT DISTINCT c.id, c.name, c.course_code
            FROM courses c
            JOIN enrollments e ON e.course_id = c.id AND e.user_id = $1
                 AND e.workflow_state <> 'deleted'
            WHERE c.workflow_state <> 'deleted'
              AND (c.name ILIKE $2 OR c.course_code ILIKE $2)
            ORDER BY c.name
            LIMIT 8
            "#,
        )
        .bind(user.id)
        .bind(&pattern)
        .fetch_all(&app.db)
        .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|(id, name, code)| QuickJumpCourse {
                    href: format!("/courses/{id}"),
                    name,
                    course_code: code,
                })
                .collect(),
            Err(e) => return ApiError::Db(e.to_string()).into_response(),
        };
    let assignments: Vec<QuickJumpAssignment> =
        match sqlx::query_as::<_, (i64, i64, String, String)>(
            r#"
            SELECT DISTINCT a.id, a.course_id, a.title, c.course_code
            FROM assignments a
            JOIN courses c ON c.id = a.course_id
            JOIN enrollments e ON e.course_id = c.id AND e.user_id = $1
                 AND e.workflow_state <> 'deleted'
            WHERE a.workflow_state = 'published' AND a.title ILIKE $2
            ORDER BY a.title
            LIMIT 8
            "#,
        )
        .bind(user.id)
        .bind(&pattern)
        .fetch_all(&app.db)
        .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|(id, course_id, title, code)| QuickJumpAssignment {
                    href: format!("/courses/{course_id}/assignments/{id}"),
                    title,
                    course_code: code,
                })
                .collect(),
            Err(e) => return ApiError::Db(e.to_string()).into_response(),
        };

    if courses.len() == 1 && assignments.is_empty() {
        return Redirect::to(&courses[0].href).into_response();
    }
    if assignments.len() == 1 && courses.is_empty() {
        return Redirect::to(&assignments[0].href).into_response();
    }
    page(&app, &user, jar, origin, term, courses, assignments).await
}

#[allow(clippy::too_many_arguments)]
async fn page(
    app: &AppState,
    user: &SessionUser,
    jar: axum_extra::extract::CookieJar,
    origin: String,
    query: String,
    courses: Vec<QuickJumpCourse>,
    assignments: Vec<QuickJumpAssignment>,
) -> Response {
    let (preferences, use_high_contrast) = match queries::load_preferences(&app.db, user.id).await
    {
        Ok(loaded) => loaded,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let settings = match queries::load_settings(&app.db, user.id).await {
        Ok(s) => s,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let user_name = match queries::load_user_name(&app.db, user.id).await {
        Ok(name) => name.unwrap_or_default(),
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let unread_badge = match star_conversations::ui::load_unread_count(&app.db, user.id).await {
        Ok(count) => count,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let mut env = JsEnv::for_user(
        user.id,
        &user.role,
        preferences,
        use_high_contrast,
        &origin,
        None,
        "",
        Vec::new(),
    );
    env.settings = settings;
    let (jar, csrf_token) = crate::session::ensure_csrf_cookie(jar);

    let page = QuickJumpPage {
        title: "Quick jump".to_string(),
        body_class: "quick-jump-page".to_string(),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name,
        active_path: "dashboard",
        breadcrumb: vec![("Dashboard".to_string(), "/dashboard".to_string())],
        csrf_token,
        unread_badge,
        query,
        courses,
        assignments,
    };
    (jar, page).into_response()
}
