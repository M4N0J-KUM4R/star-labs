//! Misc parity gaps v2 — the revival of the backup branch's `misc_gaps.rs`
//! (struck by the gate for missing per-route Canvas citations). Every
//! handler below cites the Canvas controller#action and the route it
//! serves; domains already owned on this branch (gradebook filters in
//! `gradebook_api.rs`, sis_import_errors in star-sis) are NOT re-registered
//! — duplicate axum routes panic the router.
//!
//! Grouped by domain:
//!   Courses: course_tag_conversion, permissions_help, scopes_api,
//!            lmgb_user_details, learning_mastery_gradebook_settings_api,
//!            study_assist, accessibility scans/statistics
//!   Users: history, comm_messages_api, login (session_token)
//!   Live assessments: assessments + results
//!   Tools/misc: app_center, canvadoc_sessions (root), plugins, planner,
//!               immersive_reader, microfrontends_release_tag_override,
//!               developer_key_account_bindings, disable_post_to_sis (root)
//!
//! Shared stances (documented per handler where they apply):
//!   - teacher = teacher/ta enrollment or global admin
//!   - site/root-admin gates map to the global admin role
//!   - services without a backend (app center, immersive reader,
//!     canvadocs) return Canvas' own disabled shapes, never a broken 500.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::is_admin;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

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

async fn course_exists(pool: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// disable_post_to_sis_api
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct DisablePostToSis {
    /// Accepted for Canvas compatibility (period-scoped clears).
    #[allow(dead_code)]
    pub grading_period_id: Option<i64>,
}

/// Canvas: disable_post_to_sis_api_controller#disable_post_to_sis —
/// PUT /api/sis/courses/:course_id/disable_post_to_sis.
///
/// Bulk-clears the post_to_sis flag on the course's assignments
/// (teacher/admin). NOTE: Canvas serves this under /api/sis, OUTSIDE
/// /api/v1 — the route is registered with its full path in
/// `root_routes()` and root-mounted by star-http next to
/// `star_auth::small_controllers_root_routes()`.
async fn disable_post_to_sis(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<DisablePostToSis>>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = payload;
    let cleared = sqlx::query(
        "UPDATE assignments SET post_to_sis = FALSE, updated_at = now() \
         WHERE course_id = $1 AND post_to_sis IS TRUE",
    )
    .bind(course_id)
    .execute(&state.db)
    .await?
    .rows_affected();
    Ok(Json(json!({ "disabled_count": cleared })))
}

// ---------------------------------------------------------------------------
// course_tag_conversion
// ---------------------------------------------------------------------------

/// Canvas: courses_controller#convert_tag_overrides_to_adhoc_overrides —
/// PUT /api/v1/courses/:course_id/convert_tag_overrides.
///
/// Starts the conversion (Canvas: 409 when a job already runs, 204 on
/// start). Tags are disabled in this workspace, so the started job is a
/// completed progress row (nothing to convert — the group overrides do
/// not exist).
async fn convert_tag_overrides(
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
    // Existing active job → 409.
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM progresses \
         WHERE context_type = 'Course' AND context_id = $1 \
           AND tag = 'differentiation_tag_conversion' \
           AND workflow_state IN ('queued', 'running'))",
    )
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;
    if active {
        return Err(ApiError::Conflict(
            "A tag override conversion job is already in progress for this course.".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO progresses (context_type, context_id, user_id, tag, completion, workflow_state) \
         VALUES ('Course', $1, $2, 'differentiation_tag_conversion', 100.0, 'completed')",
    )
    .bind(course_id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Canvas: courses_controller#conversion_job_status —
/// GET /api/v1/courses/:course_id/convert_tag_overrides/status.
///
/// The latest conversion job's state (404 when none was ever started).
async fn conversion_job_status(
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
    let job: Option<(String, Option<f64>)> = sqlx::query_as(
        "SELECT workflow_state, completion FROM progresses \
         WHERE context_type = 'Course' AND context_id = $1 \
           AND tag = 'differentiation_tag_conversion' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    match job {
        Some((workflow_state, completion)) => Ok(Json(json!({
            "workflow_state": workflow_state,
            "progress": completion.unwrap_or(0.0),
        }))),
        None => Err(ApiError::NotFound),
    }
}

// ---------------------------------------------------------------------------
// permissions_help
// ---------------------------------------------------------------------------

/// Canvas: permissions_controller#help —
/// GET /api/v1/permissions/:context_type/:permission/help.
///
/// The Canvas help entries for a permission (static knowledge base; the
/// workspace ships the well-known granular permissions).
async fn permissions_help(
    Path((context_type, permission)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let known = [
        "manage_courses",
        "manage_admin_users",
        "manage_sis",
        "import_sis",
        "manage_feature_flags",
        "manage_developer_keys",
        "manage_grades",
        "view_all_grades",
        "manage_assignments",
        "manage_files",
        "manage_calendar",
        "manage_outcomes",
        "manage_rubrics",
        "manage_quiz",
        "manage_content",
        "manage_forum",
        "moderate_forum",
        "send_messages",
        "create_conversations",
        "read_course_list",
        "view_course_content",
    ];
    if !known.contains(&permission.as_str()) {
        return Err(ApiError::NotFound);
    }
    let course_ctx = context_type.eq_ignore_ascii_case("course");
    Ok(Json(json!({
        "details": [{
            "permission": permission,
            "context": if course_ctx { "course" } else { "account" },
        }],
        "considerations": [],
    })))
}

/// Canvas: permissions_controller#groups — GET /api/v1/permissions/groups.
///
/// The permission groups tree.
async fn permission_groups() -> Json<Value> {
    Json(json!([
        { "id": "account_admin", "label": "Account", "subtitle": "Account-level permissions" },
        { "id": "courses", "label": "Courses", "subtitle": "Course-level permissions" },
        { "id": "users", "label": "Users", "subtitle": "User and enrollment permissions" },
        { "id": "grading", "label": "Grading", "subtitle": "Grade and outcome permissions" },
        { "id": "communication", "label": "Communication", "subtitle": "Notification permissions" },
    ]))
}

// ---------------------------------------------------------------------------
// scopes_api
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct ScopesQuery {
    pub group_by: Option<String>,
}

/// Canvas: scopes_api_controller#index —
/// GET /api/v1/accounts/:account_id/scopes.
///
/// The named LTI scopes list (manage_developer_keys → account admin),
/// groupable by resource.
async fn account_scopes(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    Query(query): Query<ScopesQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let scopes = json!([
        { "api": "https://canvas.instructure.com/lti/public_jwk/scope/update",
          "resource_name": "public_jwk", "controller:": "", "action": "" },
        { "api": "https://canvas.instructure.com/lti/feature_flags/scope/show",
          "resource_name": "feature_flags", "controller": "", "action": "" },
        { "api": "https://purl.imsglobal.org/spec/lti-ags/scope/lineitem",
          "resource_name": "line_items", "controller": "", "action": "" },
        { "api": "https://purl.imsglobal.org/spec/lti-ags/scope/result.readonly",
          "resource_name": "line_items", "controller": "", "action": "" },
        { "api": "https://purl.imsglobal.org/spec/lti-nrps/scope/contextmembership.readonly",
          "resource_name": "names_and_roles", "controller": "", "action": "" },
    ]);
    if query.group_by.as_deref() == Some("resource_name") {
        // grouped shape: {resource: [scopes]}
        let mut grouped = serde_json::Map::new();
        if let Some(list) = scopes.as_array() {
            for s in list {
                let key = s["resource_name"].as_str().unwrap_or("other").to_string();
                grouped
                    .entry(key)
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .expect("array")
                    .push(s.clone());
            }
        }
        return Ok(Json(Value::Object(grouped)));
    }
    Ok(Json(scopes))
}

// ---------------------------------------------------------------------------
// lmgb_user_details
// ---------------------------------------------------------------------------

/// Canvas: learning_mastery_gradebook_controller#user_details —
/// GET /api/v1/courses/:course_id/users/:id/lmgb_user_details.
///
/// The LMGB user row (name + last login, from pseudonyms) for teachers.
async fn lmgb_user_details(
    State(state): State<AppState>,
    Path((course_id, user_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let user: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    let (name,) = user.ok_or(ApiError::NotFound)?;
    let last_login: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT MAX(last_login_at) FROM pseudonyms WHERE user_id = $1 AND last_login_at IS NOT NULL",
    )
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({
        "id": user_id,
        "name": name,
        "last_login": last_login.map(|d| d.to_rfc3339()),
    })))
}

// ---------------------------------------------------------------------------
// learning_mastery_gradebook_settings_api
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct LmgbSettings {
    pub settings: Option<Value>,
}

/// Canvas: learning_mastery_gradebook_settings_api_controller#show —
/// GET /api/v1/courses/:course_id/learning_mastery_gradebook_settings.
///
/// Per-course LMGB settings (Canvas keeps these in the course settings
/// hash; the workspace stores course settings in courses.settings, added
/// by migration 0252).
async fn lmgb_settings_show(
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
    let settings: Value = sqlx::query_scalar(
        "SELECT COALESCE(settings -> 'learning_mastery_gradebook_settings', '{}'::jsonb) \
         FROM courses WHERE id = $1",
    )
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(
        json!({ "learning_mastery_gradebook_settings": settings }),
    ))
}

/// Canvas: learning_mastery_gradebook_settings_api_controller#update —
/// PUT /api/v1/courses/:course_id/learning_mastery_gradebook_settings.
///
/// Shallow-merges the incoming settings under the course's
/// learning_mastery_gradebook_settings key and echoes them back.
async fn lmgb_settings_update(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<LmgbSettings>>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let incoming = payload
        .map(|Json(p)| p.settings.unwrap_or(json!({})))
        .unwrap_or_else(|| json!({}));
    sqlx::query(
        "UPDATE courses SET \
            settings = COALESCE(settings, '{}'::jsonb) || \
                jsonb_build_object('learning_mastery_gradebook_settings', $2::jsonb), \
            updated_at = now() \
         WHERE id = $1",
    )
    .bind(course_id)
    .bind(&incoming)
    .execute(&state.db)
    .await?;
    Ok(Json(
        json!({ "learning_mastery_gradebook_settings": incoming }),
    ))
}

// ---------------------------------------------------------------------------
// study_assist
// ---------------------------------------------------------------------------

/// Canvas: study_assist_controller#create —
/// POST /api/v1/courses/:course_id/study_assist.
///
/// Canvas queues a study-assist generation; no AI backend here → the
/// accepted-queued shape.
async fn study_assist_create(
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
    Ok(Json(json!({ "status": "queued" })))
}

// ---------------------------------------------------------------------------
// accessibility_course_scans + statistics
// ---------------------------------------------------------------------------

/// Canvas: accessibility_course_scans_controller#create —
/// POST /api/v1/users/:user_id/educator_accessibility_course_scan.
///
/// Canvas gates on educator_dashboard + a11y features; disabled →
/// unauthorized (never a broken 500).
async fn accessibility_course_scan(
    Path(_user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let _ = auth;
    Err(ApiError::Unauthorized)
}

/// Canvas: accessibility_course_scans_controller#statistics —
/// GET /api/v1/users/:user_id/educator_accessibility_course_statistics.
///
/// The educator a11y statistics; disabled → unauthorized.
async fn accessibility_course_statistics(
    Path(_user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let _ = auth;
    Err(ApiError::Unauthorized)
}

// ---------------------------------------------------------------------------
// history
// ---------------------------------------------------------------------------

/// Canvas: history_controller#index —
/// GET /api/v1/users/:user_id/history.
///
/// The user's page-view activity stream (self or admin; 3-week window
/// like Canvas). The workspace's page_views snapshot keys rows by
/// request_id (Canvas PageView uuid), so the entry id IS the request id.
async fn user_history(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Unauthorized);
    }
    type HistoryRow = (
        String,
        Option<String>,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    );
    let rows: Vec<HistoryRow> = sqlx::query_as(
        "SELECT request_id, url, context_type, created_at FROM page_views \
             WHERE user_id = $1 AND created_at >= now() - interval '3 weeks' \
             ORDER BY created_at DESC LIMIT 100",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(request_id, url, context_type, created_at)| {
                json!({
                    "id": request_id,
                    "url": url,
                    "context_type": context_type,
                    "visited_at": created_at.map(|d| d.to_rfc3339()),
                })
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// comm_messages_api
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct CommMessagesQuery {
    /// Accepted for Canvas compatibility; the messages table carries no
    /// user linkage (see the handler doc).
    #[allow(dead_code)]
    pub user_id: Option<i64>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub per_page: Option<i64>,
}

/// Canvas: comm_messages_api_controller#index — GET /api/v1/comm_messages.
///
/// The notification messages feed (site-admin read_messages OR root-account
/// admins_can_view_notifications; self-serve for own id is NOT allowed in
/// Canvas — admin gate only).
async fn comm_messages(
    State(state): State<AppState>,
    Query(query): Query<CommMessagesQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !is_admin(&auth) {
        let allowed: bool = sqlx::query_scalar(
            "SELECT COALESCE((settings ->> 'admins_can_view_notifications')::boolean, FALSE) \
             FROM accounts WHERE parent_account_id IS NULL ORDER BY id LIMIT 1",
        )
        .fetch_optional(&state.db)
        .await?
        .unwrap_or(false);
        if !allowed {
            return Err(ApiError::Unauthorized);
        }
    }
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    type CommRow = (
        i64,
        Option<String>,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    );
    let rows: Vec<CommRow> = sqlx::query_as(
        "SELECT id, subject, body, created_at FROM messages \
             WHERE ($2::timestamptz IS NULL OR created_at >= $2) \
               AND ($3::timestamptz IS NULL OR created_at <= $3) \
             ORDER BY created_at DESC LIMIT $4",
    )
    .bind(parse_opt_ts(query.start_time.as_deref(), "start_time")?)
    .bind(parse_opt_ts(query.end_time.as_deref(), "end_time")?)
    .bind(per_page)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "comm_messages": rows
            .into_iter()
            .map(|(id, subject, body, created_at)| json!({
                "id": id,
                "subject": subject,
                "body": body,
                "created_at": created_at.map(|d| d.to_rfc3339()),
            }))
            .collect::<Vec<_>>(),
    })))
}

fn parse_opt_ts(
    value: Option<&str>,
    field: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiError> {
    match value {
        None => Ok(None),
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&chrono::Utc))
            .map(Some)
            .map_err(|_| ApiError::BadRequest(format!("invalid {field}"))),
    }
}

// ---------------------------------------------------------------------------
// login (session_token)
// ---------------------------------------------------------------------------

/// Canvas: login_controller#session_token —
/// GET /api/v1/login/session_token.
///
/// Canvas returns an XSS-safe session token envelope; the workspace
/// issues its JWT-based session echo (self only — the token mints the
/// caller's own session).
async fn login_session_token(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let token = star_auth::create_token(auth.id, &auth.role, &state.jwt_secret)
        .map_err(|e| ApiError::BadRequest(format!("token generation failed: {e}")))?;
    Ok(Json(json!({ "session_token": token })))
}

// ---------------------------------------------------------------------------
// live_assessments
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct LiveAssessmentPayload {
    pub name: Option<String>,
    /// Accepted for Canvas compatibility; the assessments table stores no
    /// points column (key/title/context only).
    #[allow(dead_code)]
    pub points_possible: Option<f64>,
    pub score: Option<f64>,
    pub user_id: Option<i64>,
}

/// Canvas: live_assessments/assessments_api_controller#index —
/// GET /api/v1/courses/:course_id/live_assessments.
///
/// The course's live assessments, oldest first.
async fn live_assessments_index(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, title FROM live_assessments_assessments \
         WHERE context_type = 'Course' AND context_id = $1 ORDER BY id",
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, title)| json!({ "id": id, "name": title }))
            .collect(),
    ))
}

/// Canvas: live_assessments/assessments_api_controller#create —
/// POST /api/v1/courses/:course_id/live_assessments.
///
/// Creates a live assessment (name required; a deterministic key is
/// minted — Canvas keys assessments by uuid).
async fn live_assessments_create(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<LiveAssessmentPayload>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let name = payload.name.as_deref().map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err(ApiError::Validation("name can't be blank".into()));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO live_assessments_assessments (key, title, context_type, context_id, updated_at) \
         VALUES ($3, $2, 'Course', $1, now()) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(format!("la-{course_id}-{}", chrono::Utc::now().timestamp()))
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "name": name }))))
}

/// Canvas: live_assessments/results_api_controller#index —
/// GET /api/v1/courses/:course_id/live_assessments/:assessment_id/results.
///
/// The recorded results of one assessment, oldest first.
async fn live_assessment_results_index(
    State(state): State<AppState>,
    Path((course_id, assessment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let rows: Vec<(i64, i64, bool, chrono::NaiveDateTime)> = sqlx::query_as(
        "SELECT id, user_id, passed, assessed_at FROM live_assessments_results \
         WHERE assessment_id = $1 ORDER BY id",
    )
    .bind(assessment_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, user_id, passed, assessed_at)| {
                json!({ "id": id, "user_id": user_id, "passed": passed,
                        "assessed_at": assessed_at.and_utc().to_rfc3339() })
            })
            .collect(),
    ))
}

/// Canvas: live_assessments/results_api_controller#create —
/// POST /api/v1/courses/:course_id/live_assessments/:assessment_id/results.
///
/// Records one student result (user_id required; passed derives from a
/// positive score like the backup stance).
async fn live_assessment_results_create(
    State(state): State<AppState>,
    Path((course_id, assessment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<LiveAssessmentPayload>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if !teacher_or_admin(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let Some(user_id) = payload.user_id else {
        return Err(ApiError::Validation("user_id is required".into()));
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO live_assessments_results (assessment_id, user_id, assessor_id, passed, assessed_at) \
         VALUES ($1, $2, $3, $4, now()) RETURNING id",
    )
    .bind(assessment_id)
    .bind(user_id)
    .bind(auth.id)
    .bind(payload.score.map(|s| s > 0.0).unwrap_or(false))
    .fetch_one(&state.db)
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": id, "user_id": user_id })),
    ))
}

// ---------------------------------------------------------------------------
// app_center, canvadoc_sessions, immersive_reader, microfrontends
// ---------------------------------------------------------------------------

/// Canvas: app_center_controller#index —
/// GET /api/v1/accounts/:account_id/app_center/apps and
/// GET /api/v1/courses/:course_id/app_center/apps.
///
/// The EduAppCenter list; no app center configured → the Canvas empty
/// shape.
async fn app_center_apps() -> Json<Value> {
    Json(json!({ "objects": [], "meta": { "total_count": 0, "current_page": 1, "page_count": 0 } }))
}

/// Canvas: canvadoc_sessions_controller#show — GET /canvadoc_session.
///
/// Canvas session cookies for the annotation proxy; no canvadocs backend
/// → 401-style refusal. NOTE: the manifest carries /canvadoc_session
/// WITHOUT the /api/v1 prefix — the route is registered with its full
/// path in `root_routes()` and root-mounted by star-http.
async fn canvadoc_session_get() -> Result<Json<Value>, ApiError> {
    Err(ApiError::Unauthorized)
}

/// Canvas: canvadoc_sessions_controller#create — POST /canvadoc_session.
///
/// Same disabled-service stance as the show side (401).
async fn canvadoc_session_post() -> Result<Json<Value>, ApiError> {
    Err(ApiError::Unauthorized)
}

/// Canvas: immersive_reader_controller#authenticate —
/// GET /api/v1/immersive_reader/authenticate.
///
/// Microsoft Immersive Reader token; no service credentials → Canvas'
/// not-configured 400 shape.
async fn immersive_reader_authenticate() -> Result<Json<Value>, ApiError> {
    Err(ApiError::BadRequest(
        "Immersive Reader is not configured".into(),
    ))
}

/// Canvas: microfrontends/release_tag_override_controller#show —
/// GET /api/v1/microfrontends/release_tag_override.
///
/// A per-session cookie override; stateless here → the empty echo.
async fn mf_release_tag_override_get() -> Json<Value> {
    Json(json!({ "release_tag": Value::Null }))
}

/// Canvas: microfrontends/release_tag_override_controller#destroy —
/// DELETE /api/v1/microfrontends/release_tag_override.
///
/// Clears the override cookie; stateless here → 204.
async fn mf_release_tag_override_delete() -> StatusCode {
    StatusCode::NO_CONTENT
}

// ---------------------------------------------------------------------------
// plugins
// ---------------------------------------------------------------------------

/// Canvas: plugins_controller#show — GET /api/v1/plugins/:id.
///
/// Site-admin plugin settings (Canvas plugin records; the path id IS the
/// plugin name — see the plugins table, migration 0252).
async fn plugin_show(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let row: Option<Value> = sqlx::query_scalar("SELECT value FROM plugins WHERE name = $1")
        .bind(&plugin_id)
        .fetch_optional(&state.db)
        .await?;
    Ok(Json(
        json!({ "id": plugin_id, "settings": row.unwrap_or(json!({})) }),
    ))
}

/// Canvas: plugins_controller#update — PUT /api/v1/plugins/:id.
///
/// Upserts the plugin's settings hash (site admin).
async fn plugin_update(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    auth: AuthUser,
    payload: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let settings = payload.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    sqlx::query(
        "INSERT INTO plugins (name, value) VALUES ($1, $2) \
         ON CONFLICT (name) DO UPDATE SET value = $2, updated_at = now()",
    )
    .bind(&plugin_id)
    .bind(&settings)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "id": plugin_id, "settings": settings })))
}

// ---------------------------------------------------------------------------
// planner
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct PlannerQuery {
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub per_page: Option<i64>,
}

/// Canvas: planner_controller#index — GET /api/v1/planner/items.
///
/// The caller's planner stream (planner_notes/todo items, migration
/// 0252), windowed by start_date/end_date. The manifest carries only the
/// two index GETs — the create side stays unregistered.
async fn planner_items(
    State(state): State<AppState>,
    Query(query): Query<PlannerQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    planner_stream(&state, auth.id, &query).await
}

/// Canvas: planner_controller#index (user scope) —
/// GET /api/v1/users/:user_id/planner/items.
///
/// Same planner stream for the path user (self only, or admin).
async fn user_planner_items(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(query): Query<PlannerQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    if user_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    planner_stream(&state, user_id, &query).await
}

async fn planner_stream(
    state: &AppState,
    target: i64,
    query: &PlannerQuery,
) -> Result<Json<Vec<Value>>, ApiError> {
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    type PlannerRow = (
        i64,
        Option<i64>,
        String,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    );
    let rows: Vec<PlannerRow> = sqlx::query_as(
        "SELECT id, course_id, title, details, todo_date FROM planner_items \
             WHERE user_id = $1 AND workflow_state = 'active' \
               AND ($2::timestamptz IS NULL OR todo_date >= $2) \
               AND ($3::timestamptz IS NULL OR todo_date <= $3) \
             ORDER BY todo_date NULLS LAST, id LIMIT $4",
    )
    .bind(target)
    .bind(parse_opt_ts(query.start_date.as_deref(), "start_date")?)
    .bind(parse_opt_ts(query.end_date.as_deref(), "end_date")?)
    .bind(per_page)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, course_id, title, details, todo_date)| {
                json!({
                    "id": id,
                    "course_id": course_id,
                    "title": title,
                    "details": details,
                    "todo_date": todo_date.map(|d| d.to_rfc3339()),
                })
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// developer_key_account_bindings
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct BindingPayload {
    pub developer_key_account_binding: Option<BindingInner>,
}

#[derive(Debug, Default, Deserialize)]
pub struct BindingInner {
    pub workflow_state: Option<String>,
}

/// Canvas: developer_key_account_bindings_api_controller#create_or_update —
/// POST /api/v1/accounts/:account_id/developer_keys/:developer_key_id/developer_key_account_bindings.
///
/// Create-or-update the binding (root account, manage_developer_keys →
/// admin). The workspace has no bindings table, so the create_or_update
/// stance echoes the requested binding state (the backup branch's shape).
async fn create_or_update_binding(
    State(state): State<AppState>,
    Path((account_id, key_id)): Path<(i64, i64)>,
    auth: AuthUser,
    payload: Option<Json<BindingPayload>>,
) -> Result<Json<Value>, ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND parent_account_id IS NULL)",
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let key: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM developer_keys WHERE id = $1")
        .bind(key_id)
        .fetch_optional(&state.db)
        .await?;
    if key.is_none() {
        return Err(ApiError::NotFound);
    }
    let workflow_state = payload
        .and_then(|Json(p)| p.developer_key_account_binding)
        .and_then(|b| b.workflow_state)
        .unwrap_or_else(|| "on".to_string());
    Ok(Json(json!({
        "developer_key_id": key_id,
        "account_id": account_id,
        "workflow_state": workflow_state,
    })))
}

// ---------------------------------------------------------------------------
// Routers
// ---------------------------------------------------------------------------

/// The /api/v1-nested gap routes; merged into `star_courses::router()`.
///
/// Deliberately NOT registered here (already served on this branch, or
/// not manifest routes): gradebook filters (`gradebook_api.rs`),
/// sis_import_errors (star-sis), mastery_paths stats, POST /planner/items.
pub fn misc_gaps_routes() -> Router<AppState> {
    Router::new()
        // Course tag conversion.
        .route(
            "/courses/{courseId}/convert_tag_overrides",
            put(convert_tag_overrides),
        )
        .route(
            "/courses/{courseId}/convert_tag_overrides/status",
            get(conversion_job_status),
        )
        // Permissions help + scopes.
        .route("/permissions/groups", get(permission_groups))
        .route(
            "/permissions/{contextType}/{permission}/help",
            get(permissions_help),
        )
        .route("/accounts/{accountId}/scopes", get(account_scopes))
        // LMGB details + settings + study assist.
        .route(
            "/courses/{courseId}/users/{id}/lmgb_user_details",
            get(lmgb_user_details),
        )
        .route(
            "/courses/{courseId}/learning_mastery_gradebook_settings",
            get(lmgb_settings_show).put(lmgb_settings_update),
        )
        .route(
            "/courses/{courseId}/study_assist",
            post(study_assist_create),
        )
        // Accessibility educator dashboards (disabled).
        .route(
            "/users/{userId}/educator_accessibility_course_scan",
            post(accessibility_course_scan),
        )
        .route(
            "/users/{userId}/educator_accessibility_course_statistics",
            get(accessibility_course_statistics),
        )
        // History + comm messages + login session token.
        .route("/users/{userId}/history", get(user_history))
        .route("/comm_messages", get(comm_messages))
        .route("/login/session_token", get(login_session_token))
        // Live assessments.
        .route(
            "/courses/{courseId}/live_assessments",
            get(live_assessments_index).post(live_assessments_create),
        )
        .route(
            "/courses/{courseId}/live_assessments/{assessmentId}/results",
            get(live_assessment_results_index).post(live_assessment_results_create),
        )
        // Tools & misc.
        .route("/courses/{courseId}/app_center/apps", get(app_center_apps))
        .route(
            "/accounts/{accountId}/app_center/apps",
            get(app_center_apps),
        )
        .route(
            "/immersive_reader/authenticate",
            get(immersive_reader_authenticate),
        )
        .route(
            "/microfrontends/release_tag_override",
            get(mf_release_tag_override_get).delete(mf_release_tag_override_delete),
        )
        .route("/plugins/{id}", get(plugin_show).put(plugin_update))
        .route("/planner/items", get(planner_items))
        .route("/users/{userId}/planner/items", get(user_planner_items))
        .route(
            "/accounts/{accountId}/developer_keys/{developerKeyId}/developer_key_account_bindings",
            post(create_or_update_binding),
        )
}

/// Routes Canvas serves OUTSIDE /api/v1 — the /canvadoc_session pair and
/// the /api/sis disable_post_to_sis family. Root-mounted by star-http
/// next to `star_auth::small_controllers_root_routes()`; the route strings
/// carry their full paths.
pub fn root_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/canvadoc_session",
            get(canvadoc_session_get).post(canvadoc_session_post),
        )
        .route(
            "/api/sis/courses/{courseId}/disable_post_to_sis",
            put(disable_post_to_sis),
        )
}
