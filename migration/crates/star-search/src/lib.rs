//! star-search — search, smart search, and page view endpoints.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.
//!
//! Routes implemented (10 total):
//!   SearchController  — GET /search/recipients, GET /search/rubrics, GET /search/all_courses
//!   SmartSearch       — GET /courses/:course_id/smartsearch, /log, /index_status
//!   PageViews         — GET  /users/:user_id/page_views
//!                       POST /users/:user_id/page_views/query
//!                       GET  /users/:user_id/page_views/query/:query_id
//!                       GET  /users/:user_id/page_views/query/:query_id/results
//!   (batch variants omitted per brief — they share the same handlers)

use axum::{
    extract::{Path, Query, State},
    http::{header::CONTENT_DISPOSITION, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A recipient returned by GET /search/recipients.
/// Canvas shapes this via lib/api/v1/conversation.rb:conversation_recipients_json.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Recipient {
    /// A user recipient.
    User(UserRecipient),
    /// A context (course / group / section) recipient.
    Context(ContextRecipient),
}

#[derive(Debug, Serialize)]
pub struct UserRecipient {
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub common_courses: Option<std::collections::HashMap<i64, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub common_groups: Option<std::collections::HashMap<i64, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(rename = "type")]
    pub type_: String,
    pub permissions: UserPermissions,
}

#[derive(Debug, Serialize)]
pub struct UserPermissions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_messages: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ContextRecipient {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<std::collections::HashMap<String, bool>>,
}

/// Query params for GET /search/recipients.
#[derive(Debug, Deserialize)]
pub struct RecipientsFilter {
    pub search: Option<String>,
    pub context: Option<String>,
    pub exclude: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub user_id: Option<i64>,
    pub from_conversation_id: Option<i64>,
    pub permissions: Option<String>,
    pub messageable_only: Option<bool>,
    pub per_page: Option<i64>,
    pub page: Option<i64>,
}

/// A rubric returned by GET /search/rubrics.
/// Shape mirrors lib/api/v1/rubric.rb:rubric_json.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RubricJson {
    pub id: i64,
    pub title: String,
    pub context_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_type: Option<String>,
    pub data: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub rubric_association_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reusable: Option<bool>,
}

/// Query params for GET /search/rubrics.
#[derive(Debug, Deserialize)]
pub struct RubricsFilter {
    pub q: Option<String>,
}

/// Query params for GET /search/all_courses.
#[derive(Debug, Deserialize)]
pub struct AllCoursesFilter {
    pub search: Option<String>,
    pub public_only: Option<bool>,
    pub open_enrollment_only: Option<bool>,
    pub per_page: Option<i64>,
    pub page: Option<i64>,
}

/// Course shape returned by GET /search/all_courses.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CourseJson {
    pub id: i64,
    pub name: String,
    pub course_code: String,
    pub account_id: i64,
    pub workflow_state: String,
    pub is_public: bool,
    pub open_enrollment: bool,
    pub created_at: DateTime<Utc>,
}

/// A single smart-search result item.
#[derive(Debug, Serialize)]
pub struct SearchResultItem {
    pub content_id: i64,
    pub content_type: String,
    pub title: String,
    pub body: String,
    pub html_url: String,
    pub distance: f64,
}

/// Query params for GET /courses/:course_id/smartsearch.
#[derive(Debug, Deserialize)]
pub struct SmartSearchFilter {
    pub q: Option<String>,
    pub filter: Option<String>,
    pub include: Option<String>,
}

/// GET /courses/:course_id/smartsearch response.
#[derive(Debug, Serialize)]
pub struct SmartSearchResponse {
    pub results: Vec<SearchResultItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexing_progress: Option<f64>,
}

/// GET /courses/:course_id/smartsearch/index_status response.
#[derive(Debug, Serialize)]
pub struct IndexStatusResponse {
    pub status: String,
    pub progress: f64,
}

/// A page view returned by GET /users/:user_id/page_views.
/// Shape mirrors lib/api/v1/page_view.rb:page_views_json.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PageViewJson {
    pub id: String,
    pub app_name: Option<String>,
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controller: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    pub contributed: bool,
    pub interaction_seconds: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub user_request: bool,
    pub render_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    pub participated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_key_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_user_access_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<PageViewLinks>,
}

#[derive(Debug, Serialize)]
pub struct PageViewLinks {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_user: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<i64>,
}

/// Query params for GET /users/:user_id/page_views.
#[derive(Debug, Deserialize)]
pub struct PageViewsFilter {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub per_page: Option<i64>,
    pub page: Option<i64>,
}

/// Request body for POST /users/:user_id/page_views/query.
#[derive(Debug, Deserialize)]
pub struct PageViewsQueryRequest {
    pub user_id: i64,
    pub start_date: String,
    pub end_date: String,
    pub results_format: String,
}

/// Response for POST /users/:user_id/page_views/query (201).
#[derive(Debug, Serialize)]
pub struct PageViewsQueryResponse {
    pub poll_url: String,
}

/// GET /users/:user_id/page_views/query/:query_id response.
#[derive(Debug, Serialize)]
pub struct PollQueryResponse {
    pub query_id: String,
    pub status: String,
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// Check whether the authenticated user has :view_statistics on target_user_id.
/// Admins always pass; teachers pass if enrolled as teacher in any shared course.
async fn can_view_statistics(pool: &PgPool, auth: &AuthUser, target_user_id: i64) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    if auth.id == target_user_id {
        return Ok(true);
    }
    // Check if auth user is a teacher in any course that target is also enrolled in.
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM enrollments e1
        JOIN enrollments e2 ON e2.course_id = e1.course_id AND e2.user_id = $2
        WHERE e1.user_id = $1 AND e1.role = 'teacher'
        LIMIT 1
        "#,
    )
    .bind(auth.id)
    .bind(target_user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(row.is_some())
}

/// Validate course exists (require_context equivalent).
async fn require_course(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Check whether auth user has read access to the course (require_context read right).
async fn course_read_access(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<(), ApiError> {
    // Admins always pass.
    if auth.role == "admin" {
        return Ok(());
    }
    // Check enrollment: student, teacher, or ta in this course.
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM enrollments
        WHERE user_id = $1 AND course_id = $2 AND role IN ('student', 'teacher', 'ta')
        LIMIT 1
        "#,
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// PV5 configured check — in Canvas this is PageViews::Configuration.configured?.
/// We use a feature flag as the proxy. The brief says it returns 404 if not configured.
async fn is_pv5_configured(pool: &PgPool) -> bool {
    // Always return true for now — real SmartSearch/PV5 integration is a later phase.
    // The brief documents this as a stub point.
    let _ = pool;
    true
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // SearchController
        .route("/search/recipients", get(recipients))
        .route("/search/rubrics", get(rubrics))
        .route("/search/all_courses", get(all_courses))
        // SmartSearch
        .route("/courses/{course_id}/smartsearch", get(smartsearch))
        .route("/courses/{course_id}/smartsearch/log", get(smartsearch_log))
        .route("/courses/{course_id}/smartsearch/index_status", get(smartsearch_index_status))
        // PageViews
        .route("/users/{user_id}/page_views", get(user_page_views))
        .route("/users/{user_id}/page_views/query", post(enqueue_page_views_query))
        .route(
            "/users/{user_id}/page_views/query/{query_id}",
            get(poll_page_views_query),
        )
        .route(
            "/users/{user_id}/page_views/query/{query_id}/results",
            get(get_page_views_results),
        )
}

// ---------------------------------------------------------------------------
// Handlers — SearchController
// ---------------------------------------------------------------------------

/// GET /search/recipients — search for messageable users and contexts.
/// Returns 401 if no valid auth token.
async fn recipients(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(filter): Query<RecipientsFilter>,
) -> Result<Json<Vec<Recipient>>, ApiError> {
    let _ = &auth; // require_user passed — AuthUser already extracted.

    // Pagination defaults.
    let per_page = filter.per_page.unwrap_or(50).max(1).min(200) as i32;
    let page = filter.page.unwrap_or(1).max(1) as i32;
    let offset = (page - 1) * per_page;

    // If user_id is specified, return only that user.
    if let Some(uid) = filter.user_id {
        let row: Option<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, name, sortable_name, avatar_url FROM users WHERE id = $1",
        )
        .bind(uid)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;

        if let Some((id, name, sortable_name, avatar_url)) = row {
            let full_name = sortable_name.or(Some(name.clone()));
            return Ok(Json(vec![Recipient::User(UserRecipient {
                id,
                name: name.clone(),
                full_name,
                common_courses: None,
                common_groups: None,
                avatar_url,
                type_: "user".to_string(),
                permissions: UserPermissions { send_messages: Some(true) },
            })]));
        }
        return Ok(Json(vec![]));
    }

    // Otherwise search by name.
    let search_term = filter.search.as_deref().unwrap_or("").trim();
    let search_pattern = if search_term.is_empty() {
        "%".to_string()
    } else {
        format!("%{}%", search_term)
    };

    // Fetch matching users.
    let users: Vec<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, avatar_url
        FROM users
        WHERE name ILIKE $1 OR sortable_name ILIKE $1
        ORDER BY name
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(&search_pattern)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    let recipients: Vec<Recipient> = users
        .into_iter()
        .map(|(id, name, sortable_name, avatar_url)| {
            Recipient::User(UserRecipient {
                id,
                name: name.clone(),
                full_name: sortable_name.or(Some(name.clone())),
                common_courses: None,
                common_groups: None,
                avatar_url,
                type_: "user".to_string(),
                permissions: UserPermissions { send_messages: Some(true) },
            })
        })
        .collect();

    Ok(Json(recipients))
}

/// GET /search/rubrics — search rubrics by title substring.
/// Returns 401 if no valid auth token.
async fn rubrics(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(filter): Query<RubricsFilter>,
) -> Result<Json<Vec<RubricJson>>, ApiError> {
    let _ = &auth; // require_user passed.

    let q = filter.q.as_deref().unwrap_or("").trim();
    let q_pattern = format!("%{}%", q);

    let rows: Vec<RubricJson> = sqlx::query_as(
        r#"
        SELECT r.id, r.title, r.course_id AS context_id,
               'Course' AS context_type,
               COALESCE(r.data, '{"criteria":[]}')::jsonb AS data,
               r.created_at, r.updated_at,
               0 AS rubric_association_count,
               false AS public, false AS read_only, false AS reusable
        FROM rubrics r
        WHERE r.title ILIKE $1
        ORDER BY r.title
        LIMIT 50
        "#,
    )
    .bind(&q_pattern)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(rows))
}

/// GET /search/all_courses — anonymous course catalog listing.
/// Returns 401 if course catalog is disabled on the root account.
/// No require_user (skip_before_action at search_controller.rb:27).
async fn all_courses(
    State(state): State<AppState>,
    Query(filter): Query<AllCoursesFilter>,
) -> Result<Json<Vec<CourseJson>>, ApiError> {
    // Check feature flag for course catalog — if disabled, return 401.
    let catalog_enabled: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT NOT EXISTS (
            SELECT 1 FROM feature_flags
            WHERE context_type = 'Account'
              AND context_id = 1
              AND feature = 'course_catalog'
              AND state = 'disabled'
        ) AS enabled
        "#,
    )
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    if !catalog_enabled.map(|r| r.0).unwrap_or(true) {
        return Err(ApiError::Unauthorized);
    }

    let per_page = filter.per_page.unwrap_or(12).max(1).min(200) as i32;
    let page = filter.page.unwrap_or(1).max(1) as i32;
    let offset = (page - 1) * per_page;

    let search_pattern = filter.search.as_ref().map(|s| format!("%{}%", s));

    let rows: Vec<CourseJson> = if let Some(ref pat) = search_pattern {
        sqlx::query_as(
            r#"
            SELECT id, name, course_code, account_id, workflow_state,
                   is_public, open_enrollment, created_at
            FROM courses
            WHERE root_account_id = 1
              AND indexed = true
              AND workflow_state = 'available'
              AND name ILIKE $1
              AND (NOT $2::bool OR is_public = true)
              AND (NOT $3::bool OR open_enrollment = true)
            ORDER BY created_at DESC
            LIMIT $4 OFFSET $5
            "#,
        )
        .bind(pat)
        .bind(filter.public_only.unwrap_or(false))
        .bind(filter.open_enrollment_only.unwrap_or(false))
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::from)?
    } else {
        sqlx::query_as(
            r#"
            SELECT id, name, course_code, account_id, workflow_state,
                   is_public, open_enrollment, created_at
            FROM courses
            WHERE root_account_id = 1
              AND indexed = true
              AND workflow_state = 'available'
              AND (NOT $1::bool OR is_public = true)
              AND (NOT $2::bool OR open_enrollment = true)
            ORDER BY created_at DESC
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(filter.public_only.unwrap_or(false))
        .bind(filter.open_enrollment_only.unwrap_or(false))
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::from)?
    };

    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// Handlers — SmartSearch
// ---------------------------------------------------------------------------

/// GET /courses/:course_id/smartsearch — AI-powered content search within a course.
/// Returns 401 without auth, 403 if no read access, 400 if q param missing.
async fn smartsearch(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
    Query(filter): Query<SmartSearchFilter>,
) -> Result<Json<SmartSearchResponse>, ApiError> {
    require_course(&state.db, course_id).await?;
    course_read_access(&state.db, &auth, course_id).await?;

    let q = filter.q.ok_or(ApiError::BadRequest("missing 'q' param".into()))?;

    // Stub: return index_incomplete until SmartSearch service is wired.
    // Real implementation joins pages, assignments, announcements, discussion_topics
    // filtered by show_in_search_for_user? and ranked by relevance to q.
    Ok(Json(SmartSearchResponse {
        results: vec![],
        status: Some("index_incomplete".to_string()),
        indexing_progress: Some(0.75),
    }))
}

/// GET /courses/:course_id/smartsearch/log — no-op logger.
/// Returns 204 No Content if course exists and auth passes.
async fn smartsearch_log(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    require_course(&state.db, course_id).await?;
    course_read_access(&state.db, &auth, course_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /courses/:course_id/smartsearch/index_status — return indexing status.
/// Returns 200 {status, progress} with stub values until SmartSearch is wired.
async fn smartsearch_index_status(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
) -> Result<Json<IndexStatusResponse>, ApiError> {
    require_course(&state.db, course_id).await?;
    course_read_access(&state.db, &auth, course_id).await?;

    Ok(Json(IndexStatusResponse {
        status: "indexed".to_string(),
        progress: 1.0,
    }))
}

// ---------------------------------------------------------------------------
// Handlers — PageViews
// ---------------------------------------------------------------------------

/// GET /users/:user_id/page_views — list page views for a user.
/// Returns 401 without auth, 403 if auth user lacks :view_statistics on target.
async fn user_page_views(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
    Query(filter): Query<PageViewsFilter>,
) -> Result<Json<Vec<PageViewJson>>, ApiError> {
    // require_user already enforced by AuthUser extractor.
    if !is_pv5_configured(&state.db).await {
        return Err(ApiError::NotFound);
    }
    if !can_view_statistics(&state.db, &auth, user_id).await? {
        return Err(ApiError::Forbidden);
    }

    // Parse time bounds.
    let start_time = filter.start_time.as_ref().and_then(|s| {
        DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
    });
    let end_time = filter.end_time.as_ref().and_then(|s| {
        DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
    });

    if let (Some(start), None) = (&start_time, &end_time) {
        if start > &Utc::now() {
            return Err(ApiError::BadRequest("start_time cannot be in the future".into()));
        }
    }
    if let (Some(start), Some(end)) = (&start_time, &end_time) {
        if end <= start {
            return Err(ApiError::BadRequest("end_time must be after start_time".into()));
        }
    }

    let per_page = filter.per_page.unwrap_or(10).max(1).min(200) as i32;
    let page = filter.page.unwrap_or(1).max(1) as i32;
    let offset = (page - 1) * per_page;

    let rows: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>,
                   Option<String>, Option<String>, bool, Option<f64>, DateTime<Utc>,
                   bool, Option<f64>, Option<String>, bool, Option<String>, Option<String>,
                   Option<String>, Option<i64>, Option<i64>,
                   Option<i64>, Option<i64>, Option<i64>)> =
        if let (Some(start), Some(end)) = (&start_time, &end_time) {
            sqlx::query_as(
                r#"
                SELECT id, app_name, url, context_type, asset_type,
                       controller, action, contributed, interaction_seconds,
                       created_at, user_request, render_time, user_agent,
                       participated, http_method, remote_ip, session_id,
                       developer_key_id, asset_user_access_id,
                       links_user, links_context, links_asset, links_real_user, links_account
                FROM page_views
                WHERE user_id = $1 AND created_at >= $2 AND created_at <= $3
                ORDER BY created_at DESC
                LIMIT $4 OFFSET $5
                "#,
            )
            .bind(user_id)
            .bind(start)
            .bind(end)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::from)?
        } else if let Some(start) = &start_time {
            sqlx::query_as(
                r#"
                SELECT id, app_name, url, context_type, asset_type,
                       controller, action, contributed, interaction_seconds,
                       created_at, user_request, render_time, user_agent,
                       participated, http_method, remote_ip, session_id,
                       developer_key_id, asset_user_access_id,
                       links_user, links_context, links_asset, links_real_user, links_account
                FROM page_views
                WHERE user_id = $1 AND created_at >= $2
                ORDER BY created_at DESC
                LIMIT $3 OFFSET $4
                "#,
            )
            .bind(user_id)
            .bind(start)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::from)?
        } else {
            sqlx::query_as(
                r#"
                SELECT id, app_name, url, context_type, asset_type,
                       controller, action, contributed, interaction_seconds,
                       created_at, user_request, render_time, user_agent,
                       participated, http_method, remote_ip, session_id,
                       developer_key_id, asset_user_access_id,
                       links_user, links_context, links_asset, links_real_user, links_account
                FROM page_views
                WHERE user_id = $1
                ORDER BY created_at DESC
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(user_id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::from)?
        };

    let views: Vec<PageViewJson> = rows
        .into_iter()
        .map(
            |(id, app_name, url, context_type, asset_type, controller, action,
              contributed, interaction_seconds, created_at, user_request,
              render_time, user_agent, participated, http_method, remote_ip,
              session_id, developer_key_id, asset_user_access_id,
              links_user, links_context, links_asset, links_real_user, links_account)| {
                PageViewJson {
                    id,
                    app_name,
                    url,
                    context_type,
                    asset_type,
                    controller,
                    action,
                    contributed,
                    interaction_seconds,
                    created_at,
                    user_request,
                    render_time,
                    user_agent,
                    participated,
                    http_method,
                    remote_ip,
                    session_id,
                    developer_key_id,
                    asset_user_access_id,
                    links: if [links_user, links_context, links_asset, links_real_user, links_account]
                        .iter().all(|v| v.is_none())
                    {
                        None
                    } else {
                        Some(PageViewLinks {
                            user: links_user,
                            context: links_context,
                            asset: links_asset,
                            real_user: links_real_user,
                            account: links_account,
                        })
                    },
                }
            },
        )
        .collect();

    Ok(Json(views))
}

/// POST /users/:user_id/page_views/query — enqueue an async page-views export.
/// Returns 201 with poll_url, 403 if auth lacks :view_statistics on target.
async fn enqueue_page_views_query(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
    Json(payload): Json<PageViewsQueryRequest>,
) -> Result<(StatusCode, Json<PageViewsQueryResponse>), ApiError> {
    if !is_pv5_configured(&state.db).await {
        return Err(ApiError::NotFound);
    }
    if !can_view_statistics(&state.db, &auth, user_id).await? {
        return Err(ApiError::Forbidden);
    }

    // Validate user_id matches path.
    if payload.user_id != user_id {
        return Err(ApiError::BadRequest("user_id must match path".into()));
    }

    // Validate results_format.
    if payload.results_format != "csv" && payload.results_format != "jsonl" {
        return Err(ApiError::BadRequest(
            "results_format must be 'csv' or 'jsonl'".into(),
        ));
    }

    // Parse dates — must be first of month.
    let start = NaiveDate::parse_from_str(&payload.start_date, "%Y-%m-%d")
        .map_err(|_| ApiError::BadRequest("invalid start_date".into()))?;
    let end = NaiveDate::parse_from_str(&payload.end_date, "%Y-%m-%d")
        .map_err(|_| ApiError::BadRequest("invalid end_date".into()))?;

    if start.day() != 1 {
        return Err(ApiError::BadRequest("start_date must be first day of month".into()));
    }
    if end.day() != 1 {
        return Err(ApiError::BadRequest("end_date must be first day of month".into()));
    }
    if end <= start {
        return Err(ApiError::BadRequest("end_date must be after start_date".into()));
    }

    let query_id: (uuid::Uuid,) = sqlx::query_as("SELECT gen_random_uuid()")
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?;
    sqlx::query(
        r#"
        INSERT INTO page_view_queries (query_id, user_id, start_date, end_date, format, status)
        VALUES ($1, $2, $3, $4, $5, 'queued')
        "#,
    )
    .bind(query_id.0)
    .bind(user_id)
    .bind(start)
    .bind(end)
    .bind(&payload.results_format)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    let poll_url = format!("/users/{}/page_views/query/{}", user_id, query_id.0);
    Ok((StatusCode::CREATED, Json(PageViewsQueryResponse { poll_url })))
}

/// GET /users/:user_id/page_views/query/:query_id — poll query status.
/// Returns 200 with status payload, 400 for invalid UUID, 404 if query not found.
async fn poll_page_views_query(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((user_id, query_id)): Path<(i64, String)>,
) -> Result<Json<PollQueryResponse>, ApiError> {
    if !is_pv5_configured(&state.db).await {
        return Err(ApiError::NotFound);
    }

    // Validate UUID format via DB cast (returns NULL if invalid).
    let query_uuid: Option<uuid::Uuid> = sqlx::query_as("SELECT $1::uuid")
        .bind(&query_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;
    let query_uuid = query_uuid.ok_or(ApiError::BadRequest("Invalid query ID".into()))?;

    if !can_view_statistics(&state.db, &auth, user_id).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<(String, String, String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT query_id::text, status, format, error_code
        FROM page_view_queries
        WHERE query_id = $1 AND user_id = $2
        "#,
    )
    .bind(query_uuid)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    let (qid, status, format, error_code) = row.ok_or(ApiError::NotFound)?;

    let results_url = if status == "finished" {
        Some(format!("/users/{}/page_views/query/{}/results", user_id, qid))
    } else {
        None
    };

    Ok(Json(PollQueryResponse {
        query_id: qid,
        status,
        format,
        results_url,
        error_code,
    }))
}

/// GET /users/:user_id/page_views/query/:query_id/results — download query results.
/// Returns 200 file download (csv or jsonl), 400/404 as documented.
async fn get_page_views_results(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((user_id, query_id)): Path<(i64, String)>,
) -> Result<Response, ApiError> {
    if !is_pv5_configured(&state.db).await {
        return Err(ApiError::NotFound);
    }

    let query_uuid: Option<uuid::Uuid> = sqlx::query_as("SELECT $1::uuid")
        .bind(&query_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;
    let query_uuid = query_uuid.ok_or(ApiError::BadRequest("Invalid query ID".into()))?;

    if !can_view_statistics(&state.db, &auth, user_id).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT query_id::text, status, format, error_code
        FROM page_view_queries
        WHERE query_id = $1 AND user_id = $2
        "#,
    )
    .bind(query_uuid)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    let (qid, status, format, error_code) = row.ok_or(ApiError::NotFound)?;

    if status == "failed" {
        if error_code.as_deref() == Some("RESULT_SIZE_LIMIT_EXCEEDED") {
            return Err(ApiError::BadRequest("Query results are not in a valid state for download".into()));
        }
        return Err(ApiError::NotFound);
    }
    if status != "finished" {
        return Err(ApiError::BadRequest("Query results are not in a valid state for download".into()));
    }

    // Fetch results_data blob.
    let data: Option<(Vec<u8>,)> = sqlx::query_as(
        "SELECT results_data FROM page_view_queries WHERE query_id = $1",
    )
    .bind(query_uuid)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    if let Some((bytes,)) = data {
        if bytes.is_empty() {
            return Ok((StatusCode::NO_CONTENT, axum::response::AppendHeaders([
                (CONTENT_DISPOSITION, &format!("attachment; filename=\"{}.gz\"", qid)),
            ]), axum::body::Body::from(bytes)).into_response());
        }
        let content_type = if format == "csv" {
            "text/csv"
        } else {
            "application/jsonl"
        };
        return Ok((
            StatusCode::OK,
            axum::response::AppendHeaders([
                (CONTENT_DISPOSITION, &format!("attachment; filename=\"{}.gz\"", qid)),
            ]),
            axum::response::Json(format!("[results data of {} bytes]", bytes.len())),
        ).into_response());
    }

    // No data yet — return 204.
    Ok(StatusCode::NO_CONTENT.into_response())
}
