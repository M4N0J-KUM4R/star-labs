//! ExternalTool object-type family + `Course.externalToolsConnection`
//! (GraphQL parity gap completion).
//!
//! Mirrors Canvas `app/graphql/types/external_tool_type.rb`,
//! `external_tool_placement_type.rb` / `external_tool_placements_type.rb`
//! and the `externalToolsConnection` field on `course_type.rb`. Schema
//! backing is migration 0085 `external_tools` (course or account
//! context; `enabled`/`visible` flags; JSONB configuration) — the
//! placements in Canvas come from the LTI 1.3 registration settings,
//! which star-lms stores inside `tool_configuration` JSONB; this slice
//! surfaces the placement keys found there (documented shape choice).
//!
//! Canvas shape (subset, camelCase):
//! - ExternalTool: `id`, `name`, `description`, `url`, `domain`,
//!   `iconUrl`, `workflowState`, `createdAt`, `updatedAt`, and a
//!   nested `placements { ... }` object;
//! - ExternalToolPlacement: `placementType` (the Canvas placement key,
//!   e.g. `course_assignments_menu`) — Canvas resolves more members
//!   (message, url, icon_url) from the registration; star-lms surfaces
//!   the placement key set with the configured values from JSONB.
//! - ExternalToolPlacements: the nested container.
//!
//! Authorization: visibility is inherited from `course(id)` — parity
//! with the star-lti REST tool listing, which gates on course
//! visibility. Deleted tools are filtered (Canvas `active` scope).

use async_graphql::{Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, decode_cursor, encode_cursor, PageInfo, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

// ---------------------------------------------------------------------------
// Database row
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct ExternalToolRow {
    id: i64,
    name: String,
    description: Option<String>,
    url: String,
    domain: Option<String>,
    icon_url: Option<String>,
    workflow_state: String,
    tool_configuration: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `ExternalToolPlacement`
/// (`app/graphql/types/external_tool_placement_type.rb`, subset): one
/// configured placement. Canvas members beyond the type come from the
/// LTI registration; star-lms surfaces the placement key and any
/// configured `url`/`message`/`iconUrl` from the JSONB configuration.
#[derive(Debug, Clone, SimpleObject)]
pub struct ExternalToolPlacement {
    pub placement_type: String,
    pub url: Option<String>,
    pub message: Option<String>,
    pub icon_url: Option<String>,
}

/// Canvas GraphQL `ExternalToolPlacements`
/// (`app/graphql/types/external_tool_placements_type.rb`): the nested
/// placement container on the tool.
#[derive(Debug, Clone, SimpleObject)]
pub struct ExternalToolPlacements {
    /// The tool's configured placements, key-sorted for determinism.
    pub placements: Vec<ExternalToolPlacement>,
}

/// Canvas GraphQL `ExternalTool`
/// (`app/graphql/types/external_tool_type.rb`, subset).
#[derive(Debug, Clone)]
pub struct ExternalTool {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub url: String,
    pub domain: Option<String>,
    pub icon_url: Option<String>,
    pub workflow_state: String,
    pub tool_configuration: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[Object]
impl ExternalTool {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Canvas `url` — the launch URL.
    async fn url(&self) -> &str {
        &self.url
    }

    /// Canvas `domain` — nullable domain-scoped matching.
    async fn domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }

    async fn icon_url(&self) -> Option<&str> {
        self.icon_url.as_deref()
    }

    /// Canvas `workflowState` — active|deleted.
    async fn workflow_state(&self) -> &str {
        &self.workflow_state
    }

    /// Canvas `placements` — the tool's configured LTI placements.
    /// star-lms stores them in the `tool_configuration` JSONB under a
    /// `placements` object (0085); missing/empty configuration resolves
    /// to an empty placement list, not null (Canvas always exposes the
    /// container).
    async fn placements(&self) -> ExternalToolPlacements {
        let mut out: Vec<ExternalToolPlacement> = Vec::new();
        if let Some(map) = self
            .tool_configuration
            .get("placements")
            .and_then(|v| v.as_object())
        {
            for (key, value) in map {
                let obj = value.as_object();
                out.push(ExternalToolPlacement {
                    placement_type: key.clone(),
                    url: obj
                        .and_then(|o| o.get("url"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    message: obj
                        .and_then(|o| o.get("message"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    icon_url: obj
                        .and_then(|o| o.get("icon_url"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                });
            }
        }
        out.sort_by(|a, b| a.placement_type.cmp(&b.placement_type));
        ExternalToolPlacements { placements: out }
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct ExternalToolEdge {
    pub cursor: String,
    pub node: ExternalTool,
}

/// Relay connection for external tools.
#[derive(Debug, Clone, SimpleObject)]
pub struct ExternalToolConnection {
    pub edges: Vec<ExternalToolEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetch
// ---------------------------------------------------------------------------

/// Fetch one page of a course's enabled, non-deleted tools
/// (`LIMIT first+1`, ordered by id), optionally resuming after a
/// cursor. The extra row drives `hasNextPage`.
pub(crate) async fn fetch_external_tool_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<ExternalToolConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<ExternalToolRow> = sqlx::query_as(
        r#"
        SELECT id, name, description, url, domain, icon_url, workflow_state,
               tool_configuration, created_at, updated_at
        FROM external_tools
        WHERE course_id = $1
          AND workflow_state <> 'deleted'
          AND enabled
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
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

    let edges: Vec<ExternalToolEdge> = rows
        .into_iter()
        .map(|row| {
            let id = row.id;
            ExternalToolEdge {
                cursor: encode_cursor(id),
                node: ExternalTool {
                    id: row.id,
                    name: row.name,
                    description: row.description,
                    url: row.url,
                    domain: row.domain,
                    icon_url: row.icon_url,
                    workflow_state: row.workflow_state,
                    tool_configuration: row.tool_configuration,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(ExternalToolConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}
