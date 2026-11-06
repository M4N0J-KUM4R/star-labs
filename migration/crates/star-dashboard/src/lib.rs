//! star-dashboard — activity view for logged-in users.
//!
//! Provides the GET /dashboard endpoint with three sections:
//! - upcoming: assignments due within 7 days + recent announcements (last 7 days)
//! - courses: enrolled courses
//! - recent_announcements: last 3 announcements across enrolled courses
//!
//! Uses raw CTE/JOIN queries — no N+1.

use axum::{
    extract::State,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use star_common::{ApiError, AppState};

use star_auth::AuthUser;

// -----------------------------------------------------------------------
// Response types
// -----------------------------------------------------------------------

/// Unified upcoming item (assignment or announcement).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct UpcomingItem {
    pub id: i64,
    pub title: String,
    #[serde(rename = "type")]
    #[sqlx(rename = "item_type")]
    pub item_type: String, // "assignment" | "announcement"
    pub date: DateTime<Utc>,
    pub course_name: String,
}

/// Enrolled course row.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EnrolledCourse {
    pub id: i64,
    pub name: String,
    pub course_code: String,
    pub role: String,
}

/// Announcement with course name.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AnnouncementWithCourse {
    pub id: i64,
    pub course_id: i64,
    pub course_name: String,
    pub title: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

/// Full dashboard payload.
#[derive(Debug, Clone, Serialize)]
pub struct DashboardResponse {
    pub upcoming: Vec<UpcomingItem>,
    pub courses: Vec<EnrolledCourse>,
    pub recent_announcements: Vec<AnnouncementWithCourse>,
}

// -----------------------------------------------------------------------
// Handlers
// -----------------------------------------------------------------------

/// GET /dashboard — activity view for the authenticated user.
///
/// Runs three queries:
/// 1. Upcoming (assignments due within 7 days + announcements from last 7 days, limit 5)
/// 2. Enrolled courses (joined via enrollments)
/// 3. Recent announcements (last 3 across enrolled courses)
///
/// Uses raw SQL with CTEs to combine assignments + announcements in a single query.
async fn dashboard_handler(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<DashboardResponse>, ApiError> {
    let user_id = auth.id;

    // Query 1: Upcoming items — assignments due within 7 days + announcements from last 7 days.
    // Combines both via a CTE with a type discriminator.
    let upcoming_rows: Vec<UpcomingItem> = sqlx::query_as(
        r#"
        WITH upcoming_assignments AS (
            SELECT
                a.id,
                a.title,
                'assignment'     AS item_type,
                a.due_at         AS date,
                c.name           AS course_name
            FROM assignments a
            JOIN enrollments e ON e.course_id = a.course_id
            JOIN courses c ON c.id = a.course_id
            WHERE e.user_id = $1
              AND a.due_at IS NOT NULL
              AND a.due_at >= now()
              AND a.due_at <= now() + interval '7 days'
        ), upcoming_announcements AS (
            SELECT
                ann.id,
                ann.title,
                'announcement' AS item_type,
                ann.created_at AS date,
                c.name         AS course_name
            FROM announcements ann
            JOIN enrollments e ON e.course_id = ann.course_id
            JOIN courses c ON c.id = ann.course_id
            WHERE e.user_id = $1
              AND ann.created_at >= now() - interval '7 days'
        )
        SELECT id, title, item_type, date, course_name
        FROM (
            SELECT * FROM upcoming_assignments
            UNION ALL
            SELECT * FROM upcoming_announcements
        ) combined
        ORDER BY date ASC
        LIMIT 5
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;

    // Query 2: Enrolled courses.
    let courses: Vec<EnrolledCourse> = sqlx::query_as(
        r#"
        SELECT c.id, c.name, c.course_code, e.role
        FROM courses c
        JOIN enrollments e ON e.course_id = c.id
        WHERE e.user_id = $1
        ORDER BY c.name ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;

    // Query 3: Last 3 announcements across enrolled courses.
    let recent_announcements: Vec<AnnouncementWithCourse> = sqlx::query_as(
        r#"
        SELECT ann.id, ann.course_id, c.name AS course_name, ann.title, ann.message, ann.created_at
        FROM announcements ann
        JOIN enrollments e ON e.course_id = ann.course_id
        JOIN courses c ON c.id = ann.course_id
        WHERE e.user_id = $1
        ORDER BY ann.created_at DESC
        LIMIT 3
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(DashboardResponse {
        upcoming: upcoming_rows,
        courses,
        recent_announcements,
    }))
}

// -----------------------------------------------------------------------
// Router
// -----------------------------------------------------------------------

/// Returns the dashboard router with GET /dashboard.
pub fn router() -> Router<AppState> {
    Router::new().route("/dashboard", get(dashboard_handler))
}
