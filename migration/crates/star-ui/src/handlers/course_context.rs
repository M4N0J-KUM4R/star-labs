//! Shared helpers for the course-scoped journey pages (assignments, grades,
//! gradebook).
//!
//! Every course-scoped shell page repeats the same prologue: gate through
//! `star_courses::load_course_for_viewer` (404-before-403, exactly like the
//! course home page and `GET /api/v1/courses/{id}`), load the section tabs,
//! preferences, display name, and issue the CSRF cookie. This module keeps
//! that prologue in one place so the journeys cannot drift from the shell's
//! authorization contract (spec §4, acceptance criterion 7).

use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;

use star_common::{ApiError, AppState};
use star_courses::{Course, CourseTab};

use crate::handlers::error::from_api_error;
use crate::session::SessionUser;
use crate::{env::JsEnv, queries};

/// The AuthUser view of a session user for the domain-crate gates (they take
/// `&AuthUser` for their enrolled-or-admin checks).
pub(crate) fn auth_of(user: &SessionUser) -> star_auth::AuthUser {
    star_auth::AuthUser {
        id: user.id,
        role: user.role.clone(),
    }
}

/// The shared prologue result for a course-scoped page.
pub(crate) struct CourseContext {
    pub course: Course,
    pub tabs: Vec<CourseTab>,
    pub preferences: crate::env::EnvPreferences,
    pub use_high_contrast: bool,
    pub user_name: String,
    /// `true` when the viewer is a teacher of this course (or an admin).
    pub is_teacher: bool,
    /// `true` when the viewer holds a student enrollment in this course.
    pub is_student: bool,
}

/// Run the course-scoped prologue: course gate, tabs, preferences, name and
/// the teacher/student enrollment flags used by the journeys. `Err` carries
/// the ready-to-render error response.
pub(crate) async fn course_context(
    app: &AppState,
    user: &SessionUser,
    course_id: i64,
    origin: &str,
) -> Result<CourseContext, Box<Response>> {
    // Boxed: `Response` is large enough to trip clippy::result_large_err, and
    // every caller only ever returns the error as-is.
    let course = star_courses::load_course_for_viewer(&app.db, course_id, &auth_of(user))
        .await
        .map_err(|e| Box::new(from_api_error(&e, origin).into_response()))?;

    let tabs = star_courses::load_course_tabs(&app.db, course_id, &auth_of(user))
        .await
        .map_err(|e| Box::new(from_api_error(&e, origin).into_response()))?;

    let (preferences, use_high_contrast) = queries::load_preferences(&app.db, user.id)
        .await
        .map_err(|e| Box::new(from_api_error(&e, origin).into_response()))?;

    let user_name = queries::load_user_name(&app.db, user.id)
        .await
        .map_err(|e| Box::new(from_api_error(&e, origin).into_response()))?
        .unwrap_or_default();

    let is_teacher =
        star_courses::check_teacher_or_admin(&app.db, user.id, course_id, &auth_of(user))
            .await
            .map_err(|e| Box::new(from_api_error(&e, origin).into_response()))?;

    let is_student = !star_courses::is_admin(&auth_of(user))
        && sqlx::query_scalar::<_, i32>(
            r#"SELECT 1 FROM enrollments
               WHERE course_id = $1 AND user_id = $2 AND role = 'student'
                 AND workflow_state <> 'deleted'
               LIMIT 1"#,
        )
        .bind(course_id)
        .bind(user.id)
        .fetch_optional(&app.db)
        .await
        .map_err(|e| Box::new(from_api_error(&ApiError::from(e), origin).into_response()))?
        .is_some();

    Ok(CourseContext {
        course,
        tabs,
        preferences,
        use_high_contrast,
        user_name,
        is_teacher,
        is_student,
    })
}

/// Build the ENV payload for a course-scoped journey page.
pub(crate) fn course_env(
    ctx: &CourseContext,
    user: &SessionUser,
    origin: &str,
    active_tab: &str,
    course_id: i64,
) -> JsEnv {
    let context_asset_string = format!("course_{course_id}");
    JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences.clone(),
        ctx.use_high_contrast,
        origin,
        Some(active_tab),
        &context_asset_string,
        Vec::new(),
    )
}

/// The standard breadcrumb for course-scoped pages: Dashboard / course.
pub(crate) fn course_breadcrumb(ctx: &CourseContext, course_id: i64) -> Vec<(String, String)> {
    vec![
        ("Dashboard".to_string(), "/dashboard".to_string()),
        (ctx.course.name.clone(), format!("/courses/{course_id}")),
    ]
}

/// Ensure the CSRF cookie, shared by every page handler that renders a form.
pub(crate) fn ensure_csrf(jar: CookieJar) -> (CookieJar, String) {
    crate::session::ensure_csrf_cookie(jar)
}
