//! lti/resource_links — Canvas `Lti::ResourceLinksController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /courses/{courseId}/lti_resource_links` — the resource links of a course
//! - `POST   /courses/{courseId}/lti_resource_links` — create a resource link
//! - `POST   /courses/{courseId}/lti_resource_links/bulk` — create many
//! - `GET    /courses/{courseId}/lti_resource_links/{id}` — show a resource link
//! - `PUT    /courses/{courseId}/lti_resource_links/{id}` — update a resource link
//! - `DELETE /courses/{courseId}/lti_resource_links/{id}` — delete a resource link
//!
//! Canvas routes (config/routes.rb, the course API block):
//!   get    "courses/:course_id/lti_resource_links", action: :index
//!   post   "courses/:course_id/lti_resource_links", action: :create
//!   post   "courses/:course_id/lti_resource_links/bulk", action: :bulk_create
//!   get    "courses/:course_id/lti_resource_links/:id", action: :show
//!   put    "courses/:course_id/lti_resource_links/:id", action: :update
//!   delete "courses/:course_id/lti_resource_links/:id", action: :destroy
//!
//! Canvas controller (`app/controllers/lti/resource_links_controller.rb`): the
//! `before_action` chain is `require_context_instrumented` (404 when the
//! course does not exist), `require_manage_assignments_add_permission`
//! (403), `require_manage_lti_add_permission` (403, except for the listing
//! and the lookup), `validate_custom`, `validate_url` and, for the update,
//! `validate_tool` and `validate_url_match_tool` (400).
//!
//! The resource link JSON is the Canvas `Api::V1::Lti::ResourceLink`
//! shape: the model attributes plus `resource_type` and
//! `canvas_launch_url`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// An `lti_resource_links` row.
#[derive(Debug, sqlx::FromRow)]
struct ResourceLink {
    id: i64,
    context_external_tool_id: i64,
    workflow_state: String,
    root_account_id: i64,
    context_id: i64,
    context_type: String,
    custom: Option<Value>,
    lookup_uuid: String,
    resource_link_uuid: String,
    url: Option<String>,
    lti_1_1_id: Option<String>,
    title: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// The columns of an `lti_resource_links` row.
const LINK_COLUMNS: &str = "id, context_external_tool_id, workflow_state, root_account_id, \
     context_id, context_type, custom, lookup_uuid, resource_link_uuid, url, lti_1_1_id, title, \
     created_at, updated_at";

/// The query parameters of the listing.
#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    include_deleted: Option<String>,
    per_page: Option<String>,
    page: Option<String>,
}

/// The body of a create or update request.
#[derive(Debug, Default, Deserialize)]
struct LinkRequest {
    url: Option<String>,
    title: Option<String>,
    custom: Option<Value>,
    lookup_uuid: Option<String>,
    resource_link_uuid: Option<String>,
    lti_1_1_id: Option<String>,
}

/// Domain router for the Canvas resource-link routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{course_id}/lti_resource_links",
            get(index_links).post(create_link),
        )
        .route(
            "/courses/{course_id}/lti_resource_links/bulk",
            axum::routing::post(bulk_create_links),
        )
        .route(
            "/courses/{course_id}/lti_resource_links/{id}",
            get(show_link).put(update_link).delete(destroy_link),
        )
}

/// `GET /courses/:course_id/lti_resource_links` — resource_links#index.
async fn index_links(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    require_manage_assignments(&state.db, &auth, course_id).await?;

    let include_deleted = value_to_boolean(params.include_deleted.as_deref());
    let per_page = params
        .per_page
        .as_deref()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(50);
    let page = params
        .page
        .as_deref()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    let offset = (page - 1) * per_page;

    // Canvas searches the links of the course and the links of its
    // assignments.
    let mut sql = format!(
        "SELECT {LINK_COLUMNS} FROM lti_resource_links \
         WHERE (context_type = 'Course' AND context_id = $1 \
                OR context_type = 'Assignment' AND context_id IN \
                    (SELECT id FROM assignments WHERE course_id = $1))"
    );
    if !include_deleted {
        sql.push_str(" AND workflow_state = 'active'");
    }
    sql.push_str(" ORDER BY created_at ASC, id ASC LIMIT $2 OFFSET $3");

    let links: Vec<ResourceLink> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    let items = links.iter().map(link_json).collect::<Vec<Value>>();
    Ok(Json(items).into_response())
}

/// `GET /courses/:course_id/lti_resource_links/:id` — resource_links#show.
async fn show_link(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    require_manage_assignments(&state.db, &auth, course_id).await?;
    let link = load_link(&state.db, course_id, &id).await?;
    Ok(Json(link_json(&link)).into_response())
}

/// `POST /courses/:course_id/lti_resource_links` — resource_links#create.
async fn create_link(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<LinkRequest>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    require_manage_assignments(&state.db, &auth, course_id).await?;
    require_manage_lti(&state.db, &auth, course_id).await?;

    // Canvas finds the tool by the launch URL of the link.
    let Some(url) = body.url.as_ref().filter(|value| !value.is_empty()) else {
        return Err(ApiError::BadRequest("url is required".to_string()));
    };
    let tool = tool_for_url(&state.db, course_id, url).await?;
    let Some(tool) = tool else {
        return Err(ApiError::BadRequest(
            "No tool found for the provided URL".to_string(),
        ));
    };

    let link = insert_link(
        &state.db,
        course_id,
        tool,
        body.custom.clone(),
        Some(url.clone()),
        body.title.clone(),
        body.lookup_uuid.clone(),
        body.resource_link_uuid.clone(),
        body.lti_1_1_id.clone(),
    )
    .await?;
    Ok(Json(link_json(&link)).into_response())
}

/// `POST /courses/:course_id/lti_resource_links/bulk` —
/// resource_links#bulk_create.
async fn bulk_create_links(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<Vec<LinkRequest>>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    require_manage_assignments(&state.db, &auth, course_id).await?;
    require_manage_lti(&state.db, &auth, course_id).await?;

    // Canvas limits a bulk create to 100 links.
    if body.len() > 100 {
        return Err(ApiError::BadRequest(
            "Too many resource links. The maximum is 100.".to_string(),
        ));
    }

    let mut created = Vec::with_capacity(body.len());
    for request in &body {
        let Some(url) = request.url.as_ref().filter(|value| !value.is_empty()) else {
            continue;
        };
        let Some(tool) = tool_for_url(&state.db, course_id, url).await? else {
            continue;
        };
        let link = insert_link(
            &state.db,
            course_id,
            tool,
            request.custom.clone(),
            Some(url.clone()),
            request.title.clone(),
            request.lookup_uuid.clone(),
            request.resource_link_uuid.clone(),
            request.lti_1_1_id.clone(),
        )
        .await?;
        created.push(link_json(&link));
    }
    Ok((StatusCode::CREATED, Json(created)).into_response())
}

/// `PUT /courses/:course_id/lti_resource_links/:id` — resource_links#update.
async fn update_link(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, String)>,
    auth: AuthUser,
    Json(body): Json<LinkRequest>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    require_manage_assignments(&state.db, &auth, course_id).await?;
    require_manage_lti(&state.db, &auth, course_id).await?;
    let link = load_link(&state.db, course_id, &id).await?;

    // Canvas validates that the tool of the link still matches the URL.
    if let Some(url) = body.url.as_ref().filter(|value| !value.is_empty()) {
        let tool = tool_for_url(&state.db, course_id, url).await?;
        if tool != Some(link.context_external_tool_id) {
            return Err(ApiError::BadRequest(
                "The provided url does not match the tool of the resource link".to_string(),
            ));
        }
    }

    sqlx::query(
        "UPDATE lti_resource_links SET title = COALESCE($1, title), url = COALESCE($2, url), \
             custom = COALESCE($3, custom), lti_1_1_id = COALESCE($4, lti_1_1_id), \
             updated_at = now() WHERE id = $5",
    )
    .bind(body.title.clone())
    .bind(body.url.clone())
    .bind(body.custom.clone())
    .bind(body.lti_1_1_id.clone())
    .bind(link.id)
    .execute(&state.db)
    .await?;

    let link = load_link_by_id(&state.db, link.id).await?;
    Ok(Json(link_json(&link)).into_response())
}

/// `DELETE /courses/:course_id/lti_resource_links/:id` —
/// resource_links#destroy.
async fn destroy_link(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    require_manage_assignments(&state.db, &auth, course_id).await?;
    require_manage_lti(&state.db, &auth, course_id).await?;
    let link = load_link(&state.db, course_id, &id).await?;

    sqlx::query("UPDATE lti_resource_links SET workflow_state = 'deleted', updated_at = now() WHERE id = $1")
        .bind(link.id)
        .execute(&state.db)
        .await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// The course has to exist — 404.
async fn require_course(db: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// `require_manage_assignments_add_permission`.
async fn require_manage_assignments(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE course_id = $1 AND user_id = $2 AND role = 'teacher' LIMIT 1",
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// `require_manage_lti_add_permission`.
async fn require_manage_lti(db: &PgPool, auth: &AuthUser, course_id: i64) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM lti_registrations \
         WHERE course_id = $1 AND workflow_state <> 'deleted' LIMIT 1",
    )
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// The resource link of a course — by id or by `lookup_uuid:`.
async fn load_link(db: &PgPool, course_id: i64, id: &str) -> Result<ResourceLink, ApiError> {
    let sql = format!(
        "SELECT {LINK_COLUMNS} FROM lti_resource_links \
         WHERE (context_type = 'Course' AND context_id = $1 \
                OR context_type = 'Assignment' AND context_id IN \
                    (SELECT id FROM assignments WHERE course_id = $1)) \
           AND (id::text = $2 OR lookup_uuid::text = $3) LIMIT 1"
    );
    let lookup_uuid = id.strip_prefix("lookup_uuid:").unwrap_or(id);
    let link: Option<ResourceLink> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(id)
        .bind(lookup_uuid)
        .fetch_optional(db)
        .await?;
    link.ok_or(ApiError::NotFound)
}

/// The resource link of an id.
async fn load_link_by_id(db: &PgPool, id: i64) -> Result<ResourceLink, ApiError> {
    let sql = format!("SELECT {LINK_COLUMNS} FROM lti_resource_links WHERE id = $1");
    let link: Option<ResourceLink> = sqlx::query_as(&sql).bind(id).fetch_optional(db).await?;
    link.ok_or(ApiError::NotFound)
}

/// The tool of a launch URL — Canvas `Lti::ToolFinder.from_url`.
async fn tool_for_url(db: &PgPool, course_id: i64, url: &str) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM external_tools \
         WHERE url = $1 AND workflow_state = 'active' \
           AND (course_id = $2 OR account_id = (SELECT account_id FROM courses WHERE id = $2)) \
         LIMIT 1",
    )
    .bind(url)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|row| row.0))
}

/// Store a resource link.
#[allow(clippy::too_many_arguments)]
async fn insert_link(
    db: &PgPool,
    course_id: i64,
    tool_id: i64,
    custom: Option<Value>,
    url: Option<String>,
    title: Option<String>,
    lookup_uuid: Option<String>,
    resource_link_uuid: Option<String>,
    lti_1_1_id: Option<String>,
) -> Result<ResourceLink, ApiError> {
    let root_account_id: i64 = sqlx::query_scalar("SELECT account_id FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_one(db)
        .await?;
    let lookup_uuid = lookup_uuid.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let resource_link_uuid = resource_link_uuid.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let sql = format!(
        "INSERT INTO lti_resource_links \
             (created_at, updated_at, context_external_tool_id, workflow_state, root_account_id, \
              context_id, context_type, custom, lookup_uuid, resource_link_uuid, url, \
              lti_1_1_id, title) \
         VALUES (now(), now(), $1, 'active', $2, $3, 'Course', $4, $5, $6, $7, $8, $9) \
         RETURNING {LINK_COLUMNS}"
    );
    let link: ResourceLink = sqlx::query_as(&sql)
        .bind(tool_id)
        .bind(root_account_id)
        .bind(course_id)
        .bind(custom.unwrap_or_else(|| json!({})))
        .bind(&lookup_uuid)
        .bind(&resource_link_uuid)
        .bind(&url)
        .bind(&lti_1_1_id)
        .bind(&title)
        .fetch_one(db)
        .await?;
    Ok(link)
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::Lti::ResourceLink#lti_resource_link_json`.
fn link_json(link: &ResourceLink) -> Value {
    let resource_type = if link.context_type == "Assignment" {
        "assignment"
    } else {
        "rich_content"
    };
    json!({
        "id": link.id,
        "context_id": link.context_id,
        "context_type": link.context_type,
        "workflow_state": link.workflow_state,
        "root_account_id": link.root_account_id,
        "context_external_tool_id": link.context_external_tool_id,
        "lookup_uuid": link.lookup_uuid,
        "resource_link_uuid": link.resource_link_uuid,
        "url": link.url,
        "lti_1_1_id": link.lti_1_1_id,
        "title": link.title,
        "custom": link.custom,
        "created_at": link.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "updated_at": link.updated_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "resource_type": resource_type,
        "canvas_launch_url": format!(
            "/courses/{}/external_tools/{}?context_type=Course&context_id={}",
            link.context_id, link.lookup_uuid, link.context_id
        ),
    })
}

/// Canvas `value_to_boolean`.
fn value_to_boolean(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|value| value.trim().to_lowercase()),
        Some(ref value) if value == "1" || value == "true" || value == "yes" || value == "on"
    )
}
