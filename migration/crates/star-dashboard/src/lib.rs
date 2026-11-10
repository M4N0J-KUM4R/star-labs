//! star-dashboard — activity views for logged-in users.
//!
//! Phase 2: GET /dashboard with three sections:
//! - upcoming: assignments due within 7 days + recent announcements (last 7 days)
//! - courses: enrolled courses
//! - recent_announcements: last 3 announcements across enrolled courses
//!
//! Phase 3 (Canvas parity):
//! - GET /users/self/todo            — grading + submitting todo items
//! - GET /users/self/upcoming_events — assignments + calendar events, next 7 days
//! - GET /dashboard/dashboard_cards  — menu-style course cards
//!
//! Uses raw CTE/JOIN queries — no N+1.

use axum::{
    extract::{Path, RawQuery, State},
    http::{header::AUTHORIZATION, HeaderMap},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use star_assignments::Assignment;
use star_common::{ApiError, AppState};
use std::collections::BTreeMap;

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
// Phase 3: todo items (GET /users/self/todo)
// -----------------------------------------------------------------------

/// One row of `GET /users/self/todo` (Canvas `todo_item_json`).
///
/// Canvas shape: context_type/context_id + context_name/context_short_name,
/// type ("grading" | "submitting"), the ignore URLs, the nested assignment,
/// html_url, and needs_grading_count on grading items only.
#[derive(Debug, Clone, Serialize)]
pub struct TodoItem {
    pub context_type: String,
    pub course_id: i64,
    pub context_name: String,
    pub context_short_name: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub ignore: String,
    pub ignore_permanently: String,
    pub assignment: Assignment,
    pub html_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_grading_count: Option<i64>,
}

/// Shared row shape for the grading and submitting todo queries.
#[derive(Debug, Clone, sqlx::FromRow)]
struct TodoRow {
    id: i64,
    course_id: i64,
    title: String,
    description: Option<String>,
    points_possible: f64,
    due_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    context_name: String,
    context_short_name: String,
    needs_grading_count: i64,
}

impl TodoRow {
    /// Convert a row into the Canvas todo item shape for the given purpose.
    fn into_todo_item(self, item_type: &str) -> TodoItem {
        let ignore_base = format!("/api/v1/users/self/todo/assignment_{}/{item_type}", self.id);
        let html_url = if item_type == "grading" {
            // Canvas speed_grader_course_gradebook_url
            format!(
                "/courses/{}/gradebook/speed_grader?assignment_id={}",
                self.course_id, self.id
            )
        } else {
            // Canvas course_assignment_url + "#submit"
            format!("/courses/{}/assignments/{}#submit", self.course_id, self.id)
        };

        TodoItem {
            context_type: "Course".to_string(),
            course_id: self.course_id,
            context_name: self.context_name,
            context_short_name: self.context_short_name,
            item_type: item_type.to_string(),
            ignore: format!("{ignore_base}?permanent=0"),
            ignore_permanently: format!("{ignore_base}?permanent=1"),
            assignment: Assignment {
                id: self.id,
                course_id: self.course_id,
                title: self.title,
                description: self.description,
                points_possible: self.points_possible,
                due_at: self.due_at,
                created_at: self.created_at,
                updated_at: self.updated_at,
            },
            html_url,
            needs_grading_count: (item_type == "grading").then_some(self.needs_grading_count),
        }
    }
}

/// Canvas `assignments_needing_grading`: assignments in courses where the user
/// teaches that have at least one submitted-but-ungraded submission.
async fn fetch_grading_todos(
    db: &sqlx::PgPool,
    user_id: i64,
    course_ids: &[i64],
) -> Result<Vec<TodoItem>, ApiError> {
    let rows: Vec<TodoRow> = sqlx::query_as(
        r#"
        SELECT a.id, a.course_id, a.title, a.description, a.points_possible::float8 AS points_possible,
               a.due_at, a.created_at, a.updated_at,
               c.name        AS context_name,
               c.course_code AS context_short_name,
               (SELECT COUNT(*) FROM submissions s
                 WHERE s.assignment_id = a.id
                   AND s.body IS NOT NULL
                   AND s.score IS NULL) AS needs_grading_count
        FROM assignments a
        JOIN courses c ON c.id = a.course_id
        WHERE EXISTS (
                SELECT 1 FROM enrollments e
                 WHERE e.course_id = a.course_id
                   AND e.user_id = $1
                   AND e.role IN ('teacher', 'ta')
                   AND e.workflow_state = 'active')
          AND EXISTS (
                SELECT 1 FROM submissions s
                 WHERE s.assignment_id = a.id
                   AND s.body IS NOT NULL
                   AND s.score IS NULL)
          AND (cardinality($2::bigint[]) = 0 OR a.course_id = ANY($2::bigint[]))
        ORDER BY a.due_at NULLS LAST, a.id
        LIMIT 50
        "#,
    )
    .bind(user_id)
    .bind(course_ids)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| row.into_todo_item("grading"))
        .collect())
}

/// Canvas `assignments_needing_submitting`: assignments due in
/// [now - 4 weeks, now + 1 week] for the user's student enrollments that have
/// no submitted (body-bearing) submission yet.
async fn fetch_submitting_todos(
    db: &sqlx::PgPool,
    user_id: i64,
    course_ids: &[i64],
) -> Result<Vec<TodoItem>, ApiError> {
    let rows: Vec<TodoRow> = sqlx::query_as(
        r#"
        SELECT a.id, a.course_id, a.title, a.description, a.points_possible::float8 AS points_possible,
               a.due_at, a.created_at, a.updated_at,
               c.name        AS context_name,
               c.course_code AS context_short_name,
               0::bigint     AS needs_grading_count
        FROM assignments a
        JOIN courses c ON c.id = a.course_id
        WHERE EXISTS (
                SELECT 1 FROM enrollments e
                 WHERE e.course_id = a.course_id
                   AND e.user_id = $1
                   AND e.role = 'student'
                   AND e.workflow_state = 'active')
          AND a.due_at IS NOT NULL
          AND a.due_at >= now() - interval '4 weeks'
          AND a.due_at <= now() + interval '1 week'
          AND NOT EXISTS (
                SELECT 1 FROM submissions s
                 WHERE s.assignment_id = a.id
                   AND s.user_id = $1
                   AND s.body IS NOT NULL)
          AND (cardinality($2::bigint[]) = 0 OR a.course_id = ANY($2::bigint[]))
        ORDER BY a.due_at, a.id
        LIMIT 50
        "#,
    )
    .bind(user_id)
    .bind(course_ids)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| row.into_todo_item("submitting"))
        .collect())
}

/// GET /users/self/todo — the current user's Canvas todo list.
///
/// Merges grading (teacher) and submitting (student) items, ordered by due
/// date like Canvas' BookmarkedCollection merge. Supports `course_ids[]`.
async fn todo_handler(
    State(state): State<AppState>,
    auth: AuthUser,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<Vec<TodoItem>>, ApiError> {
    let course_filter = parse_course_ids(raw_query.as_deref());
    // Canvas: a present-but-empty filter yields an empty list, not everything.
    if matches!(&course_filter, Some(ids) if ids.is_empty()) {
        return Ok(Json(Vec::new()));
    }
    let course_ids = course_filter.unwrap_or_default();

    let mut items = fetch_grading_todos(&state.db, auth.id, &course_ids).await?;
    items.extend(fetch_submitting_todos(&state.db, auth.id, &course_ids).await?);

    // Canvas sorts the merged collections by due_at then id.
    items.sort_by(|a, b| match (&a.assignment.due_at, &b.assignment.due_at) {
        (Some(left), Some(right)) => left
            .cmp(right)
            .then_with(|| a.assignment.id.cmp(&b.assignment.id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.assignment.id.cmp(&b.assignment.id),
    });
    items.truncate(50);

    Ok(Json(items))
}

// -----------------------------------------------------------------------
// Phase 3: upcoming events (GET /users/self/upcoming_events)
// -----------------------------------------------------------------------

/// Union of assignment and calendar events returned by upcoming_events.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum UpcomingEvent {
    Assignment(AssignmentUpcomingEvent),
    Calendar(CalendarUpcomingEvent),
}

/// Canvas `assignment_event_json` (CalendarEvent API module).
#[derive(Debug, Clone, Serialize)]
pub struct AssignmentUpcomingEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub title: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub workflow_state: String,
    pub submission_types: Vec<String>,
    pub all_day: bool,
    pub all_day_date: Option<String>,
    pub assignment: Assignment,
    pub html_url: String,
    pub context_code: String,
    pub context_name: String,
    pub context_color: Option<String>,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub url: String,
    pub important_dates: bool,
}

/// Canvas `calendar_event_json` subset for the user's own/course events.
#[derive(Debug, Clone, Serialize)]
pub struct CalendarUpcomingEvent {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub start_at: DateTime<Utc>,
    pub end_at: Option<DateTime<Utc>>,
    pub all_day: bool,
    pub all_day_date: Option<String>,
    pub title: String,
    pub workflow_state: String,
    pub comments: Option<String>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub location_address: Option<String>,
    pub location_name: Option<String>,
    pub description: Option<String>,
    pub context_code: String,
    pub context_name: Option<String>,
    pub context_color: Option<String>,
    pub all_context_codes: String,
    pub child_events_count: i64,
    pub html_url: String,
    pub url: String,
}

/// Assignment row for upcoming_events (due_at is non-null in the query).
#[derive(Debug, Clone, sqlx::FromRow)]
struct AssignmentEventRow {
    id: i64,
    course_id: i64,
    title: String,
    description: Option<String>,
    points_possible: f64,
    due_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    context_name: String,
}

/// Calendar event row for upcoming_events.
#[derive(Debug, Clone, sqlx::FromRow)]
struct CalendarEventEventRow {
    id: i64,
    course_id: Option<i64>,
    user_id: Option<i64>,
    title: String,
    description: Option<String>,
    start_at: DateTime<Utc>,
    end_at: Option<DateTime<Utc>>,
    workflow_state: String,
    location_name: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    context_name: Option<String>,
}

/// GET /users/self/upcoming_events — assignments due plus calendar events in
/// the next 7 days across the user's courses (and personal calendar), ordered
/// by start date and capped at 20 like Canvas `User#upcoming_events`.
async fn upcoming_events_handler(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<UpcomingEvent>>, ApiError> {
    let assignment_rows: Vec<AssignmentEventRow> = sqlx::query_as(
        r#"
        SELECT a.id, a.course_id, a.title, a.description, a.points_possible::float8 AS points_possible,
               a.due_at, a.created_at, a.updated_at, c.name AS context_name
        FROM assignments a
        JOIN courses c ON c.id = a.course_id
        WHERE EXISTS (
                SELECT 1 FROM enrollments e
                 WHERE e.course_id = a.course_id
                   AND e.user_id = $1
                   AND e.workflow_state = 'active')
          AND a.due_at IS NOT NULL
          AND a.due_at > now()
          AND a.due_at <= now() + interval '7 days'
        ORDER BY a.due_at ASC, a.id ASC
        LIMIT 20
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    let calendar_rows: Vec<CalendarEventEventRow> = sqlx::query_as(
        r#"
        SELECT ce.id, ce.course_id, ce.user_id, ce.title, ce.description,
               ce.start_at, ce.end_at, ce.workflow_state, ce.location_name,
               ce.created_at, ce.updated_at,
               COALESCE(c.name, u.name) AS context_name
        FROM calendar_events ce
        LEFT JOIN courses c ON c.id = ce.course_id
        LEFT JOIN users u ON u.id = ce.user_id
        WHERE ce.workflow_state <> 'deleted'
          AND ce.start_at > now()
          AND ce.start_at <= now() + interval '7 days'
          AND ((ce.course_id IS NOT NULL AND EXISTS (
                    SELECT 1 FROM enrollments e
                     WHERE e.course_id = ce.course_id
                       AND e.user_id = $1
                       AND e.workflow_state = 'active'))
               OR ce.user_id = $1)
        ORDER BY ce.start_at ASC, ce.id ASC
        LIMIT 20
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    let mut events: Vec<(DateTime<Utc>, UpcomingEvent)> = Vec::new();

    for row in assignment_rows {
        let html_url = format!("/courses/{}/assignments/{}", row.course_id, row.id);
        events.push((
            row.due_at,
            UpcomingEvent::Assignment(AssignmentUpcomingEvent {
                id: format!("assignment_{}", row.id),
                event_type: "assignment".to_string(),
                title: row.title.clone(),
                description: row.description.clone(),
                created_at: row.created_at,
                updated_at: row.updated_at,
                workflow_state: "published".to_string(),
                submission_types: vec!["online".to_string()],
                all_day: false,
                all_day_date: None,
                assignment: Assignment {
                    id: row.id,
                    course_id: row.course_id,
                    title: row.title,
                    description: row.description,
                    points_possible: row.points_possible,
                    due_at: Some(row.due_at),
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
                html_url,
                context_code: format!("course_{}", row.course_id),
                context_name: row.context_name,
                context_color: None,
                start_at: row.due_at,
                end_at: row.due_at,
                url: format!("/api/v1/calendar_events/assignment_{}", row.id),
                important_dates: false,
            }),
        ));
    }

    for row in calendar_rows {
        let context_code = match (row.course_id, row.user_id) {
            (Some(course_id), _) => format!("course_{course_id}"),
            (None, Some(user_id)) => format!("user_{user_id}"),
            (None, None) => format!("user_{}", auth.id),
        };
        let html_url = match row.course_id {
            Some(course_id) => format!("/courses/{course_id}/calendar_events/{}", row.id),
            None => format!("/calendar_events/{}", row.id),
        };
        events.push((
            row.start_at,
            UpcomingEvent::Calendar(CalendarUpcomingEvent {
                id: row.id,
                created_at: row.created_at,
                updated_at: row.updated_at,
                start_at: row.start_at,
                end_at: row.end_at,
                all_day: false,
                all_day_date: None,
                title: row.title,
                workflow_state: row.workflow_state,
                comments: None,
                event_type: "event".to_string(),
                location_address: None,
                location_name: row.location_name,
                description: row.description,
                context_code: context_code.clone(),
                context_name: row.context_name,
                context_color: None,
                all_context_codes: context_code,
                child_events_count: 0,
                html_url,
                url: format!("/api/v1/calendar_events/{}", row.id),
            }),
        ));
    }

    events.sort_by_key(|event| event.0);
    events.truncate(20);

    Ok(Json(events.into_iter().map(|(_, event)| event).collect()))
}

// -----------------------------------------------------------------------
// Phase 3: dashboard cards (GET /dashboard/dashboard_cards)
// -----------------------------------------------------------------------

/// A card link in the Canvas `CourseForMenuPresenter` tab shape.
#[derive(Debug, Clone, Serialize)]
pub struct DashboardCardLink {
    pub css_class: String,
    pub icon: Option<String>,
    pub hidden: bool,
    pub path: String,
    pub label: String,
}

/// Canvas `CourseForMenuPresenter#to_h` card shape (camelCase JSON).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardCard {
    pub long_name: String,
    pub short_name: String,
    pub original_name: String,
    pub course_code: String,
    pub asset_string: String,
    pub href: String,
    pub term: Option<String>,
    pub subtitle: String,
    pub enrollment_state: String,
    pub enrollment_type: String,
    pub observee: Option<String>,
    pub id: i64,
    pub is_favorited: bool,
    pub is_k5_subject: bool,
    pub is_homeroom: bool,
    pub use_classic_font: bool,
    pub can_manage: bool,
    pub can_read_announcements: bool,
    pub image: Option<String>,
    pub color: Option<String>,
    pub position: Option<i32>,
    pub published: bool,
    pub links: Vec<DashboardCardLink>,
    pub can_change_course_publish_state: bool,
    pub default_view: String,
    pub pages_url: String,
    pub front_page_title: Option<String>,
}

/// Row backing a dashboard card (one course, primary enrollment, position).
#[derive(Debug, Clone, sqlx::FromRow)]
struct DashboardCardRow {
    id: i64,
    name: String,
    course_code: String,
    workflow_state: String,
    role: String,
    enrollment_state: String,
    position: Option<i32>,
}

/// Canvas `DASHBOARD_CARD_TABS`: discussions, assignments, announcements, files.
fn dashboard_card_links(course_id: i64) -> Vec<DashboardCardLink> {
    [
        (
            "discussions",
            "Discussions",
            format!("/courses/{course_id}/discussion_topics"),
        ),
        (
            "assignments",
            "Assignments",
            format!("/courses/{course_id}/assignments"),
        ),
        (
            "announcements",
            "Announcements",
            format!("/courses/{course_id}/announcements"),
        ),
        ("files", "Files", format!("/courses/{course_id}/files")),
    ]
    .into_iter()
    .map(|(css_class, label, path)| DashboardCardLink {
        css_class: css_class.to_string(),
        icon: None,
        hidden: false,
        path,
        label: label.to_string(),
    })
    .collect()
}

/// Canvas `primary_enrollment_type` / `primary_enrollment_role` names.
fn enrollment_type_for(role: &str) -> &'static str {
    match role {
        "teacher" => "TeacherEnrollment",
        "ta" => "TaEnrollment",
        "observer" => "ObserverEnrollment",
        "designer" => "DesignerEnrollment",
        _ => "StudentEnrollment",
    }
}

/// Human role label used in the card subtitle ("enrolled as Student").
fn enrollment_role_label(role: &str) -> &'static str {
    match role {
        "teacher" => "Teacher",
        "ta" => "TA",
        "observer" => "Observer",
        "designer" => "Designer",
        _ => "Student",
    }
}

/// GET /dashboard/dashboard_cards — Canvas dashboard course cards for the
/// current user, ordered by dashboard position (unpositioned last) then name.
async fn dashboard_cards_handler(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<DashboardCard>>, ApiError> {
    let rows: Vec<DashboardCardRow> = sqlx::query_as(
        r#"
        SELECT id, name, course_code, workflow_state, role, enrollment_state, position
        FROM (
            SELECT DISTINCT ON (c.id)
                   c.id, c.name, c.course_code, c.workflow_state,
                   e.role,
                   e.workflow_state AS enrollment_state,
                   dp.position,
                   CASE e.role
                       WHEN 'teacher' THEN 0
                       WHEN 'ta' THEN 1
                       WHEN 'designer' THEN 2
                       ELSE 3
                   END AS role_rank
            FROM courses c
            JOIN enrollments e
              ON e.course_id = c.id
             AND e.user_id = $1
             AND e.workflow_state <> 'deleted'
            LEFT JOIN dashboard_positions dp
              ON dp.user_id = $1 AND dp.course_id = c.id
            ORDER BY c.id, role_rank
        ) cards
        ORDER BY position NULLS LAST, name ASC
        LIMIT 50
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    let cards = rows
        .into_iter()
        .map(|row| {
            let can_manage =
                matches!(row.role.as_str(), "teacher" | "ta" | "designer") || auth.role == "admin";
            DashboardCard {
                long_name: format!("{} - {}", row.name, row.course_code),
                short_name: row.course_code.clone(),
                original_name: row.name,
                course_code: row.course_code,
                asset_string: format!("course_{}", row.id),
                href: format!("/courses/{}", row.id),
                term: None,
                subtitle: format!("enrolled as {}", enrollment_role_label(&row.role)),
                enrollment_state: row.enrollment_state,
                enrollment_type: enrollment_type_for(&row.role).to_string(),
                observee: None,
                id: row.id,
                is_favorited: false,
                is_k5_subject: false,
                is_homeroom: false,
                use_classic_font: false,
                can_manage,
                can_read_announcements: true,
                image: None,
                color: None,
                position: row.position,
                published: matches!(row.workflow_state.as_str(), "available" | "completed"),
                links: dashboard_card_links(row.id),
                can_change_course_publish_state: can_manage,
                default_view: "modules".to_string(),
                pages_url: format!("/courses/{}/pages", row.id),
                front_page_title: None,
            }
        })
        .collect();

    Ok(Json(cards))
}

// -----------------------------------------------------------------------
// Phase 4: favorites (GET/POST/DELETE /users/self/favorites/courses)
// -----------------------------------------------------------------------

/// Canvas `favorite_json`: only id, context_id and context_type are exposed
/// (`Api::V1::Favorite#favorite_json` -> `api_json(only: [...])`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FavoriteJson {
    pub id: i64,
    pub context_id: i64,
    pub context_type: String,
}

/// An enrolled course returned by `GET /users/self/favorites/courses`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FavoriteCourse {
    pub id: i64,
    pub name: String,
    pub course_code: String,
    pub workflow_state: String,
}

/// `{}` — the body Canvas returns for no-op favorites mutations.
#[derive(Debug, Clone, Serialize)]
pub struct EmptyObject {}

/// Untagged union: a removed favorite renders the Favorite object, while a
/// favorite that did not exist renders `{}` (Canvas deliberately does not 404
/// here so browser state can resync).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum FavoriteMutation {
    Favorite(FavoriteJson),
    Empty(EmptyObject),
}

/// GET /users/self/favorites/courses — Canvas `menu_courses`: the user's
/// favorited courses, or every enrolled course when no favorites are set.
async fn list_favorite_courses(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<FavoriteCourse>>, ApiError> {
    let courses: Vec<FavoriteCourse> = sqlx::query_as(
        r#"
        SELECT c.id, c.name, c.course_code, c.workflow_state
        FROM courses c
        WHERE EXISTS (
                SELECT 1 FROM enrollments e
                 WHERE e.course_id = c.id
                   AND e.user_id = $1
                   AND e.workflow_state <> 'deleted')
          AND (EXISTS (SELECT 1 FROM favorites f
                        WHERE f.user_id = $1
                          AND f.context_type = 'Course'
                          AND f.context_id = c.id)
               OR NOT EXISTS (SELECT 1 FROM favorites f2
                               WHERE f2.user_id = $1
                                 AND f2.context_type = 'Course'))
        ORDER BY c.name ASC
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(courses))
}

/// POST /users/self/favorites/courses/{id} — Canvas `create_or_find_by`, so a
/// repeated POST returns the same Favorite.
async fn add_favorite_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
) -> Result<Json<FavoriteJson>, ApiError> {
    let course: Option<(i64,)> = sqlx::query_as("SELECT id FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;
    course.ok_or(ApiError::NotFound)?;

    let favorite: FavoriteJson = sqlx::query_as(
        r#"
        INSERT INTO favorites (user_id, context_type, context_id)
        VALUES ($1, 'Course', $2)
        ON CONFLICT (user_id, context_type, context_id)
        DO UPDATE SET updated_at = favorites.updated_at
        RETURNING id, context_id, context_type
        "#,
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(favorite))
}

/// DELETE /users/self/favorites/courses/{id} — removes the course favorite.
async fn remove_favorite_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
) -> Result<Json<FavoriteMutation>, ApiError> {
    let course: Option<(i64,)> = sqlx::query_as("SELECT id FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;
    course.ok_or(ApiError::NotFound)?;

    let favorite: Option<FavoriteJson> = sqlx::query_as(
        r#"
        DELETE FROM favorites
        WHERE user_id = $1 AND context_type = 'Course' AND context_id = $2
        RETURNING id, context_id, context_type
        "#,
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    Ok(Json(match favorite {
        Some(favorite) => FavoriteMutation::Favorite(favorite),
        None => FavoriteMutation::Empty(EmptyObject {}),
    }))
}

/// DELETE /users/self/favorites/courses — Canvas `reset_course_favorites`:
/// drops every course favorite so the default enrolled list applies again.
async fn reset_course_favorites(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<EmptyObject>, ApiError> {
    sqlx::query("DELETE FROM favorites WHERE user_id = $1 AND context_type = 'Course'")
        .bind(auth.id)
        .execute(&state.db)
        .await?;

    Ok(Json(EmptyObject {}))
}

// -----------------------------------------------------------------------
// Phase 4: release notes (GET/POST /release_notes, GET /release_notes/latest,
// GET /release_notes/unread_count, PUT/DELETE /release_notes/{id})
// -----------------------------------------------------------------------

/// Stored row; `show_ats` is the env -> time map from Canvas `ReleaseNote`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ReleaseNoteRow {
    id: i64,
    title: String,
    description: Option<String>,
    url: Option<String>,
    published: bool,
    target_roles: Vec<String>,
    show_at_production: Option<DateTime<Utc>>,
    show_at_beta: Option<DateTime<Utc>>,
    show_at_test: Option<DateTime<Utc>>,
}

/// One locale's text (Canvas `langs[lang]`).
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseNoteLang {
    pub title: String,
    pub description: Option<String>,
    pub url: Option<String>,
}

/// Canvas `ReleaseNote#to_json`: id, show_ats, target_roles, published and
/// (only when requested) langs.
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseNoteJson {
    pub id: i64,
    pub show_ats: BTreeMap<String, DateTime<Utc>>,
    pub target_roles: Vec<String>,
    pub published: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub langs: Option<BTreeMap<String, ReleaseNoteLang>>,
}

/// Canvas `latest` entry: `{id, title, description, url, date, new}`.
#[derive(Debug, Clone, Serialize)]
pub struct LatestReleaseNote {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub date: DateTime<Utc>,
    #[serde(rename = "new")]
    pub is_new: bool,
}

/// Canvas `unread_count`: `{unread_count: n}`.
#[derive(Debug, Clone, Serialize)]
pub struct UnreadCount {
    pub unread_count: i64,
}

/// Canvas `destroy`: `{status: "ok"}`.
#[derive(Debug, Clone, Serialize)]
pub struct StatusOk {
    pub status: String,
}

/// Body accepted by POST/PUT /release_notes (Canvas `upsert_params`).
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseNoteParams {
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub published: Option<bool>,
    pub target_roles: Option<Vec<String>>,
    /// env -> RFC 3339 timestamp.
    pub show_ats: Option<BTreeMap<String, String>>,
    /// lang -> localized text; only "en" is stored by this API.
    pub langs: Option<BTreeMap<String, ReleaseNoteLangParams>>,
}

/// Localized text payload inside `langs`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseNoteLangParams {
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
}

/// Mutable note fields, seeded from defaults (create) or the stored row
/// (update), then overlaid with any present request keys.
struct NoteFields {
    title: String,
    description: Option<String>,
    url: Option<String>,
    published: bool,
    target_roles: Vec<String>,
    show_at_production: Option<DateTime<Utc>>,
    show_at_beta: Option<DateTime<Utc>>,
    show_at_test: Option<DateTime<Utc>>,
}

impl NoteFields {
    fn new() -> Self {
        Self {
            title: String::new(),
            description: None,
            url: None,
            published: false,
            target_roles: vec!["user".to_string()],
            show_at_production: None,
            show_at_beta: None,
            show_at_test: None,
        }
    }

    fn from_row(row: &ReleaseNoteRow) -> Self {
        Self {
            title: row.title.clone(),
            description: row.description.clone(),
            url: row.url.clone(),
            published: row.published,
            target_roles: row.target_roles.clone(),
            show_at_production: row.show_at_production,
            show_at_beta: row.show_at_beta,
            show_at_test: row.show_at_test,
        }
    }

    fn apply(&mut self, params: &ReleaseNoteParams) -> Result<(), ApiError> {
        if let Some(en) = params.langs.as_ref().and_then(|langs| langs.get("en")) {
            if let Some(title) = &en.title {
                self.title = title.clone();
            }
            if en.description.is_some() {
                self.description = en.description.clone();
            }
            if en.url.is_some() {
                self.url = en.url.clone();
            }
        }
        if let Some(title) = &params.title {
            self.title = title.clone();
        }
        if params.description.is_some() {
            self.description = params.description.clone();
        }
        if params.url.is_some() {
            self.url = params.url.clone();
        }
        if let Some(published) = params.published {
            self.published = published;
        }
        if let Some(roles) = &params.target_roles {
            self.target_roles = roles.clone();
        }
        if let Some(show_ats) = &params.show_ats {
            for (env, value) in show_ats {
                let at = DateTime::parse_from_rfc3339(value)
                    .map_err(|_| ApiError::Validation(format!("invalid show_ats time: {value}")))?
                    .with_timezone(&Utc);
                match env.as_str() {
                    "beta" => self.show_at_beta = Some(at),
                    "test" => self.show_at_test = Some(at),
                    _ => self.show_at_production = Some(at),
                }
            }
        }
        Ok(())
    }
}

/// Canvas `require_manage_release_notes` -> site admin only.
fn require_release_notes_admin(auth: &AuthUser) -> Result<(), ApiError> {
    if auth.role == "admin" {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

const RELEASE_NOTE_COLUMNS: &str = "id, title, description, url, published, target_roles, \
     show_at_production, show_at_beta, show_at_test";

/// Whitelisted env -> column mapping; the returned name is safe to interpolate.
fn show_at_column(env: &str) -> &'static str {
    match env {
        "beta" => "show_at_beta",
        "test" => "show_at_test",
        _ => "show_at_production",
    }
}

/// Canvas `env` query param (defaults to production).
fn release_note_env(raw_query: Option<&str>) -> String {
    query_value(raw_query, "env").unwrap_or_else(|| "production".to_string())
}

/// Optional authentication: release note reads also serve anonymous callers.
async fn optional_user(headers: &HeaderMap, state: &AppState) -> Option<AuthUser> {
    let token = headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    let claims = star_auth::verify_token(token, &state.jwt_secret)?;
    let (role,): (String,) = sqlx::query_as("SELECT role FROM users WHERE id = $1")
        .bind(claims.sub)
        .fetch_optional(&state.db)
        .await
        .ok()??;
    Some(AuthUser {
        id: claims.sub,
        role,
    })
}

/// Canvas keeps the acknowledged note time on the user row.
async fn last_seen_release_note(
    db: &sqlx::PgPool,
    user_id: i64,
) -> Result<Option<DateTime<Utc>>, ApiError> {
    let last_seen: Option<Option<DateTime<Utc>>> =
        sqlx::query_scalar("SELECT last_seen_release_note FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(last_seen.flatten())
}

impl ReleaseNoteRow {
    fn into_json(self, include_langs: bool) -> ReleaseNoteJson {
        let mut show_ats = BTreeMap::new();
        if let Some(at) = self.show_at_production {
            show_ats.insert("production".to_string(), at);
        }
        if let Some(at) = self.show_at_beta {
            show_ats.insert("beta".to_string(), at);
        }
        if let Some(at) = self.show_at_test {
            show_ats.insert("test".to_string(), at);
        }

        let langs = include_langs.then(|| {
            let mut langs = BTreeMap::new();
            langs.insert(
                "en".to_string(),
                ReleaseNoteLang {
                    title: self.title.clone(),
                    description: self.description.clone(),
                    url: self.url.clone(),
                },
            );
            langs
        });

        ReleaseNoteJson {
            id: self.id,
            show_ats,
            target_roles: self.target_roles,
            published: self.published,
            langs,
        }
    }
}

/// GET /release_notes — paginated list of every note, newest first.
async fn index_release_notes(
    State(state): State<AppState>,
    auth: AuthUser,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<Vec<ReleaseNoteJson>>, ApiError> {
    require_release_notes_admin(&auth)?;

    let include_langs = query_flag(raw_query.as_deref(), "include_langs");
    let sql = format!(
        "SELECT {RELEASE_NOTE_COLUMNS} FROM release_notes ORDER BY created_at DESC, id DESC"
    );
    let rows: Vec<ReleaseNoteRow> = sqlx::query_as(&sql).fetch_all(&state.db).await?;

    Ok(Json(
        rows.into_iter()
            .map(|row| row.into_json(include_langs))
            .collect(),
    ))
}

/// POST /release_notes — creates a note (Canvas `upsert(ReleaseNote.new)`).
async fn create_release_note(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(params): Json<ReleaseNoteParams>,
) -> Result<Json<ReleaseNoteJson>, ApiError> {
    require_release_notes_admin(&auth)?;

    let mut fields = NoteFields::new();
    fields.apply(&params)?;

    let sql = format!(
        r#"
        INSERT INTO release_notes
            (title, description, url, published, target_roles,
             show_at_production, show_at_beta, show_at_test)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING {RELEASE_NOTE_COLUMNS}
        "#
    );
    let row: ReleaseNoteRow = sqlx::query_as(&sql)
        .bind(&fields.title)
        .bind(&fields.description)
        .bind(&fields.url)
        .bind(fields.published)
        .bind(&fields.target_roles)
        .bind(fields.show_at_production)
        .bind(fields.show_at_beta)
        .bind(fields.show_at_test)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(row.into_json(true)))
}

/// PUT /release_notes/{id} — updates an existing note.
async fn update_release_note(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(params): Json<ReleaseNoteParams>,
) -> Result<Json<ReleaseNoteJson>, ApiError> {
    require_release_notes_admin(&auth)?;

    let sql = format!("SELECT {RELEASE_NOTE_COLUMNS} FROM release_notes WHERE id = $1");
    let existing: Option<ReleaseNoteRow> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let mut fields = NoteFields::from_row(&existing.ok_or(ApiError::NotFound)?);
    fields.apply(&params)?;

    let sql = format!(
        r#"
        UPDATE release_notes
           SET title = $2, description = $3, url = $4, published = $5,
               target_roles = $6, show_at_production = $7, show_at_beta = $8,
               show_at_test = $9, updated_at = now()
         WHERE id = $1
        RETURNING {RELEASE_NOTE_COLUMNS}
        "#
    );
    let row: ReleaseNoteRow = sqlx::query_as(&sql)
        .bind(id)
        .bind(&fields.title)
        .bind(&fields.description)
        .bind(&fields.url)
        .bind(fields.published)
        .bind(&fields.target_roles)
        .bind(fields.show_at_production)
        .bind(fields.show_at_beta)
        .bind(fields.show_at_test)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(row.into_json(true)))
}

/// DELETE /release_notes/{id} — Canvas `destroy` returns `{status: "ok"}`.
async fn destroy_release_note(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<StatusOk>, ApiError> {
    require_release_notes_admin(&auth)?;

    let deleted: Option<(i64,)> =
        sqlx::query_as("DELETE FROM release_notes WHERE id = $1 RETURNING id")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    deleted.ok_or(ApiError::NotFound)?;

    Ok(Json(StatusOk {
        status: "ok".to_string(),
    }))
}

/// GET /release_notes/latest — the notes already shown in this environment
/// for the caller's roles, newest first, limit 10. Marks the newest as seen.
async fn latest_release_notes(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<Vec<LatestReleaseNote>>, ApiError> {
    let column = show_at_column(&release_note_env(raw_query.as_deref()));
    let user = optional_user(&headers, &state).await;
    let role = user
        .as_ref()
        .map(|u| u.role.clone())
        .unwrap_or_else(|| "user".to_string());

    let sql = format!(
        r#"
        SELECT id, title, description, url, {column} AS shown_at
        FROM release_notes
        WHERE published
          AND {column} IS NOT NULL
          AND {column} <= now()
          AND ('user' = ANY(target_roles) OR $1 = ANY(target_roles))
        ORDER BY {column} DESC, id DESC
        LIMIT 10
        "#
    );
    let rows: Vec<(i64, String, Option<String>, Option<String>, DateTime<Utc>)> =
        sqlx::query_as(&sql)
            .bind(&role)
            .fetch_all(&state.db)
            .await?;

    let last_seen = match &user {
        Some(user) => last_seen_release_note(&state.db, user.id)
            .await?
            .unwrap_or_else(Utc::now),
        None => Utc::now(),
    };

    let notes: Vec<LatestReleaseNote> = rows
        .into_iter()
        .map(|(id, title, description, url, date)| LatestReleaseNote {
            id,
            title,
            description,
            url,
            date,
            is_new: date > last_seen,
        })
        .collect();

    if let (Some(user), Some(newest)) = (&user, notes.first()) {
        if newest.is_new {
            sqlx::query("UPDATE users SET last_seen_release_note = $1 WHERE id = $2")
                .bind(newest.date)
                .bind(user.id)
                .execute(&state.db)
                .await?;
        }
    }

    Ok(Json(notes))
}

/// GET /release_notes/unread_count — notes shown for the caller's roles after
/// the last acknowledged note.
async fn unread_release_notes_count(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Result<Json<UnreadCount>, ApiError> {
    let column = show_at_column(&release_note_env(raw_query.as_deref()));
    let user = optional_user(&headers, &state).await;
    let role = user
        .as_ref()
        .map(|u| u.role.clone())
        .unwrap_or_else(|| "user".to_string());
    let last_seen = match &user {
        Some(user) => last_seen_release_note(&state.db, user.id)
            .await?
            .unwrap_or_else(Utc::now),
        None => Utc::now(),
    };

    let sql = format!(
        r#"
        SELECT COUNT(*)
        FROM release_notes
        WHERE published
          AND {column} IS NOT NULL
          AND {column} > $2
          AND ('user' = ANY(target_roles) OR $1 = ANY(target_roles))
        "#
    );
    let unread_count: i64 = sqlx::query_scalar(&sql)
        .bind(&role)
        .bind(last_seen)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(UnreadCount { unread_count }))
}

// -----------------------------------------------------------------------
// Query helpers
// -----------------------------------------------------------------------

/// Parse Canvas `course_ids[]` from the raw query string (also accepts the
/// percent-encoded `course_ids%5B%5D` spelling). Returns None when absent and
/// Some(empty) when present with no numeric ids.
fn parse_course_ids(raw_query: Option<&str>) -> Option<Vec<i64>> {
    let raw = raw_query?;
    let mut seen = false;
    let mut ids = Vec::new();

    for pair in raw.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let key = key
            .to_ascii_lowercase()
            .replace("%5b", "[")
            .replace("%5d", "]");
        if key != "course_ids" && key != "course_ids[]" {
            continue;
        }
        seen = true;
        if let Ok(id) = value.parse::<i64>() {
            ids.push(id);
        }
    }

    seen.then_some(ids)
}

/// First value for `key` in a raw query string (no percent-decoding).
fn query_value(raw_query: Option<&str>, key: &str) -> Option<String> {
    raw_query?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == key).then(|| value.to_string())
    })
}

/// Canvas `value_to_boolean` for a query-string flag.
fn query_flag(raw_query: Option<&str>, key: &str) -> bool {
    matches!(
        query_value(raw_query, key).as_deref(),
        Some("true" | "1" | "t" | "yes")
    )
}

// -----------------------------------------------------------------------
// Router
// -----------------------------------------------------------------------

/// Returns the dashboard router: the existing GET /dashboard plus the Phase 3
/// todo, upcoming-events, and dashboard-card routes and the Phase 4 favorites
/// and release notes routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dashboard", get(dashboard_handler))
        .route("/dashboard/dashboard_cards", get(dashboard_cards_handler))
        .route("/users/self/todo", get(todo_handler))
        .route("/users/self/upcoming_events", get(upcoming_events_handler))
        .route(
            "/users/self/favorites/courses",
            get(list_favorite_courses).delete(reset_course_favorites),
        )
        .route(
            "/users/self/favorites/courses/{id}",
            post(add_favorite_course).delete(remove_favorite_course),
        )
        .route(
            "/release_notes",
            get(index_release_notes).post(create_release_note),
        )
        .route("/release_notes/latest", get(latest_release_notes))
        .route(
            "/release_notes/unread_count",
            get(unread_release_notes_count),
        )
        .route(
            "/release_notes/{id}",
            put(update_release_note).delete(destroy_release_note),
        )
}

#[cfg(test)]
mod tests {
    use super::parse_course_ids;

    #[test]
    fn parses_course_ids_filters() {
        assert_eq!(parse_course_ids(None), None);
        assert_eq!(parse_course_ids(Some("page=2")), None);
        assert_eq!(
            parse_course_ids(Some("course_ids[]=1&course_ids%5B%5D=2")),
            Some(vec![1, 2])
        );
        // Present but empty / non-numeric -> Some(empty), never unfiltered.
        assert_eq!(parse_course_ids(Some("course_ids=")), Some(vec![]));
        assert_eq!(parse_course_ids(Some("course_ids[]=abc")), Some(vec![]));
    }
}
