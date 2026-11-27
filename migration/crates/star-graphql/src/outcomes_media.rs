//! LearningOutcome + LearningOutcomeGroup object types with their
//! `Course` fields, plus MediaObject/MediaTrack and
//! `Query.mediaObject` (GraphQL parity gap completion).
//!
//! Mirrors Canvas `app/graphql/types/learning_outcome_type.rb`,
//! `learning_outcome_group_type.rb`, `media_object_type.rb` /
//! `media_source_type.rb` / `media_track_type.rb` and the
//! `outcomesConnection` / `outcomeGroups` surfaces on `course_type.rb`.
//!
//! Schema backing:
//! - migration 0022: `learning_outcomes`, `learning_outcome_groups`
//!   (course_id) and `learning_outcome_links` (outcome ↔ group, UNIQUE
//!   pair) — the REST star-outcomes tables;
//! - migration 0098: `media_objects` (user-scoped, `media_id` string)
//!   and `media_tracks` (kind/locale/content).
//!
//! Canvas shape (subset, camelCase):
//! - LearningOutcome: `id`, `title`, `description`, `displayName`,
//!   `createdAt`, `updatedAt`;
//! - LearningOutcomeGroup: `id`, `title`, `createdAt`, `updatedAt`
//!   (Canvas's `description`/`vendorGuid` need columns the 0022 table
//!   does not carry — documented omission) with its linked outcomes;
//! - MediaObject: `id`, `mediaId`, `mediaType`, `title`,
//!   `userEnteredTitle`, `createdAt` + nested `mediaTracks`;
//! - MediaTrack: `id`, `kind`, `locale`, `content`, `workflowState`.
//!
//! Authorization: outcome visibility is inherited from `course(id)`
//! (REST parity with the star-outcomes handlers). `Query.mediaObject`
//! is self-or-admin — Canvas scopes media objects to their owner
//! (`media_objects.user_id`), and the 0098 listing is explicitly
//! user-scoped.

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{
    db_err, decode_cursor, encode_cursor, GraphQLContext, PageInfo, DEFAULT_PAGE_SIZE,
    MAX_PAGE_SIZE,
};

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

// The outcome rows carry the full Canvas column set; the shared
// `root_data` row types and their mappers are reused so the objects built
// here are the same ones `Query.learningOutcome` returns.
use crate::root_data::{learning_outcome_from_row, LearningOutcomeRow};

#[derive(Debug, Clone, sqlx::FromRow)]
struct MediaObjectRow {
    id: i64,
    media_id: String,
    media_type: String,
    title: Option<String>,
    user_entered_title: Option<String>,
    user_id: i64,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct MediaTrackRow {
    id: i64,
    kind: String,
    locale: String,
    content: Option<String>,
    workflow_state: String,
}

// ---------------------------------------------------------------------------
// GraphQL types — outcomes
// ---------------------------------------------------------------------------

// NOTE: the `LearningOutcome` and `LearningOutcomeGroup` objects live in
// `root_types.rs` — they are the only ones in the schema (they answer
// `Query.learningOutcome`, `Query.learningOutcomeGroup` and the `Node`
// interface), and they carry the Canvas field names. The group's linked
// outcomes are exposed as Canvas' `ContentTagConnection`.

// ---------------------------------------------------------------------------
// GraphQL types — media
// ---------------------------------------------------------------------------

/// Canvas GraphQL `MediaTrack`
/// (`app/graphql/types/media_track_type.rb`, subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct MediaTrack {
    pub id: ID,
    /// `subtitles` | `captions` | `descriptions` | `chapters` |
    /// `metadata` (the 0098 CHECK values).
    pub kind: String,
    pub locale: String,
    pub content: Option<String>,
    pub workflow_state: String,
}

/// Canvas GraphQL `MediaObject`
/// (`app/graphql/types/media_object_type.rb`, subset).
#[derive(Debug, Clone)]
pub struct MediaObject {
    pub id: i64,
    pub media_id: String,
    pub media_type: String,
    pub title: Option<String>,
    pub user_entered_title: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[Object]
impl MediaObject {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `mediaId` — the string media identifier (0098 column).
    async fn media_id(&self) -> &str {
        &self.media_id
    }

    /// Canvas `mediaType` — `video` | `audio`.
    async fn media_type(&self) -> &str {
        &self.media_type
    }

    async fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    async fn user_entered_title(&self) -> Option<&str> {
        self.user_entered_title.as_deref()
    }

    /// The object's active tracks (0044-adjacent 0098 table; kind
    /// order not defined by Canvas — id order for determinism).
    async fn media_tracks(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Vec<MediaTrack>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_media_tracks(&gctx.pool, self.id).await
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct LearningOutcomeEdge {
    pub cursor: String,
    pub node: crate::root_types::LearningOutcome,
}

/// Relay connection for learning outcomes.
#[derive(Debug, Clone, SimpleObject)]
pub struct LearningOutcomeConnection {
    pub edges: Vec<LearningOutcomeEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Fetchers
// ---------------------------------------------------------------------------

/// A course's active groups (0022 learning_outcome_groups, id order).
pub(crate) async fn fetch_course_outcome_groups(
    pool: &PgPool,
    course_id: i64,
) -> Result<Vec<crate::root_types::LearningOutcomeGroup>, async_graphql::Error> {
    let rows: Vec<crate::root_data::LearningOutcomeGroupRow> = sqlx::query_as(
        r#"
        SELECT id, title, description, context_id, context_type, vendor_guid,
               workflow_state, created_at, updated_at
        FROM learning_outcome_groups
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        ORDER BY id
        "#,
    )
    .bind(course_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(crate::root_data::learning_outcome_group_from_row)
        .collect())
}

/// One page of a course's outcomes — every active outcome whose link
/// points at one of the course's groups, plus (Canvas
/// `Course.outcomesConnection` is account-tree scoped in Canvas;
/// star-lms has no account tree, so the course's linked outcomes are
/// the set — documented deviation). `LIMIT first+1` ordered by id.
pub(crate) async fn fetch_course_outcome_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<LearningOutcomeConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<LearningOutcomeRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT o.id, o.title, o.description, o.display_name,
               o.context_id, o.context_type, o.calculation_int,
               o.calculation_method, o.mastery_points, o.points_possible,
               o.assessed, o.vendor_guid, o.workflow_state,
               o.created_at, o.updated_at
        FROM learning_outcomes o
        JOIN learning_outcome_links l ON l.outcome_id = o.id
        JOIN learning_outcome_groups g ON g.id = l.outcome_group_id
        WHERE g.course_id = $1
          AND l.workflow_state = 'active'
          AND g.workflow_state <> 'deleted'
          AND o.workflow_state <> 'deleted'
          AND ($2::bigint IS NULL OR o.id > $2)
        ORDER BY o.id
        LIMIT $3
        "#,
    )
    .bind(course_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<LearningOutcomeEdge> = rows
        .into_iter()
        .map(|r| {
            let id = r.id;
            LearningOutcomeEdge {
                cursor: encode_cursor(id),
                node: learning_outcome_from_row(r),
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(LearningOutcomeConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// A media object by its string media id, owner-scoped: `None` when
/// missing, deleted, or owned by someone else (Canvas scopes media to
/// the owner; self-or-admin).
pub(crate) async fn fetch_media_object_for_caller(
    pool: &PgPool,
    caller: &GraphQLContext,
    media_id: &str,
) -> Result<Option<MediaObject>, async_graphql::Error> {
    let row: Option<MediaObjectRow> = sqlx::query_as(
        r#"
        SELECT id, media_id, media_type, title, user_entered_title, user_id, created_at
        FROM media_objects
        WHERE media_id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(media_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    let Some(row) = row else { return Ok(None) };
    if row.user_id != caller.auth_user.id && !caller.is_admin() {
        return Ok(None);
    }
    Ok(Some(MediaObject {
        id: row.id,
        media_id: row.media_id,
        media_type: row.media_type,
        title: row.title,
        user_entered_title: row.user_entered_title,
        created_at: row.created_at,
    }))
}

/// A media object's active tracks (id order).
async fn fetch_media_tracks(
    pool: &PgPool,
    media_object_id: i64,
) -> Result<Vec<MediaTrack>, async_graphql::Error> {
    let rows: Vec<MediaTrackRow> = sqlx::query_as(
        r#"
        SELECT id, kind, locale, content, workflow_state
        FROM media_tracks
        WHERE media_object_id = $1 AND workflow_state <> 'deleted'
        ORDER BY id
        "#,
    )
    .bind(media_object_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|r| MediaTrack {
            id: ID(r.id.to_string()),
            kind: r.kind,
            locale: r.locale,
            content: r.content,
            workflow_state: r.workflow_state,
        })
        .collect())
}
