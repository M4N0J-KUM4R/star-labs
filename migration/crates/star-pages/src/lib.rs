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
//! - GET/POST /courses/{courseId}/pages/{url}/revisions/{revisionId}
//!   (one version; POST reverts the page to it)
//! - GET/PUT /courses/{courseId}/front_page                    (course front page)
//! - POST /courses/{courseId}/pages/{urlOrId}/duplicate        (copy a page)
//! - POST /courses/{courseId}/pages_ai/alt_text                (stub alt text)
//! - GET/POST /groups/{groupId}/pages                          (group wiki; declined)
//! - GET/PUT/DELETE /groups/{groupId}/pages/{urlOrId}          (group wiki; declined)
//! - GET/PUT /groups/{groupId}/front_page                      (group wiki; declined)
//!
//! Canvas parity notes (wiki_page.rb, wiki_pages_api_controller.rb):
//! - pages carry `url` (slug derived from title), `workflow_state`
//!   ('published'/'unpublished'/'deleted') and `editing_roles`.
//! - updates accept `published` and the deprecated `hidden` booleans besides
//!   `workflow_state`, with Canvas precedence published > hidden >
//!   workflow_state; 'active' is Canvas' name for the published state.
//! - the list endpoint omits `body` (Canvas pages#index returns summaries).
//! - DELETE soft-deletes: workflow_state = 'deleted'.
//! - revisions mirror simply_versioned: every page save appends a version with
//!   a per-page 1-based number, newest first; `show_revision` needs
//!   `:read_revisions` (teacher/admin) while `/revisions/latest` only needs
//!   read access (enrolled student/admin). `summary=true` drops content.
//! - revert restores a prior version's title/body/url and appends the
//!   restored state as a NEW revision (history is never rewritten).
//! - Canvas serves group wikis through the same pages API, but wiki pages are
//!   course-scoped in this schema (`pages` has no group column), so the group
//!   routes run the context+auth checks and then decline with 403, mirroring
//!   the star-discussions group-summaries pattern.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
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
    /// Canvas `wiki_page[hidden]` (deprecated): inverse of `published`.
    pub hidden: Option<bool>,
    /// Canvas `wiki_page[published]`: boolean form of `workflow_state`.
    pub published: Option<bool>,
}

impl UpdatePage {
    /// True when the request carries no page attributes at all.
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.body.is_none()
            && self.workflow_state.is_none()
            && self.editing_roles.is_none()
            && self.hidden.is_none()
            && self.published.is_none()
    }

    /// Requested workflow_state with Canvas precedence on update:
    /// `published` > `hidden` > `workflow_state` (wiki_pages_api_controller
    /// `get_update_params` turns `published` into the state). `hidden` is the
    /// deprecated inverse of `published`, and workflow_state 'active' is
    /// Canvas' name for the published state.
    fn requested_workflow_state(&self) -> Option<&'static str> {
        if let Some(published) = self.published {
            return Some(if published {
                "published"
            } else {
                "unpublished"
            });
        }
        if let Some(hidden) = self.hidden {
            return Some(if hidden { "unpublished" } else { "published" });
        }
        let requested = self
            .workflow_state
            .as_deref()
            .map(str::trim)
            .filter(|state| !state.is_empty())?;
        Some(if matches!(requested, "published" | "active") {
            "published"
        } else {
            "unpublished"
        })
    }
}

/// Request body for PUT /courses/{courseId}/front_page.
///
/// Mirrors the Canvas `update_front_page` arguments (`wiki_page[title|body|
/// editing_roles|published]`); `published` is Canvas' boolean form of
/// `workflow_state` and takes precedence when both are supplied.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateFrontPage {
    pub title: Option<String>,
    pub body: Option<String>,
    pub editing_roles: Option<String>,
    pub workflow_state: Option<String>,
    pub published: Option<bool>,
}

impl UpdateFrontPage {
    /// True when the request carries no page attributes (Canvas `@page.update({})`).
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.body.is_none()
            && self.editing_roles.is_none()
            && self.workflow_state.is_none()
            && self.published.is_none()
    }

    /// Requested workflow_state, normalized onto this schema's vocabulary
    /// ('published'/'unpublished'); Canvas calls the published state 'active'.
    fn requested_workflow_state(&self) -> Option<&'static str> {
        let requested = match self.published {
            Some(true) => "published",
            Some(false) => "unpublished",
            None => self
                .workflow_state
                .as_deref()
                .map(str::trim)
                .filter(|state| !state.is_empty())?,
        };
        if matches!(requested, "published" | "active") {
            Some("published")
        } else {
            Some("unpublished")
        }
    }
}

/// Front-page page row (Canvas wiki_page_json source, minus the marker).
#[derive(Debug, sqlx::FromRow)]
pub struct FrontPageRow {
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

/// Canvas `wiki_page_json` subset returned by the front_page endpoints.
#[derive(Debug, Serialize)]
pub struct FrontPage {
    pub id: i64,
    pub page_id: i64,
    pub course_id: i64,
    pub title: String,
    pub body: Option<String>,
    pub url: Option<String>,
    pub workflow_state: String,
    pub editing_roles: String,
    pub published: bool,
    pub hide_from_students: bool,
    pub front_page: bool,
    pub html_url: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Canvas `require_wiki_page` body for a course without a front page.
enum FrontPageError {
    NoFrontPage,
    Api(ApiError),
    Db(sqlx::Error),
}

impl From<ApiError> for FrontPageError {
    fn from(err: ApiError) -> Self {
        FrontPageError::Api(err)
    }
}

impl From<sqlx::Error> for FrontPageError {
    fn from(err: sqlx::Error) -> Self {
        FrontPageError::Db(err)
    }
}

impl IntoResponse for FrontPageError {
    fn into_response(self) -> Response {
        match self {
            FrontPageError::NoFrontPage => (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "No front page has been set" })),
            )
                .into_response(),
            FrontPageError::Api(err) => err.into_response(),
            FrontPageError::Db(err) => ApiError::from(err).into_response(),
        }
    }
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
// Front page helpers (Canvas Wiki#front_page / wiki_page_json parity)
// ---------------------------------------------------------------------------

/// Canvas treats 'active' and 'published' as the same published state.
fn is_published(workflow_state: &str) -> bool {
    matches!(workflow_state, "published" | "active")
}

impl FrontPageRow {
    fn into_front_page(self) -> FrontPage {
        let published = is_published(&self.workflow_state);
        let reference = self.url.clone().unwrap_or_else(|| self.id.to_string());
        FrontPage {
            id: self.id,
            page_id: self.id,
            course_id: self.course_id,
            title: self.title,
            body: self.body,
            url: self.url,
            workflow_state: self.workflow_state,
            editing_roles: self.editing_roles,
            published,
            hide_from_students: !published,
            front_page: true,
            html_url: format!("/courses/{}/pages/{reference}", self.course_id),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Load the live page designated as the course front page.
///
/// Canvas resolves the wiki's `front_page_url`; this schema keeps the marker
/// on the page row (`pages.front_page`, migration 0075). Soft-deleted pages
/// are ignored, so deleting the front page leaves the course without one.
async fn load_front_page(db: &PgPool, course_id: i64) -> Result<Option<FrontPageRow>, ApiError> {
    let row: Option<FrontPageRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, body, url, workflow_state, editing_roles,
               created_at, updated_at
        FROM pages
        WHERE course_id = $1 AND front_page AND workflow_state <> 'deleted'
        ORDER BY updated_at DESC
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
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
    if payload.is_empty() {
        return Err(ApiError::Validation(
            "at least one page attribute is required".into(),
        ));
    }

    // If the title changed, Canvas regenerates the url slug (wiki_page.rb).
    let new_url = payload.title.as_ref().map(|t| generate_url_slug(t));
    // Canvas precedence on update: published > hidden > workflow_state.
    let workflow_state = payload.requested_workflow_state();

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
    .bind(workflow_state)
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
// Front page (Canvas wiki_pages_api_controller#show_front_page / #update_front_page)
// ---------------------------------------------------------------------------

/// Current live front page id for a course (inside the update transaction).
async fn front_page_id_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    course_id: i64,
) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT id FROM pages
        WHERE course_id = $1 AND front_page AND workflow_state <> 'deleted'
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(id,)| id))
}

/// GET /courses/{courseId}/front_page — Canvas `show_front_page` (delegates to
/// `show`, so the body is included).
///
/// Canvas runs `require_wiki_page` before authorization, so a course without a
/// designated page answers 404 `{"message":"No front page has been set"}` even
/// for users who could not read the course.
async fn get_front_page(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<FrontPage>, FrontPageError> {
    let page = load_front_page(&state.db, course_id)
        .await?
        .ok_or(FrontPageError::NoFrontPage)?;

    if !user_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden.into());
    }

    Ok(Json(page.into_front_page()))
}

/// PUT /courses/{courseId}/front_page — Canvas `update_front_page`.
///
/// Updates the existing front page, or creates one (published, title required)
/// when the course has none — Canvas builds a new wiki page in that case.
/// Changing the title regenerates the url slug and appends a revision, exactly
/// like a regular page update. The front page cannot be unpublished.
async fn update_front_page(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<UpdateFrontPage>>,
) -> Result<Json<FrontPage>, FrontPageError> {
    // Canvas authorizes :update on an existing page, :create for the first one.
    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden.into());
    }

    let course: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;
    if course.is_none() {
        return Err(ApiError::NotFound.into());
    }

    let payload = payload.map(|Json(body)| body).unwrap_or_default();

    // Canvas `cannot_have_unpublished_front_page`: 400 for draft front pages.
    if payload.requested_workflow_state() == Some("unpublished") {
        return Err(ApiError::BadRequest("The front page cannot be unpublished".into()).into());
    }

    let mut tx = state.db.begin().await?;
    let existing_page = front_page_id_in_tx(&mut tx, course_id).await?;

    // Canvas `unset_front_page!`: move the marker first so the per-course
    // unique index (idx_pages_course_front_page) never sees two front pages.
    sqlx::query("UPDATE pages SET front_page = false WHERE course_id = $1 AND front_page")
        .bind(course_id)
        .execute(&mut *tx)
        .await?;

    let page: FrontPageRow = if let Some(page_id) = existing_page {
        if payload.is_empty() {
            // Canvas `@page.update({})`: keep the page untouched, just re-mark it.
            sqlx::query("UPDATE pages SET front_page = true WHERE id = $1")
                .bind(page_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query_as(
                r#"
                SELECT id, course_id, title, body, url, workflow_state, editing_roles,
                       created_at, updated_at
                FROM pages WHERE id = $1
                "#,
            )
            .bind(page_id)
            .fetch_one(&mut *tx)
            .await?
        } else {
            // Canvas regenerates the url slug when the title changes.
            let new_url = payload.title.as_ref().map(|title| generate_url_slug(title));
            let row: FrontPageRow = sqlx::query_as(
                r#"
                UPDATE pages
                SET title = COALESCE($1, title),
                    body = COALESCE($2, body),
                    url = COALESCE($3, url),
                    editing_roles = COALESCE($4, editing_roles),
                    workflow_state = COALESCE($5, workflow_state),
                    front_page = true,
                    updated_at = now()
                WHERE id = $6
                RETURNING id, course_id, title, body, url, workflow_state, editing_roles,
                          created_at, updated_at
                "#,
            )
            .bind(&payload.title)
            .bind(&payload.body)
            .bind(&new_url)
            .bind(&payload.editing_roles)
            .bind(payload.requested_workflow_state())
            .bind(page_id)
            .fetch_one(&mut *tx)
            .await?;

            // simply_versioned: the save appends a revision.
            insert_revision(
                &mut tx,
                row.id,
                auth.id,
                &row.title,
                row.body.as_deref(),
                row.url.as_deref(),
            )
            .await?;
            row
        }
    } else {
        // Canvas `update_front_page` creates the page when none is set; the
        // wiki page title is required (WikiPage validates title presence).
        let title = payload
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .ok_or_else(|| ApiError::BadRequest("title is required".into()))?;
        let url = generate_url_slug(title);
        let editing_roles = payload
            .editing_roles
            .clone()
            .unwrap_or_else(|| "teachers".to_string());

        let row: FrontPageRow = sqlx::query_as(
            r#"
            INSERT INTO pages (course_id, title, body, url, workflow_state, editing_roles, front_page)
            VALUES ($1, $2, $3, $4, 'published', $5, true)
            RETURNING id, course_id, title, body, url, workflow_state, editing_roles,
                      created_at, updated_at
            "#,
        )
        .bind(course_id)
        .bind(title)
        .bind(&payload.body)
        .bind(&url)
        .bind(&editing_roles)
        .fetch_one(&mut *tx)
        .await?;

        // simply_versioned: the creating save is revision 1.
        insert_revision(
            &mut tx,
            row.id,
            auth.id,
            &row.title,
            row.body.as_deref(),
            row.url.as_deref(),
        )
        .await?;
        row
    };

    tx.commit().await?;

    Ok(Json(page.into_front_page()))
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

/// POST /courses/{courseId}/pages/{urlOrId}/revisions/{revisionId} — revert.
///
/// Canvas `revert`: requires `:read_revisions` and `:update` on the page
/// (teachers/admins), restores the revision's body/title/url and saves, which
/// appends the restored state as a NEW revision (history is never rewritten).
/// The response is the current version's revision JSON including content.
async fn revert_page(
    State(state): State<AppState>,
    Path((course_id, page_ref, revision_ref)): Path<(i64, String, String)>,
    auth: AuthUser,
) -> Result<Json<RevisionDetail>, ApiError> {
    let page = load_page_in_course(&state.db, course_id, &page_ref).await?;

    // Canvas authorizes :read_revisions AND :update (teacher/admin).
    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas does `params[:revision_id].to_i` then `first!` -> 404.
    let revision_id = revision_ref
        .parse::<i32>()
        .map_err(|_| ApiError::NotFound)?;
    let revision = fetch_revision(&state.db, page.id, revision_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let mut tx = state.db.begin().await?;

    // Canvas sets body/title/url straight from the version's model.
    let restored: PageDetail = sqlx::query_as(
        r#"
        UPDATE pages
        SET title = $1, body = $2, url = $3, updated_at = now()
        WHERE id = $4
        RETURNING id, course_id, title, body, url, workflow_state, editing_roles,
                  created_at, updated_at
        "#,
    )
    .bind(&revision.title)
    .bind(&revision.body)
    .bind(&revision.url)
    .bind(page.id)
    .fetch_one(&mut *tx)
    .await?;

    // simply_versioned: the revert save appends the restored state as the new
    // current revision; the prior versions stay untouched.
    let current_revision_id = insert_revision(
        &mut tx,
        restored.id,
        auth.id,
        &restored.title,
        restored.body.as_deref(),
        restored.url.as_deref(),
    )
    .await?;

    tx.commit().await?;

    // Canvas renders `wiki_page_revision_json(@page.versions.current, ...)`
    // with include_content: true.
    let current = fetch_revision(&state.db, page.id, current_revision_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(current.detail(current_revision_id, true)))
}

// ---------------------------------------------------------------------------
// Page duplication (Canvas wiki_pages_api_controller#duplicate)
// ---------------------------------------------------------------------------

/// Canvas `get_copy_title(page, t("Copy"), title)`: "Title" -> "Title Copy".
fn duplicate_title(title: &str) -> String {
    format!("{title} Copy")
}

/// POST /courses/{courseId}/pages/{urlOrId}/duplicate — copy a page.
///
/// Canvas authorizes `:create` on the page (teachers/admins), then saves the
/// copy as an unpublished page with the source body and editing roles and
/// answers 200 with `wiki_page_json`. Soft-deleted pages are not resolvable
/// here, so they answer 404 rather than Canvas' 400
/// `{"error":"cannot duplicate deleted page"}`.
async fn duplicate_page(
    State(state): State<AppState>,
    Path((course_id, page_ref)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<PageDetail>, ApiError> {
    let page = load_page_in_course(&state.db, course_id, &page_ref).await?;

    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let title = duplicate_title(&page.title);
    let url = generate_url_slug(&title);

    let mut tx = state.db.begin().await?;

    let copy: PageDetail = sqlx::query_as(
        r#"
        INSERT INTO pages (course_id, title, body, url, workflow_state, editing_roles,
                           duplicated_from_page_id)
        VALUES ($1, $2, $3, $4, 'unpublished', $5, $6)
        RETURNING id, course_id, title, body, url, workflow_state, editing_roles,
                  created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&title)
    .bind(&page.body)
    .bind(&url)
    .bind(&page.editing_roles)
    .bind(page.id)
    .fetch_one(&mut *tx)
    .await?;

    // simply_versioned: the creating save is revision 1.
    insert_revision(
        &mut tx,
        copy.id,
        auth.id,
        &copy.title,
        copy.body.as_deref(),
        copy.url.as_deref(),
    )
    .await?;

    tx.commit().await?;

    Ok(Json(copy))
}

// ---------------------------------------------------------------------------
// AI alt text (Canvas wiki_pages_api_controller#ai_generate_alt_text)
// ---------------------------------------------------------------------------

/// Request body for POST /courses/{courseId}/pages_ai/alt_text.
#[derive(Debug, Deserialize)]
pub struct AltTextRequest {
    pub attachment_id: Option<i64>,
}

/// Canvas `{ image: { altText } }` response of `ai_generate_alt_text`.
#[derive(Debug, Serialize)]
pub struct AltTextResponse {
    pub image: AltTextImage,
}

#[derive(Debug, Serialize)]
pub struct AltTextImage {
    #[serde(rename = "altText")]
    pub alt_text: String,
}

/// POST /courses/{courseId}/pages_ai/alt_text — stub.
///
/// Canvas requires content-manage rights (teachers/admins), a valid
/// `attachment_id` and a configured AI client. No AI client exists here, so
/// the handler validates the request and answers the Canvas response shape
/// with placeholder text instead of calling an external service.
async fn generate_alt_text(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<AltTextRequest>>,
) -> Result<Json<AltTextResponse>, ApiError> {
    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas `ai_alt_text_params_valid?`: blank attachment_id -> 400.
    let attachment_id = payload
        .and_then(|Json(body)| body.attachment_id)
        .ok_or_else(|| ApiError::BadRequest("attachment_id is required".into()))?;

    Ok(Json(AltTextResponse {
        image: AltTextImage {
            alt_text: format!("Image of attachment {attachment_id}"),
        },
    }))
}

// ---------------------------------------------------------------------------
// Group wiki routes (Canvas group contexts, declined in this schema)
// ---------------------------------------------------------------------------

/// 404 unless the group context exists (Canvas `get_context` -> Group.find).
async fn ensure_group(db: &PgPool, group_id: i64) -> Result<(), ApiError> {
    let group: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM groups WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(group_id)
            .fetch_optional(db)
            .await?;
    group.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `require_context_and_read_access` for a group context: group
/// members, the parent course's teachers and admins may read it.
async fn ensure_group_read_access(
    db: &PgPool,
    group_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    if is_admin(auth) {
        return Ok(());
    }
    let membership: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM group_memberships
        WHERE group_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'
        LIMIT 1
        "#,
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    if membership.is_some() {
        return Ok(());
    }
    let teacher: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM enrollments e
        JOIN groups g ON g.course_id = e.course_id
        WHERE g.id = $1 AND e.user_id = $2 AND e.role = 'teacher'
        LIMIT 1
        "#,
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    if teacher.is_some() {
        return Ok(());
    }
    Err(ApiError::Forbidden)
}

/// Manage rights in a group context: the parent course's teachers and admins
/// (Canvas wiki `manage` rights; group members cannot manage pages here).
async fn ensure_group_manage_access(
    db: &PgPool,
    group_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    if is_admin(auth) {
        return Ok(());
    }
    let teacher: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM enrollments e
        JOIN groups g ON g.course_id = e.course_id
        WHERE g.id = $1 AND e.user_id = $2 AND e.role = 'teacher'
        LIMIT 1
        "#,
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    if teacher.is_some() {
        return Ok(());
    }
    Err(ApiError::Forbidden)
}

/// GET /groups/{groupId}/pages — Canvas group wiki pages#index.
///
/// Canvas serves group wikis through the same pages API, but wiki pages are
/// course-scoped in this schema (`pages` has no group column), so no page can
/// belong to a group context. After the context and read-access checks the
/// endpoint therefore declines with 403, mirroring the star-discussions
/// group-summaries pattern.
async fn list_group_pages(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<PageSummary>>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

/// POST /groups/{groupId}/pages — Canvas group wiki pages#create (declined).
async fn create_group_page(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<PageDetail>), ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_manage_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

/// POST /groups/{groupId}/pages/{urlOrId} — same decline as the collection
/// create; registered so the method is authorized (403) rather than a bare
/// 405 from the router.
async fn create_group_page_at(
    state: State<AppState>,
    Path((group_id, _page_ref)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<PageDetail>), ApiError> {
    create_group_page(state, Path(group_id), auth).await
}

/// GET /groups/{groupId}/pages/{urlOrId} — Canvas group wiki pages#show
/// (declined: no page can live in a group context in this schema).
async fn show_group_page(
    State(state): State<AppState>,
    Path((group_id, _page_ref)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<PageDetail>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

/// PUT /groups/{groupId}/pages/{urlOrId} — Canvas group wiki pages#update
/// (declined: no page can live in a group context in this schema).
async fn update_group_page(
    State(state): State<AppState>,
    Path((group_id, _page_ref)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<PageDetail>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_manage_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

/// DELETE /groups/{groupId}/pages/{urlOrId} — Canvas group wiki pages#destroy
/// (declined: no page can live in a group context in this schema).
async fn delete_group_page(
    State(state): State<AppState>,
    Path((group_id, _page_ref)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_manage_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

/// GET /groups/{groupId}/front_page — Canvas group wiki `show_front_page`
/// (declined: no page can live in a group context in this schema).
async fn show_group_front_page(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<FrontPage>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
}

/// PUT /groups/{groupId}/front_page — Canvas group wiki `update_front_page`
/// (declined: no page can live in a group context in this schema).
async fn update_group_front_page(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<FrontPage>, ApiError> {
    ensure_group(&state.db, group_id).await?;
    ensure_group_manage_access(&state.db, group_id, &auth).await?;
    Err(ApiError::Forbidden)
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
            get(show_revision).post(revert_page),
        )
        .route(
            "/courses/{courseId}/front_page",
            get(get_front_page).put(update_front_page),
        )
        .route(
            "/courses/{courseId}/pages/{url}/duplicate",
            post(duplicate_page),
        )
        .route(
            "/courses/{courseId}/pages_ai/alt_text",
            post(generate_alt_text),
        )
        .route(
            "/groups/{groupId}/pages",
            get(list_group_pages).post(create_group_page),
        )
        .route(
            "/groups/{groupId}/pages/{urlOrId}",
            get(show_group_page)
                .post(create_group_page_at)
                .put(update_group_page)
                .delete(delete_group_page),
        )
        .route(
            "/groups/{groupId}/front_page",
            get(show_group_front_page).put(update_group_front_page),
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
