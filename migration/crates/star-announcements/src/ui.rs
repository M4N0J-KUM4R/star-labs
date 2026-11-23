//! Pub data loaders for the server-rendered UI shell (star-ui, issue #111).
//!
//! Mirrors the `star_discussions::ui` extraction pattern: each loader applies
//! the SAME authorization gates as the REST handlers above and is the single
//! data path for both the JSON API and the HTML pages. No handler-level
//! re-implementation of permissions.
//!
//! Canvas parity notes (AnnouncementsApiController / announcements/index):
//! - index order is `posted_at DESC, id DESC` (Canvas uses a sticky-first
//!   sort then recency; there are no pinned announcements in this workspace,
//!   so plain recency). Unread state joins `announcement_reads` exactly like
//!   the REST `unread_count` endpoint (no `state = 'read'` row = unread).
//! - The announcements table has no author column, so the index shows only
//!   title / posted date / unread (a documented deviation from Canvas).

use sqlx::PgPool;
use star_common::ApiError;

use crate::{
    check_enrolled_or_admin, check_teacher_or_admin, ensure_announcement_in_course, AuthUser,
};

/// One row of the UI announcements index (Canvas announcements#index table).
#[derive(Debug, sqlx::FromRow)]
pub struct AnnouncementIndexRow {
    pub id: i64,
    pub title: String,
    pub posted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub unread: bool,
    pub workflow_state: String,
}

/// Announcement detail for the UI show page.
#[derive(Debug, sqlx::FromRow)]
pub struct AnnouncementDetail {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    pub message: String,
    pub workflow_state: String,
    pub posted_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A row of the course-summary table on the syllabus page (Canvas
/// courses#syllabus right column: assignment groups with their assignments).
#[derive(Debug, sqlx::FromRow)]
pub struct SummaryAssignment {
    pub id: i64,
    pub title: String,
    pub group_name: String,
    pub due_at: Option<chrono::DateTime<chrono::Utc>>,
    pub points_possible: Option<f64>,
}

/// Paginated announcements index for the UI (20 per page, like Canvas
/// `announcements#index` default `per_page`). Enrolled-or-admin gated.
pub async fn load_index_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    page: i64,
    per_page: i64,
) -> Result<Vec<AnnouncementIndexRow>, ApiError> {
    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }
    let per_page = per_page.clamp(1, 50);
    let offset = (page.max(1) - 1) * per_page;

    let rows = sqlx::query_as::<_, AnnouncementIndexRow>(
        r#"
        SELECT a.id, a.title, a.posted_at,
               (ar.id IS NULL OR ar.state <> 'read') AS unread,
               a.workflow_state
        FROM announcements a
        LEFT JOIN announcement_reads ar
            ON ar.announcement_id = a.id AND ar.user_id = $2
        WHERE a.course_id = $1 AND a.workflow_state <> 'deleted'
        ORDER BY a.created_at DESC, a.id DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(db)
    .await?;

    Ok(rows)
}

/// Total non-deleted announcements for a course (index pagination count).
/// Enrolled-or-admin gated, same as the index.
pub async fn count_index_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<i64, ApiError> {
    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM announcements WHERE course_id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(count)
}

/// Load one announcement for the UI show page and mark it read for the
/// viewer (Canvas `AnnouncementsController#show` -> topic read on view).
/// Enrolled-or-admin gated; soft-deleted announcements are 404 like the REST
/// `get_announcement`.
pub async fn load_detail_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    announcement_id: i64,
) -> Result<AnnouncementDetail, ApiError> {
    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_announcement_in_course(db, course_id, announcement_id).await?;

    let row: Option<AnnouncementDetail> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, posted_at
        FROM announcements
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(announcement_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    let detail = row.ok_or(ApiError::NotFound)?;

    // Viewing marks the topic read (best effort: read-state failure must not
    // hide the page).
    let _ = sqlx::query(
        r#"
        INSERT INTO announcement_reads (announcement_id, user_id, state)
        VALUES ($1, $2, 'read')
        ON CONFLICT (announcement_id, user_id)
        DO UPDATE SET state = 'read', updated_at = now()
        "#,
    )
    .bind(announcement_id)
    .bind(auth.id)
    .execute(db)
    .await;

    Ok(detail)
}

/// Create an announcement from the UI new-announcement form. Same validation
/// and gate as the REST `create_announcement`; returns the new id.
pub async fn create_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    title: &str,
    message: &str,
) -> Result<i64, ApiError> {
    if !check_teacher_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }
    if title.trim().is_empty() {
        return Err(ApiError::Validation("title cannot be empty".into()));
    }
    if message.trim().is_empty() {
        return Err(ApiError::Validation("message cannot be empty".into()));
    }

    let (id,): (i64,) = sqlx::query_as(
        r#"
        INSERT INTO announcements (course_id, title, message, workflow_state, posted_at)
        VALUES ($1, $2, $3, 'active', now())
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title.trim())
    .bind(message.trim())
    .fetch_one(db)
    .await?;

    Ok(id)
}

/// Load the course syllabus body for the syllabus page (admin-or-enrolled
/// gate like the REST course show; `is_admin` covers the account-admin case).
pub async fn load_syllabus_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<Option<String>, ApiError> {
    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT syllabus_body FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    Ok(row.ok_or(ApiError::NotFound)?.0)
}

/// Course summary for the syllabus right column: published assignments with
/// their assignment-group names, ordered by group position then due date.
/// Same viewer gate as the syllabus body.
pub async fn load_course_summary_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<Vec<SummaryAssignment>, ApiError> {
    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows = sqlx::query_as::<_, SummaryAssignment>(
        r#"
        SELECT a.id, a.title, g.name AS group_name, a.due_at, a.points_possible::float8
        FROM assignments a
        JOIN assignment_groups g ON g.id = a.assignment_group_id
        WHERE a.course_id = $1
          AND (a.workflow_state IS NULL OR a.workflow_state <> 'deleted')
          AND (g.workflow_state IS NULL OR g.workflow_state <> 'deleted')
        ORDER BY g.position NULLS LAST, g.id, a.due_at NULLS LAST, a.id
        "#,
    )
    .bind(course_id)
    .fetch_all(db)
    .await?;

    Ok(rows)
}
