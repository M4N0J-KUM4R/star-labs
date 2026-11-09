//! star-pages domain crate — wiki pages for courses.
//!
//! Exposes `pub fn router() -> Router<AppState>` with endpoints:
//! - GET/POST /courses/{courseId}/pages
//! - GET/PUT/DELETE /pages/{id}   (id may be numeric id or url slug)
//!
//! Canvas parity notes (wiki_page.rb, wiki_pages_api_controller.rb):
//! - pages carry `url` (slug derived from title), `workflow_state`
//!   ('published'/'unpublished'/'deleted') and `editing_roles`.
//! - the list endpoint omits `body` (Canvas pages#index returns summaries).
//! - DELETE soft-deletes: workflow_state = 'deleted'.

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
    .fetch_one(&state.db)
    .await?;

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
    let row: Option<(i64,)> = if let Ok(numeric_id) = page_id.parse::<i64>() {
        sqlx::query_as("SELECT course_id FROM pages WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(numeric_id)
            .fetch_optional(&state.db)
            .await?
    } else {
        sqlx::query_as("SELECT course_id FROM pages WHERE url = $1 AND workflow_state <> 'deleted'")
            .bind(&page_id)
            .fetch_optional(&state.db)
            .await?
    };

    let (course_id,) = row.ok_or(ApiError::NotFound)?;

    // Check view access
    if !user_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let page: PageDetail = if let Ok(numeric_id) = page_id.parse::<i64>() {
        sqlx::query_as(
            r#"
            SELECT id, course_id, title, body, url, workflow_state, editing_roles,
                   created_at, updated_at
            FROM pages WHERE id = $1 AND workflow_state <> 'deleted'
            "#,
        )
        .bind(numeric_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?
    } else {
        sqlx::query_as(
            r#"
            SELECT id, course_id, title, body, url, workflow_state, editing_roles,
                   created_at, updated_at
            FROM pages WHERE url = $1 AND workflow_state <> 'deleted'
            "#,
        )
        .bind(&page_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?
    };

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
    let row: Option<(i64,)> = if let Ok(numeric_id) = page_id.parse::<i64>() {
        sqlx::query_as("SELECT course_id FROM pages WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(numeric_id)
            .fetch_optional(&state.db)
            .await?
    } else {
        sqlx::query_as("SELECT course_id FROM pages WHERE url = $1 AND workflow_state <> 'deleted'")
            .bind(&page_id)
            .fetch_optional(&state.db)
            .await?
    };

    let (course_id,) = row.ok_or(ApiError::NotFound)?;

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
    .bind(page_id.parse::<i64>().ok())
    .fetch_one(&state.db)
    .await?;

    Ok(Json(page))
}

/// DELETE /pages/{id} — Canvas parity soft delete: workflow_state = 'deleted'.
async fn delete_page(
    State(state): State<AppState>,
    Path(page_id): Path<String>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Get page to find course_id
    let row: Option<(i64,)> = if let Ok(numeric_id) = page_id.parse::<i64>() {
        sqlx::query_as("SELECT course_id FROM pages WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(numeric_id)
            .fetch_optional(&state.db)
            .await?
    } else {
        sqlx::query_as("SELECT course_id FROM pages WHERE url = $1 AND workflow_state <> 'deleted'")
            .bind(&page_id)
            .fetch_optional(&state.db)
            .await?
    };

    let (course_id,) = row.ok_or(ApiError::NotFound)?;

    // Check modify access (teacher/admin)
    if !user_teacher_of_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = if let Ok(numeric_id) = page_id.parse::<i64>() {
        sqlx::query("UPDATE pages SET workflow_state = 'deleted', updated_at = now() WHERE id = $1")
            .bind(numeric_id)
            .execute(&state.db)
            .await?
    } else {
        sqlx::query(
            "UPDATE pages SET workflow_state = 'deleted', updated_at = now() WHERE url = $1",
        )
        .bind(&page_id)
        .execute(&state.db)
        .await?
    };

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::generate_url_slug;

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
}
