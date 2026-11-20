//! Grades + gradebook journey handlers (issue #71) — Canvas
//! `gradebooks#grade_summary` (`/grades`) and `gradebooks#show`
//! (`/gradebook`, `/gradebook.csv`), server-rendered.
//!
//! `/grades` shows one student's own scores + course total (a teacher may
//! pass `?student_id=`); `/gradebook` renders the teacher's grid
//! (rows = enrollments, columns = assignments by group+position); the CSV is
//! computed server-side from the same data (no gradebook_csv API exists —
//! documented deviation). The interactive grading UI (SpeedGrader) is
//! issue #108.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use star_assignments::{
    compute_grade_percent, grade_arrays_from_submissions, load_course_assignments_with_groups,
    load_gradebook_rows, UiAssignment,
};
use star_common::{ApiError, AppState};

use crate::assets;
use crate::handlers::course_context::{course_breadcrumb, course_context, course_env};
use crate::handlers::error::from_api_error;
use crate::session::SessionUser;

/// Presentation-ready row of the grades summary table (student-facing).
/// All display strings are precomputed so the template only does field
/// access.
pub struct GradeRowView {
    pub assignment_id: i64,
    pub title: String,
    pub group_name: Option<String>,
    /// "Jan 5, 2026" or "–".
    pub due_label: String,
    /// "8 / 10" or "–".
    pub score_label: String,
    /// "graded" | "submitted" | "unsubmitted".
    pub pill_class: String,
    /// "Graded" | "Submitted" | "Not Submitted".
    pub pill_label: String,
}

/// The grades page template context.
#[derive(Template)]
#[template(path = "grades.html")]
pub struct GradesPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    /// Whose grades these are (the student's display name).
    pub student_name: String,
    pub rows: Vec<GradeRowView>,
    /// "17 / 20".
    pub total_label: String,
    /// "85%" — "–" when nothing is graded.
    pub percent_label: String,
    /// Letter grade from the course's active grading standard, when one
    /// exists (star-grading-standards compute_letter_grade).
    pub letter_grade: Option<String>,
    pub is_teacher: bool,
}

impl IntoResponse for GradesPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("grades template render failed: {err}")).into_response()
            }
        }
    }
}

#[derive(Deserialize)]
pub struct GradesQuery {
    /// Teacher view of one student's grades (Canvas `params[:id]`).
    pub student_id: Option<i64>,
}

/// GET /courses/{id}/grades — the grade summary for the current student.
///
/// A teacher without `student_id` gets their own summary; with
/// `?student_id=N` they see that student's summary, provided that student is
/// enrolled.
pub async fn grades(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<GradesQuery>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    // Whose grades: the viewer, or (teacher-only) ?student_id.
    let student_id = match query.student_id {
        Some(sid) if sid != user.id => {
            if !ctx.is_teacher {
                return from_api_error(&ApiError::Forbidden, &origin).into_response();
            }
            // The target must be enrolled in the course.
            let enrolled: Option<i32> = sqlx::query_scalar(
                r#"SELECT 1 FROM enrollments
                   WHERE course_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'
                   LIMIT 1"#,
            )
            .bind(course_id)
            .bind(sid)
            .fetch_optional(&app.db)
            .await
            .unwrap_or(None);
            if enrolled.is_none() {
                return from_api_error(&ApiError::NotFound, &origin).into_response();
            }
            sid
        }
        _ => user.id,
    };

    let rows = match load_course_assignments_with_groups(&app.db, course_id, student_id).await {
        Ok(rows) => rows,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    // The summary always scopes to what the student can see: published.
    let rows: Vec<UiAssignment> = rows
        .into_iter()
        .filter(|r| r.workflow_state == "published")
        .collect();

    // Totals reuse the star-assignments pure functions (the same ones the
    // /final_grade API uses): build the parallel score/points arrays.
    let pseudo_submissions: Vec<star_assignments::Submission> = rows
        .iter()
        .filter(|r| r.submission_score.is_some())
        .map(|r| star_assignments::Submission {
            id: 0,
            assignment_id: r.id,
            user_id: student_id,
            body: None,
            score: r.submission_score,
            submitted_at: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
        .collect();
    let pseudo_assignments: Vec<star_assignments::Assignment> = rows
        .iter()
        .map(|r| star_assignments::Assignment {
            id: r.id,
            course_id: r.course_id,
            title: r.title.clone(),
            description: r.description.clone(),
            points_possible: r.points_possible,
            due_at: r.due_at,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
        .collect();
    let (scores, points) = grade_arrays_from_submissions(&pseudo_assignments, &pseudo_submissions);
    let percent = compute_grade_percent(&scores, &points);

    let total_score: f64 = rows
        .iter()
        .filter_map(|r| r.submission_score)
        .sum();
    let total_possible: f64 = rows
        .iter()
        .filter(|r| r.submission_score.is_some())
        .map(|r| r.points_possible)
        .sum();
    let total_label = if total_possible > 0.0 {
        format!("{} / {}", trim(total_score), trim(total_possible))
    } else {
        "–".to_string()
    };
    let percent_label = percent
        .map(|p| format!("{}%", trim(p)))
        .unwrap_or_else(|| "–".to_string());

    // Letter grade via the course's active grading standard (the same
    // SQL shape as the /final_grade API handler).
    let letter_grade = match percent {
        Some(p) => letter_grade_for_percent(&app.db, course_id, p).await,
        None => None,
    };

    let grade_rows: Vec<GradeRowView> = rows
        .into_iter()
        .map(|r| {
            let (pill_class, pill_label) = match r.submission_state.as_deref() {
                Some("graded") => ("graded", "Graded"),
                Some("submitted") => ("submitted", "Submitted"),
                _ => ("unsubmitted", "Not Submitted"),
            };
            let due_label = r
                .due_at
                .map(|d| d.format("%b %-d, %Y").to_string())
                .unwrap_or_else(|| "–".to_string());
            let score_label = r
                .submission_score
                .map(|s| format!("{} / {}", trim(s), trim(r.points_possible)))
                .unwrap_or_else(|| "–".to_string());
            GradeRowView {
                assignment_id: r.id,
                title: r.title,
                group_name: r.assignment_group_name,
                due_label,
                score_label,
                pill_class: pill_class.to_string(),
                pill_label: pill_label.to_string(),
            }
        })
        .collect();

    // The student's display name.
    let student_name = if student_id == user.id {
        ctx.user_name.clone()
    } else {
        sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
            .bind(student_id)
            .fetch_optional(&app.db)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "Student".to_string())
    };

    let env = course_env(&ctx, &user, &origin, "grades", course_id);
    let (jar, csrf_token) = crate::session::ensure_csrf_cookie(jar);

    // The breadcrumb borrows `ctx` while the page struct below moves its
    // fields, so it is built first.
    let breadcrumb = course_breadcrumb(&ctx, course_id);

    let page = GradesPage {
        title: format!("{} - Grades", ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} grades"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb,
        csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "grades".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        student_name,
        rows: grade_rows,
        total_label,
        percent_label,
        letter_grade,
        is_teacher: ctx.is_teacher,
    };
    (jar, page).into_response()
}

/// Render `f64` without a trailing `.0` (12 -> "12", 12.5 -> "12.5").
fn trim(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Compute the letter grade for a percent from the course's ACTIVE grading
/// standard (same query shape as star-grading-standards' /final_grade).
async fn letter_grade_for_percent(
    db: &sqlx::PgPool,
    course_id: i64,
    percent: f64,
) -> Option<String> {
    // The row type is the plain tuple `(i64,)`: `.fetch_optional()` supplies
    // the outer Option, so the `query_as` target must not wrap it in another
    // (same shape as star-assignments' `find_submission`). No active
    // standard -> no letter grade.
    let (standard_id,): (i64,) = sqlx::query_as(
        r#"SELECT id FROM grading_standards
           WHERE course_id = $1 AND workflow_state = 'active'
           LIMIT 1"#,
    )
    .bind(course_id)
    .fetch_optional(db)
    .await
    .ok()??;

    let entries: Vec<star_grading_standards::GradingSchemeEntry> = sqlx::query_as(
        r#"SELECT id, name, min_score::float8, position
           FROM grading_scheme_entries
           WHERE grading_standard_id = $1
           ORDER BY position ASC"#,
    )
    .bind(standard_id)
    .fetch_all(db)
    .await
    .ok()?;

    star_grading_standards::compute_letter_grade(percent, &entries)
}

/// The gradebook grid template context.
#[derive(Template)]
#[template(path = "gradebook.html")]
pub struct GradebookPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    /// Column headers: one per assignment (ordered group position, then
    /// insertion order like the API list).
    pub assignments: Vec<UiAssignment>,
    /// One row per enrollment.
    pub rows: Vec<star_assignments::UiGradebookRow>,
    pub csv_url: String,
}

impl IntoResponse for GradebookPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("gradebook template render failed: {err}")).into_response()
            }
        }
    }
}

/// Load the gradebook data (assignments ordered by group+position, rows =
/// enrollments with per-assignment cells). Shared by the HTML grid and the
/// CSV export so the two can never disagree.
pub(crate) async fn gradebook_data(
    app: &AppState,
    course_id: i64,
) -> Result<(Vec<UiAssignment>, Vec<star_assignments::UiGradebookRow>), ApiError> {
    // Viewer id only personalizes submission_state columns the grid does not
    // show; pass a sentinel that matches no user.
    let mut assignments = load_course_assignments_with_groups(&app.db, course_id, -1).await?;
    // Column order: group position, then insertion order — the Canvas
    // gradebook column order.
    assignments.sort_by(|a, b| {
        let ka = (a.assignment_group_position.unwrap_or(i32::MAX), a.id);
        let kb = (b.assignment_group_position.unwrap_or(i32::MAX), b.id);
        ka.cmp(&kb)
    });
    let rows = load_gradebook_rows(&app.db, course_id, &assignments).await?;
    Ok((assignments, rows))
}

/// GET /courses/{id}/gradebook — the teacher grid.
///
/// Teacher-or-admin only (`is_teacher` from course_context, which runs
/// star_courses::check_teacher_or_admin after the 404-before-403 course
/// gate).
pub async fn gradebook(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !ctx.is_teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    let (assignments, rows) = match gradebook_data(&app, course_id).await {
        Ok(data) => data,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let env = course_env(&ctx, &user, &origin, "grades", course_id);
    let (jar, csrf_token) = crate::session::ensure_csrf_cookie(jar);

    // The breadcrumb borrows `ctx` while the page struct below moves its
    // fields, so it is built first.
    let breadcrumb = course_breadcrumb(&ctx, course_id);

    let page = GradebookPage {
        title: format!("{} - Gradebook", ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} gradebook full-width"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb,
        csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "grades".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        assignments,
        rows,
        csv_url: format!("/courses/{course_id}/gradebook.csv"),
    };
    (jar, page).into_response()
}

/// GET /courses/{id}/gradebook.csv — CSV export (teacher-only).
///
/// Computed server-side from the same data as the grid (no gradebook_csv API
/// exists in star-lms — documented deviation). Shape: Student, then one
/// column per assignment, then Total.
pub async fn gradebook_csv(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !ctx.is_teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    let (assignments, rows) = match gradebook_data(&app, course_id).await {
        Ok(data) => data,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let mut csv = String::from("Student");
    for a in &assignments {
        csv.push(',');
        csv.push_str(&csv_escape(&a.title));
    }
    csv.push_str(",Total\n");

    for row in &rows {
        csv.push_str(&csv_escape(&row.user_name));
        for cell in &row.cells {
            csv.push(',');
            match (cell.score, cell.submission_state.as_deref()) {
                (Some(score), _) => csv.push_str(&format!("{score}")),
                (None, Some(_)) => csv.push('-'),
                (None, None) => {}
            }
        }
        csv.push(',');
        if let Some(grade) = row.current_grade {
            csv.push_str(&format!("{grade:.1}"));
        }
        csv.push('\n');
    }

    let disposition = format!("attachment; filename=\"gradebook-{course_id}.csv\"");
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (header::CONTENT_DISPOSITION, disposition.as_str()),
        ],
        csv,
    )
        .into_response()
}

/// CSV field escaping (RFC 4180): quote when the value contains a comma,
/// quote, newline or leading/trailing space; double embedded quotes.
fn csv_escape(value: &str) -> String {
    let needs_quotes = value.contains(',')
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r')
        || value != value.trim();
    if !needs_quotes {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escape_quotes_when_needed() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("line\nbreak"), "\"line\nbreak\"");
        assert_eq!(csv_escape(" padded "), "\" padded \"");
    }

    #[test]
    fn trim_drops_trailing_zero() {
        assert_eq!(trim(12.0), "12");
        assert_eq!(trim(12.5), "12.5");
        assert_eq!(trim(100.0), "100");
    }
}
