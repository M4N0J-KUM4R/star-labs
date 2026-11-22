//! Port-C parity batch — Microsoft Teams sync groups, section crosslisting
//! and the account-scoped course batch endpoints (ported from the backup
//! branch's `ai_paces_gaps.rs`; the Smart Search and Course Pacing sections
//! of that file already live on this branch via `ai_experiences.rs` and
//! star-modules course_pacing, so only these three families are ported).
//!
//! Canvas controllers closed by this module (app/controllers):
//!   - `microsoft_sync/groups_controller` (create/show/destroy/sync).
//!   - `sections_controller` (users/crosslist/uncrosslist).
//!   - `courses_controller#batch_update` and the account-scoped course show
//!     (GET /accounts/:account_id/courses/:id).
//!
//! Shared stances (documented per handler where they apply):
//!   - Gates follow the Canvas chain 401 (no/invalid token) -> 404
//!     (unknown course/section) -> 403 (known context, insufficient
//!     rights); admin = global admin role, teacher = teacher/ta enrollment.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{is_admin, CourseUser, PaginatedCourseUsers, Pagination};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn course_exists(pool: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

/// Course participant: any active enrollment (any role) or admin — the
/// Canvas read gate for the section roster.
async fn course_participant(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

/// Course teacher/ta or admin — the Canvas manage gate for this batch.
async fn teacher_or_admin(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
           AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// microsoft_sync/groups (Canvas Microsoft Teams sync)
// ---------------------------------------------------------------------------

type MsGroupRow = (
    i64,                   // id
    i64,                   // course_id
    Option<String>,        // ms_group_id
    String,                // workflow_state
    Option<DateTime<Utc>>, // last_synced_at
    DateTime<Utc>,         // created_at
    DateTime<Utc>,         // updated_at
);

const MS_GROUP_COLUMNS: &str =
    "id, course_id, ms_group_id, workflow_state, last_synced_at, created_at, updated_at";

fn ms_group_json(row: MsGroupRow) -> Value {
    let (id, course_id, ms_group_id, workflow_state, last_synced_at, created_at, updated_at) = row;
    json!({
        "id": id,
        "course_id": course_id,
        "ms_group_id": ms_group_id,
        "workflow_state": workflow_state,
        "last_synced_at": last_synced_at,
        "created_at": created_at,
        "updated_at": updated_at,
    })
}

async fn load_ms_group(pool: &PgPool, course_id: i64) -> Result<Option<MsGroupRow>, ApiError> {
    sqlx::query_as(&format!(
        "SELECT {MS_GROUP_COLUMNS} FROM microsoft_sync_groups WHERE course_id = $1"
    ))
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Canvas: microsoft_sync/groups_controller#create —
/// POST /api/v1/courses/:course_id/microsoft_sync/group.
///
/// Idempotent create: the sync group for the course is created in
/// 'pending' state, or the existing row is returned. Canvas returns the
/// group object directly (unwrapped), so this handler does too. The
/// workspace's microsoft_sync_groups snapshot (0225 Canvas schema) requires
/// root_account_id, so the course's account (root account 1 fallback) is
/// recorded on create.
async fn ms_group_create(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    if let Some(existing) = load_ms_group(&state.db, course_id).await? {
        return Ok(Json(ms_group_json(existing)));
    }
    let row: MsGroupRow = sqlx::query_as(&format!(
        "INSERT INTO microsoft_sync_groups (course_id, root_account_id) \
         SELECT $1, COALESCE(c.account_id, 1) FROM courses c WHERE c.id = $1 \
         RETURNING {MS_GROUP_COLUMNS}"
    ))
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(ms_group_json(row)))
}

/// Canvas: microsoft_sync/groups_controller#show —
/// GET /api/v1/courses/:course_id/microsoft_sync/group.
///
/// 404 when the course has no sync group yet.
async fn ms_group_show(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let row = load_ms_group(&state.db, course_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(ms_group_json(row)))
}

/// Canvas: microsoft_sync/groups_controller#destroy —
/// DELETE /api/v1/courses/:course_id/microsoft_sync/group.
///
/// Removes the sync group row; 204 (404 when none exists).
async fn ms_group_destroy(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let removed = sqlx::query("DELETE FROM microsoft_sync_groups WHERE course_id = $1")
        .bind(course_id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if removed == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Canvas: microsoft_sync/groups_controller#sync —
/// POST /api/v1/courses/:course_id/microsoft_sync/schedule_sync.
///
/// Schedules a sync: workflow_state -> 'running' and last_synced_at set;
/// returns the updated group object (404 when the course has no group).
async fn ms_schedule_sync(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let scheduled: Option<MsGroupRow> = sqlx::query_as(&format!(
        "UPDATE microsoft_sync_groups \
         SET workflow_state = 'running', last_synced_at = now(), updated_at = now() \
         WHERE course_id = $1 \
         RETURNING {MS_GROUP_COLUMNS}"
    ))
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let scheduled = scheduled.ok_or(ApiError::NotFound)?;
    Ok(Json(ms_group_json(scheduled)))
}

// ---------------------------------------------------------------------------
// sections: users / crosslist / uncrosslist
// ---------------------------------------------------------------------------

type SectionRow = (
    i64,                   // id
    String,                // name
    i64,                   // course_id
    Option<DateTime<Utc>>, // start_at
    Option<DateTime<Utc>>, // end_at
    String,                // workflow_state
    Option<i64>,           // original_course_id
);

/// Canvas Section JSON: original_course_id is only emitted while the
/// section is crosslisted away from its birth course.
fn section_json(row: SectionRow) -> Value {
    let (id, name, course_id, start_at, end_at, workflow_state, original_course_id) = row;
    let mut value = json!({
        "id": id,
        "name": name,
        "course_id": course_id,
        "start_at": start_at,
        "end_at": end_at,
        "workflow_state": workflow_state,
    });
    if let Some(original) = original_course_id {
        value["original_course_id"] = json!(original);
    }
    value
}

async fn load_section(pool: &PgPool, section_id: i64) -> Result<SectionRow, ApiError> {
    sqlx::query_as(
        "SELECT id, name, course_id, start_at, end_at, workflow_state, original_course_id \
         FROM course_sections WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(section_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

#[derive(Debug, Default, Deserialize)]
pub struct SectionUsersQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    /// Accepted for Canvas compatibility (roster include[] params); the
    /// payload always carries the full id/name set.
    #[allow(dead_code)]
    #[serde(rename = "include[]", default)]
    pub include: Vec<String>,
}

/// Canvas: sections_controller#users — GET /api/v1/sections/:id/users.
///
/// The section roster mirrors the course users list shape (one row per
/// user, teacher role preferred). Readable by the section's course
/// participants and admins.
async fn section_users(
    State(state): State<AppState>,
    Path(section_id): Path<i64>,
    Query(query): Query<SectionUsersQuery>,
    auth: AuthUser,
) -> Result<Json<PaginatedCourseUsers>, ApiError> {
    let section = load_section(&state.db, section_id).await?;
    if !course_participant(&state.db, &auth, section.2).await? {
        return Err(ApiError::Forbidden);
    }
    let (page, per_page, offset) = Pagination {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve();

    let (total,): (i64,) = sqlx::query_as(
        "SELECT COUNT(DISTINCT u.id) \
         FROM enrollments e JOIN users u ON u.id = e.user_id \
         WHERE e.course_section_id = $1 AND e.workflow_state <> 'deleted'",
    )
    .bind(section_id)
    .fetch_one(&state.db)
    .await?;

    let rows: Vec<(i64, String, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, name, sortable_name, email, role, created_at FROM ( \
             SELECT DISTINCT ON (u.id) \
                    u.id, u.name, \
                    COALESCE(u.sortable_name, u.name) AS sortable_name, \
                    u.email, e.role, u.created_at \
             FROM enrollments e JOIN users u ON u.id = e.user_id \
             WHERE e.course_section_id = $1 AND e.workflow_state <> 'deleted' \
             ORDER BY u.id, (e.role = 'teacher') DESC, e.role \
         ) AS roster \
         ORDER BY sortable_name ASC, id ASC LIMIT $2 OFFSET $3",
    )
    .bind(section_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let data = rows
        .into_iter()
        .map(
            |(id, name, sortable_name, email, role, created_at)| CourseUser {
                id,
                short_name: name.clone(),
                name,
                sortable_name,
                email,
                role,
                created_at,
            },
        )
        .collect();
    Ok(Json(PaginatedCourseUsers {
        data,
        page,
        per_page,
        total,
    }))
}

/// Canvas: sections_controller#crosslist —
/// POST /api/v1/sections/:id/crosslist/:new_course_id.
///
/// Moves the section to the target course, storing its birth course in
/// original_course_id (an already-crosslisted section keeps its first
/// original; the column lands via migration 0256). Requires the teacher
/// of BOTH courses (or an admin). Returns the section JSON with course_id
/// and original_course_id.
async fn section_crosslist(
    State(state): State<AppState>,
    Path((section_id, new_course_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let section = load_section(&state.db, section_id).await?;
    if !course_exists(&state.db, new_course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, section.2).await?
        || !teacher_or_admin(&state.db, &auth, new_course_id).await?
    {
        return Err(ApiError::Forbidden);
    }
    let updated: SectionRow = sqlx::query_as(
        "UPDATE course_sections \
         SET original_course_id = COALESCE(original_course_id, course_id), \
             course_id = $2, updated_at = now() \
         WHERE id = $1 \
         RETURNING id, name, course_id, start_at, end_at, workflow_state, original_course_id",
    )
    .bind(section_id)
    .bind(new_course_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(section_json(updated)))
}

/// Canvas: sections_controller#uncrosslist — DELETE /api/v1/sections/:id/crosslist.
///
/// Restores course_id to the birth course and clears original_course_id.
/// 400 {"errors":[{"message":"can't uncrosslist a non-crosslisted
/// section"}]} when the section was never crosslisted. Same gate as
/// crosslist: teacher of the section's current and original course (or
/// admin).
async fn section_uncrosslist(
    State(state): State<AppState>,
    Path(section_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let section = load_section(&state.db, section_id).await?;
    if !teacher_or_admin(&state.db, &auth, section.2).await? {
        return Err(ApiError::Forbidden);
    }
    if let Some(original_course_id) = section.6 {
        if !teacher_or_admin(&state.db, &auth, original_course_id).await? {
            return Err(ApiError::Forbidden);
        }
        let updated: SectionRow = sqlx::query_as(
            "UPDATE course_sections \
             SET course_id = $2, original_course_id = NULL, updated_at = now() \
             WHERE id = $1 \
             RETURNING id, name, course_id, start_at, end_at, workflow_state, original_course_id",
        )
        .bind(section_id)
        .bind(original_course_id)
        .fetch_one(&state.db)
        .await?;
        return Ok((StatusCode::OK, Json(section_json(updated))).into_response());
    }
    Ok((
        StatusCode::BAD_REQUEST,
        Json(json!({
            "errors": [{ "message": "can't uncrosslist a non-crosslisted section" }]
        })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// courses: account batch_update + account-scoped show
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct BatchCourseFields {
    pub id: i64,
    pub name: Option<String>,
    pub course_code: Option<String>,
    pub workflow_state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct BatchUpdatePayload {
    /// The stance body: {courses: [{id, name?, course_code?, workflow_state?}]}
    /// (Canvas's params are a course[...] hash keyed by id).
    pub courses: Option<Vec<BatchCourseFields>>,
    /// Accepted for Canvas compatibility (the ids-only form); it carries
    /// no fields, so it only drives the completed progress.
    #[allow(dead_code)]
    pub course_ids: Option<Vec<i64>>,
}

/// Canvas: courses_controller#batch_update — PUT /api/v1/accounts/:account_id/courses.
///
/// Admin-only batch course update. Each listed course is patched by id
/// (unknown ids are skipped); the response is a completed Progress object
/// persisted in the progresses table (the Canvas batch job runs
/// synchronously in this stance).
async fn account_courses_batch_update(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<BatchUpdatePayload>>,
) -> Result<Json<Value>, ApiError> {
    let account: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(&state.db)
        .await?;
    if account.is_none() {
        return Err(ApiError::NotFound);
    }
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let fields = payload.map(|Json(p)| p).unwrap_or_default();
    for course in fields.courses.unwrap_or_default() {
        if let Some(workflow_state) = course.workflow_state.as_deref() {
            if !matches!(
                workflow_state,
                "available" | "unpublished" | "completed" | "deleted"
            ) {
                return Err(ApiError::Validation(
                    "workflow_state must be one of: available, unpublished, completed, deleted"
                        .into(),
                ));
            }
        }
        sqlx::query(
            "UPDATE courses \
             SET name = COALESCE($2, name), \
                 course_code = COALESCE($3, course_code), \
                 workflow_state = COALESCE($4, workflow_state), \
                 updated_at = now() \
             WHERE id = $1",
        )
        .bind(course.id)
        .bind(course.name.as_deref())
        .bind(course.course_code.as_deref())
        .bind(course.workflow_state.as_deref())
        .execute(&state.db)
        .await?;
    }
    let progress_id: i64 = sqlx::query_scalar(
        "INSERT INTO progresses \
             (context_type, context_id, user_id, tag, completion, workflow_state, \
              created_at, updated_at) \
         VALUES ('Account', $1, $2, 'course_batch_update', 100.0, 'completed', now(), now()) \
         RETURNING id",
    )
    .bind(account_id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({
        "progress": {
            "id": progress_id,
            "context_id": account_id,
            "context_type": "Account",
            "user_id": auth.id,
            "tag": "course_batch_update",
            "workflow_state": "completed",
            "completion": 100,
            "message": Value::Null,
            "url": format!("/api/v1/progress/{progress_id}"),
            "results": Value::Null,
        }
    })))
}

/// Canvas: courses_controller#show (account scope) —
/// GET /api/v1/accounts/:account_id/courses/:id.
///
/// The account-scoped alias of GET /courses/:id: 404 unless the course
/// lives in that account, then the same visibility gate (enrolled or
/// admin) and course show serializer.
async fn account_course_show(
    State(state): State<AppState>,
    Path((account_id, course_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<crate::Course>, ApiError> {
    let in_account: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM courses \
         WHERE id = $1 AND account_id = $2 AND workflow_state <> 'deleted')",
    )
    .bind(course_id)
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;
    if !in_account {
        return Err(ApiError::NotFound);
    }
    let course = crate::load_course_for_viewer(&state.db, course_id, &auth).await?;
    Ok(Json(course))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Routes for this gap batch; merged into `star_courses::router()`.
pub fn ai_paces_gaps_routes() -> Router<AppState> {
    Router::new()
        // Microsoft Teams sync.
        .route(
            "/courses/{courseId}/microsoft_sync/group",
            post(ms_group_create)
                .get(ms_group_show)
                .delete(ms_group_destroy),
        )
        .route(
            "/courses/{courseId}/microsoft_sync/schedule_sync",
            post(ms_schedule_sync),
        )
        // Sections roster + crosslisting.
        .route("/sections/{id}/users", get(section_users))
        .route(
            "/sections/{id}/crosslist/{newCourseId}",
            post(section_crosslist),
        )
        .route("/sections/{id}/crosslist", delete(section_uncrosslist))
        // Account-scoped courses. The account path parameter is named `id`
        // to match the existing POST /accounts/{id}/courses (and
        // star-accounts' GET): axum merges method routers for identical
        // path strings and panics on the same shape with a different name.
        .route("/accounts/{id}/courses", put(account_courses_batch_update))
        .route(
            "/accounts/{id}/courses/{courseId}",
            get(account_course_show),
        )
}
