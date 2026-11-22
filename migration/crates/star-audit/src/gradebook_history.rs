//! Gradebook history routes (Canvas `gradebook_history_api_controller`,
//! spec p5 slice B, issue #32) — appended to star-audit.
//!
//! Data source deviation (documented in the PR): Canvas reads submission
//! `Version` rows via the `submission_versions` index table. The Star
//! snapshot has the tables (migration 0225) but no writer — no route ever
//! inserts into `versions`/`submission_versions`. The audit trail the
//! snapshot actually keeps is `grade_change_events` (migrations 0092/0118),
//! one row per graded submission change. Each event is mapped to one
//! "submission version": `score_after`/`grade_after` are the version's
//! score/grade, `grader_id` the version's grader, `created_at` the grading
//! timestamp, and `version_number` the version ordering key.
//!
//! Routes:
//! - GET /courses/{courseId}/gradebook_history/days
//! - GET /courses/{courseId}/gradebook_history/{date}
//! - GET /courses/{courseId}/gradebook_history/{date}/graders/{graderId}/assignments/{assignmentId}/submissions
//! - GET /courses/{courseId}/gradebook_history/feed
//!
//! All four require course `manage_grades` (Canvas `require_manage_grades`
//! before_action → `authorized_action(@context, @current_user,
//! :manage_grades)`): 401 anonymous, 403 student/view-only, 404 unknown
//! course (Canvas `require_context` raises RecordNotFound for missing
//! courses).

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use star_common::{ApiError, AppState};

use crate::{check_teacher_or_admin, ensure_course};

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

/// `GET .../gradebook_history/feed` params (Canvas `feed` action:
/// `assignment_id`, `user_id`, `ascending`, `page`, `per_page`).
#[derive(Debug, Default, serde::Deserialize)]
pub struct FeedQuery {
    pub assignment_id: Option<i64>,
    pub user_id: Option<i64>,
    pub ascending: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl FeedQuery {
    fn offset(&self) -> i64 {
        let page = self.page.unwrap_or(1).max(1);
        (page - 1) * self.limit()
    }

    fn limit(&self) -> i64 {
        self.per_page.unwrap_or(20).clamp(1, 50)
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One grade change event shaped as a Canvas "submission version".
#[derive(Debug, Clone, sqlx::FromRow)]
struct HistoryEventRow {
    /// The submission the event belongs to.
    submission_id: Option<i64>,
    student_id: Option<i64>,
    grader_id: Option<i64>,
    assignment_id: Option<i64>,
    grade_after: Option<String>,
    score_after: Option<f64>,
    /// Canvas `SubmissionVersion#version_id` surrogate: the audit
    /// `version_number` (ordering ties are broken by the event row id
    /// in SQL).
    version_number: Option<i32>,
    created_at: DateTime<Utc>,
}

/// Assignment fields needed for the `graders[].assignments[]` entries
/// (Canvas serializes the full `assignment_json`; the Star snapshot's audit
/// module renders the lite shape shared with the compound `linked` section).
#[derive(Debug, Clone, sqlx::FromRow)]
struct HistoryAssignment {
    id: i64,
    course_id: i64,
    title: String,
    points_possible: f64,
}

async fn history_events(
    pool: &sqlx::PgPool,
    course_id: i64,
    assignment_id: Option<i64>,
    grader_filter: Option<i64>,
    student_filter: Option<i64>,
) -> Result<Vec<HistoryEventRow>, ApiError> {
    // Canvas `submissions_set` grader rule: grader_id "0" selects autograded
    // submissions via `grader_id <= 0`; the audit events mirror that by
    // storing the actual (possibly negative) grader id.
    let rows: Vec<HistoryEventRow> = sqlx::query_as(
        r#"
        SELECT sub.id AS submission_id, e.student_id, e.grader_id,
               e.assignment_id, e.grade_after,
               e.score_after::double precision AS score_after,
               e.version_number, e.created_at
        FROM grade_change_events e
        LEFT JOIN submissions sub
          ON sub.assignment_id = e.assignment_id AND sub.user_id = e.student_id
        WHERE e.course_id = $1
          AND e.assignment_id IS NOT NULL
          AND ($2::bigint IS NULL OR e.assignment_id = $2)
          AND ($3::bigint IS NULL OR e.student_id = $3)
          AND (
            $4::bigint IS NULL
            OR CASE WHEN $4 = 0 THEN COALESCE(e.grader_id, 0) <= 0
                    ELSE e.grader_id = $4 END
          )
        ORDER BY e.created_at DESC, e.id DESC
        "#,
    )
    .bind(course_id)
    .bind(assignment_id)
    .bind(student_filter)
    .bind(grader_filter)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Grader display (Canvas `update_graders_hash`): a NULL grader falls back
/// to the DefaultGrader `id: 0, name: "Graded on submission"`.
fn grader_bucket(grader_id: Option<i64>) -> i64 {
    match grader_id {
        Some(id) if id > 0 => id,
        _ => 0,
    }
}

async fn grader_names(
    pool: &sqlx::PgPool,
    grader_ids: &[i64],
) -> Result<std::collections::HashMap<i64, String>, ApiError> {
    let mut names = std::collections::HashMap::new();
    let ids: Vec<i64> = grader_ids.iter().copied().filter(|id| *id > 0).collect();
    if ids.is_empty() {
        return Ok(names);
    }
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM users WHERE id = ANY($1)")
        .bind(&ids)
        .fetch_all(pool)
        .await?;
    for (id, name) in rows {
        names.insert(id, name);
    }
    Ok(names)
}

async fn load_assignments(
    pool: &sqlx::PgPool,
    assignment_ids: &[i64],
) -> Result<std::collections::HashMap<i64, HistoryAssignment>, ApiError> {
    let mut map = std::collections::HashMap::new();
    if assignment_ids.is_empty() {
        return Ok(map);
    }
    let rows: Vec<HistoryAssignment> = sqlx::query_as(
        r#"SELECT id, course_id, title,
                  points_possible::double precision AS points_possible
           FROM assignments WHERE id = ANY($1)"#,
    )
    .bind(assignment_ids)
    .fetch_all(pool)
    .await?;
    for a in rows {
        map.insert(a.id, a);
    }
    Ok(map)
}

fn assignment_json(a: &HistoryAssignment) -> Value {
    json!({
        "id": a.id,
        "course_id": a.course_id,
        "name": a.title,
        "points_possible": a.points_possible,
    })
}

/// Day key for an event: Canvas groups by `graded_at.in_time_zone.to_date`
/// in the course's time zone; Star courses carry no per-course zone, so the
/// snapshot's convention (UTC) applies.
fn day_key(row: &HistoryEventRow) -> String {
    row.created_at.format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// Permission gate (Canvas require_manage_grades + require_context)
// ---------------------------------------------------------------------------

async fn require_manage_grades(
    state: &AppState,
    auth: &star_auth::AuthUser,
    course_id: i64,
) -> Result<(), ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// days / day_details
// ---------------------------------------------------------------------------

/// Shared grader aggregation for `days` and `day_details` (Canvas
/// `days_json` / `json_for_date` → `update_graders_hash` + `compress`).
async fn graders_for_events(
    pool: &sqlx::PgPool,
    rows: &[HistoryEventRow],
) -> Result<Vec<Value>, ApiError> {
    use std::collections::BTreeMap;
    // grader bucket -> (name, assignment_id set)
    let mut graders: BTreeMap<i64, (String, std::collections::BTreeSet<i64>)> = BTreeMap::new();
    for row in rows {
        let bucket = grader_bucket(row.grader_id);
        let entry = graders
            .entry(bucket)
            .or_insert_with(|| (String::new(), std::collections::BTreeSet::new()));
        if let Some(assignment_id) = row.assignment_id {
            entry.1.insert(assignment_id);
        }
    }
    let names = grader_names(pool, &graders.keys().copied().collect::<Vec<_>>()).await?;
    let mut assignment_ids: Vec<i64> = Vec::new();
    for (_, set) in graders.values() {
        assignment_ids.extend(set.iter().copied());
    }
    let assignments = load_assignments(pool, &assignment_ids).await?;

    let mut out = Vec::with_capacity(graders.len());
    for (bucket, (_, set)) in graders {
        let name = if bucket == 0 {
            // Canvas DefaultGrader: I18n "Graded on submission".
            "Graded on submission".to_string()
        } else {
            names.get(&bucket).cloned().unwrap_or_default()
        };
        let assignment_list: Vec<Value> = set
            .iter()
            .filter_map(|id| assignments.get(id).map(assignment_json))
            .collect();
        out.push(json!({ "id": bucket, "name": name, "assignments": assignment_list }));
    }
    Ok(out)
}

/// GET /courses/{courseId}/gradebook_history/days — Canvas `days`.
///
/// Groups every graded submission in the course by calendar day, newest day
/// first; each day lists the graders that graded that day and, per grader,
/// the assignments they touched.
async fn gradebook_history_days(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: star_auth::AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_manage_grades(&state, &auth, course_id).await?;
    let rows = history_events(&state.db, course_id, None, None, None).await?;

    use std::collections::BTreeMap;
    let mut by_day: BTreeMap<String, Vec<HistoryEventRow>> = BTreeMap::new();
    for row in rows {
        by_day.entry(day_key(&row)).or_default().push(row);
    }

    let mut days: Vec<(String, Vec<Value>)> = Vec::with_capacity(by_day.len());
    for (day, day_rows) in &by_day {
        let graders = graders_for_events(&state.db, day_rows).await?;
        days.push((day.clone(), graders));
    }
    // Canvas `sort_by { |a| a[:date] }.reverse` — date descending.
    days.sort_by(|a, b| b.0.cmp(&a.0));

    Ok(Json(
        days.into_iter()
            .map(|(date, graders)| json!({ "date": date, "graders": graders }))
            .collect(),
    ))
}

/// Parse the `:date` path segment (Canvas `Date.strptime("%Y-%m-%d")` —
/// anything else is a 400).
fn parse_history_date(raw: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map_err(|_| ApiError::BadRequest("invalid date".into()))
}

/// GET /courses/{courseId}/gradebook_history/{date} — Canvas `day_details`.
///
/// Same grader aggregation scoped to one calendar day.
async fn gradebook_history_day_details(
    State(state): State<AppState>,
    Path((course_id, date)): Path<(i64, String)>,
    auth: star_auth::AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_manage_grades(&state, &auth, course_id).await?;
    let date = parse_history_date(&date)?;
    let rows = history_events(&state.db, course_id, None, None, None).await?;
    let day_rows: Vec<HistoryEventRow> = rows
        .into_iter()
        .filter(|r| day_key(r) == date.to_string())
        .collect();
    Ok(Json(graders_for_events(&state.db, &day_rows).await?))
}

// ---------------------------------------------------------------------------
// submissions (nested version list)
// ---------------------------------------------------------------------------

/// (grade, score, graded_at, grader_name) of the prior kept version in
/// `gradebook_history_submissions` collation.
type PriorKeptVersion = (
    Option<String>,
    Option<f64>,
    Option<DateTime<Utc>>,
    Option<String>,
);

/// GET .../gradebook_history/{date}/graders/{graderId}/assignments/{assignmentId}/submissions
/// — Canvas `submissions`.
///
/// All versions for the grader/assignment/day grouped per submission
/// (`{submission_id, versions: [...]}`). Canvas keeps only versions whose
/// `score` is present and changed from the prior scored version; each kept
/// version gets `previous_{grade,graded_at,grader}` (nil on the first) and
/// `new_{grade,graded_at,grader,score}` aliases, newest-first.
async fn gradebook_history_submissions(
    State(state): State<AppState>,
    Path((course_id, date, grader_id, assignment_id)): Path<(i64, String, i64, i64)>,
    auth: star_auth::AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_manage_grades(&state, &auth, course_id).await?;
    let date = parse_history_date(&date)?;

    // Canvas `::Assignment.find(assignment_id)` — unknown assignment 404
    // (the manage_grades gate already ran).
    let assignment: Option<HistoryAssignment> = sqlx::query_as(
        r#"SELECT id, course_id, title,
                  points_possible::double precision AS points_possible
           FROM assignments WHERE id = $1"#,
    )
    .bind(assignment_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(assignment) = assignment else {
        return Err(ApiError::NotFound);
    };

    let mut rows = history_events(
        &state.db,
        course_id,
        Some(assignment_id),
        Some(grader_id),
        None,
    )
    .await?;
    // Oldest first for the previous/new collation pass, like Canvas
    // `Version.order(:number)`.
    rows.reverse();
    let day_rows: Vec<HistoryEventRow> = rows
        .into_iter()
        .filter(|r| day_key(r) == date.to_string())
        .collect();

    let names = grader_names(
        &state.db,
        &day_rows
            .iter()
            .filter_map(|r| r.grader_id)
            .collect::<Vec<_>>(),
    )
    .await?;

    // Group per submission, preserving first-seen (ascending) order.
    let mut order: Vec<Option<i64>> = Vec::new();
    let mut grouped: std::collections::HashMap<Option<i64>, Vec<&HistoryEventRow>> =
        std::collections::HashMap::new();
    for row in &day_rows {
        if !grouped.contains_key(&row.submission_id) {
            order.push(row.submission_id);
        }
        grouped.entry(row.submission_id).or_default().push(row);
    }

    // Canvas versions_json fields rendered per version (submission_attempt_json
    // core + gradebook-history additions).
    let mut out = Vec::with_capacity(order.len());
    for submission_id in order {
        let rows = grouped.remove(&submission_id).unwrap_or_default();
        let mut versions: Vec<Value> = Vec::new();
        // (grade, score, graded_at, grader_name) of the prior kept version.
        let mut prior: Option<PriorKeptVersion> = None;
        for row in rows {
            let Some(score) = row.score_after else {
                continue;
            };
            let first = prior.is_none();
            let score_changed = match &prior {
                Some((_, prior_score, _, _)) => *prior_score != Some(score),
                None => true,
            };
            if !first && !score_changed {
                continue;
            }
            let grader_name = row
                .grader_id
                .filter(|id| *id > 0)
                .and_then(|id| names.get(&id).cloned())
                .unwrap_or_else(|| "Graded on submission".to_string());
            let graded_at = Some(row.created_at);
            let grade = row.grade_after.clone();
            let (previous_grade, previous_graded_at, previous_grader) = match &prior {
                Some((g, _, at, grader)) => (g.clone(), *at, grader.clone()),
                None => (None, None, None),
            };
            let mut version = json!({
                "id": row.submission_id,
                "assignment_id": row.assignment_id,
                "assignment_name": assignment.title,
                "user_id": row.student_id,
                "score": row.score_after,
                "grade": row.grade_after,
                "graded_at": row.created_at,
                "grader": grader_name,
                "grader_id": row.grader_id,
                "submission_type": "online_text_entry",
                "workflow_state": "graded",
                "previous_grade": previous_grade,
                "previous_graded_at": previous_graded_at,
                "previous_grader": previous_grader,
                "new_grade": row.grade_after,
                "new_graded_at": row.created_at,
                "new_grader": grader_name,
                "new_score": row.score_after,
            });
            if let Some(sid) = row.submission_id {
                version["submission_id"] = json!(sid);
            }
            versions.push(version);
            prior = Some((grade, Some(score), graded_at, Some(grader_name)));
        }
        // Newest first within each submission (Canvas `.reverse`).
        versions.reverse();
        out.push(json!({ "submission_id": submission_id, "versions": versions }));
    }
    Ok(Json(out))
}

// ---------------------------------------------------------------------------
// feed
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/gradebook_history/feed — Canvas `feed`.
///
/// Paginated, uncollated submission versions for the course, optional
/// `assignment_id`/`user_id` filters, ordered by `version_id` descending
/// unless `ascending=true`. Plain `grade`/`graded_at`/`grader` fields (no
/// `new_*`/`previous_*`).
async fn gradebook_history_feed(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<FeedQuery>,
    auth: star_auth::AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_manage_grades(&state, &auth, course_id).await?;

    // Canvas `api_find(User, ...)` / `api_find(Assignment, ...)` — unknown
    // ids are 404.
    if let Some(user_id) = params.user_id {
        crate::ensure_active_user(&state.db, user_id).await?;
    }
    if let Some(assignment_id) = params.assignment_id {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM assignments WHERE id = $1 AND course_id = $2")
                .bind(assignment_id)
                .bind(course_id)
                .fetch_optional(&state.db)
                .await?;
        if row.is_none() {
            return Err(ApiError::NotFound);
        }
    }

    let ascending = matches!(
        params.ascending.as_deref(),
        Some("true") | Some("1") | Some("yes")
    );
    let order = if ascending { "ASC" } else { "DESC" };

    let rows: Vec<HistoryEventRow> = sqlx::query_as(&format!(
        r#"
        SELECT sub.id AS submission_id, e.student_id, e.grader_id,
               e.assignment_id, e.grade_after,
               e.score_after::double precision AS score_after,
               e.version_number, e.created_at
        FROM grade_change_events e
        LEFT JOIN submissions sub
          ON sub.assignment_id = e.assignment_id AND sub.user_id = e.student_id
        WHERE e.course_id = $1
          AND e.assignment_id IS NOT NULL
          AND ($2::bigint IS NULL OR e.assignment_id = $2)
          AND ($3::bigint IS NULL OR e.student_id = $3)
        ORDER BY e.version_number {order}, e.id {order}
        LIMIT $4 OFFSET $5
        "#,
    ))
    .bind(course_id)
    .bind(params.assignment_id)
    .bind(params.user_id)
    .bind(params.limit())
    .bind(params.offset())
    .fetch_all(&state.db)
    .await?;

    let names = grader_names(
        &state.db,
        &rows.iter().filter_map(|r| r.grader_id).collect::<Vec<_>>(),
    )
    .await?;
    let assignment_ids: Vec<i64> = rows.iter().filter_map(|r| r.assignment_id).collect();
    let assignments = load_assignments(&state.db, &assignment_ids).await?;

    let feed: Vec<Value> = rows
        .iter()
        .map(|row| {
            let grader_name = row
                .grader_id
                .filter(|id| *id > 0)
                .and_then(|id| names.get(&id).cloned())
                .unwrap_or_else(|| "Graded on submission".to_string());
            let assignment_title = row
                .assignment_id
                .and_then(|id| assignments.get(&id))
                .map(|a| a.title.clone());
            json!({
                "id": row.submission_id,
                "assignment_id": row.assignment_id,
                "assignment_name": assignment_title,
                "user_id": row.student_id,
                "score": row.score_after,
                "grade": row.grade_after,
                "graded_at": row.created_at,
                "grader": grader_name,
                "grader_id": row.grader_id,
                "version_number": row.version_number,
                "submission_type": "online_text_entry",
                "workflow_state": "graded",
            })
        })
        .collect();
    Ok(Json(feed))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Router for the gradebook history routes. Mounted by the composition root
/// next to the audit router (same `/api/v1` prefix).
pub fn gradebook_history_router() -> Router<AppState> {
    Router::new()
        .route("/courses/{course_id}/gradebook_history/days", get(gradebook_history_days))
        .route("/courses/{course_id}/gradebook_history/feed", get(gradebook_history_feed))
        .route("/courses/{course_id}/gradebook_history/{date}", get(gradebook_history_day_details))
        .route(
            "/courses/{course_id}/gradebook_history/{date}/graders/{grader_id}/assignments/{assignment_id}/submissions",
            get(gradebook_history_submissions),
        )
}
