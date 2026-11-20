//! star-audit — Canvas grade change audit log
//! (`grade_change_audit_api_controller`).
//!
//! Endpoints (compound document: `{ events, linked }`):
//! - GET /audit/grade_change                                          — advanced query
//! - GET /audit/grade_change/courses/{courseId}                       — by course
//! - GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}
//! - GET /audit/grade_change/courses/{courseId}/students/{studentId}
//! - GET /audit/grade_change/courses/{courseId}/graders/{graderId}
//! - GET /audit/grade_change/assignments/{assignmentId}
//! - GET /audit/grade_change/students/{studentId}
//! - GET /audit/grade_change/graders/{graderId}
//! - GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}/graders/{graderId}
//! - GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}/students/{studentId}
//! - GET /audit/grade_change/courses/{courseId}/graders/{graderId}/students/{studentId}
//! - GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}/graders/{graderId}/students/{studentId}

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
    /// Canvas `for_root_account_student` / `for_root_account_grader` stream
    /// indexes filter on the event's root account.
    root_account_id: Option<i64>,
    /// Canvas `filter_by_assignment`: a NULL assignment filter selects
    /// course final grade override events (instead of matching nothing).
    assignment_is_null: bool,
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

/// Raw query pairs for endpoints that accept `include[]=current_grade`
/// (Canvas parses repeated `include[]` params, which serde struct decoding
/// rejects).
#[derive(Debug, Default)]
struct IncludeQuery {
    /// Normalized `include[]` names.
    includes: Vec<String>,
}

impl IncludeQuery {
    fn from_pairs(pairs: &[(String, String)]) -> Self {
        let mut query = IncludeQuery::default();
        for (key, value) in pairs {
            match key.as_str() {
                // Canvas sends `include[]=current_grade`; `include=a,b` is
                // also accepted (star-accounts parses it the same way).
                "include[]" | "include" => {
                    for part in value.split(',') {
                        let name = part.trim().to_ascii_lowercase();
                        if !name.is_empty() && !query.includes.contains(&name) {
                            query.includes.push(name);
                        }
                    }
                }
                _ => {}
            }
        }
        query
    }

    fn wants_current_grade(&self) -> bool {
        self.includes.iter().any(|name| name == "current_grade")
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

/// Canvas `course_authorized?`: `manage_grades` / `view_all_grades`. In the
/// Star snapshot those rights belong to teachers, TAs and (grade-capable)
/// designers; Canvas grants `manage_grades` to `TeacherEnrollment` and
/// `TaEnrollment` by default and `view_all_grades` to teachers/TAs as well.
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
           WHERE user_id = $1 AND course_id = $2
             AND role IN ('teacher', 'ta', 'designer')
             AND workflow_state <> 'deleted'
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `admin_authorized?` (root-account `:view_grade_changes`): global
/// admins or account admins of the root account hold it.
async fn check_view_grade_changes(pool: &PgPool, auth: &AuthUser) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM account_users WHERE account_id = 1 AND user_id = $1 LIMIT 1"#,
    )
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

async fn ensure_course(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as(r#"SELECT id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'"#)
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `api_find(User.active, ...)` — the Star `users` table has no
/// soft-delete column, so this is an existence check.
async fn ensure_active_user(pool: &PgPool, user_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `@domain_root_account.associated_user?(user)` — a user belongs to
/// the root account when they have an `account_users` grant or an enrollment
/// in one of its courses. Star courses default to the root account (id 1).
async fn root_account_associated_user(
    pool: &PgPool,
    root_account_id: i64,
    user_id: i64,
) -> Result<bool, ApiError> {
    let account_ids = root_account_subtree_ids(pool, root_account_id).await?;
    let row: (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM account_users
            WHERE user_id = $1 AND account_id = ANY($2)
          UNION ALL
            SELECT 1
            FROM enrollments e
            JOIN courses c ON c.id = e.course_id
            WHERE e.user_id = $1 AND c.account_id = ANY($2)
        )
        "#,
    )
    .bind(user_id)
    .bind(&account_ids)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Ids of `account_id` plus every descendant (bounded walk over
/// `parent_account_id`; the tree is rooted at the Canvas root account).
async fn root_account_subtree_ids(pool: &PgPool, account_id: i64) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, parent_account_id FROM accounts")
            .fetch_all(pool)
            .await?;
    let mut ids = vec![account_id];
    // Iterate to fixed point; |accounts| is small and the walk is bounded.
    let mut changed = true;
    while changed {
        changed = false;
        for (id, parent) in &rows {
            if ids.contains(id) {
                continue;
            }
            if let Some(parent) = parent {
                if ids.contains(parent) {
                    ids.push(*id);
                    changed = true;
                }
            }
        }
    }
    Ok(ids)
}

/// Canvas resolves students/graders through `course.all_users` (every
/// enrollment state, `distinct`); unknown course users are a 404.
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
               points_possible_before::double precision AS points_possible_before,
               points_possible_after::double precision AS points_possible_after,
               excused_before, excused_after, graded_anonymously,
               version_number, request_id, created_at
        FROM grade_change_events
        WHERE ($1::bigint IS NULL OR course_id = $1)
          AND ($2::bigint IS NULL OR assignment_id = $2)
          AND ($3::bigint IS NULL OR student_id = $3)
          AND ($4::bigint IS NULL OR grader_id = $4)
          AND ($5::timestamptz IS NULL OR created_at >= $5)
          AND ($6::timestamptz IS NULL OR created_at <= $6)
          AND ($9::bigint IS NULL OR root_account_id = $9)
          AND ($10::boolean IS FALSE OR assignment_id IS NULL)
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
    .bind(filter.root_account_id)
    .bind(filter.assignment_is_null)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// `true` for course final grade override events. Canvas stores those with a
/// `NULL` assignment (`submission_id` NULL placeholder / `override_grade?`).
fn is_override_event(row: &EventRow) -> bool {
    row.assignment_id.is_none()
}

/// Canvas `grade_change_event_json`.
///
/// `grade_current` (string or "N/A") is only present when the caller asked
/// for `include[]=current_grade`; `course_override_grade: true` marks course
/// final grade override events (which omit the `assignment` link).
fn event_json(row: EventRow, grade_current: Option<&str>) -> Value {
    let mut links = json!({
        "course": row.course_id,
        "student": row.student_id.map(|id| id.to_string()),
        "grader": row.grader_id.map(|id| id.to_string()),
        "page_view": row.request_id,
    });
    if let Some(assignment_id) = row.assignment_id {
        links["assignment"] = json!(assignment_id);
    }
    let mut json = json!({
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
    });
    if let Some(grade_current) = grade_current {
        json["grade_current"] = json!(grade_current);
    }
    if is_override_event(&row) {
        json["course_override_grade"] = json!(true);
    }
    json
}

// ---------------------------------------------------------------------------
// render_events pipeline (Canvas grade_change_audit_api_controller)
// ---------------------------------------------------------------------------

/// Inputs to Canvas `render_events`, resolved by each handler before the
/// shared pipeline runs.
struct RenderContext<'a> {
    /// Course whose events are being listed; `None` for course-less queries
    /// (Canvas then skips `events_visible_to_current_user` and always
    /// includes override grades).
    course_id: Option<i64>,
    /// Canvas `remove_anonymous:` — `true` for student-scoped queries, where
    /// anonymous-and-muted events are dropped instead of anonymized.
    remove_anonymous: bool,
    /// Parsed `include[]` params.
    includes: &'a IncludeQuery,
}

/// Canvas `render_events` post-fetch pipeline, applied in order:
/// 1. drop course final grade override events unless allowed
///    (`include_override_grades?`),
/// 2. `include[]=current_grade` — attach `grade_current` from the current
///    submission grade,
/// 3. `events_visible_to_current_user` (course-scoped queries only) — keep
///    only events for students visible to the caller,
/// 4. anonymize (`student_id = null`) or drop events whose assignment is
///    anonymous AND muted.
async fn render_events(
    pool: &PgPool,
    auth: &AuthUser,
    mut rows: Vec<EventRow>,
    ctx: &RenderContext<'_>,
) -> Result<Json<Compound>, ApiError> {
    // 1. Override visibility (Canvas filters override events unless
    //    `include_override_grades?`, i.e. course-less or override-allowed).
    let allow_override = match ctx.course_id {
        None => true,
        Some(course_id) => course_allows_override(pool, course_id).await?,
    };
    if !allow_override {
        rows.retain(|row| !is_override_event(row));
    }

    // 2. include[]=current_grade — Canvas reads the CURRENT submission grade
    //    for the event's (assignment, student) pair: "N/A" when there is no
    //    submission, and the field is omitted when the submission's grade is
    //    NULL (`current_grade_for_event` + `if present?`). Resolved before
    //    step 4: anonymization nulls `student_id` afterwards, but the grade
    //    still keys on the original student.
    let wants_grades = ctx.includes.wants_current_grade();
    let current_grades: std::collections::HashMap<(i64, i64), Option<Option<String>>> =
        if wants_grades {
            fetch_current_grades(pool, &rows).await?
        } else {
            Default::default()
        };
    // uuid -> grade_current to render (`None` = omit the field).
    let grade_current_by_uuid: std::collections::HashMap<String, Option<String>> = rows
        .iter()
        .map(|row| {
            let grade = match current_grades
                .get(&(row.assignment_id.unwrap_or(0), row.student_id.unwrap_or(0)))
            {
                // No submission row: Canvas renders "N/A".
                None | Some(None) => Some("N/A".to_string()),
                // Submission exists but ungraded: Canvas omits the field.
                Some(Some(None)) => None,
                Some(Some(Some(grade))) => Some(grade.clone()),
            };
            (row.uuid.clone(), grade)
        })
        .collect();

    // 3. events_visible_to_current_user — course-scoped queries only.
    if let Some(course_id) = ctx.course_id {
        rows = events_visible_to_current_user(pool, auth, course_id, rows).await?;
    }

    // 4. anonymous AND muted assignments: null out `student_id`, or drop the
    //    event entirely on student-scoped queries (Canvas `remove_anonymous`).
    let anonymous_and_muted = anonymous_and_muted_assignments(pool, &rows).await?;
    if ctx.remove_anonymous {
        rows.retain(|row| {
            row.assignment_id
                .map(|id| !anonymous_and_muted.contains(&id))
                .unwrap_or(true)
        });
    } else {
        for row in &mut rows {
            if row
                .assignment_id
                .map(|id| anonymous_and_muted.contains(&id))
                .unwrap_or(false)
            {
                row.student_id = None;
            }
        }
    }

    // Serialize with optional grade_current.
    let event_values: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let grade_current = if wants_grades {
                grade_current_by_uuid.get(&row.uuid).cloned().flatten()
            } else {
                None
            };
            event_json(row, grade_current.as_deref())
        })
        .collect();
    compound_values(pool, event_values).await
}

/// Canvas `include_override_grades?` course check:
/// `Course#allow_final_grade_override?`.
async fn course_allows_override(pool: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(bool,)> =
        sqlx::query_as(r#"SELECT allow_final_grade_override FROM courses WHERE id = $1"#)
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(allow,)| allow).unwrap_or(false))
}

/// Current submission grades for the events (Canvas `current_grades` reads
/// `submissions.grade` by submission id; Star stores the numeric score and
/// renders it as text — like the rest of the snapshot — and events carry no
/// submission id, so the lookup keys on (assignment_id, student_id)).
///
/// Value is two-layered: outer `None` = no submission row (Canvas renders
/// "N/A"), inner `None` = submission exists but ungraded (Canvas omits the
/// field).
async fn fetch_current_grades(
    pool: &PgPool,
    rows: &[EventRow],
) -> Result<std::collections::HashMap<(i64, i64), Option<Option<String>>>, ApiError> {
    let mut grades = std::collections::HashMap::new();
    for row in rows {
        let (Some(assignment_id), Some(student_id)) = (row.assignment_id, row.student_id) else {
            continue;
        };
        let submission: Option<(Option<String>,)> = sqlx::query_as(
            r#"SELECT score::float8::text FROM submissions
               WHERE assignment_id = $1 AND user_id = $2"#,
        )
        .bind(assignment_id)
        .bind(student_id)
        .fetch_optional(pool)
        .await?;
        let grade = submission.map(|(g,)| g);
        grades.insert((assignment_id, student_id), grade);
    }
    Ok(grades)
}

/// Canvas `events_visible_to_current_user` — keep only events whose student
/// is visible to the caller. Full rosters see everyone; teachers/TAs limited
/// to course sections see only students in their sections (Canvas
/// `students_visible_to(user, include: :priors_and_deleted)` with
/// `enrollment_visibility_level_for`).
async fn events_visible_to_current_user(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    rows: Vec<EventRow>,
) -> Result<Vec<EventRow>, ApiError> {
    if is_admin(auth) {
        return Ok(rows);
    }
    // The caller reached this code with course rights, so they are at least
    // enrolled. `limit_privileges_to_course_section` marks section-limited
    // teachers/TAs (Star has no per-section admin visibility rows, so the
    // restriction rides on the enrollment itself).
    let limited: Option<(bool,)> = sqlx::query_as(
        r#"SELECT limit_privileges_to_course_section FROM enrollments
           WHERE user_id = $1 AND course_id = $2
             AND role IN ('teacher', 'ta', 'designer')
             AND workflow_state <> 'deleted'
           LIMIT 1"#,
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    if !limited.map(|(l,)| l).unwrap_or(false) {
        return Ok(rows);
    }
    let section_ids: Vec<(i64,)> = sqlx::query_as(
        r#"SELECT DISTINCT course_section_id FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND course_section_id IS NOT NULL"#,
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_all(pool)
    .await?;
    let section_ids: Vec<i64> = section_ids.into_iter().map(|(id,)| id).collect();
    let visible: Vec<(i64,)> = sqlx::query_as(
        r#"SELECT DISTINCT user_id FROM enrollments
           WHERE course_id = $1 AND role = 'student'
             AND (course_section_id = ANY($2) OR user_id = $3)"#,
    )
    .bind(course_id)
    .bind(&section_ids)
    .bind(auth.id)
    .fetch_all(pool)
    .await?;
    let visible: std::collections::HashSet<i64> = visible.into_iter().map(|(id,)| id).collect();
    Ok(rows
        .into_iter()
        .filter(|row| {
            row.student_id
                .map(|id| visible.contains(&id))
                .unwrap_or(true)
        })
        .collect())
}

/// Canvas `anonymous_and_muted` — assignments that are anonymous AND muted.
async fn anonymous_and_muted_assignments(
    pool: &PgPool,
    rows: &[EventRow],
) -> Result<std::collections::HashSet<i64>, ApiError> {
    let assignment_ids: Vec<i64> = rows.iter().filter_map(|r| r.assignment_id).collect();
    if assignment_ids.is_empty() {
        return Ok(Default::default());
    }
    let muted: Vec<(i64,)> = sqlx::query_as(
        r#"SELECT id FROM assignments
           WHERE id = ANY($1) AND anonymous_grading AND muted"#,
    )
    .bind(&assignment_ids)
    .fetch_all(pool)
    .await?;
    Ok(muted.into_iter().map(|(id,)| id).collect())
}

/// Compound document from already-rendered event JSON values (Canvas
/// `grade_change_events_compound_json`).
async fn compound_values(pool: &PgPool, events: Vec<Value>) -> Result<Json<Compound>, ApiError> {
    let course_ids: Vec<i64> = events
        .iter()
        .filter_map(|e| e["links"]["course"].as_i64())
        .collect();
    let assignment_ids: Vec<i64> = events
        .iter()
        .filter_map(|e| e["links"]["assignment"].as_i64())
        .collect();
    let mut user_ids: Vec<i64> = events
        .iter()
        .filter_map(|e| {
            e["links"]["grader"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
        })
        .collect();
    user_ids.extend(events.iter().filter_map(|e| {
        e["links"]["student"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
    }));

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
            "SELECT id, course_id, title,
                    points_possible::double precision AS points_possible
             FROM assignments WHERE id = ANY($1) ORDER BY id",
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

/// Parse `include[]` params from the raw query pairs (Canvas accepts
/// repeated `include[]` keys, which serde struct decoding rejects).
fn parse_includes(pairs: &[(String, String)]) -> IncludeQuery {
    IncludeQuery::from_pairs(pairs)
}

/// GET /audit/grade_change/courses/{courseId}
async fn for_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let includes = parse_includes(&raw_query);
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
    let ctx = RenderContext {
        course_id: Some(course_id),
        remove_anonymous: false,
        includes: &includes,
    };
    render_events(&state.db, &auth, rows, &ctx).await
}

/// GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}
async fn for_course_assignment(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
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
    let includes = parse_includes(&raw_query);
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
    let ctx = RenderContext {
        course_id: Some(course_id),
        remove_anonymous: false,
        includes: &includes,
    };
    render_events(&state.db, &auth, rows, &ctx).await
}

/// GET /audit/grade_change/courses/{courseId}/students/{studentId}
async fn for_course_student(
    State(state): State<AppState>,
    Path((course_id, student_id)): Path<(i64, i64)>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_course_user(&state.db, course_id, student_id).await?;
    let includes = parse_includes(&raw_query);
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
    let ctx = RenderContext {
        course_id: Some(course_id),
        remove_anonymous: false,
        includes: &includes,
    };
    render_events(&state.db, &auth, rows, &ctx).await
}

/// GET /audit/grade_change/courses/{courseId}/graders/{graderId}
async fn for_course_grader(
    State(state): State<AppState>,
    Path((course_id, grader_id)): Path<(i64, i64)>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_course_user(&state.db, course_id, grader_id).await?;
    let includes = parse_includes(&raw_query);
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
    let ctx = RenderContext {
        course_id: Some(course_id),
        remove_anonymous: false,
        includes: &includes,
    };
    render_events(&state.db, &auth, rows, &ctx).await
}

/// GET /audit/grade_change — advanced query. Queries without `course_id` or
/// `assignment_id` require administrator rights (Canvas `admin_authorized?`).
async fn advanced_query(
    State(state): State<AppState>,
    Query(params): Query<AdvancedQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    let mut filter = EventFilter {
        student_id: params.student_id,
        grader_id: params.grader_id,
        ..Default::default()
    };

    let restrict_to_override_grades = params.assignment_id.as_deref() == Some("override");
    let requested_assignment: Option<i64> = match params.assignment_id.as_deref() {
        None => None,
        Some("override") => None,
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
        if restrict_to_override_grades {
            filter.assignment_is_null = true;
        } else if let Some(assignment_id) = requested_assignment {
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
    } else if restrict_to_override_grades {
        if !check_view_grade_changes(&state.db, &auth).await? {
            return Err(ApiError::Forbidden);
        }
        filter.assignment_is_null = true;
    } else if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    if filter.course_id.is_none()
        && filter.assignment_id.is_none()
        && filter.student_id.is_none()
        && filter.grader_id.is_none()
        && !filter.assignment_is_null
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
    let includes = parse_includes(&raw_query);
    let ctx = RenderContext {
        course_id: filter.course_id,
        // Canvas passes remove_anonymous: params[:student_id].present?
        remove_anonymous: params.student_id.is_some(),
        includes: &includes,
    };
    render_events(&state.db, &auth, rows, &ctx).await
}

/// GET /audit/grade_change/assignments/{assignmentId}
///
/// Canvas `for_assignment`: 404 for unknown assignments AND for assignments
/// whose course the caller cannot read (the controller raises
/// RecordNotFound when either the root account mismatches or
/// `course_authorized?` fails — students get 404, not 403).
async fn for_assignment(
    State(state): State<AppState>,
    Path(assignment_id): Path<i64>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    let assignment: Option<(i64,)> =
        sqlx::query_as("SELECT course_id FROM assignments WHERE id = $1")
            .bind(assignment_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((course_id,)) = assignment else {
        return Err(ApiError::NotFound);
    };
    // Canvas: assignment.context.root_account == @domain_root_account (the
    // Star snapshot has a single root account, so this is vacuously true)
    // AND course_authorized?(course) — otherwise 404.
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::NotFound);
    }
    let includes = parse_includes(&raw_query);
    let filter = EventFilter {
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
    let ctx = RenderContext {
        course_id: Some(course_id),
        remove_anonymous: false,
        includes: &includes,
    };
    render_events(&state.db, &auth, rows, &ctx).await
}

/// GET /audit/grade_change/students/{studentId}
///
/// Canvas `for_student`: root-account admins only (403 otherwise —
/// `render_unauthorized_action` is 403 for logged-in users); events for the
/// whole root account, anonymous-and-muted events dropped.
async fn for_student(
    State(state): State<AppState>,
    Path(student_id): Path<i64>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    if !check_view_grade_changes(&state.db, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_active_user(&state.db, student_id).await?;
    if !root_account_associated_user(&state.db, 1, student_id).await? {
        return Err(ApiError::NotFound);
    }
    let includes = parse_includes(&raw_query);
    let filter = EventFilter {
        student_id: Some(student_id),
        root_account_id: Some(1),
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
    let ctx = RenderContext {
        course_id: None,
        remove_anonymous: true,
        includes: &includes,
    };
    render_events(&state.db, &auth, rows, &ctx).await
}

/// GET /audit/grade_change/graders/{graderId}
///
/// Canvas `for_grader`: root-account admins only (403 otherwise); events for
/// the whole root account.
async fn for_grader(
    State(state): State<AppState>,
    Path(grader_id): Path<i64>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    if !check_view_grade_changes(&state.db, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_active_user(&state.db, grader_id).await?;
    if !root_account_associated_user(&state.db, 1, grader_id).await? {
        return Err(ApiError::NotFound);
    }
    let includes = parse_includes(&raw_query);
    let filter = EventFilter {
        grader_id: Some(grader_id),
        root_account_id: Some(1),
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
    let ctx = RenderContext {
        course_id: None,
        remove_anonymous: false,
        includes: &includes,
    };
    render_events(&state.db, &auth, rows, &ctx).await
}

/// Shared body of the four-parameter
/// `for_course_and_other_parameters` routes. Canvas resolves the assignment,
/// grader and student against the course (404 when absent) before querying.
#[allow(clippy::too_many_arguments)]
async fn course_combo_events(
    state: &AppState,
    auth: &AuthUser,
    params: &AuditQuery,
    includes: &IncludeQuery,
    course_id: i64,
    assignment_id: Option<i64>,
    grader_id: Option<i64>,
    student_id: Option<i64>,
) -> Result<Json<Compound>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }
    if let Some(assignment_id) = assignment_id {
        let assignment: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM assignments WHERE id = $1 AND course_id = $2")
                .bind(assignment_id)
                .bind(course_id)
                .fetch_optional(&state.db)
                .await?;
        if assignment.is_none() {
            return Err(ApiError::NotFound);
        }
    }
    if let Some(grader_id) = grader_id {
        ensure_course_user(&state.db, course_id, grader_id).await?;
    }
    if let Some(student_id) = student_id {
        ensure_course_user(&state.db, course_id, student_id).await?;
    }
    let filter = EventFilter {
        course_id: Some(course_id),
        assignment_id,
        grader_id,
        student_id,
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
    let ctx = RenderContext {
        course_id: Some(course_id),
        // Canvas passes remove_anonymous: params[:student_id].present?
        remove_anonymous: student_id.is_some(),
        includes,
    };
    render_events(&state.db, auth, rows, &ctx).await
}

/// GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}/graders/{graderId}
async fn for_course_assignment_grader(
    State(state): State<AppState>,
    Path((course_id, assignment_id, grader_id)): Path<(i64, i64, i64)>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    let includes = parse_includes(&raw_query);
    course_combo_events(
        &state,
        &auth,
        &params,
        &includes,
        course_id,
        Some(assignment_id),
        Some(grader_id),
        None,
    )
    .await
}

/// GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}/students/{studentId}
async fn for_course_assignment_student(
    State(state): State<AppState>,
    Path((course_id, assignment_id, student_id)): Path<(i64, i64, i64)>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    let includes = parse_includes(&raw_query);
    course_combo_events(
        &state,
        &auth,
        &params,
        &includes,
        course_id,
        Some(assignment_id),
        None,
        Some(student_id),
    )
    .await
}

/// GET /audit/grade_change/courses/{courseId}/graders/{graderId}/students/{studentId}
async fn for_course_grader_student(
    State(state): State<AppState>,
    Path((course_id, grader_id, student_id)): Path<(i64, i64, i64)>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    let includes = parse_includes(&raw_query);
    course_combo_events(
        &state,
        &auth,
        &params,
        &includes,
        course_id,
        None,
        Some(grader_id),
        Some(student_id),
    )
    .await
}

/// GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}/graders/{graderId}/students/{studentId}
async fn for_course_assignment_grader_student(
    State(state): State<AppState>,
    Path((course_id, assignment_id, grader_id, student_id)): Path<(i64, i64, i64, i64)>,
    Query(params): Query<AuditQuery>,
    Query(raw_query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Compound>, ApiError> {
    let includes = parse_includes(&raw_query);
    course_combo_events(
        &state,
        &auth,
        &params,
        &includes,
        course_id,
        Some(assignment_id),
        Some(grader_id),
        Some(student_id),
    )
    .await
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/audit/grade_change", get(advanced_query))
        .route("/audit/grade_change/assignments/{assignment_id}", get(for_assignment))
        .route("/audit/grade_change/students/{student_id}", get(for_student))
        .route("/audit/grade_change/graders/{grader_id}", get(for_grader))
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
        .route(
            "/audit/grade_change/courses/{course_id}/assignments/{assignment_id}/graders/{grader_id}",
            get(for_course_assignment_grader),
        )
        .route(
            "/audit/grade_change/courses/{course_id}/assignments/{assignment_id}/students/{student_id}",
            get(for_course_assignment_student),
        )
        .route(
            "/audit/grade_change/courses/{course_id}/graders/{grader_id}/students/{student_id}",
            get(for_course_grader_student),
        )
        .route(
            "/audit/grade_change/courses/{course_id}/assignments/{assignment_id}/graders/{grader_id}/students/{student_id}",
            get(for_course_assignment_grader_student),
        )
}
