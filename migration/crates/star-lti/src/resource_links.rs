//! LTI Resource Links API — the port of Canvas
//! `app/controllers/lti/resource_links_controller.rb`.
//!
//! Routes (Canvas `config/routes.rb`, `ApiRouteSet::V1.draw` under
//! `/api/v1`):
//! - `GET    /courses/:course_id/lti_resource_links`      → `index`
//! - `POST   /courses/:course_id/lti_resource_links`      → `create`
//! - `POST   /courses/:course_id/lti_resource_links/bulk` → `bulk_create`
//! - `GET    /courses/:course_id/lti_resource_links/:id`  → `show`
//! - `PUT    /courses/:course_id/lti_resource_links/:id`  → `update`
//! - `DELETE /courses/:course_id/lti_resource_links/:id`  → `destroy`
//!
//! Permission order: `manage_assignments_add` for every action (Canvas
//! `require_manage_assignments_add_permission`), plus `manage_lti_add` for
//! the write actions. The port models both with the course teacher /
//! account-admin checks used across the crate (the port's teacher role
//! carries both permissions by default, like Canvas' built-in role).
//!
//! The `id` parameter accepts Canvas' three formats: the numeric id,
//! `resource_link_uuid:<uuid>` and `lookup_uuid:<uuid>`.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `MAX_BULK_CREATE`.
const MAX_BULK_CREATE: usize = 100;

/// Canvas `Api.per_page_for(self, default: 50)`.
const DEFAULT_PER_PAGE: i64 = 50;
const MAX_PER_PAGE: i64 = 100;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{course_id}/lti_resource_links",
            get(index).post(create),
        )
        .route(
            "/courses/{course_id}/lti_resource_links/bulk",
            post(bulk_create),
        )
        .route(
            "/courses/{course_id}/lti_resource_links/{id}",
            get(show).put(update).delete(destroy),
        )
}

// ---------------------------------------------------------------------------
// Row model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct ResourceLinkRow {
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

const LINK_COLUMNS: &str = "id, context_external_tool_id, workflow_state, root_account_id, \
     context_id, context_type, custom, lookup_uuid, resource_link_uuid, url, lti_1_1_id, \
     title, created_at, updated_at";

/// The slice of `external_tools` the tool finder needs.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ToolRow {
    id: i64,
    url: String,
    domain: Option<String>,
    account_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Shared scope helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ScopeQuery {
    include_deleted: Option<String>,
    #[serde(default)]
    per_page: Option<i64>,
    #[serde(default)]
    page: Option<i64>,
}

fn include_deleted(flag: &Option<String>) -> bool {
    matches!(flag.as_deref(), Some(v) if v != "false" && v != "0" && !v.is_empty())
}

/// Canvas `require_context_instrumented` → `require_course_context`.
async fn require_course(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
    if row.is_none() {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// Canvas `require_manage_assignments_add_permission` /
/// `require_manage_lti_add_permission` — the port's course-management check
/// (teacher in the course, admin of the course's account chain, or site
/// admin). Shared with the deep-linking module (Canvas
/// `require_context_update_rights`).
pub(crate) async fn can_manage_course(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let teacher: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 AND role = 'teacher' \
         LIMIT 1",
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if teacher.is_some() {
        return Ok(true);
    }
    let account: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    if let Some((Some(account_id),)) = account {
        return crate::can_manage_account(pool, auth, account_id).await;
    }
    Ok(false)
}

/// Canvas `base_scope`: the course's own links plus links on the course's
/// assignments, `active` unless `include_deleted`.
async fn fetch_scope(
    pool: &PgPool,
    course_id: i64,
    with_deleted: bool,
    id_param: &str,
) -> Result<Option<ResourceLinkRow>, ApiError> {
    let state_clause = if with_deleted {
        ""
    } else {
        "AND workflow_state = 'active'"
    };
    let base = format!(
        "SELECT {LINK_COLUMNS} FROM lti_resource_links \
         WHERE ((context_type = 'Course' AND context_id = $1) \
             OR (context_type = 'Assignment' AND context_id IN \
                 (SELECT id FROM assignments WHERE course_id = $1))) \
         {state_clause}"
    );
    // Canvas `resource_link`: the id, or a lookup by one of the uuid
    // formats (`find_by!` → 404 when missing, including malformed ids).
    let row: Option<ResourceLinkRow> = if let Some(uuid) = id_param.strip_prefix("lookup_uuid:") {
        sqlx::query_as(&format!("{base} AND lookup_uuid = $2 LIMIT 1"))
            .bind(course_id)
            .bind(uuid)
            .fetch_optional(pool)
            .await?
    } else if let Some(uuid) = id_param.strip_prefix("resource_link_uuid:") {
        sqlx::query_as(&format!("{base} AND resource_link_uuid = $2 LIMIT 1"))
            .bind(course_id)
            .bind(uuid)
            .fetch_optional(pool)
            .await?
    } else {
        let numeric: i64 = id_param.parse().map_err(|_| ApiError::NotFound)?;
        sqlx::query_as(&format!("{base} AND id = $2 LIMIT 1"))
            .bind(course_id)
            .bind(numeric)
            .fetch_optional(pool)
            .await?
    };
    Ok(row)
}

// ---------------------------------------------------------------------------
// JSON shape
// ---------------------------------------------------------------------------

/// Canvas `lti_resource_link_json`
/// (`lib/api/v1/lti/resource_link.rb`): the model attributes plus
/// `resource_type`, `canvas_launch_url` and, for module items, the
/// associated content fields.
fn link_json(
    link: &ResourceLinkRow,
    resource_type: &str,
    module_item_id: Option<i64>,
    headers: &HeaderMap,
    course_id: i64,
) -> Value {
    let host = crate::lti_authorization::request_host(headers);
    let canvas_launch_url = if resource_type == "assignment" {
        // Canvas: course_assignment_url(assignment.course, assignment).
        format!(
            "http://{host}/courses/{course_id}/assignments/{}",
            link.context_id
        )
    } else {
        // Canvas: retrieve_course_external_tools_url(course, resource_link_lookup_uuid:).
        format!(
            "http://{host}/courses/{course_id}/external_tools/retrieve?resource_link_lookup_uuid={}",
            link.lookup_uuid
        )
    };

    let mut payload = json!({
        "id": link.id,
        "context_id": link.context_id,
        "context_type": link.context_type,
        "context_external_tool_id": link.context_external_tool_id,
        "custom": link.custom.clone().unwrap_or(Value::Null),
        "lookup_uuid": link.lookup_uuid,
        "resource_link_uuid": link.resource_link_uuid,
        "title": link.title,
        "url": link.url,
        "lti_1_1_id": link.lti_1_1_id,
        "created_at": link.created_at.to_rfc3339(),
        "updated_at": link.updated_at.to_rfc3339(),
        "workflow_state": link.workflow_state,
        "resource_type": resource_type,
        "canvas_launch_url": canvas_launch_url,
    });
    if let Some(tag_id) = module_item_id {
        payload["associated_content_type"] = json!("ModuleItem");
        payload["associated_content_id"] = json!(tag_id);
    }
    payload
}

/// Canvas `resource_link_type` — assignment links, module items,
/// collaborations, else rich content. The lookups are batched for a page of
/// links.
struct TypeContext {
    module_item_ids: std::collections::HashMap<i64, i64>,
    collaboration_uuids: std::collections::HashSet<String>,
    include_deleted: bool,
}

impl TypeContext {
    async fn load(
        pool: &PgPool,
        course_id: i64,
        links: &[ResourceLinkRow],
        include_deleted: bool,
    ) -> Result<Self, ApiError> {
        if links.is_empty() {
            return Ok(Self {
                module_item_ids: Default::default(),
                collaboration_uuids: Default::default(),
                include_deleted,
            });
        }
        let ids: Vec<i64> = links.iter().map(|l| l.id).collect();
        let uuids: Vec<String> = links.iter().map(|l| l.lookup_uuid.clone()).collect();
        let state_clause = if include_deleted {
            ""
        } else {
            "AND workflow_state <> 'deleted'"
        };
        // Canvas `@context.context_module_tags` with
        // `associated_asset_type = 'Lti::ResourceLink'`.
        let tags: Vec<(i64, i64)> = sqlx::query_as(&format!(
            "SELECT associated_asset_id, id FROM content_tags \
                 WHERE context_id = $1 AND context_type = 'Course' \
                 AND tag_type = 'context_module' AND associated_asset_type = 'Lti::ResourceLink' \
                 AND associated_asset_id = ANY($2) {state_clause}"
        ))
        .bind(course_id)
        .bind(&ids)
        .fetch_all(pool)
        .await?;
        let module_item_ids: std::collections::HashMap<i64, i64> = tags.into_iter().collect();

        // Canvas `collaboration_lookup_uuids`
        // (`ExternalToolCollaboration.where(context: @context)`).
        let collabs: Vec<(String,)> = sqlx::query_as(&format!(
            "SELECT resource_link_lookup_uuid FROM collaborations \
                 WHERE context_id = $1 AND context_type = 'Course' \
                 AND resource_link_lookup_uuid = ANY($2) {state_clause}"
        ))
        .bind(course_id)
        .bind(&uuids)
        .fetch_all(pool)
        .await?;
        let collaboration_uuids: std::collections::HashSet<String> =
            collabs.into_iter().map(|(uuid,)| uuid).collect();

        Ok(Self {
            module_item_ids,
            collaboration_uuids,
            include_deleted,
        })
    }

    fn classify(&self, link: &ResourceLinkRow) -> (&'static str, Option<i64>) {
        if link.context_type == "Assignment" {
            return ("assignment", None);
        }
        if let Some(tag_id) = self.module_item_ids.get(&link.id) {
            return ("module_item", Some(*tag_id));
        }
        if self.collaboration_uuids.contains(&link.lookup_uuid) {
            return ("collaboration", None);
        }
        ("rich_content", None)
    }
}

// ---------------------------------------------------------------------------
// index / show
// ---------------------------------------------------------------------------

async fn index(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<ScopeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let with_deleted = include_deleted(&query.include_deleted);
    let per_page = query
        .per_page
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = query.page.unwrap_or(1).max(1);
    let offset = (page - 1) * per_page;
    let state_clause = if with_deleted {
        ""
    } else {
        "AND workflow_state = 'active'"
    };
    let links: Vec<ResourceLinkRow> = sqlx::query_as(&format!(
        "SELECT {LINK_COLUMNS} FROM lti_resource_links \
         WHERE ((context_type = 'Course' AND context_id = $1) \
             OR (context_type = 'Assignment' AND context_id IN \
                 (SELECT id FROM assignments WHERE course_id = $1))) \
         {state_clause} \
         ORDER BY created_at, id LIMIT $2 OFFSET $3"
    ))
    .bind(course_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let types = TypeContext::load(&state.db, course_id, &links, with_deleted).await?;
    let items: Vec<Value> = links
        .iter()
        .map(|link| {
            let (resource_type, tag_id) = types.classify(link);
            link_json(link, resource_type, tag_id, &headers, course_id)
        })
        .collect();
    Ok((StatusCode::OK, Json(Value::Array(items))).into_response())
}

async fn show(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, String)>,
    auth: AuthUser,
    Query(query): Query<ScopeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let link = fetch_scope(
        &state.db,
        course_id,
        include_deleted(&query.include_deleted),
        &id,
    )
    .await?
    .ok_or(ApiError::NotFound)?;
    let types = TypeContext::load(&state.db, course_id, std::slice::from_ref(&link), false).await?;
    let (resource_type, tag_id) = types.classify(&link);
    Ok((
        StatusCode::OK,
        Json(link_json(&link, resource_type, tag_id, &headers, course_id)),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// create / bulk_create
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateLinkBody {
    url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    custom: Option<Value>,
}

fn validation_error(code: &str, message: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "errors": [{ "code": code, "message": message }] })),
    )
        .into_response()
}

/// Canvas `valid_custom?` — null, or an object whose keys and values are
/// all strings (JSON object keys are always strings, so only the values are
/// checked; numbers, booleans, arrays and scalars are invalid, matching
/// Canvas' `to_unsafe_h.all? { |k, v| k.is_a?(String) && v.is_a?(String) }`).
fn valid_custom(custom: &Option<Value>) -> bool {
    match custom {
        None | Some(Value::Null) => true,
        Some(Value::Object(map)) => map.values().all(|v| v.as_str().is_some()),
        _ => false,
    }
}

/// Canvas `Lti::DeepLinkingUtil.validate_custom_params` — drop entries that
/// are not string key / scalar value pairs.
fn filter_custom(custom: Option<Value>) -> Value {
    match custom {
        Some(Value::Object(map)) => Value::Object(
            map.into_iter()
                .filter(|(k, v)| {
                    !k.is_empty()
                        && matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_))
                })
                .collect::<Map<String, Value>>(),
        ),
        _ => Value::Null,
    }
}

/// Canvas `valid_url?` — nil passes (the tool lookup reports it), anything
/// parseable passes.
fn valid_url(url: &Option<String>) -> bool {
    match url {
        None => true,
        Some(url) => url::Url::parse(url).is_ok(),
    }
}

/// Canvas `Lti::ToolFinder.from_url(url, context, only_1_3: true)`: the
/// LTI 1.3 tools available in the course context chain (course first, then
/// its account chain), preferring an exact URL match, then a domain match.
async fn find_tool_by_url(
    pool: &PgPool,
    course_id: i64,
    url: &Option<String>,
) -> Result<Option<ToolRow>, ApiError> {
    let Some(url) = url.as_deref() else {
        return Ok(None);
    };
    let account_chain = course_account_chain(pool, course_id).await?;
    let tools: Vec<ToolRow> = sqlx::query_as(
        "SELECT id, url, domain, account_id FROM external_tools \
         WHERE workflow_state <> 'deleted' AND lti_version = '1.3' \
         AND ((context_type = 'Course' AND context_id = $1) \
           OR (context_type = 'Account' AND context_id = ANY($2))) \
         ORDER BY (context_type = 'Course') DESC, id",
    )
    .bind(course_id)
    .bind(&account_chain)
    .fetch_all(pool)
    .await?;
    if tools.is_empty() {
        return Ok(None);
    }

    let parsed = url::Url::parse(url).ok();
    let host = parsed.as_ref().map(|u| u.host_str().unwrap_or_default());

    // Exact URL match first (Canvas `matches_url?`).
    if let Some(tool) = tools.iter().find(|t| t.url == url) {
        return Ok(Some(tool.clone()));
    }
    // Domain match (Canvas `matches_host?`): the tool's domain, or the host
    // of the tool's own URL.
    if let Some(host) = host {
        if let Some(tool) = tools.iter().find(|t| {
            let domain = t.domain.as_deref().unwrap_or_default();
            (!domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}"))))
                || url::Url::parse(&t.url)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| h == host))
                    .unwrap_or(false)
        }) {
            return Ok(Some(tool.clone()));
        }
    }
    Ok(None)
}

/// The course's account plus every ancestor — Canvas
/// `Lti::ToolFinderUtils.contexts_to_search`.
async fn course_account_chain(pool: &PgPool, course_id: i64) -> Result<Vec<i64>, ApiError> {
    let mut chain = Vec::new();
    let mut current: Option<i64> =
        sqlx::query_scalar("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let mut depth = 0;
    while let Some(id) = current {
        if depth >= 20 {
            break;
        }
        depth += 1;
        chain.push(id);
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        current = parent.and_then(|(p,)| p);
    }
    Ok(chain)
}

async fn insert_link(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    tool: &ToolRow,
    url: Option<&str>,
    title: Option<&str>,
    custom: Value,
    root_account_id: i64,
) -> Result<ResourceLinkRow, ApiError> {
    let row: ResourceLinkRow = sqlx::query_as(&format!(
        "INSERT INTO lti_resource_links \
         (context_external_tool_id, workflow_state, root_account_id, context_id, context_type, \
          custom, lookup_uuid, resource_link_uuid, url, lti_1_1_id, title, created_at, updated_at) \
         VALUES ($1, 'active', $2, $3, $4, $5, $6, $7, $8, NULL, $9, now(), now()) \
         RETURNING {LINK_COLUMNS}"
    ))
    .bind(tool.id)
    .bind(root_account_id)
    .bind(context_id)
    .bind(context_type)
    .bind(custom)
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(url)
    .bind(title)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

async fn root_account_for_course(pool: &PgPool, course_id: i64) -> Result<i64, ApiError> {
    let chain = course_account_chain(pool, course_id).await?;
    Ok(chain.last().copied().unwrap_or(1))
}

async fn create(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(body): Json<CreateLinkBody>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas before_action order: validate_custom → validate_url → the
    // action's tool lookup.
    if !valid_custom(&body.custom) {
        return Ok(validation_error(
            "invalid_custom",
            "'custom' param must contain key/value pairs",
        ));
    }
    if !valid_url(&body.url) {
        return Ok(validation_error(
            "invalid_url",
            "'url' param must be a valid URL",
        ));
    }
    let Some(tool) = find_tool_by_url(&state.db, course_id, &body.url).await? else {
        return Ok(validation_error(
            "invalid_url",
            "No tool found for the provided URL",
        ));
    };
    // Canvas `set_root_account`: the link's root account comes from the
    // tool, falling back to the course's account chain.
    let root_account = tool
        .account_id
        .unwrap_or(root_account_for_course(&state.db, course_id).await?);
    let link = insert_link(
        &state.db,
        "Course",
        course_id,
        &tool,
        body.url.as_deref(),
        body.title.as_deref(),
        filter_custom(body.custom.clone()),
        root_account,
    )
    .await?;
    // Canvas renders rich-content JSON for API-created links.
    Ok((
        StatusCode::OK,
        Json(link_json(&link, "rich_content", None, &headers, course_id)),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
struct BulkLinkBody {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    custom: Option<Value>,
}

async fn bulk_create(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(items): Json<Vec<BulkLinkBody>>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    if items.len() > MAX_BULK_CREATE {
        return Ok(validation_error(
            "invalid_request",
            &format!("Cannot create more than {MAX_BULK_CREATE} resource links at once"),
        ));
    }

    // Canvas validates each item (url → custom → tool) and creates nothing
    // when any item fails.
    let mut errors: Vec<Value> = Vec::new();
    let mut tools: Vec<Option<ToolRow>> = Vec::new();
    for item in &items {
        if !valid_url(&item.url) {
            errors.push(json!({
                "value": item.url, "error": "invalid_url",
                "message": "'url' param must be a valid URL",
            }));
            tools.push(None);
            continue;
        }
        if !valid_custom(&item.custom) {
            errors.push(json!({
                "value": item.custom, "error": "invalid_custom",
                "message": "'custom' param must contain key/value pairs",
            }));
            tools.push(None);
            continue;
        }
        match find_tool_by_url(&state.db, course_id, &item.url).await? {
            Some(tool) => tools.push(Some(tool)),
            None => {
                errors.push(json!({
                    "value": item.url, "error": "invalid_url",
                    "message": "No tool found for the provided URL",
                }));
                tools.push(None);
            }
        }
    }
    if !errors.is_empty() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "errors": errors })),
        )
            .into_response());
    }

    let course_root = root_account_for_course(&state.db, course_id).await?;
    let mut created: Vec<Value> = Vec::new();
    for (item, tool) in items.iter().zip(tools.iter()) {
        let tool = tool.as_ref().expect("validated above");
        let root_account = tool.account_id.unwrap_or(course_root);
        let link = insert_link(
            &state.db,
            "Course",
            course_id,
            tool,
            item.url.as_deref(),
            item.title.as_deref(),
            filter_custom(item.custom.clone()),
            root_account,
        )
        .await?;
        created.push(link_json(&link, "rich_content", None, &headers, course_id));
    }
    Ok((StatusCode::OK, Json(Value::Array(created))).into_response())
}

// ---------------------------------------------------------------------------
// update / destroy
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UpdateLinkBody {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    custom: Option<Value>,
    #[serde(default)]
    context_external_tool_id: Option<i64>,
    #[serde(default)]
    include_deleted: Option<String>,
}

/// Does the URL point at the tool (Canvas `matches_host?`).
fn url_matches_tool(tool: &ToolRow, url: &str) -> bool {
    let Some(parsed) = url::Url::parse(url).ok() else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let domain = tool.domain.as_deref().unwrap_or_default();
    (!domain.is_empty() && (host == domain || host.ends_with(&format!(".{domain}"))))
        || url::Url::parse(&tool.url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h == host))
            .unwrap_or(false)
}

async fn fetch_tool_by_id(pool: &PgPool, tool_id: i64) -> Result<Option<ToolRow>, ApiError> {
    let row: Option<ToolRow> =
        sqlx::query_as("SELECT id, url, domain, account_id FROM external_tools WHERE id = $1")
            .bind(tool_id)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

async fn update(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, String)>,
    auth: AuthUser,
    Query(query): Query<ScopeQuery>,
    headers: HeaderMap,
    Json(body): Json<UpdateLinkBody>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas before_action order: validate_custom → validate_url → (find
    // link inside validate_tool) → validate_tool → validate_url_match_tool.
    if !valid_custom(&body.custom) {
        return Ok(validation_error(
            "invalid_custom",
            "'custom' param must contain key/value pairs",
        ));
    }
    if !valid_url(&body.url) {
        return Ok(validation_error(
            "invalid_url",
            "'url' param must be a valid URL",
        ));
    }
    let with_deleted =
        include_deleted(&query.include_deleted) || include_deleted(&body.include_deleted);
    let link = fetch_scope(&state.db, course_id, with_deleted, &id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // validate_tool: a replacement tool must exist (Canvas also requires the
    // same root account; `external_tools` has no root account column, so the
    // port checks existence only).
    let new_tool = match body.context_external_tool_id {
        Some(tool_id) => match fetch_tool_by_id(&state.db, tool_id).await? {
            Some(tool) => Some(tool),
            None => {
                return Ok(validation_error(
                    "context_external_tool_id",
                    "'context_external_tool_id' param must be a valid ContextExternalTool ID",
                ))
            }
        },
        None => None,
    };

    // validate_url_match_tool: the effective URL must match the effective
    // tool (the new tool, or the link's original tool).
    if !(body.url.is_none() && new_tool.is_none()) {
        let effective_url = body
            .url
            .clone()
            .or_else(|| link.url.clone())
            .unwrap_or_default();
        let effective_tool = match &new_tool {
            Some(tool) => Some(tool.clone()),
            None => fetch_tool_by_id(&state.db, link.context_external_tool_id).await?,
        };
        if let (false, Some(tool)) = (effective_url.is_empty(), effective_tool) {
            if !url_matches_tool(&tool, &effective_url) {
                return Ok(validation_error(
                    "url_tool_mismatch",
                    "'url' param must match the tool associated with 'context_external_tool_id'",
                ));
            }
        }
    }

    // update! — only the permitted params.
    let updated: ResourceLinkRow = sqlx::query_as(&format!(
        "UPDATE lti_resource_links SET \
         url = COALESCE($2, url), \
         custom = COALESCE($3, custom), \
         context_external_tool_id = COALESCE($4, context_external_tool_id), \
         updated_at = now() \
         WHERE id = $1 RETURNING {LINK_COLUMNS}"
    ))
    .bind(link.id)
    .bind(&body.url)
    .bind(filter_custom(body.custom.clone()))
    .bind(body.context_external_tool_id)
    .fetch_one(&state.db)
    .await?;

    let types = TypeContext::load(
        &state.db,
        course_id,
        std::slice::from_ref(&updated),
        with_deleted,
    )
    .await?;
    let (resource_type, tag_id) = types.classify(&updated);
    Ok((
        StatusCode::OK,
        Json(link_json(
            &updated,
            resource_type,
            tag_id,
            &headers,
            course_id,
        )),
    )
        .into_response())
}

async fn destroy(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, String)>,
    auth: AuthUser,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas deletes the include_deleted param first: deleted links are
    // never addressable by destroy.
    let link = fetch_scope(&state.db, course_id, false, &id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Only rich-content links can be deleted.
    let types = TypeContext::load(&state.db, course_id, std::slice::from_ref(&link), false).await?;
    let (resource_type, _) = types.classify(&link);
    if resource_type != "rich_content" {
        return Ok(validation_error(
            "invalid_resource_link",
            "Cannot delete resource links associated with Assignments, Module Items, or Collaborations",
        ));
    }

    let deleted: ResourceLinkRow = sqlx::query_as(&format!(
        "UPDATE lti_resource_links SET workflow_state = 'deleted', updated_at = now() \
         WHERE id = $1 RETURNING {LINK_COLUMNS}"
    ))
    .bind(link.id)
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::OK,
        Json(link_json(
            &deleted,
            "rich_content",
            None,
            &headers,
            course_id,
        )),
    )
        .into_response())
}
