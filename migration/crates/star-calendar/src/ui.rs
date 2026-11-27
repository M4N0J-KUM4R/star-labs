//! Pub data loaders for the server-rendered UI shell (star-ui, issue #111).
//!
//! Mirrors the `star_discussions::ui` extraction pattern: loaders apply the
//! SAME authorization gates as the REST handlers above.
//!
//! Canvas parity notes:
//! - Canvas's calendar is a React app; per the UI spec we port the essential
//!   server-rendered month list, not the SPA. Week view, mini-calendar,
//!   scheduler and the .ics feed are deferred.
//! - calendar_events are course-scoped in this workspace (course_id column),
//!   so the user calendar is the union of the events of the courses the
//!   viewer is enrolled in (admins see all courses' events).
//! - Loaders are read-only. Canvas' web calendar is
//!   `get "calendar" => "calendars#show"` (config/routes.rb:1103); event
//!   writes live in the per-context `resources :calendar_events` scopes and
//!   are served by the REST handlers in `lib.rs`, not by an HTML form.

use sqlx::PgPool;
use star_common::ApiError;

use crate::AuthUser;
use chrono::{DateTime, Utc};

/// A calendar event chip for the month grid.
#[derive(Debug, sqlx::FromRow)]
pub struct UiCalendarEvent {
    pub id: i64,
    pub course_id: i64,
    pub course_name: String,
    pub title: String,
    pub start_at: DateTime<Utc>,
    pub end_at: Option<DateTime<Utc>>,
    pub location_name: Option<String>,
}

/// A course context for the sidebar checkbox list.
#[derive(Debug, sqlx::FromRow)]
pub struct UiContext {
    pub course_id: i64,
    pub course_name: String,
}

/// The course contexts whose events the viewer may see (their enrollments,
/// or every course for admins). Backs the sidebar checkbox list.
pub async fn load_visible_contexts_for_ui(
    db: &PgPool,
    auth: &AuthUser,
) -> Result<Vec<UiContext>, ApiError> {
    if auth.role == "admin" {
        let rows = sqlx::query_as::<_, UiContext>(
            "SELECT id AS course_id, name AS course_name FROM courses \
             WHERE workflow_state <> 'deleted' ORDER BY name, id",
        )
        .fetch_all(db)
        .await?;
        return Ok(rows);
    }

    let rows = sqlx::query_as::<_, UiContext>(
        r#"
        SELECT c.id AS course_id, c.name AS course_name
        FROM enrollments e JOIN courses c ON c.id = e.course_id
        WHERE e.user_id = $1 AND c.workflow_state <> 'deleted'
        ORDER BY c.name, c.id
        "#,
    )
    .bind(auth.id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Events in `[start, end)` across the viewer's visible contexts, optionally
/// narrowed to the given course ids (the sidebar filter). Each returned event
/// is gated: the viewer must be enrolled in (or admin of) its course.
pub async fn load_events_for_range_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    context_filter: &[i64],
) -> Result<Vec<UiCalendarEvent>, ApiError> {
    let contexts = load_visible_contexts_for_ui(db, auth).await?;
    let visible: Vec<i64> = contexts.iter().map(|c| c.course_id).collect();
    let course_ids: Vec<i64> = if context_filter.is_empty() {
        visible
    } else {
        context_filter
            .iter()
            .filter(|id| visible.contains(id))
            .copied()
            .collect()
    };
    if course_ids.is_empty() {
        return Ok(Vec::new());
    }

    let rows = sqlx::query_as::<_, UiCalendarEvent>(
        r#"
        SELECT e.id, e.course_id, c.name AS course_name, e.title,
               e.start_at, e.end_at, e.location_name
        FROM calendar_events e JOIN courses c ON c.id = e.course_id
        WHERE e.course_id = ANY($1)
          AND e.workflow_state <> 'deleted'
          AND e.start_at >= $2 AND e.start_at < $3
        ORDER BY e.start_at, e.id
        "#,
    )
    .bind(&course_ids)
    .bind(start)
    .bind(end)
    .fetch_all(db)
    .await?;
    Ok(rows)
}
