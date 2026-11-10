//! star-audit — Canvas grade change audit log
//! (`grade_change_audit_api_controller`).
//!
//! Endpoints (compound document: `{ events, linked }`):
//! - GET /audit/grade_change                                          — advanced query
//! - GET /audit/grade_change/courses/{courseId}                       — by course
//! - GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}
//! - GET /audit/grade_change/courses/{courseId}/students/{studentId}
//! - GET /audit/grade_change/courses/{courseId}/graders/{graderId}

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// One grade change event as stored in `grade_change_events`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct EventRow {
    uuid: String,
    course_id: Option<i64>,
    assignment_id: Option<i64>,
    student_id: Option<i64>,
    grader_id: Option<i64>,
    event_type: String,
    grade_before: Option<String>,
    grade_after: Option<String>,
    points_possible_before: Option<f64>,
    points_possible_after: Option<f64>,
    excused_before: bool,
    excused_after: bool,
    graded_anonymously: Option<bool>,
    version_number: Option<i32>,
    request_id: Option<String>,
    created_at: DateTime<Utc>,
}

/// Optional event filters; `None` means "do not filter".
#[derive(Debug, Default)]
struct EventFilter {
    course_id: Option<i64>,
    assignment_id: Option<i64>,
    student_id: Option<i64>,
    grader_id: Option<i64>,
}

/// Time range plus pagination query parameters (Canvas `start_time`,
/// `end_time`, `page`, `per_page`).
#[derive(Debug, Deserialize, Default)]
pub struct AuditQuery {
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl AuditQuery {
    fn offset(&self) -> i64 {
        let page = self.page.unwrap_or(1).max(1);
        (page - 1) * self.limit()
    }

    fn limit(&self) -> i64 {
        self.per_page.unwrap_or(20).clamp(1, 50)
    }
}

/// Advanced query parameters (`GET /audit/grade_change`). `assignment_id` is
/// a string because Canvas accepts the literal `override`.
#[derive(Debug, Deserialize, Default)]
pub struct AdvancedQuery {
    pub course_id: Option<i64>,
    pub assignment_id: Option<String>,
    pub student_id: Option<i64>,
    pub grader_id: Option<i64>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl AdvancedQuery {
    fn offset(&self) -> i64 {
        let page = self.page.unwrap_or(1).max(1);
        (page - 1) * self.limit()
    }

    fn limit(&self) -> i64 {
        self.per_page.unwrap_or(20).clamp(1, 50)
    }
}

/// Linked secondary collections (Canvas `linked_json`).
#[derive(Debug, Serialize)]
struct Linked {
    page_views: Vec<Value>,
    assignments: Vec<Value>,
    courses: Vec<Value>,
    users: Vec<Value>,
}

#[derive(Debug, Serialize)]
struct Compound {
    events: Vec<Value>,
    linked: Linked,
}

#[derive(Debug, sqlx::FromRow)]
struct CourseLite {
    id: i64,
    name: String,
    course_code: String,
}

#[derive(Debug, sqlx::FromRow)]
struct AssignmentLite {
    id: i64,
    course_id: i64,
    title: String,
    points_possible: f64,
}

#[derive(Debug, sqlx::FromRow)]
struct UserLite {
    id: i64,
    name: String,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas `course_authorized?`: `manage_grades` / `view_all_grades`.
async fn check_teacher_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

async fn ensure_course(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas resolves students/graders through `course.all_users`; unknown
/// course users are a 404.
async fn ensure_course_user(pool: &PgPool, course_id: i64, user_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"SELECT id FROM enrollments
           WHERE user_id = $1 AND course_id = $2 LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Data access
// ---------------------------------------------------------------------------

async fn fetch_events(
    pool: &PgPool,
    filter: &EventFilter,
    start_time: Option<DateTime<Utc>>,
    end_time: Option<DateTime<Utc>>,
    offset: i64,
    limit: i64,
) -> Result<Vec<EventRow>, ApiError> {
    let rows: Vec<EventRow> = sqlx::query_as(
        r#"
        SELECT uuid, course_id, assignment_id, student_id, grader_id,
               event_type, grade_before, grade_after,
               points_possible_before, points_possible_after,
               excused_before, excused_after, graded_anonymously,
               version_number, request_id, created_at
        FROM grade_change_events
        WHERE ($1::bigint IS NULL OR course_id = $1)
          AND ($2::bigint IS NULL OR assignment_id = $2)
          AND ($3::bigint IS NULL OR student_id = $3)
          AND ($4::bigint IS NULL OR grader_id = $4)
          AND ($5::timestamptz IS NULL OR created_at >= $5)
          AND ($6::timestamptz IS NULL OR created_at <= $6)
        ORDER BY created_at DESC, id DESC
        LIMIT $7 OFFSET $8
        "#,
    )
    .bind(filter.course_id)
    .bind(filter.assignment_id)
    .bind(filter.student_id)
    .bind(filter.grader_id)
    .bind(start_time)
    .bind(end_time)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Canvas `grade_change_event_json`.
fn event_json(row: EventRow) -> Value {
    let mut links = json!({
        "course": row.course_id,
        "student": row.student_id.map(|id| id.to_string()),
        "grader": row.grader_id.map(|id| id.to_string()),
        "page_view": row.request_id,
    });
    if let Some(assignment_id) = row.assignment_id {
        links["assignment"] = json!(assignment_id);
    }
    json!({
        "id": row.uuid,
        "created_at": row.created_at,
        "event_type": row.event_type,
        "grade_before": row.grade_before,
        "grade_after": row.grade_after,
        "excused_before": row.excused_before,
        "excused_after": row.excused_after,
        "graded_anonymously": row.graded_anonymously,
        "points_possible_before": row.points_possible_before,
        "points_possible_after": row.points_possible_after,
        "version_number": row.version_number,
        "links": links,
    })
}

/// Build the compound document from the fetched rows (Canvas `linked_json`).
async fn compound(pool: &PgPool, rows: Vec<EventRow>) -> Result<Json<Compound>, ApiError> {
    let course_ids: Vec<i64> = rows.iter().filter_map(|r| r.course_id).collect();
    let assignment_ids: Vec<i64> = rows.iter().filter_map(|r| r.assignment_id).collect();
    let mut user_ids: Vec<i64> = rows.iter().filter_map(|r| r.grader_id).collect();
    user_ids.extend(rows.iter().filter_map(|r| r.student_id));

    let courses: Vec<CourseLite> = if course_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as("SELECT id, name, course_code FROM courses WHERE id = ANY($1) ORDER BY id")
            .bind(&course_ids)
            .fetch_all(pool)
            .await?
    };

    let assignments: Vec<AssignmentLite> = if assignment_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT id, course_id, title, points_possible FROM assignments
             WHERE id = ANY($1) ORDER BY id",
        )
        .bind(&assignment_ids)
        .fetch_all(pool)
        .await?
    };

    let users: Vec<UserLite> = if user_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as("SELECT id, name FROM users WHERE id = ANY($1) ORDER BY id")
            .bind(&user_ids)
            .fetch_all(pool)
            .await?
    };

    let events: Vec<Value> = rows.into_iter().map(event_json).collect();
    Ok(Json(Compound {
        events,
        linked: Linked {
            page_views: Vec::new(),
            assignments: assignments
                .iter()
                .map(|a| {
                    json!({
                        "id": a.id,
                        "course_id": a.course_id,
                        "name": a.title,
                        "points_possible": a.points_possible,
                    })
                })
                .collect(),
            courses: courses
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "name": c.name,
                        "course_code": c.course_code,
                    })
                })
                .collect(),
            users: users
                .iter()
                .map(|u| json!({ "id": u.id, "name": u.name }))
                .collect(),
        },
    }))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /audit/grade_change/courses/{courseId}
async fn for_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<AuditQuery>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let filter = EventFilter {
        course_id: Some(course_id),
        ..Default::default()
    };
    let rows = fetch_events(
        &state.db,
        &filter,
        params.start_time,
        params.end_time,
        params.offset(),
        params.limit(),
    )
    .await?;
    compound(&state.db, rows).await
}

/// GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}
async fn for_course_assignment(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    Query(params): Query<AuditQuery>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let assignment: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM assignments WHERE id = $1 AND course_id = $2")
            .bind(assignment_id)
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    if assignment.is_none() {
        return Err(ApiError::NotFound);
    }
    let filter = EventFilter {
        course_id: Some(course_id),
        assignment_id: Some(assignment_id),
        ..Default::default()
    };
    let rows = fetch_events(
        &state.db,
        &filter,
        params.start_time,
        params.end_time,
        params.offset(),
        params.limit(),
    )
    .await?;
    compound(&state.db, rows).await
}

/// GET /audit/grade_change/courses/{courseId}/students/{studentId}
async fn for_course_student(
    State(state): State<AppState>,
    Path((course_id, student_id)): Path<(i64, i64)>,
    Query(params): Query<AuditQuery>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_course_user(&state.db, course_id, student_id).await?;
    let filter = EventFilter {
        course_id: Some(course_id),
        student_id: Some(student_id),
        ..Default::default()
    };
    let rows = fetch_events(
        &state.db,
        &filter,
        params.start_time,
        params.end_time,
        params.offset(),
        params.limit(),
    )
    .await?;
    compound(&state.db, rows).await
}

/// GET /audit/grade_change/courses/{courseId}/graders/{graderId}
async fn for_course_grader(
    State(state): State<AppState>,
    Path((course_id, grader_id)): Path<(i64, i64)>,
    Query(params): Query<AuditQuery>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_course_user(&state.db, course_id, grader_id).await?;
    let filter = EventFilter {
        course_id: Some(course_id),
        grader_id: Some(grader_id),
        ..Default::default()
    };
    let rows = fetch_events(
        &state.db,
        &filter,
        params.start_time,
        params.end_time,
        params.offset(),
        params.limit(),
    )
    .await?;
    compound(&state.db, rows).await
}

/// GET /audit/grade_change — advanced query. Queries without `course_id` or
/// `assignment_id` require administrator rights (Canvas `admin_authorized?`).
async fn advanced_query(
    State(state): State<AppState>,
    Query(params): Query<AdvancedQuery>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    let mut filter = EventFilter {
        student_id: params.student_id,
        grader_id: params.grader_id,
        ..Default::default()
    };

    let requested_assignment: Option<i64> = match params.assignment_id.as_deref() {
        None => None,
        Some("override") => {
            return Err(ApiError::BadRequest(
                "assignment_id 'override' is not supported".into(),
            ))
        }
        Some(raw) => Some(raw.parse::<i64>().map_err(|_| {
            ApiError::Validation("assignment_id must be an integer or 'override'".into())
        })?),
    };

    if let Some(course_id) = params.course_id {
        ensure_course(&state.db, course_id).await?;
        if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
            return Err(ApiError::Forbidden);
        }
        filter.course_id = Some(course_id);
        if let Some(assignment_id) = requested_assignment {
            let assignment: Option<(i64,)> =
                sqlx::query_as("SELECT id FROM assignments WHERE id = $1 AND course_id = $2")
                    .bind(assignment_id)
                    .bind(course_id)
                    .fetch_optional(&state.db)
                    .await?;
            if assignment.is_none() {
                return Err(ApiError::NotFound);
            }
            filter.assignment_id = Some(assignment_id);
        }
    } else if let Some(assignment_id) = requested_assignment {
        let assignment: Option<(i64, i64)> =
            sqlx::query_as("SELECT id, course_id FROM assignments WHERE id = $1")
                .bind(assignment_id)
                .fetch_optional(&state.db)
                .await?;
        let Some((_, course_id)) = assignment else {
            return Err(ApiError::NotFound);
        };
        if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
            return Err(ApiError::Forbidden);
        }
        filter.course_id = Some(course_id);
        filter.assignment_id = Some(assignment_id);
    } else if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    if filter.course_id.is_none()
        && filter.assignment_id.is_none()
        && filter.student_id.is_none()
        && filter.grader_id.is_none()
    {
        return Err(ApiError::BadRequest(
            "Must specify at least one query condition".into(),
        ));
    }

    let rows = fetch_events(
        &state.db,
        &filter,
        params.start_time,
        params.end_time,
        params.offset(),
        params.limit(),
    )
    .await?;
    compound(&state.db, rows).await
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/audit/grade_change", get(advanced_query))
        .route("/audit/grade_change/courses/{course_id}", get(for_course))
        .route(
            "/audit/grade_change/courses/{course_id}/assignments/{assignment_id}",
            get(for_course_assignment),
        )
        .route(
            "/audit/grade_change/courses/{course_id}/students/{student_id}",
            get(for_course_student),
        )
        .route(
            "/audit/grade_change/courses/{course_id}/graders/{grader_id}",
            get(for_course_grader),
        )
}
