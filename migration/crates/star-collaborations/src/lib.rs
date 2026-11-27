//! star-collaborations — course/group collaborations (Canvas
//! CollaborationsController: api_index / members / potential_collaborators /
//! destroy) — issue #91, spec `specs/p5-rest-collaborations.md`.
//!
//! Endpoints (spec §2, 7 routes):
//! - GET    /courses/{course_id}/collaborations            — api_index (course)
//! - GET    /groups/{group_id}/collaborations             — api_index (group twin)
//! - GET    /collaborations/{id}/members                  — members
//! - GET    /courses/{course_id}/potential_collaborators  — potential_collaborators
//! - GET    /groups/{group_id}/potential_collaborators    — potential_collaborators (twin)
//! - DELETE /courses/{course_id}/collaborations/{id}      — destroy
//! - DELETE /groups/{group_id}/collaborations/{id}        — destroy (twin)
//!
//! Behavior contract (spec §6 + Canvas source, verified this issue):
//! - Guard order: 401 anonymous → 404 unknown context (require_context;
//!   members instead resolves a GLOBAL collaboration then adopts its context)
//!   → 403 `require_collaborations_configured` → 403 failed ability → tab
//!   check. `require_collaborations_configured`
//!   (app/controllers/collaborations_controller.rb:132, :446-451) refuses a
//!   context that owns no collaboration provider; the API twin answers 403
//!   `{"errors": ["Collaborations have not been enabled for this Canvas
//!   site"]}` (spec §8 Q2).
//! - api_index: `:read` on the context, then the collaborations tab must be
//!   enabled, else 404 `{"message": "That page has been disabled for this
//!   course"}` (Canvas render_tab_disabled). Rows: active
//!   (`workflow_state <> 'deleted'`) AND `type = 'ExternalToolCollaboration'`.
//!   Non-managers only see rows they authored, are a user-collaborator on,
//!   or (course context) are in a collaborating group of the course.
//!   BARE ARRAY response (Canvas `render json: collaborations.map`).
//! - members: `:read` on the collaboration; paginated flat array of
//!   collaborator_json {id, type ("group" iff group_id else "user"),
//!   name (user.sortable_name || group.name), collaborator_id
//!   (user.id || group.id)}.
//! - potential_collaborators: `:read_roster` on the context. Course →
//!   distinct enrolled users (staff only — a plain student is refused);
//!   group → members + parent-course active teacher/ta/designer enrollments
//!   (Canvas group#potential_collaborators: participating_users +
//!   context.participating_admins). Ordered by sortable_name, paginated,
//!   `[User]` shape.
//! - destroy: scoped `context.collaborations.find(id)` (cross-context id →
//!   404) → `:delete` → soft delete (`workflow_state='deleted'`,
//!   `deleted_at=now()`) → 200 + the row JSON (Canvas
//!   `format.json { render json: @collaboration }`).
//! - Pagination: Canvas `Api` defaults — per_page 10, clamp [1, 100].
//!
//! Serializer whitelist (lib/api/v1/collaboration.rb): id, collaboration_type,
//! document_id, user_id, context_id, context_type, url, created_at,
//! updated_at, description, title, type, update_url + user_name and
//! permissions {update, delete}.
//!
//! `update_url` is `data["updateUrl"]` (Canvas
//! app/models/external_tool_collaboration.rb:26), not the launch url plus
//! "/edit".

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Collaborations-domain errors. `TabDisabled` carries Canvas'
/// `render_tab_disabled` body (`{"message": "That page has been disabled for
/// this course"}`, application_controller.rb:1284-1295); `NotConfigured`
/// carries the API twin of Canvas' `require_collaborations_configured` flash
/// error (collaborations_controller.rb:446-451), pinned by
/// `specs/p5-rest-collaborations.md` §8 Q2; the plain variants map to the
/// same status codes as `star_common::ApiError`.
enum CollabError {
    NotFound,
    Forbidden,
    NotConfigured,
    TabDisabled,
    Db,
}

impl From<sqlx::Error> for CollabError {
    fn from(e: sqlx::Error) -> Self {
        eprintln!("database error: {e}");
        CollabError::Db
    }
}

impl IntoResponse for CollabError {
    fn into_response(self) -> Response {
        // Canvas answers an API error with JSON
        // (app/controllers/application_controller.rb:2296), never with plain
        // text.
        match self {
            CollabError::NotFound => star_common::ApiError::NotFound.into_response(),
            CollabError::Forbidden => star_common::ApiError::Forbidden.into_response(),
            // Canvas flashes "Collaborations have not been enabled for this
            // Canvas site" and redirects; the API twin answers 403 with the
            // same sentence in an `errors` array (spec §8 Q2).
            CollabError::NotConfigured => star_common::ApiError::canvas_json(
                StatusCode::FORBIDDEN,
                json!({"errors": ["Collaborations have not been enabled for this Canvas site"]}),
            )
            .into_response(),
            CollabError::TabDisabled => star_common::ApiError::canvas_json(
                StatusCode::NOT_FOUND,
                json!({"message": "That page has been disabled for this course"}),
            )
            .into_response(),
            CollabError::Db => star_common::ApiError::Db("database error".into()).into_response(),
        }
    }
}

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// Pagination query params (Canvas `Api.paginate`): 1-based page, per_page
/// defaulting to Canvas' PER_PAGE=10 and clamped to [1, 100].
#[derive(Debug, Default, Deserialize)]
struct PaginationParams {
    page: Option<i64>,
    per_page: Option<i64>,
}

fn pagination_values(params: &PaginationParams) -> (i64, i64, i64) {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(10).clamp(1, 100);
    // Saturating arithmetic: an extreme `page` must not overflow the offset.
    let offset = page.saturating_sub(1).saturating_mul(per_page);
    (page, per_page, offset)
}

/// Optional `delete_doc` boolean (string "true"/"false" or JSON bool).
fn bool_param(query: Option<&str>) -> Option<bool> {
    match query {
        Some("true") | Some("1") => Some(true),
        Some("false") | Some("0") => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Row shapes (only the columns the API serializes; snapshot 0223 layout)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct CollaborationRow {
    id: i64,
    user_id: Option<i64>,
    context_id: i64,
    context_type: String,
    collaboration_type: Option<String>,
    document_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    url: Option<String>,
    r#type: Option<String>,
    data: Option<String>,
    created_at: Option<NaiveDateTime>,
    updated_at: Option<NaiveDateTime>,
    workflow_state: Option<String>,
    deleted_at: Option<NaiveDateTime>,
}

const COLLABORATION_COLUMNS: &str = "id, user_id, context_id, context_type, \
     collaboration_type, document_id, title, description, url, type, data, \
     created_at, updated_at, workflow_state, deleted_at";

// ---------------------------------------------------------------------------
// Context resolution
// ---------------------------------------------------------------------------

/// A resolved course or group context (Canvas `require_context`): unknown or
/// soft-deleted context → 404 before any ability check.
#[derive(Debug, Clone, Copy)]
struct ContextRef {
    id: i64,
    kind: ContextKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ContextKind {
    Course,
    Group,
}

impl ContextKind {
    fn as_str(self) -> &'static str {
        match self {
            ContextKind::Course => "Course",
            ContextKind::Group => "Group",
        }
    }
}

async fn load_course(pool: &PgPool, course_id: i64) -> Result<ContextRef, CollabError> {
    let exists: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    exists.ok_or(CollabError::NotFound).map(|_| ContextRef {
        id: course_id,
        kind: ContextKind::Course,
    })
}

async fn load_group(pool: &PgPool, group_id: i64) -> Result<ContextRef, CollabError> {
    let exists: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(group_id)
            .fetch_optional(pool)
            .await?;
    exists.ok_or(CollabError::NotFound).map(|_| ContextRef {
        id: group_id,
        kind: ContextKind::Group,
    })
}

// ---------------------------------------------------------------------------
// Abilities (star-lms role model; Canvas authorized_action order)
// ---------------------------------------------------------------------------

/// The parent course of a group (Canvas `Group#context`).
async fn group_parent_course_id(pool: &PgPool, group_id: i64) -> Result<Option<i64>, CollabError> {
    let row: Option<(Option<i64>,)> = sqlx::query_as("SELECT course_id FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|(course_id,)| course_id))
}

/// Canvas grants `manage_groups_add` / `manage_groups_manage` and
/// `manage_course_content_edit` / `manage_course_content_delete` to
/// Ta/Designer/Teacher enrollments and AccountAdmin
/// (config/initializers/permissions_registry.rb:979-995 and :1165-1180).
/// star-lms models roles through enrollments, so "course staff" is an active
/// teacher/ta/designer enrollment in the course.
async fn course_staff(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, CollabError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM enrollments \
         WHERE course_id = $1 AND user_id = $2 AND workflow_state = 'active' \
         AND role IN ('teacher','ta','designer')",
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// An accepted membership in the group (Canvas `Group#has_member?`).
async fn group_member(pool: &PgPool, auth: &AuthUser, group_id: i64) -> Result<bool, CollabError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM group_memberships \
         WHERE group_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// `:read` on the context: any active enrollment in the course or a site
/// admin; for a group a member of the group or an admin of the parent
/// course. Students pass the plain `:read` branch (enrollment), handled
/// separately per route.
async fn context_read(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: ContextRef,
) -> Result<bool, CollabError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    match ctx.kind {
        ContextKind::Course => {
            let row: Option<(i64,)> = sqlx::query_as(
                "SELECT 1::bigint FROM enrollments \
                 WHERE course_id = $1 AND user_id = $2 AND workflow_state = 'active'",
            )
            .bind(ctx.id)
            .bind(auth.id)
            .fetch_optional(pool)
            .await?;
            Ok(row.is_some())
        }
        ContextKind::Group => {
            if group_member(pool, auth, ctx.id).await? {
                return Ok(true);
            }
            // Canvas Group policy: the parent course's `manage_groups_add`
            // grants `:read` (and `:read_files`, `:create`) on the group
            // (app/models/group.rb:711-717) — a course teacher who is not a
            // group member can still read the group's collaborations.
            match group_parent_course_id(pool, ctx.id).await? {
                Some(course_id) => course_staff(pool, auth, course_id).await,
                None => Ok(false),
            }
        }
    }
}

/// `manage_course_content_edit` / `manage_course_content_delete` on the
/// context (Canvas `RoleOverride::GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS`
/// branch of api_index, and the Collaboration policy): course
/// teacher/ta/designer or admin; group member (star-lms groups have no
/// separate moderator role) or an admin of the parent course — the Group
/// policy grants those rights on the group to the parent course's
/// `manage_groups_manage` holder (app/models/group.rb:719-760).
async fn context_manage_content(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: ContextRef,
) -> Result<bool, CollabError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    match ctx.kind {
        ContextKind::Course => course_staff(pool, auth, ctx.id).await,
        ContextKind::Group => {
            if group_member(pool, auth, ctx.id).await? {
                return Ok(true);
            }
            match group_parent_course_id(pool, ctx.id).await? {
                Some(course_id) => course_staff(pool, auth, course_id).await,
                None => Ok(false),
            }
        }
    }
}

/// `create_collaborations` on the context.
///
/// Canvas' role-override default grants `create_collaborations` to
/// StudentEnrollment as well
/// (config/initializers/permissions_registry.rb:682-690) and the Group
/// policy grants it to every participating group member
/// (app/models/group.rb:634-646). star-lms' role model is enrollment-based
/// and the port treats a plain student as NOT holding `create_collaborations`
/// on a course (documented deviation): only course staff hold it there. On a
/// group the Canvas rule is kept — the members hold it, and so does the
/// parent course's `manage_groups_manage` holder
/// (app/models/group.rb:719-760).
async fn context_create_collaborations(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: ContextRef,
) -> Result<bool, CollabError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    match ctx.kind {
        ContextKind::Course => course_staff(pool, auth, ctx.id).await,
        ContextKind::Group => {
            if group_member(pool, auth, ctx.id).await? {
                return Ok(true);
            }
            match group_parent_course_id(pool, ctx.id).await? {
                Some(course_id) => course_staff(pool, auth, course_id).await,
                None => Ok(false),
            }
        }
    }
}

/// `:read_roster` on the context (potential_collaborators).
///
/// Canvas' `read_roster` ("Users - view list") is a course-staff right for
/// the collaborations roster: a plain enrolled student is refused. Staff
/// (teacher/ta/designer) and site admins pass. For a group a member of the
/// group passes, and so does the parent course's `manage_groups_manage`
/// holder (app/models/group.rb:665-672 and :719-760).
async fn context_read_roster(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: ContextRef,
) -> Result<bool, CollabError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    match ctx.kind {
        ContextKind::Course => course_staff(pool, auth, ctx.id).await,
        ContextKind::Group => {
            if group_member(pool, auth, ctx.id).await? {
                return Ok(true);
            }
            match group_parent_course_id(pool, ctx.id).await? {
                Some(course_id) => course_staff(pool, auth, course_id).await,
                None => Ok(false),
            }
        }
    }
}

/// The context a collaboration row belongs to.
fn row_context(row: &CollaborationRow) -> ContextRef {
    ContextRef {
        id: row.context_id,
        kind: if row.context_type.eq_ignore_ascii_case("Course") {
            ContextKind::Course
        } else {
            ContextKind::Group
        },
    }
}

/// Canvas `:read` on a collaboration row (Collaboration policy,
/// app/models/collaboration.rb:70-97): author, user-collaborator, member of a
/// collaborating group, or context content-manager.
async fn collaboration_readable(
    pool: &PgPool,
    auth: &AuthUser,
    row: &CollaborationRow,
) -> Result<bool, CollabError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    if row.user_id == Some(auth.id) {
        return Ok(true);
    }
    // user collaborator?
    let user_collab: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM collaborators \
         WHERE collaboration_id = $1 AND user_id = $2",
    )
    .bind(row.id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if user_collab.is_some() {
        return Ok(true);
    }
    // member of a collaborating group (course context: any group of the
    // course that collaborates; group context: the group itself)?
    let group_collab: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM collaborators c \
         JOIN group_memberships gm ON gm.group_id = c.group_id \
         WHERE c.collaboration_id = $1 AND gm.user_id = $2 \
         AND gm.workflow_state <> 'deleted'",
    )
    .bind(row.id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if group_collab.is_some() {
        return Ok(true);
    }
    // context content-manager
    context_manage_content(pool, auth, row_context(row)).await
}

/// Canvas `:update` and `:delete` on a collaboration row (Collaboration
/// policy, app/models/collaboration.rb:70-97):
///   * the context grants `manage_course_content_edit` (update) /
///     `manage_course_content_delete` (delete), or
///   * the caller IS the author AND the context grants
///     `create_collaborations`.
///
/// Being the author is therefore NOT enough — the author branch needs
/// `create_collaborations` on the context as well.
async fn collaboration_writable(
    pool: &PgPool,
    auth: &AuthUser,
    row: &CollaborationRow,
) -> Result<bool, CollabError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let ctx = row_context(row);
    if context_manage_content(pool, auth, ctx).await? {
        return Ok(true);
    }
    if row.user_id == Some(auth.id) && context_create_collaborations(pool, auth, ctx).await? {
        return Ok(true);
    }
    Ok(false)
}

/// Canvas `:update` on a collaboration row.
async fn collaboration_updatable(
    pool: &PgPool,
    auth: &AuthUser,
    row: &CollaborationRow,
) -> Result<bool, CollabError> {
    collaboration_writable(pool, auth, row).await
}

/// Canvas `:delete` on a collaboration row: the context content-manager
/// (teacher/ta/designer/admin) or the author holding
/// `create_collaborations` on the context.
async fn collaboration_deletable(
    pool: &PgPool,
    auth: &AuthUser,
    row: &CollaborationRow,
) -> Result<bool, CollabError> {
    collaboration_writable(pool, auth, row).await
}

/// Canvas `require_collaborations_configured`
/// (app/controllers/collaborations_controller.rb:132 and :446-451) refuses
/// every action whose context has no collaboration provider.
///
/// `Collaboration.any_collaborations_configured?`
/// (app/models/collaboration.rb:189-199) is true when the context owns an LTI
/// tool with the `collaboration` placement — Canvas asks
/// `Lti::ContextToolFinder.all_tools_scope_union(context, placements: :collaboration)`,
/// which star-lms models as a `context_external_tools` row joined to a
/// `context_external_tool_placements` row with
/// `placement_type = 'collaboration'`.
///
/// Canvas searches the context chain: a course also sees the tools of its
/// account chain and a group sees the tools of the group AND of its parent
/// course (lib/lti/tool_finder_utils.rb:41-70). star-lms restricts the
/// lookup to the course itself and, for a group, to the group or its parent
/// course.
async fn collaborations_configured(pool: &PgPool, ctx: ContextRef) -> Result<bool, CollabError> {
    let (own_id, own_type, parent_course_id) = match ctx.kind {
        ContextKind::Course => (ctx.id, "Course", None),
        ContextKind::Group => (ctx.id, "Group", group_parent_course_id(pool, ctx.id).await?),
    };
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1::bigint FROM context_external_tools t \
         JOIN context_external_tool_placements p \
           ON p.context_external_tool_id = t.id \
         WHERE p.placement_type = 'collaboration' \
         AND t.workflow_state = 'active' \
         AND ((LOWER(t.context_type) = LOWER($1) AND t.context_id = $2) \
           OR ($3 IS NOT NULL AND LOWER(t.context_type) = 'course' \
               AND t.context_id = $3))",
    )
    .bind(own_type)
    .bind(own_id)
    .bind(parent_course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// The collaborations tab check (Canvas `tab_enabled?`): star-lms courses
/// ship no collaborations tab by default (star-courses
/// DEFAULT_COURSE_TABS), so a seeded, unhidden `course_tabs` row enables it
/// (`collaborations` or `collaborations_new`); groups always have it
/// (star-groups renders both tabs unhidden).
async fn collaborations_tab_enabled(pool: &PgPool, ctx: ContextRef) -> Result<bool, CollabError> {
    match ctx.kind {
        ContextKind::Course => {
            // Canvas' `tab_enabled?` asks the context whether the tab is
            // enabled; a course that says nothing about it has it enabled
            // (`collaborations_controller.rb:181`). Only an explicitly
            // hidden tab disables the route.
            let row: Option<(i64,)> = sqlx::query_as(
                "SELECT 1::bigint FROM course_tabs \
                 WHERE course_id = $1 AND hidden = true \
                 AND tab_id IN ('collaborations', 'collaborations_new')",
            )
            .bind(ctx.id)
            .fetch_optional(pool)
            .await?;
            // A hidden row disables the tab; no row means the tab is enabled.
            Ok(row.is_none())
        }
        ContextKind::Group => Ok(true),
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// potential_collaborators row (users table columns): id, name,
/// sortable_name, short_name, email, role, created_at.
type PotentialUserRow = (
    i64,
    String,
    Option<String>,
    String,
    String,
    String,
    DateTime<Utc>,
);

/// Paginated bare-array response (Canvas `render json: records.map {...}` +
/// `Api.paginate`): the body is the collection itself; the pagination
/// travels in the `x-total-count` / `x-per-page` / `x-page` headers.
fn paginated_array(items: Vec<Value>, page: i64, per_page: i64, total: i64) -> Response {
    // Canvas renders the collection itself: `render json: collaborations.map`
    // (app/controllers/collaborations_controller.rb:141). The pagination
    // travels in the `Link` header (`Api.paginate`, lib/api.rb:412-413),
    // which the response middleware builds from the request.
    let mut response = (StatusCode::OK, Json(items)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        "x-total-count",
        axum::http::HeaderValue::from_str(&total.to_string())
            .unwrap_or(axum::http::HeaderValue::from_static("0")),
    );
    headers.insert(
        "x-per-page",
        axum::http::HeaderValue::from_str(&per_page.to_string())
            .unwrap_or(axum::http::HeaderValue::from_static("0")),
    );
    headers.insert(
        "x-page",
        axum::http::HeaderValue::from_str(&page.to_string())
            .unwrap_or(axum::http::HeaderValue::from_static("1")),
    );
    response
}

/// Collaboration JSON (lib/api/v1/collaboration.rb whitelist + user_name +
/// permissions). `update_url` is `data["updateUrl"]` (Canvas
/// app/models/external_tool_collaboration.rb:26); permissions reflect the
/// caller's update/delete rights (Canvas Collaboration policy,
/// app/models/collaboration.rb:70-97). Timestamps are naive `timestamp`
/// columns in the snapshot schema — serialized as ISO8601 UTC.
async fn collaboration_json(
    pool: &PgPool,
    row: &CollaborationRow,
    auth: &AuthUser,
) -> Result<Value, CollabError> {
    let user_name: Option<(Option<String>,)> = match row.user_id {
        Some(uid) => {
            sqlx::query_as("SELECT name FROM users WHERE id = $1")
                .bind(uid)
                .fetch_optional(pool)
                .await?
        }
        None => None,
    };
    let can_update = collaboration_updatable(pool, auth, row).await?;
    let can_delete = collaboration_deletable(pool, auth, row).await?;
    Ok(json!({
        "id": row.id,
        "collaboration_type": row.collaboration_type,
        "document_id": row.document_id,
        "user_id": row.user_id,
        "context_id": row.context_id,
        "context_type": row.context_type,
        "url": row.url,
        "created_at": row.created_at.map(|t| t.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        "updated_at": row.updated_at.map(|t| t.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        "description": row.description,
        "title": row.title,
        "type": row.r#type,
        "update_url": collaboration_update_url(row),
        "user_name": user_name.map(|(n,)| n).unwrap_or_default(),
        "permissions": {
            "update": can_update,
            "delete": can_delete,
        },
    }))
}

/// Canvas `ExternalToolCollaboration#update_url` is `data["updateUrl"]`
/// (app/models/external_tool_collaboration.rb:26) — the LTI launch url plus
/// "/edit" is NOT what Canvas answers. `data` holds the deep-linking
/// payload as JSON; a missing column or a missing key answers null.
fn collaboration_update_url(row: &CollaborationRow) -> Value {
    let Some(raw) = row.data.as_deref() else {
        return Value::Null;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(raw) else {
        return Value::Null;
    };
    parsed.get("updateUrl").cloned().unwrap_or(Value::Null)
}

/// The record as Canvas renders it after a destroy.
///
/// `collaborations#destroy` answers `render json: @collaboration` — the
/// record itself, not the whitelist the index uses
/// (`Api::V1::Collaboration#collaboration_json`). `uuid` is in Canvas'
/// `serialization_excludes`, so it is not in the answer.
fn collaboration_record_json(row: &CollaborationRow) -> Value {
    json!({
        "id": row.id,
        "user_id": row.user_id,
        "context_id": row.context_id,
        "context_type": row.context_type,
        "collaboration_type": row.collaboration_type,
        "document_id": row.document_id,
        "title": row.title,
        "description": row.description,
        "url": row.url,
        "type": row.r#type,
        "created_at": row.created_at.map(|t| t.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        "updated_at": row.updated_at.map(|t| t.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        "workflow_state": row.workflow_state,
        "deleted_at": row.deleted_at.map(|t| t.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    })
}

/// Collaborator JSON (lib/api/v1/collaborator.rb): id, type ("group" iff
/// group_id else "user"), name (user.sortable_name || group.name),
/// collaborator_id (user.id || group.id).
fn collaborator_json(id: i64, type_: &str, name: &str, collaborator_id: i64) -> Value {
    json!({
        "id": id,
        "type": type_,
        "name": name,
        "collaborator_id": collaborator_id,
    })
}

/// `[User]` JSON (star-lms UserResponse shape, star-users/src/lib.rs:25).
///
/// Canvas `API_USER_JSON_OPTS` (lib/api/v1/user.rb:26-29) is
/// `only: %w[id name created_at], methods: %w[sortable_name short_name]` —
/// the row carries `short_name` next to `sortable_name`. Canvas'
/// `User#short_name` is `super || name` (app/models/user.rb:1213): the
/// `users.short_name` column (migration 0269) falling back to the name.
fn user_json(
    id: i64,
    name: &str,
    sortable_name: &str,
    short_name: &str,
    email: &str,
    role: &str,
    created_at: DateTime<Utc>,
) -> Value {
    json!({
        "id": id,
        "name": name,
        "sortable_name": sortable_name,
        "short_name": short_name,
        "email": email,
        "role": role,
        "created_at": created_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{course_id}/collaborations and /groups/{group_id}/...
/// (api_index). Guard order: 401 (AuthUser) → 404 context → 403 not
/// configured → 403 :read → 404 tab-disabled → 200 bare array.
async fn api_index(
    pool: &PgPool,
    auth: AuthUser,
    page: PaginationParams,
    ctx: ContextRef,
) -> Result<Response, CollabError> {
    let ctx = verify(ctx, pool, ctx.id).await?;
    if !collaborations_configured(pool, ctx).await? {
        return Err(CollabError::NotConfigured);
    }
    if !context_read(pool, &auth, ctx).await? {
        return Err(CollabError::Forbidden);
    }
    if !collaborations_tab_enabled(pool, ctx).await? {
        return Err(CollabError::TabDisabled);
    }
    let (page_no, per_page, offset) = pagination_values(&page);

    // Canvas: active + type='ExternalToolCollaboration', eager_load(:user).
    let manager = context_manage_content(pool, &auth, ctx).await?;
    let ctx_type = ctx.kind.as_str();
    let rows: Vec<CollaborationRow> = if manager {
        sqlx::query_as(&format!(
            "SELECT {COLLABORATION_COLUMNS} FROM collaborations \
             WHERE context_id = $1 AND LOWER(context_type) = LOWER($2) \
             AND workflow_state <> 'deleted' AND type = 'ExternalToolCollaboration' \
             ORDER BY id ASC LIMIT $3 OFFSET $4"
        ))
        .bind(ctx.id)
        .bind(ctx_type)
        .bind(per_page)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        // Non-manager: author OR user-collaborator OR (course) member of a
        // collaborating group of the course.
        sqlx::query_as(&format!(
            "SELECT {COLLABORATION_COLUMNS} FROM collaborations co \
             WHERE co.context_id = $1 AND LOWER(co.context_type) = LOWER($2) \
             AND co.workflow_state <> 'deleted' AND co.type = 'ExternalToolCollaboration' \
             AND ( \
               co.user_id = $3 \
               OR EXISTS (SELECT 1 FROM collaborators cr \
                          WHERE cr.collaboration_id = co.id AND cr.user_id = $3) \
               OR EXISTS (SELECT 1 FROM collaborators cr \
                          JOIN group_memberships gm ON gm.group_id = cr.group_id \
                          WHERE cr.collaboration_id = co.id AND gm.user_id = $3 \
                          AND gm.workflow_state <> 'deleted') \
             ) \
             ORDER BY co.id ASC LIMIT $4 OFFSET $5"
        ))
        .bind(ctx.id)
        .bind(ctx_type)
        .bind(auth.id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(pool)
        .await?
    };

    let total: (i64,) = if manager {
        sqlx::query_as(
            "SELECT COUNT(*) FROM collaborations \
             WHERE context_id = $1 AND LOWER(context_type) = LOWER($2) \
             AND workflow_state <> 'deleted' AND type = 'ExternalToolCollaboration'",
        )
        .bind(ctx.id)
        .bind(ctx_type)
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT COUNT(*) FROM collaborations co \
             WHERE co.context_id = $1 AND LOWER(co.context_type) = LOWER($2) \
             AND co.workflow_state <> 'deleted' AND co.type = 'ExternalToolCollaboration' \
             AND ( \
               co.user_id = $3 \
               OR EXISTS (SELECT 1 FROM collaborators cr \
                          WHERE cr.collaboration_id = co.id AND cr.user_id = $3) \
               OR EXISTS (SELECT 1 FROM collaborators cr \
                          JOIN group_memberships gm ON gm.group_id = cr.group_id \
                          WHERE cr.collaboration_id = co.id AND gm.user_id = $3 \
                          AND gm.workflow_state <> 'deleted') \
             )",
        )
        .bind(ctx.id)
        .bind(ctx_type)
        .bind(auth.id)
        .fetch_one(pool)
        .await?
    };

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(collaboration_json(pool, row, &auth).await?);
    }
    Ok(paginated_array(items, page_no, per_page, total.0))
}

/// Shared context resolution for the two index twins (course vs group).
async fn verify(ctx: ContextRef, pool: &PgPool, _ctx_id: i64) -> Result<ContextRef, CollabError> {
    match ctx.kind {
        ContextKind::Course => load_course(pool, ctx.id).await,
        ContextKind::Group => load_group(pool, ctx.id).await,
    }
}

/// GET /collaborations/{id}/members (members). Canvas skips require_context
/// and resolves a GLOBAL collaboration, then adopts its context.
async fn members(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((collaboration_id,)): Path<(i64,)>,
    Query(page): Query<PaginationParams>,
) -> Result<Response, CollabError> {
    let pool = &state.db;
    let row: Option<CollaborationRow> = sqlx::query_as(&format!(
        "SELECT {COLLABORATION_COLUMNS} FROM collaborations WHERE id = $1"
    ))
    .bind(collaboration_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Err(CollabError::NotFound);
    };
    // Canvas runs `require_collaborations_configured` on the adopted context
    // of the collaboration (collaborations_controller.rb:128-132, :437-444).
    if !collaborations_configured(pool, row_context(&row)).await? {
        return Err(CollabError::NotConfigured);
    }
    if !collaboration_readable(pool, &auth, &row).await? {
        return Err(CollabError::Forbidden);
    }
    let (page_no, per_page, offset) = pagination_values(&page);

    let collaborator_rows: Vec<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT id, user_id, group_id FROM collaborators \
         WHERE collaboration_id = $1 ORDER BY id ASC LIMIT $2 OFFSET $3",
    )
    .bind(collaboration_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let total: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM collaborators WHERE collaboration_id = $1")
            .bind(collaboration_id)
            .fetch_one(pool)
            .await?;

    let mut items = Vec::with_capacity(collaborator_rows.len());
    for (id, user_id, group_id) in collaborator_rows {
        let item = match (user_id, group_id) {
            (Some(uid), _) => {
                let user: Option<(Option<String>,)> =
                    sqlx::query_as("SELECT COALESCE(sortable_name, name) FROM users WHERE id = $1")
                        .bind(uid)
                        .fetch_optional(pool)
                        .await?;
                match user {
                    Some((Some(name),)) => collaborator_json(id, "user", &name, uid),
                    Some((None,)) => collaborator_json(id, "user", "", uid),
                    None => continue,
                }
            }
            (None, Some(gid)) => {
                let group: Option<(Option<String>,)> =
                    sqlx::query_as("SELECT name FROM groups WHERE id = $1")
                        .bind(gid)
                        .fetch_optional(pool)
                        .await?;
                match group {
                    Some((Some(name),)) => collaborator_json(id, "group", &name, gid),
                    _ => continue,
                }
            }
            (None, None) => continue,
        };
        items.push(item);
    }
    Ok(paginated_array(items, page_no, per_page, total.0))
}

/// GET /courses/{course_id}/potential_collaborators and group twin.
/// Canvas course: users_visible_to (distinct enrolled users); group:
/// participating members + parent-course active admins. Ordered by
/// sortable_name, `[User]` shape.
async fn potential_collaborators(
    pool: &PgPool,
    auth: AuthUser,
    page: PaginationParams,
    ctx: ContextRef,
) -> Result<Response, CollabError> {
    let ctx = verify(ctx, pool, ctx.id).await?;
    if !collaborations_configured(pool, ctx).await? {
        return Err(CollabError::NotConfigured);
    }
    if !context_read_roster(pool, &auth, ctx).await? {
        return Err(CollabError::Forbidden);
    }
    let (page_no, per_page, offset) = pagination_values(&page);

    let rows: Vec<PotentialUserRow> = match ctx.kind {
        ContextKind::Course => {
            sqlx::query_as(
                "SELECT DISTINCT u.id, u.name, u.sortable_name, \
                        COALESCE(u.short_name, u.name), u.email, u.role, u.created_at \
                 FROM users u JOIN enrollments e ON e.user_id = u.id \
                 WHERE e.course_id = $1 AND e.workflow_state = 'active' \
                 ORDER BY u.sortable_name NULLS LAST, u.name, u.id ASC \
                 LIMIT $2 OFFSET $3",
            )
            .bind(ctx.id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        ContextKind::Group => {
            // Canvas `Group#potential_collaborators`
            // (app/models/group.rb:419-426): the participating users of the
            // group IN the parent course (`participating_users_in_context`
            // intersects the group members with the course's active
            // enrollments, app/models/group.rb:145-151) plus the parent
            // course's `participating_admins` (active Ta/Teacher/Designer
            // enrollments, app/models/course.rb:181).
            sqlx::query_as(
                "SELECT DISTINCT u.id, u.name, u.sortable_name, \
                        COALESCE(u.short_name, u.name), u.email, u.role, u.created_at \
                 FROM groups g \
                 CROSS JOIN users u \
                 LEFT JOIN enrollments e ON e.course_id = g.course_id AND e.user_id = u.id \
                     AND e.workflow_state = 'active' \
                 LEFT JOIN group_memberships gm ON gm.group_id = g.id AND gm.user_id = u.id \
                     AND gm.workflow_state <> 'deleted' \
                 WHERE g.id = $1 \
                 AND ((gm.id IS NOT NULL AND e.id IS NOT NULL) \
                   OR (e.id IS NOT NULL AND e.role IN ('teacher','ta','designer'))) \
                 ORDER BY u.sortable_name NULLS LAST, u.name, u.id ASC \
                 LIMIT $2 OFFSET $3",
            )
            .bind(ctx.id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
    };

    let total: (i64,) =
        match ctx.kind {
            ContextKind::Course => sqlx::query_as(
                "SELECT COUNT(DISTINCT u.id) FROM users u JOIN enrollments e ON e.user_id = u.id \
                 WHERE e.course_id = $1 AND e.workflow_state = 'active'",
            )
            .bind(ctx.id)
            .fetch_one(pool)
            .await?,
            ContextKind::Group => {
                sqlx::query_as(
                    "SELECT COUNT(DISTINCT u.id) FROM groups g \
                 CROSS JOIN users u \
                 LEFT JOIN enrollments e ON e.course_id = g.course_id AND e.user_id = u.id \
                     AND e.workflow_state = 'active' \
                 LEFT JOIN group_memberships gm ON gm.group_id = g.id AND gm.user_id = u.id \
                     AND gm.workflow_state <> 'deleted' \
                 WHERE g.id = $1 \
                 AND ((gm.id IS NOT NULL AND e.id IS NOT NULL) \
                   OR (e.id IS NOT NULL AND e.role IN ('teacher','ta','designer')))",
                )
                .bind(ctx.id)
                .fetch_one(pool)
                .await?
            }
        };

    let items: Vec<Value> = rows
        .into_iter()
        .map(
            |(id, name, sortable, short_name, email, role, created_at)| {
                user_json(
                    id,
                    &name,
                    sortable.as_deref().unwrap_or(&name),
                    &short_name,
                    &email,
                    &role,
                    created_at,
                )
            },
        )
        .collect();
    Ok(paginated_array(items, page_no, per_page, total.0))
}

/// DELETE /courses/{course_id}/collaborations/{id} and group twin
/// (destroy). Guard order: 404 context → 403 not configured → scoped lookup
/// (cross-context id → 404) → :delete → soft delete → 200 + row JSON.
async fn destroy(
    pool: &PgPool,
    auth: AuthUser,
    collaboration_id: i64,
    delete_doc: Option<bool>,
    ctx: ContextRef,
) -> Result<Response, CollabError> {
    let ctx = verify(ctx, pool, ctx.id).await?;
    if !collaborations_configured(pool, ctx).await? {
        return Err(CollabError::NotConfigured);
    }
    let row: Option<CollaborationRow> = sqlx::query_as(&format!(
        "SELECT {COLLABORATION_COLUMNS} FROM collaborations \
         WHERE id = $1 AND context_id = $2 AND LOWER(context_type) = LOWER($3)"
    ))
    .bind(collaboration_id)
    .bind(ctx.id)
    .bind(ctx.kind.as_str())
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Err(CollabError::NotFound);
    };
    if !collaboration_deletable(pool, &auth, &row).await? {
        return Err(CollabError::Forbidden);
    }
    // delete_doc: Canvas calls collaboration.delete_document when truthy —
    // external-service boundary, stubbed no-op in star-lms (spec §4).
    let _ = delete_doc;

    let now = Utc::now();
    sqlx::query(
        "UPDATE collaborations SET workflow_state = 'deleted', deleted_at = $2, \
         updated_at = $2 WHERE id = $1",
    )
    .bind(row.id)
    .bind(now)
    .execute(pool)
    .await?;

    let fresh: Option<CollaborationRow> = sqlx::query_as(&format!(
        "SELECT {COLLABORATION_COLUMNS} FROM collaborations WHERE id = $1"
    ))
    .bind(row.id)
    .fetch_optional(pool)
    .await?;
    let fresh = fresh.ok_or(CollabError::NotFound)?;
    // Canvas renders the record itself, not the whitelist the index uses
    // (`render json: @collaboration`,
    // app/controllers/collaborations_controller.rb), so the answer carries
    // the workflow state and the deletion time.
    let body = collaboration_record_json(&fresh);
    Ok((StatusCode::OK, Json(body)).into_response())
}

/// Query params for destroy (`delete_doc`, Canvas value_to_boolean).
#[derive(Debug, Default, Deserialize)]
struct QueryParams {
    delete_doc: Option<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Course-twin wrapper handlers: same logic, different context kind.
macro_rules! context_twin {
    ($fn_name:ident, $load:ident, $inner:ident) => {
        async fn $fn_name(
            State(state): State<AppState>,
            auth: AuthUser,
            Path((ctx_id,)): Path<(i64,)>,
            Query(page): Query<PaginationParams>,
        ) -> Result<Response, CollabError> {
            let pool = &state.db;
            let ctx = $load(pool, ctx_id).await?;
            $inner(pool, auth, page, ctx).await
        }
    };
}

context_twin!(course_api_index, load_course, api_index);
context_twin!(group_api_index, load_group, api_index);
context_twin!(course_potential, load_course, potential_collaborators);
context_twin!(group_potential, load_group, potential_collaborators);

macro_rules! destroy_twin {
    ($fn_name:ident, $load:ident) => {
        async fn $fn_name(
            State(state): State<AppState>,
            auth: AuthUser,
            Path((ctx_id, collaboration_id)): Path<(i64, i64)>,
            Query(params): Query<QueryParams>,
        ) -> Result<Response, CollabError> {
            let pool = &state.db;
            let ctx = $load(pool, ctx_id).await?;
            let delete_doc = bool_param(params.delete_doc.as_deref());
            destroy(pool, auth, collaboration_id, delete_doc, ctx).await
        }
    };
}

destroy_twin!(course_destroy, load_course);
destroy_twin!(group_destroy, load_group);

/// Domain router for star-collaborations.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{course_id}/collaborations", get(course_api_index))
        .route("/groups/{group_id}/collaborations", get(group_api_index))
        .route("/collaborations/{id}/members", get(members))
        .route(
            "/courses/{course_id}/potential_collaborators",
            get(course_potential),
        )
        .route(
            "/groups/{group_id}/potential_collaborators",
            get(group_potential),
        )
        .route(
            "/courses/{course_id}/collaborations/{id}",
            axum::routing::delete(course_destroy),
        )
        .route(
            "/groups/{group_id}/collaborations/{id}",
            axum::routing::delete(group_destroy),
        )
}
