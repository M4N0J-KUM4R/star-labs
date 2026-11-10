//! star-pages domain crate — wiki pages for courses.
//!
//! Exposes `pub fn router() -> Router<AppState>` with endpoints:
//! - GET/POST /courses/{courseId}/pages
//! - GET/PUT/DELETE /pages/{id}   (id may be numeric id or url slug)
//! - GET/PUT/DELETE /courses/{courseId}/pages/{id}
//!   (Canvas v1 aliases for the flat routes; the page must belong to
//!   {courseId}, otherwise 404)
//! - GET /courses/{courseId}/pages/{url}/revisions             (page history)
//! - GET /courses/{courseId}/pages/{url}/revisions/latest      (current version)
//! - GET /courses/{courseId}/pages/{url}/revisions/{revisionId} (one version)
//!
//! Canvas parity notes (wiki_page.rb, wiki_pages_api_controller.rb):
//! - pages carry `url` (slug derived from title), `workflow_state`
//!   ('published'/'unpublished'/'deleted') and `editing_roles`.
//! - the list endpoint omits `body` (Canvas pages#index returns summaries).
//! - DELETE soft-deletes: workflow_state = 'deleted'.
//! - revisions mirror simply_versioned: every page save appends a version with
//!   a per-page 1-based number, newest first; `show_revision` needs
//!   `:read_revisions` (teacher/admin) while `/revisions/latest` only needs
//!   read access (enrolled student/admin). `summary=true` drops content.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use star_common::{ApiError, AppState};

use star_auth::AuthUser;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Page summary returned in list endpoints (no body — Canvas pages#index shape).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PageSummary {
    pub id: i64,
    pub title: String,
    pub url: Option<String>,
    pub workflow_state: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Full page detail returned by single-page GET (includes body).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PageDetail {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    pub body: Option<String>,
    pub url: Option<String>,
    pub workflow_state: String,
    pub editing_roles: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Request body for POST /courses/{courseId}/pages.
#[derive(Debug, Deserialize)]
pub struct CreatePage {
    pub title: String,
    pub body: Option<String>,
    /// Canvas accepts workflow_state on create; defaults to 'published'.
    pub workflow_state: Option<String>,
    pub editing_roles: Option<String>,
}

/// Request body for PUT /pages/{id}.
#[derive(Debug, Deserialize)]
pub struct UpdatePage {
    pub title: Option<String>,
    pub body: Option<String>,
    pub workflow_state: Option<String>,
    pub editing_roles: Option<String>,
}

/// Pagination params.
#[derive(Debug, Deserialize)]
pub struct Pagination {
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_per_page")]
    pub per_page: i64,
}

/// Query params for a single revision (`?summary=true` drops the content).
#[derive(Debug, Deserialize)]
pub struct RevisionQuery {
    /// Canvas `summary` boolean: true => exclude url/title/body.
    pub summary: Option<String>,
}

/// A stored revision row (Canvas SimplyVersioned::Version + editor join).
#[derive(Debug, sqlx::FromRow)]
pub struct PageRevisionRow {
    pub revision_id: i32,
    pub title: String,
    pub body: Option<String>,
    pub url: Option<String>,
    pub user_id: Option<i64>,
    pub user_name: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Canvas `user_display_json` subset returned as a revision's `edited_by`.
#[derive(Debug, Serialize)]
pub struct UserDisplay {
    pub id: i64,
    pub anonymous_id: String,
    pub display_name: String,
    pub avatar_image_url: Option<String>,
    pub html_url: String,
    pub pronouns: Option<String>,
}

/// Canvas `wiki_page_revisions_json` item: content is omitted in lists.
#[derive(Debug, Serialize)]
pub struct RevisionSummary {
    pub revision_id: i32,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub latest: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_by: Option<UserDisplay>,
}

/// Canvas `wiki_page_revision_json`: content keys omitted for `summary=true`.
#[derive(Debug, Serialize)]
pub struct RevisionDetail {
    pub revision_id: i32,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub latest: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_by: Option<UserDisplay>,
}

fn default_page() -> i64 {
    1
}

fn default_per_page() -> i64 {
    50
}

// ---------------------------------------------------------------------------
// Slug generation (wiki_page.rb parity)
// ---------------------------------------------------------------------------

/// Derive a Canvas-style url slug from a page title.
///
/// Mirrors the simple strategy of wiki_page.rb's title-based url generation:
/// lowercase, spaces become dashes, and runs of non-alphanumeric characters
/// are stripped/collapsed so the slug is URL-safe. Leading/trailing and
/// repeated dashes collapse to a single dash.
///
/// ```text
/// "Hello World"        -> "hello-world"
/// "Foo  Bar!! Baz"     -> "foo-bar-baz"
/// "  Trim -- Me  "     -> "trim-me"
/// ```
pub fn generate_url_slug(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_dash = true; // suppress leading dashes

    for ch in title.chars() {
        if ch.is_whitespace() || ch == '-' || ch == '_' {
            // Any whitespace/separator run becomes a single dash.
            if !last_was_dash {
                slug.push('-');
                last_was_dash = true;
            }
        } else if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                slug.push(lower);
            }
            last_was_dash = false;
        }
        // everything else (punctuation, symbols) is stripped
    }

    // strip trailing dash
    if slug.ends_with('-') {
        slug.pop();
    }
    slug
}

// ---------------------------------------------------------------------------
// Revision helpers (Canvas simply_versioned + user_display_json parity)
// ---------------------------------------------------------------------------

/// Canvas `id.to_s(36)` used for `edited_by.anonymous_id`.
pub fn to_base36(value: i64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut remaining = value.unsigned_abs();
    let mut digits = Vec::new();
    while remaining > 0 {
        digits.push(DIGITS[(remaining % 36) as usize]);
        remaining /= 36;
    }
    if value < 0 {
        digits.push(b'-');
    }
    digits.reverse();
    String::from_utf8(digits).expect("base36 alphabet is ASCII")
}

/// Canvas `Canvas::Plugin.value_to_boolean`: these strings are truthy.
pub fn parse_canvas_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "yes" | "y" | "true" | "t" | "on" | "1"
    )
}

/// Canvas `include_content = !value_to_boolean(params[:summary])`.
fn include_content(summary: Option<&str>) -> bool {
    !summary.map(parse_canvas_bool).unwrap_or(false)
}

impl PageRevisionRow {
    /// Canvas `user_display_json` for the revision's editor.
    fn edited_by(&self) -> Option<UserDisplay> {
        let id = self.user_id?;
        Some(UserDisplay {
            id,
            anonymous_id: to_base36(id),
            display_name: self.user_name.clone().unwrap_or_default(),
            avatar_image_url: None,
            html_url: format!("/users/{id}"),
            pronouns: None,
        })
    }

    /// History item: `revision_id`, `updated_at`, `latest`, `edited_by`.
    fn summary(&self, latest: i32) -> RevisionSummary {
        RevisionSummary {
            revision_id: self.revision_id,
            updated_at: self.updated_at,
            latest: self.revision_id == latest,
            edited_by: self.edited_by(),
        }
    }

    /// Detail item; `include_content == false` mirrors `summary=true`.
    fn detail(&self, latest: i32, include_content: bool) -> RevisionDetail {
        RevisionDetail {
            revision_id: self.revision_id,
            updated_at: self.updated_at,
            latest: self.revision_id == latest,
            url: include_content.then(|| self.url.clone()).flatten(),
            title: include_content.then(|| self.title.clone()),
            body: include_content.then(|| self.body.clone()).flatten(),
            edited_by: self.edited_by(),
        }
    }
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Check if user is enrolled in course (any role) or is admin.
async fn user_enrolled_or_admin(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1")
            .bind(user_id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

/// Check if user is the teacher of a course (or admin).
async fn user_teacher_of_course(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Page lookup + revision storage
// ---------------------------------------------------------------------------

/// Resolve a page reference (numeric id or url slug) to `(id, course_id)`,
/// excluding soft-deleted pages.
async fn resolve_page(db: &PgPool, page_ref: &str) -> Result<Option<(i64, i64)>, ApiError> {
    let row: Option<(i64, i64)> = if let Ok(numeric_id) = page_ref.parse::<i64>() {
        sqlx::query_as(
            "SELECT id, course_id FROM pages WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(numeric_id)
        .fetch_optional(db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, course_id FROM pages WHERE url = $1 AND workflow_state <> 'deleted'",
        )
        .bind(page_ref)
        .fetch_optional(db)
        .await?
    };
    Ok(row)
}

/// Load the page addressed by `page_ref` (numeric id or url slug) scoped to
/// `course_id`. Cross-course and soft-deleted pages are 404 (Canvas
/// `url_or_id` lookup under the addressed context).
async fn load_page_in_course(
    db: &PgPool,
    course_id: i64,
    page_ref: &str,
) -> Result<PageDetail, ApiError> {
    let page: Option<PageDetail> = if let Ok(numeric_id) = page_ref.parse::<i64>() {
        sqlx::query_as(
            r#"
            SELECT id, course_id, title, body, url, workflow_state, editing_roles,
                   created_at, updated_at
            FROM pages
            WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
            "#,
        )
        .bind(numeric_id)
        .bind(course_id)
        .fetch_optional(db)
        .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT id, course_id, title, body, url, workflow_state, editing_roles,
                   created_at, updated_at
            FROM pages
            WHERE url = $1 AND course_id = $2 AND workflow_state <> 'deleted'
            "#,
        )
        .bind(page_ref)
        .bind(course_id)
        .fetch_optional(db)
        .await?
    };
    page.ok_or(ApiError::NotFound)
}

/// Append a revision capturing the page's current state (simply_versioned:
/// `number = MAX(number) + 1` per page).
async fn insert_revision(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    page_id: i64,
    user_id: i64,
    title: &str,
    body: Option<&str>,
    url: Option<&str>,
) -> Result<i32, ApiError> {
    let revision_id: i32 = sqlx::query_scalar(
        r#"
        INSERT INTO page_revisions (page_id, revision_id, title, body, url, user_id)
        SELECT $1, COALESCE(MAX(revision_id), 0) + 1, $2, $3, $4, $5
        FROM page_revisions
        WHERE page_id = $1
        RETURNING revision_id
        "#,
    )
    .bind(page_id)
    .bind(title)
    .bind(body)
    .bind(url)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(revision_id)
}

/// Highest stored revision number for a page (0 when history is empty).
async fn latest_revision_number(db: &PgPool, page_id: i64) -> Result<i32, ApiError> {
    let latest: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(revision_id), 0) FROM page_revisions WHERE page_id = $1",
    )
    .bind(page_id)
    .fetch_one(db)
    .await?;
    Ok(latest)
}

/// Fetch one revision by its per-page number.
async fn fetch_revision(
    db: &PgPool,
    page_id: i64,
    revision_id: i32,
) -> Result<Option<PageRevisionRow>, ApiError> {
    let row: Option<PageRevisionRow> = sqlx::query_as(
        r#"
        SELECT r.revision_id, r.title, r.body, r.url, r.user_id,
               u.name AS user_name, r.updated_at
        FROM page_revisions r
        LEFT JOIN users u ON u.id = r.user_id
        WHERE r.page_id = $1 AND r.revision_id = $2
        "#,
    )
    .bind(page_id)
    .bind(revision_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// Paginated history, newest first (Canvas `versions` is `number DESC`).
async fn fetch_revisions(
    db: &PgPool,
    page_id: i64,
    per_page: i64,
    offset: i64,
) -> Result<Vec<PageRevisionRow>, ApiError> {
    let rows: Vec<PageRevisionRow> = sqlx::query_as(
        r#"
        SELECT r.revision_id, r.title, r.body, r.url, r.user_id,
               u.name AS user_name, r.updated_at
        FROM page_revisions r
        LEFT JOIN users u ON u.id = r.user_id
        WHERE r.page_id = $1
        ORDER BY r.revision_id DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(page_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/pages — paginated list of page summaries
/// (Canvas pages#index: id, title, url, workflow_state, timestamps; no body).
async fn list_pages(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(pagination): Query<Pagination>,
    auth: AuthUser,
) -> Result<Json<Vec<PageSummary>>, ApiError> {
    // Check view access
    if !user_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Clamp pagination: page >= 1, per_page 1..100
    let page = pagination.page.max(1);
    let per_page = pagination.per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;

    let pages: Vec<PageSummary> = sqlx::query_as(
        r#"
        SELECT id, title, url, workflow_state, created_at, updated_at
        FROM pages
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(course_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(pages))
}

/// POST /courses/{courseId}/pages — create a new page.
///
/// Parity with wiki_page.rb: the url slug is auto-generated from the title;
/// workflow_state defaults to 'published', editing_roles to 'teachers'.
async fn create_page(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreatePage>,
) -> Result<(StatusCode, Json<PageDetail>), ApiError> {
    // Check teacher access
    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Validate title
    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }

    // Canvas wiki_page.rb: url slug derived from title, editable workflow_state.
    let url = generate_url_slug(&payload.title);
    let workflow_state = payload
        .workflow_state
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("published")
        .to_string();
    let editing_roles = payload
        .editing_roles
        .clone()
        .unwrap_or_else(|| "teachers".to_string());

    let mut tx = state.db.begin().await?;

    let page: PageDetail = sqlx::query_as(
        r#"
        INSERT INTO pages (course_id, title, body, url, workflow_state, editing_roles)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, course_id, title, body, url, workflow_state, editing_roles,
                  created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .bind(&payload.body)
    .bind(&url)
    .bind(&workflow_state)
    .bind(&editing_roles)
    .fetch_one(&mut *tx)
    .await?;

    // simply_versioned: the creating save is revision 1.
    insert_revision(
        &mut tx,
        page.id,
        auth.id,
        &page.title,
        page.body.as_deref(),
        page.url.as_deref(),
    )
    .await?;

    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(page)))
}

/// GET /pages/{id} — get a single page with body.
/// `{id}` may be the numeric page id or the url slug (Canvas parity).
async fn get_page(
    State(state): State<AppState>,
    Path(page_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<PageDetail>, ApiError> {
    // Resolve by numeric id or url slug, excluding soft-deleted pages.
    let (page_id_num, course_id) = resolve_page(&state.db, &page_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Check view access
    if !user_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let page: PageDetail = sqlx::query_as(
        r#"
        SELECT id, course_id, title, body, url, workflow_state, editing_roles,
               created_at, updated_at
        FROM pages WHERE id = $1
        "#,
    )
    .bind(page_id_num)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(page))
}

/// PUT /pages/{id} — update a page (numeric id or url slug).
async fn update_page(
    State(state): State<AppState>,
    Path(page_id): Path<String>,
    auth: AuthUser,
    Json(payload): Json<UpdatePage>,
) -> Result<Json<PageDetail>, ApiError> {
    // Get page to find course_id (soft-deleted pages are not editable)
    let (page_id_num, course_id) = resolve_page(&state.db, &page_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Check modify access (teacher/admin)
    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // At least one field must be provided
    if payload.title.is_none()
        && payload.body.is_none()
        && payload.workflow_state.is_none()
        && payload.editing_roles.is_none()
    {
        return Err(ApiError::Validation(
            "at least title or body is required".into(),
        ));
    }

    // If the title changed, Canvas regenerates the url slug (wiki_page.rb).
    let new_url = payload.title.as_ref().map(|t| generate_url_slug(t));

    let mut tx = state.db.begin().await?;

    let page: PageDetail = sqlx::query_as(
        r#"
        UPDATE pages
        SET title = COALESCE($1, title),
            body = COALESCE($2, body),
            url = COALESCE($3, url),
            workflow_state = COALESCE($4, workflow_state),
            editing_roles = COALESCE($5, editing_roles),
            updated_at = now()
        WHERE id = $6
        RETURNING id, course_id, title, body, url, workflow_state, editing_roles,
                  created_at, updated_at
        "#,
    )
    .bind(&payload.title)
    .bind(&payload.body)
    .bind(&new_url)
    .bind(&payload.workflow_state)
    .bind(&payload.editing_roles)
    .bind(page_id_num)
    .fetch_one(&mut *tx)
    .await?;

    // simply_versioned: every update appends a revision of the saved state.
    insert_revision(
        &mut tx,
        page.id,
        auth.id,
        &page.title,
        page.body.as_deref(),
        page.url.as_deref(),
    )
    .await?;

    tx.commit().await?;

    Ok(Json(page))
}

/// DELETE /pages/{id} — Canvas parity soft delete: workflow_state = 'deleted'.
async fn delete_page(
    State(state): State<AppState>,
    Path(page_id): Path<String>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Get page to find course_id
    let (page_id_num, course_id) = resolve_page(&state.db, &page_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Check modify access (teacher/admin)
    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        "UPDATE pages SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(page_id_num)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Canvas v1 nested aliases
// ---------------------------------------------------------------------------

/// 404 unless `page_ref` (numeric id or url slug) resolves to a non-deleted
/// page that belongs to `course_id`.
async fn ensure_page_in_course(
    db: &PgPool,
    course_id: i64,
    page_ref: &str,
) -> Result<(), ApiError> {
    load_page_in_course(db, course_id, page_ref)
        .await
        .map(|_| ())
}

/// GET /courses/{courseId}/pages/{id} — Canvas v1 alias for GET /pages/{id}.
async fn get_course_page(
    State(state): State<AppState>,
    Path((course_id, page_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<PageDetail>, ApiError> {
    ensure_page_in_course(&state.db, course_id, &page_id).await?;
    get_page(State(state), Path(page_id), auth).await
}

/// PUT /courses/{courseId}/pages/{id} — Canvas v1 alias for PUT /pages/{id}.
async fn update_course_page(
    State(state): State<AppState>,
    Path((course_id, page_id)): Path<(i64, String)>,
    auth: AuthUser,
    Json(payload): Json<UpdatePage>,
) -> Result<Json<PageDetail>, ApiError> {
    ensure_page_in_course(&state.db, course_id, &page_id).await?;
    update_page(State(state), Path(page_id), auth, Json(payload)).await
}

/// DELETE /courses/{courseId}/pages/{id} — Canvas v1 alias for DELETE /pages/{id}.
async fn delete_course_page(
    State(state): State<AppState>,
    Path((course_id, page_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    ensure_page_in_course(&state.db, course_id, &page_id).await?;
    delete_page(State(state), Path(page_id), auth).await
}

// ---------------------------------------------------------------------------
// Page revisions (Canvas wiki_pages_api_controller#revisions / #show_revision)
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/pages/{url}/revisions — paginated history, newest
/// first. Canvas requires `:read_revisions` (i.e. update rights).
async fn list_revisions(
    State(state): State<AppState>,
    Path((course_id, page_ref)): Path<(i64, String)>,
    Query(pagination): Query<Pagination>,
    auth: AuthUser,
) -> Result<Json<Vec<RevisionSummary>>, ApiError> {
    let page = load_page_in_course(&state.db, course_id, &page_ref).await?;

    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let page_number = pagination.page.max(1);
    let per_page = pagination.per_page.clamp(1, 100);
    let offset = (page_number - 1) * per_page;

    let latest = latest_revision_number(&state.db, page.id).await?;
    let revisions = fetch_revisions(&state.db, page.id, per_page, offset).await?;
    Ok(Json(
        revisions.iter().map(|row| row.summary(latest)).collect(),
    ))
}

/// GET /courses/{courseId}/pages/{url}/revisions/latest — the current version.
/// Canvas only requires `:read` here, so enrolled students may call it.
async fn show_latest_revision(
    State(state): State<AppState>,
    Path((course_id, page_ref)): Path<(i64, String)>,
    Query(query): Query<RevisionQuery>,
    auth: AuthUser,
) -> Result<Json<RevisionDetail>, ApiError> {
    let page = load_page_in_course(&state.db, course_id, &page_ref).await?;

    if !user_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let with_content = include_content(query.summary.as_deref());
    let latest = latest_revision_number(&state.db, page.id).await?;

    // Pages that predate revision tracking have no stored history: serve their
    // current state as the (virtual) latest revision instead of erroring.
    if latest == 0 {
        let (url, title, body) = if with_content {
            (
                page.url.clone(),
                Some(page.title.clone()),
                page.body.clone(),
            )
        } else {
            (None, None, None)
        };
        return Ok(Json(RevisionDetail {
            revision_id: 0,
            updated_at: page.updated_at,
            latest: true,
            url,
            title,
            body,
            edited_by: None,
        }));
    }

    let revision = fetch_revision(&state.db, page.id, latest)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(revision.detail(latest, with_content)))
}

/// GET /courses/{courseId}/pages/{url}/revisions/{revisionId} — one version.
/// Historic versions require `:read_revisions` (teacher/admin).
async fn show_revision(
    State(state): State<AppState>,
    Path((course_id, page_ref, revision_ref)): Path<(i64, String, String)>,
    Query(query): Query<RevisionQuery>,
    auth: AuthUser,
) -> Result<Json<RevisionDetail>, ApiError> {
    let page = load_page_in_course(&state.db, course_id, &page_ref).await?;

    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas does `params[:revision_id].to_i` then `first!` -> 404.
    let revision_id = revision_ref
        .parse::<i32>()
        .map_err(|_| ApiError::NotFound)?;
    let with_content = include_content(query.summary.as_deref());
    let latest = latest_revision_number(&state.db, page.id).await?;
    let revision = fetch_revision(&state.db, page.id, revision_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(revision.detail(latest, with_content)))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the pages router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/pages",
            get(list_pages).post(create_page),
        )
        .route(
            "/pages/{id}",
            get(get_page).put(update_page).delete(delete_page),
        )
        .route(
            "/courses/{courseId}/pages/{id}",
            get(get_course_page)
                .put(update_course_page)
                .delete(delete_course_page),
        )
        .route(
            "/courses/{courseId}/pages/{url}/revisions",
            get(list_revisions),
        )
        .route(
            "/courses/{courseId}/pages/{url}/revisions/latest",
            get(show_latest_revision),
        )
        .route(
            "/courses/{courseId}/pages/{url}/revisions/{revisionId}",
            get(show_revision),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{
        generate_url_slug, include_content, parse_canvas_bool, to_base36, PageRevisionRow,
    };

    fn revision_row() -> PageRevisionRow {
        PageRevisionRow {
            revision_id: 2,
            title: "Unit Page".to_string(),
            body: Some("<p>hello</p>".to_string()),
            url: Some("unit-page".to_string()),
            user_id: Some(36),
            user_name: Some("Pat Teacher".to_string()),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn slug_basic() {
        assert_eq!(generate_url_slug("Hello World"), "hello-world");
    }

    #[test]
    fn slug_collapses_whitespace_and_punctuation() {
        assert_eq!(generate_url_slug("Foo  Bar!! Baz"), "foo-bar-baz");
        assert_eq!(generate_url_slug("Hello, World!"), "hello-world");
    }

    #[test]
    fn slug_trims_dashes() {
        assert_eq!(generate_url_slug("  Trim -- Me  "), "trim-me");
        assert_eq!(
            generate_url_slug("--leading-and-trailing--"),
            "leading-and-trailing"
        );
    }

    #[test]
    fn slug_lowercase_and_numbers() {
        assert_eq!(generate_url_slug("Chapter 12: Intro"), "chapter-12-intro");
        assert_eq!(generate_url_slug("MiXeD CaSe"), "mixed-case");
    }

    #[test]
    fn slug_empty_and_symbols_only() {
        assert_eq!(generate_url_slug(""), "");
        assert_eq!(generate_url_slug("!!! ???"), "");
    }

    #[test]
    fn slug_underscores_are_separators() {
        assert_eq!(generate_url_slug("snake_case_title"), "snake-case-title");
    }

    #[test]
    fn revision_json_and_canvas_helpers_match_canvas() {
        // History item shape: revision_id, updated_at, latest, edited_by only.
        let value = serde_json::to_value(revision_row().summary(2)).expect("serialize summary");
        assert_eq!(value["revision_id"], 2);
        assert_eq!(value["latest"], true);
        assert!(value.get("title").is_none());
        assert!(value.get("body").is_none());
        assert!(value.get("url").is_none());
        // edited_by mirrors user_display_json; 36 -> "10" in base36.
        assert_eq!(value["edited_by"]["id"], 36);
        assert_eq!(value["edited_by"]["anonymous_id"], "10");
        assert_eq!(value["edited_by"]["display_name"], "Pat Teacher");
        assert_eq!(value["edited_by"]["html_url"], "/users/36");

        // Detail keeps content; summary=true (include_content = false) drops it.
        let full = serde_json::to_value(revision_row().detail(2, true)).expect("serialize full");
        assert_eq!(full["title"], "Unit Page");
        assert_eq!(full["body"], "<p>hello</p>");
        assert_eq!(full["url"], "unit-page");

        let summary =
            serde_json::to_value(revision_row().detail(2, false)).expect("serialize summary");
        assert!(summary.get("title").is_none());
        assert!(summary.get("body").is_none());
        assert!(summary.get("url").is_none());
        assert_eq!(summary["edited_by"]["id"], 36);
        assert!(!revision_row().detail(3, false).latest);

        // anonymous_id base36 and Canvas::Plugin.value_to_boolean parity.
        assert_eq!(to_base36(0), "0");
        assert_eq!(to_base36(10), "a");
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
        assert_eq!(to_base36(1295), "zz");
        assert_eq!(to_base36(1296), "100");
        for raw in ["1", "t", "T", "true", "TRUE", "yes", "y", "on"] {
            assert!(parse_canvas_bool(raw), "{raw} should be truthy");
        }
        for raw in ["0", "f", "false", "no", "n", "off", ""] {
            assert!(!parse_canvas_bool(raw), "{raw} should be falsy");
        }
        assert!(include_content(None));
        assert!(include_content(Some("false")));
        assert!(!include_content(Some("true")));
        assert!(!include_content(Some("1")));
    }
}
