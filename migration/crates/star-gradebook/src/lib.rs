//! star-gradebook: custom gradebook columns, column data and late policies.
//!
//! Implements the Canvas API v1 endpoints from
//! `app/controllers/custom_gradebook_columns_api_controller.rb`,
//! `app/controllers/custom_gradebook_column_data_api_controller.rb` and
//! `app/controllers/late_policy_controller.rb`.
//!
//! Deviations from upstream, pinned by the integration tests:
//! 1. Column create/update validation failures answer **400** (Canvas renders
//!    `column.errors` with `:bad_request`), not the Rails default 422.
//! 2. `hidden` is not a DB column: upstream keeps it in `workflow_state`
//!    (`active|hidden|deleted`) and exposes it as a virtual boolean in the
//!    serializer. Star models it exactly that way.
//! 3. `bulk_update` runs synchronously and returns a Progress already in the
//!    `completed` state — star has no job runner, but the JSON shape matches
//!    Canvas (`lib/api/v1/progress.rb`); upstream enqueues async work.
//! 4. `reorder` performs **no** `authorized_action` upstream (only
//!    `require_context`/`require_user`), so there is no 403 gate: any
//!    authenticated caller on an existing course may reorder. Only the
//!    401/404 chain applies. See `reorder_columns`.
//! 5. No pagination envelope on the column index: no star API route emits
//!    Canvas link headers; parity is scope + order (`position ASC`).
//! 6. The late policy serializer carries no timestamps, so neither do the
//!    star responses; a duplicate create answers 400 with Canvas's
//!    "only one late policy per course is allowed" message, and validation
//!    failures answer 422.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A `custom_gradebook_columns` row. `workflow_state` carries the upstream
/// tri-state (`active|hidden|deleted`); `hidden` is derived at serialization.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ColumnRow {
    id: i64,
    title: String,
    position: i32,
    teacher_notes: bool,
    read_only: bool,
    workflow_state: String,
}

/// Canvas `CustomGradebookColumnSerializer`: exactly these six keys.
#[derive(Debug, Serialize)]
struct ColumnJson {
    id: i64,
    title: String,
    position: i32,
    teacher_notes: bool,
    read_only: bool,
    hidden: bool,
}

impl From<ColumnRow> for ColumnJson {
    fn from(row: ColumnRow) -> Self {
        ColumnJson {
            id: row.id,
            title: row.title,
            position: row.position,
            teacher_notes: row.teacher_notes,
            read_only: row.read_only,
            hidden: row.workflow_state == "hidden",
        }
    }
}

/// `column[...]` params for create and update (all fields optional on
/// update; `title` is required on create and validated to 400).
#[derive(Debug, Deserialize)]
struct ColumnParams {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    position: Option<i32>,
    #[serde(default)]
    teacher_notes: Option<bool>,
    #[serde(default)]
    hidden: Option<bool>,
    #[serde(default)]
    read_only: Option<bool>,
}

/// Body of the create/update column routes: `{"column": {...}}`.
#[derive(Debug, Deserialize)]
struct ColumnPayload {
    #[serde(default)]
    column: Option<ColumnParams>,
}

/// Body of the reorder route: `{"order": [3, 1, 2]}`.
#[derive(Debug, Deserialize)]
struct ReorderPayload {
    #[serde(default)]
    order: Vec<i64>,
}

/// One `column_data[]` entry of `bulk_update`.
#[derive(Debug, Deserialize)]
struct BulkColumnDatum {
    column_id: i64,
    user_id: i64,
    #[serde(default)]
    content: Option<String>,
}

/// Body of `bulk_update`: `{"column_data": [{column_id, user_id, content}]}`.
#[derive(Debug, Deserialize)]
struct BulkUpdatePayload {
    #[serde(default)]
    column_data: Vec<BulkColumnDatum>,
}

/// Body of the datum update route: `{"column_data": {"content": "..."}}`.
#[derive(Debug, Deserialize)]
struct ColumnDataPayload {
    column_data: ColumnDataBody,
}

#[derive(Debug, Deserialize)]
struct ColumnDataBody {
    #[serde(default)]
    content: Option<String>,
}

/// Query params of the two index routes.
#[derive(Debug, Deserialize)]
struct IndexParams {
    #[serde(default)]
    include_hidden: Option<String>,
}

impl IndexParams {
    fn include_hidden(&self) -> bool {
        matches!(self.include_hidden.as_deref(), Some("true") | Some("1"))
    }
}

/// A datum as rendered by Canvas: `{user_id, content}`.
#[derive(Debug, Serialize, sqlx::FromRow)]
struct DatumJson {
    user_id: i64,
    content: Option<String>,
}

/// `late_policies` row (numeric columns cast to float8 on read).
#[derive(Debug, Clone, sqlx::FromRow)]
struct LatePolicyRow {
    id: i64,
    course_id: i64,
    missing_submission_deduction_enabled: bool,
    missing_submission_deduction: f64,
    late_submission_deduction_enabled: bool,
    late_submission_deduction: f64,
    late_submission_interval: String,
    late_submission_minimum_percent_enabled: bool,
    late_submission_minimum_percent: f64,
}

/// Canvas `LatePolicySerializer`: id + 8 fields, no timestamps.
#[derive(Debug, Serialize)]
struct LatePolicyJson {
    id: i64,
    course_id: i64,
    missing_submission_deduction_enabled: bool,
    missing_submission_deduction: f64,
    late_submission_deduction_enabled: bool,
    late_submission_deduction: f64,
    late_submission_interval: String,
    late_submission_minimum_percent_enabled: bool,
    late_submission_minimum_percent: f64,
}

impl From<LatePolicyRow> for LatePolicyJson {
    fn from(row: LatePolicyRow) -> Self {
        LatePolicyJson {
            id: row.id,
            course_id: row.course_id,
            missing_submission_deduction_enabled: row.missing_submission_deduction_enabled,
            missing_submission_deduction: row.missing_submission_deduction,
            late_submission_deduction_enabled: row.late_submission_deduction_enabled,
            late_submission_deduction: row.late_submission_deduction,
            late_submission_interval: row.late_submission_interval,
            late_submission_minimum_percent_enabled: row.late_submission_minimum_percent_enabled,
            late_submission_minimum_percent: row.late_submission_minimum_percent,
        }
    }
}

/// `late_policy[...]` params for create and update.
#[derive(Debug, Deserialize)]
struct LatePolicyParams {
    #[serde(default)]
    missing_submission_deduction_enabled: Option<bool>,
    #[serde(default)]
    missing_submission_deduction: Option<f64>,
    #[serde(default)]
    late_submission_deduction_enabled: Option<bool>,
    #[serde(default)]
    late_submission_deduction: Option<f64>,
    #[serde(default)]
    late_submission_interval: Option<String>,
    #[serde(default)]
    late_submission_minimum_percent_enabled: Option<bool>,
    #[serde(default)]
    late_submission_minimum_percent: Option<f64>,
}

/// Body of the late policy routes: `{"late_policy": {...}}`.
#[derive(Debug, Deserialize)]
struct LatePolicyPayload {
    #[serde(default)]
    late_policy: Option<LatePolicyParams>,
}

/// A `progress` row for the bulk update response.
#[derive(Debug, sqlx::FromRow)]
struct ProgressRow {
    id: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// 404 unless the course exists (Canvas `require_context`). Checked before
/// the permission gates so the 401 → 404 → 403 chain matches upstream.
async fn require_course(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// `view_all_grades OR manage_grades` (teachers/TAs; an admin without an
/// enrollment passes, like every `authorized_action` check in star).
async fn check_view_or_manage_grades(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    check_grades_role(pool, user_id, course_id, auth).await
}

/// `manage_grades` (teachers/TAs; admins pass without enrollment).
async fn check_manage_grades(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    check_grades_role(pool, user_id, course_id, auth).await
}

async fn check_grades_role(
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
           WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta')
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

/// The students visible to a course admin, including inactive ones (Canvas
/// `students_visible_to(user, include: :inactive)` over
/// `all_accepted_students`: student enrollments not rejected/deleted).
async fn student_is_visible<'e, E>(
    executor: E,
    course_id: i64,
    user_id: i64,
) -> Result<bool, ApiError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND role = 'student'
             AND workflow_state NOT IN ('rejected', 'deleted')
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Custom gradebook columns
// ---------------------------------------------------------------------------

const SELECT_COLUMN_SQL: &str =
    "SELECT id, title, position, teacher_notes, read_only, workflow_state \
     FROM custom_gradebook_columns";

/// GET /courses/{courseId}/custom_gradebook_columns — index
/// (`custom_gradebook_columns_api_controller#index`).
///
/// Default scope: active columns only. `include_hidden=true` widens it to
/// `not_deleted` (hidden columns included). Ordered by `position ASC`; no
/// pagination envelope (deviation 5).
async fn index_columns(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<IndexParams>,
) -> Result<Json<Vec<ColumnJson>>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_view_or_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let workflow_filter = if params.include_hidden() {
        "workflow_state <> 'deleted'"
    } else {
        "workflow_state = 'active'"
    };
    let sql = format!(
        "{SELECT_COLUMN_SQL} WHERE course_id = $1 AND {workflow_filter} ORDER BY position ASC"
    );

    let rows: Vec<ColumnRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .fetch_all(&state.db)
        .await?;

    Ok(Json(rows.into_iter().map(ColumnJson::from).collect()))
}

/// POST /courses/{courseId}/custom_gradebook_columns — create
/// (`custom_gradebook_columns_api_controller#create`).
///
/// `column[title]` is required; validation failures are **400** (deviation
/// 1). `column[hidden]` seeds the workflow_state (deviation 2); position
/// defaults to bottom of the list (acts_as_list).
async fn create_column(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<ColumnPayload>,
) -> Result<(StatusCode, Json<ColumnJson>), ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let params = payload
        .column
        .ok_or_else(|| ApiError::BadRequest("column[title] is required".to_string()))?;
    let title = params.title.unwrap_or_default();
    if title.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "column[title] is required".to_string(),
        ));
    }
    if title.len() > 255 {
        return Err(ApiError::BadRequest(
            "column[title] is too long (maximum is 255 characters)".to_string(),
        ));
    }

    let workflow_state = if params.hidden.unwrap_or(false) {
        "hidden"
    } else {
        "active"
    };

    let row: ColumnRow = sqlx::query_as(
        r#"
        INSERT INTO custom_gradebook_columns
            (title, position, workflow_state, course_id, teacher_notes, read_only,
             created_at, updated_at)
        VALUES ($1,
                COALESCE($2, (SELECT COALESCE(MAX(position), 0) + 1
                              FROM custom_gradebook_columns WHERE course_id = $3)),
                $4, $3, $5, $6, now(), now())
        RETURNING id, title, position, teacher_notes, read_only, workflow_state
        "#,
    )
    .bind(&title)
    .bind(params.position)
    .bind(course_id)
    .bind(workflow_state)
    .bind(params.teacher_notes.unwrap_or(false))
    .bind(params.read_only.unwrap_or(false))
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(ColumnJson::from(row))))
}

/// PUT /courses/{courseId}/custom_gradebook_columns/{id} — update
/// (`custom_gradebook_columns_api_controller#update`). Partial merge;
/// validation failures → 400 (deviation 1); hidden toggles workflow_state
/// (deviation 2).
async fn update_column(
    State(state): State<AppState>,
    Path((course_id, column_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<ColumnPayload>,
) -> Result<Json<ColumnJson>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let existing: Option<ColumnRow> = sqlx::query_as(&format!(
        "{SELECT_COLUMN_SQL} WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'"
    ))
    .bind(column_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let existing = existing.ok_or(ApiError::NotFound)?;

    let params = payload.column.unwrap_or(ColumnParams {
        title: None,
        position: None,
        teacher_notes: None,
        hidden: None,
        read_only: None,
    });

    let title = match params.title.as_deref() {
        None => existing.title,
        Some(t) if t.trim().is_empty() => {
            return Err(ApiError::BadRequest(
                "column[title] can't be blank".to_string(),
            ))
        }
        Some(t) if t.len() > 255 => {
            return Err(ApiError::BadRequest(
                "column[title] is too long (maximum is 255 characters)".to_string(),
            ))
        }
        Some(t) => t.to_string(),
    };

    // `hidden` only moves between active/hidden; a deleted column stays
    // unreachable (the scoped SELECT above already excluded it).
    let workflow_state = match params.hidden {
        Some(true) => "hidden",
        Some(false) => "active",
        None => existing.workflow_state.as_str(),
    };

    let row: ColumnRow = sqlx::query_as(
        r#"
        UPDATE custom_gradebook_columns
        SET title = $3,
            position = COALESCE($4, $5),
            teacher_notes = COALESCE($6, $7),
            read_only = COALESCE($8, $9),
            workflow_state = $10,
            updated_at = now()
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        RETURNING id, title, position, teacher_notes, read_only, workflow_state
        "#,
    )
    .bind(column_id)
    .bind(course_id)
    .bind(&title)
    .bind(params.position)
    .bind(existing.position)
    .bind(params.teacher_notes)
    .bind(existing.teacher_notes)
    .bind(params.read_only)
    .bind(existing.read_only)
    .bind(workflow_state)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(ColumnJson::from(row)))
}

/// DELETE /courses/{courseId}/custom_gradebook_columns/{id} — destroy
/// (`custom_gradebook_columns_api_controller#destroy`). Soft-delete
/// (`workflow_state='deleted'`), then render the column JSON (hidden=false
/// once the state is no longer `hidden`).
async fn destroy_column(
    State(state): State<AppState>,
    Path((course_id, column_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<ColumnJson>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<ColumnRow> = sqlx::query_as(
        r#"
        UPDATE custom_gradebook_columns
        SET workflow_state = 'deleted', updated_at = now()
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        RETURNING id, title, position, teacher_notes, read_only, workflow_state
        "#,
    )
    .bind(column_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    row.map(ColumnJson::from)
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// POST /courses/{courseId}/custom_gradebook_columns/reorder — reorder
/// (`custom_gradebook_columns_api_controller#reorder`).
///
/// Upstream deduplicates `order[]` keeping the first occurrence, then
/// assigns positions 1..n via `acts_as_list#update_order` (a CASE update),
/// scoped to the course. It performs **no** `authorized_action` — only
/// `require_context`/`require_user` — so there is no 403 gate here
/// (deviation 4): any authenticated caller on an existing course may
/// reorder. Always `200 {}`.
async fn reorder_columns(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<ReorderPayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // 401 comes from the AuthUser extractor; 404 from require_context.
    // Deliberately no permission gate (deviation 4).
    let _ = auth;
    require_course(&state.db, course_id).await?;

    // Dedupe keeping the first occurrence, like `Array#uniq!` upstream.
    let mut ids: Vec<i64> = Vec::new();
    for id in &payload.order {
        if !ids.contains(id) {
            ids.push(*id);
        }
    }

    if ids.is_empty() {
        return Ok(Json(serde_json::json!({})));
    }

    // positions 1..n via CASE (acts_as_list update_order).
    let mut sql = String::from("UPDATE custom_gradebook_columns SET position = CASE id ");
    for (i, _) in ids.iter().enumerate() {
        sql.push_str(&format!("WHEN ${} THEN {} ", i + 1, i + 1));
    }
    sql.push_str("ELSE position END, updated_at = now() ");
    sql.push_str(&format!(
        "WHERE course_id = ${} AND id = ANY(${}) AND workflow_state <> 'deleted'",
        ids.len() + 1,
        ids.len() + 2
    ));

    let mut query = sqlx::query(&sql);
    for id in &ids {
        query = query.bind(id);
    }
    query = query.bind(course_id).bind(&ids);
    query.execute(&state.db).await?;

    Ok(Json(serde_json::json!({})))
}

// ---------------------------------------------------------------------------
// Custom gradebook column data
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/custom_gradebook_columns/{id}/data — data index
/// (`custom_gradebook_column_data_api_controller#index`).
///
/// Data for the students visible to the caller; the column scope follows
/// `include_hidden` (active by default, not_deleted when true).
async fn index_column_data(
    State(state): State<AppState>,
    Path((course_id, column_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<IndexParams>,
) -> Result<Json<Vec<DatumJson>>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_view_or_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let workflow_filter = if params.include_hidden() {
        "workflow_state <> 'deleted'"
    } else {
        "workflow_state = 'active'"
    };
    let column: Option<(i64,)> = sqlx::query_as(&format!(
        "SELECT id FROM custom_gradebook_columns \
         WHERE id = $1 AND course_id = $2 AND {workflow_filter}"
    ))
    .bind(column_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    if column.is_none() {
        return Err(ApiError::NotFound);
    }

    let rows: Vec<DatumJson> = sqlx::query_as(
        r#"
        SELECT d.user_id, d.content
        FROM custom_gradebook_column_data d
        JOIN enrollments e ON e.user_id = d.user_id
        WHERE d.custom_gradebook_column_id = $1
          AND e.course_id = $2 AND e.role = 'student'
          AND e.workflow_state NOT IN ('rejected', 'deleted')
        ORDER BY d.user_id ASC
        "#,
    )
    .bind(column_id)
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// PUT /courses/{courseId}/custom_gradebook_columns/{id}/data/{userId} —
/// datum update (`custom_gradebook_column_data_api_controller#update`).
///
/// Blank `column_data[content]` **deletes** the datum but still renders the
/// datum JSON (upstream destroys then serializes the in-memory record, so
/// the response carries the pre-delete content, or null when there was no
/// datum). A non-visible student is a 404.
async fn update_column_data(
    State(state): State<AppState>,
    Path((course_id, column_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<ColumnDataPayload>,
) -> Result<Json<DatumJson>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let column: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM custom_gradebook_columns \
         WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(column_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    if column.is_none() {
        return Err(ApiError::NotFound);
    }

    if !student_is_visible(&state.db, course_id, user_id).await? {
        return Err(ApiError::NotFound);
    }

    let content = payload.column_data.content;
    let blank = content
        .as_deref()
        .map(|c| c.trim().is_empty())
        .unwrap_or(true);

    if blank {
        // Render what upstream would: the datum as it was, or null content.
        let existing: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT content FROM custom_gradebook_column_data \
             WHERE custom_gradebook_column_id = $1 AND user_id = $2",
        )
        .bind(column_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
        let content = existing.and_then(|(c,)| c);

        sqlx::query(
            "DELETE FROM custom_gradebook_column_data \
             WHERE custom_gradebook_column_id = $1 AND user_id = $2",
        )
        .bind(column_id)
        .bind(user_id)
        .execute(&state.db)
        .await?;

        return Ok(Json(DatumJson { user_id, content }));
    }

    let content = content.expect("non-blank content");
    sqlx::query(
        r#"
        INSERT INTO custom_gradebook_column_data
            (content, user_id, custom_gradebook_column_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (custom_gradebook_column_id, user_id)
        DO UPDATE SET content = EXCLUDED.content, updated_at = now()
        "#,
    )
    .bind(&content)
    .bind(user_id)
    .bind(column_id)
    .execute(&state.db)
    .await?;

    Ok(Json(DatumJson {
        user_id,
        content: Some(content),
    }))
}

/// PUT /courses/{courseId}/custom_gradebook_column_data — bulk_update
/// (`custom_gradebook_column_data_api_controller#bulk_update`).
///
/// 400 on an empty `column_data[]`; unknown column → 404 (upstream looks
/// the column up **unscoped**, not course-scoped); non-visible user → 403;
/// blank content deletes the datum. Returns a Progress tagged
/// `custom_columns_submissions_update` in the `completed` state — star runs
/// the update synchronously (deviation 3).
async fn bulk_update_column_data(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<BulkUpdatePayload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.column_data.is_empty() {
        return Err(ApiError::BadRequest(
            "column_data can't be blank".to_string(),
        ));
    }

    let mut tx = state.db.begin().await?;

    for entry in &payload.column_data {
        // Unscoped lookup, like upstream: any non-deleted column matches.
        let column: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM custom_gradebook_columns \
             WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(entry.column_id)
        .fetch_optional(&mut *tx)
        .await?;
        if column.is_none() {
            return Err(ApiError::NotFound);
        }

        if !student_is_visible(&mut *tx, course_id, entry.user_id).await? {
            return Err(ApiError::Forbidden);
        }

        let blank = entry
            .content
            .as_deref()
            .map(|c| c.trim().is_empty())
            .unwrap_or(true);

        if blank {
            sqlx::query(
                "DELETE FROM custom_gradebook_column_data \
                 WHERE custom_gradebook_column_id = $1 AND user_id = $2",
            )
            .bind(entry.column_id)
            .bind(entry.user_id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                r#"
                INSERT INTO custom_gradebook_column_data
                    (content, user_id, custom_gradebook_column_id)
                VALUES ($1, $2, $3)
                ON CONFLICT (custom_gradebook_column_id, user_id)
                DO UPDATE SET content = EXCLUDED.content, updated_at = now()
                "#,
            )
            .bind(entry.content.as_ref().expect("non-blank content"))
            .bind(entry.user_id)
            .bind(entry.column_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    // Synchronous completion (deviation 3): the progress row is born
    // completed with the same JSON shape as lib/api/v1/progress.rb.
    let progress: ProgressRow = sqlx::query_as(
        r#"
        INSERT INTO progress
            (tag, workflow_state, completion, context_type, context_id, user_id)
        VALUES ('custom_columns_submissions_update', 'completed', 100, 'Course', $1, $2)
        RETURNING id, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "completion": 100,
        "context_id": course_id,
        "context_type": "Course",
        "created_at": progress.created_at.to_rfc3339(),
        "id": progress.id,
        "message": serde_json::Value::Null,
        "tag": "custom_columns_submissions_update",
        "updated_at": progress.updated_at.to_rfc3339(),
        "user_id": auth.id,
        "workflow_state": "completed",
        "url": format!("http://localhost:3000/api/v1/progress/{}", progress.id),
    })))
}

// ---------------------------------------------------------------------------
// Late policy
// ---------------------------------------------------------------------------

/// Canvas `LatePolicy` validations: percents 0..100 inclusive (checked on
/// the merged values, per enabled flag), interval ∈ {day, hour}.
fn validate_late_policy(
    missing_enabled: bool,
    missing_deduction: f64,
    late_enabled: bool,
    late_deduction: f64,
    interval: &str,
    min_enabled: bool,
    min_percent: f64,
) -> Result<(), ApiError> {
    let valid_percent = |v: f64| (0.0..=100.0).contains(&v);
    if missing_enabled && !valid_percent(missing_deduction) {
        return Err(ApiError::Validation(
            "missing_submission_deduction must be between 0 and 100".to_string(),
        ));
    }
    if late_enabled && !valid_percent(late_deduction) {
        return Err(ApiError::Validation(
            "late_submission_deduction must be between 0 and 100".to_string(),
        ));
    }
    if min_enabled && !valid_percent(min_percent) {
        return Err(ApiError::Validation(
            "late_submission_minimum_percent must be between 0 and 100".to_string(),
        ));
    }
    if !matches!(interval, "day" | "hour") {
        return Err(ApiError::Validation(
            "late_submission_interval must be one of day, hour".to_string(),
        ));
    }
    Ok(())
}

const SELECT_LATE_POLICY_SQL: &str = "SELECT id, course_id, missing_submission_deduction_enabled, \
     missing_submission_deduction::float8, late_submission_deduction_enabled, \
     late_submission_deduction::float8, late_submission_interval, \
     late_submission_minimum_percent_enabled, late_submission_minimum_percent::float8 \
     FROM late_policies";

async fn fetch_late_policy(
    pool: &PgPool,
    course_id: i64,
) -> Result<Option<LatePolicyRow>, ApiError> {
    let row: Option<LatePolicyRow> =
        sqlx::query_as(&format!("{SELECT_LATE_POLICY_SQL} WHERE course_id = $1"))
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

/// GET /courses/{id}/late_policy — show (`late_policy_controller#show`).
/// 404 when the course has no policy; the JSON carries no timestamps
/// (deviation 6).
async fn show_late_policy(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_view_or_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row = fetch_late_policy(&state.db, course_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(serde_json::json!({
        "late_policy": LatePolicyJson::from(row),
    })))
}

/// POST /courses/{id}/late_policy — create (`late_policy_controller#create`).
/// Duplicate → 400 "only one late policy per course is allowed"; validation
/// → 422 (deviation 6). Unspecified fields fall back to the table defaults
/// (missing deduction 100, late deduction 0, interval day).
async fn create_late_policy(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<LatePolicyPayload>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM late_policies WHERE course_id = $1")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    if existing.is_some() {
        return Err(ApiError::BadRequest(
            "only one late policy per course is allowed".to_string(),
        ));
    }

    let params = payload.late_policy.unwrap_or(LatePolicyParams {
        missing_submission_deduction_enabled: None,
        missing_submission_deduction: None,
        late_submission_deduction_enabled: None,
        late_submission_deduction: None,
        late_submission_interval: None,
        late_submission_minimum_percent_enabled: None,
        late_submission_minimum_percent: None,
    });

    let missing_enabled = params.missing_submission_deduction_enabled.unwrap_or(false);
    let missing_deduction = params.missing_submission_deduction.unwrap_or(100.0);
    let late_enabled = params.late_submission_deduction_enabled.unwrap_or(false);
    let late_deduction = params.late_submission_deduction.unwrap_or(0.0);
    let interval = params
        .late_submission_interval
        .clone()
        .unwrap_or_else(|| "day".to_string());
    let min_enabled = params
        .late_submission_minimum_percent_enabled
        .unwrap_or(false);
    let min_percent = params.late_submission_minimum_percent.unwrap_or(0.0);

    validate_late_policy(
        missing_enabled,
        missing_deduction,
        late_enabled,
        late_deduction,
        &interval,
        min_enabled,
        min_percent,
    )?;

    let row: LatePolicyRow = sqlx::query_as(
        r#"
        INSERT INTO late_policies
            (course_id,
             missing_submission_deduction_enabled, missing_submission_deduction,
             late_submission_deduction_enabled, late_submission_deduction,
             late_submission_interval,
             late_submission_minimum_percent_enabled, late_submission_minimum_percent,
             created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), now())
        RETURNING id, course_id, missing_submission_deduction_enabled,
                  missing_submission_deduction::float8, late_submission_deduction_enabled,
                  late_submission_deduction::float8, late_submission_interval,
                  late_submission_minimum_percent_enabled,
                  late_submission_minimum_percent::float8
        "#,
    )
    .bind(course_id)
    .bind(missing_enabled)
    .bind(missing_deduction)
    .bind(late_enabled)
    .bind(late_deduction)
    .bind(&interval)
    .bind(min_enabled)
    .bind(min_percent)
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "late_policy": LatePolicyJson::from(row),
        })),
    ))
}

/// PATCH /courses/{id}/late_policy — update (`late_policy_controller#update`).
/// Partial merge over the stored policy; 204 no content on success; 422 on
/// invalid merged values; 404 when absent.
async fn update_late_policy(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<LatePolicyPayload>,
) -> Result<StatusCode, ApiError> {
    require_course(&state.db, course_id).await?;
    if !check_manage_grades(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let existing = fetch_late_policy(&state.db, course_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let params = payload.late_policy.unwrap_or(LatePolicyParams {
        missing_submission_deduction_enabled: None,
        missing_submission_deduction: None,
        late_submission_deduction_enabled: None,
        late_submission_deduction: None,
        late_submission_interval: None,
        late_submission_minimum_percent_enabled: None,
        late_submission_minimum_percent: None,
    });

    let missing_enabled = params
        .missing_submission_deduction_enabled
        .unwrap_or(existing.missing_submission_deduction_enabled);
    let missing_deduction = params
        .missing_submission_deduction
        .unwrap_or(existing.missing_submission_deduction);
    let late_enabled = params
        .late_submission_deduction_enabled
        .unwrap_or(existing.late_submission_deduction_enabled);
    let late_deduction = params
        .late_submission_deduction
        .unwrap_or(existing.late_submission_deduction);
    let interval = params
        .late_submission_interval
        .clone()
        .unwrap_or(existing.late_submission_interval);
    let min_enabled = params
        .late_submission_minimum_percent_enabled
        .unwrap_or(existing.late_submission_minimum_percent_enabled);
    let min_percent = params
        .late_submission_minimum_percent
        .unwrap_or(existing.late_submission_minimum_percent);

    validate_late_policy(
        missing_enabled,
        missing_deduction,
        late_enabled,
        late_deduction,
        &interval,
        min_enabled,
        min_percent,
    )?;

    sqlx::query(
        r#"
        UPDATE late_policies
        SET missing_submission_deduction_enabled = $2,
            missing_submission_deduction = $3,
            late_submission_deduction_enabled = $4,
            late_submission_deduction = $5,
            late_submission_interval = $6,
            late_submission_minimum_percent_enabled = $7,
            late_submission_minimum_percent = $8,
            updated_at = now()
        WHERE course_id = $1
        "#,
    )
    .bind(course_id)
    .bind(missing_enabled)
    .bind(missing_deduction)
    .bind(late_enabled)
    .bind(late_deduction)
    .bind(&interval)
    .bind(min_enabled)
    .bind(min_percent)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

mod gradebooks_mutations;
mod settings_what_if;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(settings_what_if::gradebook_settings_routes())
        .merge(gradebooks_mutations::gradebooks_mutation_routes())
        .route(
            "/courses/{courseId}/custom_gradebook_columns",
            get(index_columns).post(create_column),
        )
        .route(
            "/courses/{courseId}/custom_gradebook_columns/reorder",
            post(reorder_columns),
        )
        .route(
            "/courses/{courseId}/custom_gradebook_columns/{id}",
            put(update_column).delete(destroy_column),
        )
        .route(
            "/courses/{courseId}/custom_gradebook_columns/{id}/data",
            get(index_column_data),
        )
        .route(
            "/courses/{courseId}/custom_gradebook_columns/{id}/data/{userId}",
            put(update_column_data),
        )
        .route(
            "/courses/{courseId}/custom_gradebook_column_data",
            put(bulk_update_column_data),
        )
        .route(
            "/courses/{id}/late_policy",
            get(show_late_policy)
                .post(create_late_policy)
                .patch(update_late_policy),
        )
}
