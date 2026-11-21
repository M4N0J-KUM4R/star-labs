//! Canvas collaborations API — port of
//! `canvas-src/app/controllers/collaborations_controller.rb`
//! (serializers `lib/api/v1/collaboration.rb` and `lib/api/v1/collaborator.rb`).
//!
//! Routes served here (the coordinator mounts this router under `/api/v1`):
//!
//! | Method | Path                                            | Canvas action             |
//! |--------|-------------------------------------------------|---------------------------|
//! | GET    | `/courses/{course_id}/collaborations`          | `api_index`              |
//! | GET    | `/groups/{group_id}/collaborations`             | `api_index`              |
//! | GET    | `/collaborations/{id}/members`                  | `members`                |
//! | GET    | `/courses/{course_id}/potential_collaborators`  | `potential_collaborators` |
//! | GET    | `/groups/{group_id}/potential_collaborators`     | `potential_collaborators` |
//! | DELETE | `/courses/{course_id}/collaborations/{id}`      | `destroy`                |
//! | DELETE | `/groups/{group_id}/collaborations/{id}`         | `destroy`                |
//!
//! Guard chain (Canvas `before_action` order, reproduced exactly):
//!
//! 1. `require_context` (skipped for `members`) — a missing course/group is a
//!    **404**.
//! 2. `require_collaboration_and_context` (`members` only) — the collaboration
//!    is looked up **globally** (`Collaboration.find`) because the members
//!    route has no context segment; a missing collaboration is a **404**.
//! 3. `require_collaborations_configured` — **403** when the context has no
//!    collaboration provider configured.
//! 4. `reject_student_view_student` — star-lms has no student-view students,
//!    so the filter never triggers.
//! 5. The action's own `authorized_action` — **403** for a caller without the
//!    right (an anonymous caller is a **401**, produced by the `AuthUser`
//!    extractor).
//!
//! Faithfulness notes:
//!
//! * `api_index` serializes with `collaboration_json`, whose attribute
//!   whitelist is `id collaboration_type document_id user_id context_id
//!   context_type url created_at updated_at description title type
//!   update_url`, plus `user_name` and a `permissions` object. It does **not**
//!   carry `collaborator_ids` — that method is only used by the HTML
//!   create/update paths.
//! * `api_index` only returns `ExternalToolCollaboration` rows. A caller that
//!   does not manage the course content only sees the collaborations it
//!   created or collaborates on (directly, or through one of its course
//!   groups).
//! * `members` returns a flat array of `collaborator_json` hashes mixing user
//!   and group shapes (`type` is `"user"` or `"group"`).
//! * `destroy` soft-deletes the row (`workflow_state = "deleted"`) and answers
//!   with the raw record hash, exactly like Canvas' `render json:
//!   @collaboration` (every column except `uuid`, which
//!   `Collaboration.serialization_excludes` removes).
//! * `require_collaborations_configured` redirects in Canvas. An API request
//!   cannot redirect, so the workspace answers **403** with Canvas' flash
//!   message, `{"errors": ["Collaborations have not been enabled for this
//!   Canvas site"]}`.
//! * The collaborations tab is dropped for callers that are not course admins
//!   when the teacher hid it, which Canvas renders as a **404** with
//!   `{"message": "That page has been disabled for this course"}`.
//!
//! Known approximations (star-lms has no counterpart for these Canvas
//! features):
//!
//! * `collaborator_lti_id` is read from `user_past_lti_ids` for the
//!   collaboration's context; when there is no past LTI id the field is
//!   `null`. Canvas mints an HMAC opaque id instead, which needs the
//!   account encryption key star-lms does not have.
//! * `avatar_image_url` is the `user_profiles.avatar_url` of the collaborating
//!   user; Canvas builds an absolute URL from its avatar service.
//! * `delete_doc=true` is accepted and ignored — star-lms has no external
//!   document provider to delete the file in.
//!
//! Storage: the two tables this module reads (`collaborations` and
//! `collaborators`) are already created by `star-db` migration `0223`, so
//! **no new DDL is required**. For reference, the exact statements are:
//!
//! ```sql
//! CREATE TABLE collaborations (
//!     id                               BIGSERIAL PRIMARY KEY,
//!     collaboration_type               varchar(255),
//!     document_id                      varchar(255),
//!     user_id                          bigint,
//!     context_id                       bigint,
//!     context_type                     varchar(255),
//!     url                              varchar(255),
//!     uuid                             varchar(255),
//!     data                             text,
//!     created_at                       timestamp NOT NULL,
//!     updated_at                       timestamp NOT NULL,
//!     description                      text,
//!     title                            varchar(255) NOT NULL,
//!     workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
//!     deleted_at                       timestamp,
//!     context_code                     varchar(255),
//!     type                             varchar(255),
//!     resource_link_lookup_uuid        uuid,
//!     root_account_id                  bigint
//! );
//!
//! CREATE TABLE collaborators (
//!     id                               BIGSERIAL PRIMARY KEY,
//!     user_id                          bigint,
//!     collaboration_id                 bigint,
//!     created_at                       timestamp NOT NULL,
//!     updated_at                       timestamp NOT NULL,
//!     authorized_service_user_id       varchar(255),
//!     group_id                         bigint
//! );
//! ```

use std::collections::{BTreeSet, HashMap};

use axum::{
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Canvas `Api::PER_PAGE` — the default page size of `Api.paginate`.
const DEFAULT_PER_PAGE: i64 = 10;

/// Canvas `Api::MAX_PER_PAGE` — the largest accepted page size.
const MAX_PER_PAGE: i64 = 100;

/// The `type` column value of the only collaboration kind the API lists
/// (Canvas `api_index`: `where(type: "ExternalToolCollaboration")`).
const EXTERNAL_TOOL_COLLABORATION: &str = "ExternalToolCollaboration";

/// Canvas `errors.not_enabled` — the `require_collaborations_configured`
/// flash message.
const COLLABORATIONS_NOT_ENABLED: &str =
    "Collaborations have not been enabled for this Canvas site";

/// The `collaborations` columns this module reads. `created_at`/`updated_at`/
/// `deleted_at` are plain `timestamp` columns in the star-lms schema; they
/// are read as `timestamptz` (UTC) so the JSON carries a `Z` suffix like the
/// rest of the workspace.
const COLLABORATION_COLUMNS: &str = "c.id, c.collaboration_type, c.document_id, c.user_id, \
     c.context_id, c.context_type, c.url, c.data, \
     c.created_at AT TIME ZONE 'UTC' AS created_at, \
     c.updated_at AT TIME ZONE 'UTC' AS updated_at, \
     c.description, c.title, c.workflow_state, \
     c.deleted_at AT TIME ZONE 'UTC' AS deleted_at, c.context_code, c.type, \
     c.resource_link_lookup_uuid::text AS resource_link_lookup_uuid, c.root_account_id";

/// The same columns, unqualified, for `UPDATE ... RETURNING`.
const COLLABORATION_RETURNING: &str = "id, collaboration_type, document_id, user_id, \
     context_id, context_type, url, data, \
     created_at AT TIME ZONE 'UTC' AS created_at, \
     updated_at AT TIME ZONE 'UTC' AS updated_at, \
     description, title, workflow_state, \
     deleted_at AT TIME ZONE 'UTC' AS deleted_at, context_code, type, \
     resource_link_lookup_uuid::text AS resource_link_lookup_uuid, root_account_id";

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Domain router for the Canvas collaborations routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{course_id}/collaborations",
            get(course_collaborations_index),
        )
        .route(
            "/groups/{group_id}/collaborations",
            get(group_collaborations_index),
        )
        .route("/collaborations/{id}/members", get(collaboration_members))
        .route(
            "/courses/{course_id}/potential_collaborators",
            get(course_potential_collaborators),
        )
        .route(
            "/groups/{group_id}/potential_collaborators",
            get(group_potential_collaborators),
        )
        .route(
            "/courses/{course_id}/collaborations/{id}",
            delete(course_collaboration_destroy),
        )
        .route(
            "/groups/{group_id}/collaborations/{id}",
            delete(group_collaboration_destroy),
        )
}

// ---------------------------------------------------------------------------
// Domain rows
// ---------------------------------------------------------------------------

/// One `collaborations` row.
#[derive(Debug, sqlx::FromRow)]
struct CollaborationRow {
    id: i64,
    collaboration_type: Option<String>,
    document_id: Option<String>,
    user_id: Option<i64>,
    context_id: Option<i64>,
    context_type: Option<String>,
    url: Option<String>,
    data: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    description: Option<String>,
    title: String,
    workflow_state: String,
    deleted_at: Option<DateTime<Utc>>,
    context_code: Option<String>,
    /// The Rails STI column (`type` is a Rust keyword).
    #[sqlx(rename = "type")]
    kind: Option<String>,
    resource_link_lookup_uuid: Option<String>,
    root_account_id: Option<i64>,
}

/// One `collaborators` row joined with its user and group.
#[derive(Debug, sqlx::FromRow)]
struct CollaboratorRow {
    id: i64,
    user_id: Option<i64>,
    group_id: Option<i64>,
    user_sortable_name: Option<String>,
    group_name: Option<String>,
    avatar_url: Option<String>,
    /// `true` when the collaborating user row still exists — Canvas reads
    /// `collaborator.user`, which is `nil` for a deleted user.
    user_found: bool,
}

/// One user of the potential-collaborators list.
#[derive(Debug, sqlx::FromRow)]
struct PotentialUserRow {
    id: i64,
    name: String,
    created_at: DateTime<Utc>,
    sortable_name: String,
}

// ---------------------------------------------------------------------------
// JSON shapes
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::Collaboration#collaboration_json`.
#[derive(Debug, Serialize)]
struct CollaborationJson {
    id: i64,
    collaboration_type: Option<String>,
    document_id: Option<String>,
    user_id: Option<i64>,
    context_id: Option<i64>,
    context_type: Option<String>,
    url: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    description: Option<String>,
    title: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    update_url: Option<String>,
    user_name: Option<String>,
    permissions: CollaborationPermissions,
}

/// The `permissions` object `collaboration_json` appends.
#[derive(Debug, Serialize)]
struct CollaborationPermissions {
    update: bool,
    delete: bool,
}

/// Canvas `Api::V1::Collaborator#collaborator_json`.
#[derive(Debug, Serialize)]
struct CollaboratorJson {
    id: i64,
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
    collaborator_id: i64,
    /// `None` — the key is absent (not requested); `Some(None)` — the key is
    /// present with a `null` value; `Some(Some(id))` — the LTI id.
    #[serde(skip_serializing_if = "Option::is_none")]
    collaborator_lti_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avatar_image_url: Option<Option<String>>,
}

/// Canvas `Api::V1::User#user_json` with `API_USER_JSON_OPTS`
/// (`only: id, name, created_at` + `methods: sortable_name, short_name`).
#[derive(Debug, Serialize)]
struct UserJson {
    id: i64,
    name: String,
    created_at: DateTime<Utc>,
    sortable_name: String,
    short_name: String,
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// The kind of Canvas context a collaborations route runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextKind {
    Course,
    Group,
}

impl ContextKind {
    /// The `context_type` value the polymorphic `collaborations` rows carry.
    fn polymorphic_type(self) -> &'static str {
        match self {
            ContextKind::Course => "Course",
            ContextKind::Group => "Group",
        }
    }

    /// Canvas `ApplicationController#tab_disabled_message`.
    fn disabled_message(self) -> &'static str {
        match self {
            ContextKind::Course => "That page has been disabled for this course",
            ContextKind::Group => "That page has been disabled for this group",
        }
    }
}

/// A resolved collaborations context.
#[derive(Debug, Clone)]
struct CollaborationsContext {
    kind: ContextKind,
    id: i64,
    /// The course the context belongs to. A course context is its own course;
    /// a group context is the group, and this is its parent course.
    course_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Query-string helpers
// ---------------------------------------------------------------------------

/// A parsed query string. A key may repeat (`include[]=a&include[]=b`), so
/// every key keeps all of its values.
type QueryParams = HashMap<String, Vec<String>>;

/// Ruby `String#to_i` — the leading integer prefix, `0` when there is none.
fn ruby_to_i(raw: &str) -> i64 {
    let trimmed = raw.trim_start();
    let bytes = trimmed.as_bytes();
    let mut end = 0;
    if end < bytes.len() && (bytes[end] == b'-' || bytes[end] == b'+') {
        end += 1;
    }
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    trimmed[..end].parse::<i64>().unwrap_or(0)
}

/// Minimal percent-decoder for one query-string component (`+` is a space).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                match u8::from_str_radix(&input[index + 1..index + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse a raw query string into its keys and values.
fn parse_query(raw: Option<&str>) -> QueryParams {
    let mut params: QueryParams = HashMap::new();
    let Some(raw) = raw else {
        return params;
    };
    for pair in raw.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        params
            .entry(percent_decode(key))
            .or_default()
            .push(percent_decode(value));
    }
    params
}

/// The `page` parameter of `Api.paginate` (1-based).
fn page_of(params: &QueryParams) -> i64 {
    params
        .get("page")
        .and_then(|values| values.first())
        .map(|raw| ruby_to_i(raw))
        .filter(|page| *page > 0)
        .unwrap_or(1)
}

/// The `per_page` parameter of `Api.paginate` (default 10, clamped to 1..=100).
fn per_page_of(params: &QueryParams) -> i64 {
    match params.get("per_page").and_then(|values| values.first()) {
        Some(raw) => ruby_to_i(raw).clamp(1, MAX_PER_PAGE),
        None => DEFAULT_PER_PAGE,
    }
}

/// The `include[]` values of the members route.
fn includes_of(params: &QueryParams) -> Vec<String> {
    let mut includes: Vec<String> = Vec::new();
    for key in ["include[]", "include"] {
        if let Some(values) = params.get(key) {
            for value in values {
                for part in value.split(',') {
                    let part = part.trim();
                    if !part.is_empty() {
                        includes.push(part.to_string());
                    }
                }
            }
        }
    }
    includes
}

// ---------------------------------------------------------------------------
// Context resolution
// ---------------------------------------------------------------------------

/// Canvas `require_context` for a course — `api_find(Course.active, id)`.
async fn resolve_course_context(
    pool: &PgPool,
    course_id: i64,
) -> Result<CollaborationsContext, ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    if row.is_none() {
        return Err(ApiError::NotFound);
    }
    Ok(CollaborationsContext {
        kind: ContextKind::Course,
        id: course_id,
        course_id: Some(course_id),
    })
}

/// Canvas `require_context` for a group — `api_find(Group.active, id)`.
async fn resolve_group_context(
    pool: &PgPool,
    group_id: i64,
) -> Result<CollaborationsContext, ApiError> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((course_id,)) => Ok(CollaborationsContext {
            kind: ContextKind::Group,
            id: group_id,
            course_id,
        }),
        None => Err(ApiError::NotFound),
    }
}

/// Canvas `require_collaboration_and_context` — the context of a collaboration
/// row (`collaboration.context`). A missing context is a `RecordNotFound`.
async fn resolve_collaboration_context(
    pool: &PgPool,
    row: &CollaborationRow,
) -> Result<CollaborationsContext, ApiError> {
    let context_id = row.context_id.ok_or(ApiError::NotFound)?;
    match row.context_type.as_deref() {
        Some("Course") => {
            let found: Option<(i64,)> = sqlx::query_as("SELECT id FROM courses WHERE id = $1")
                .bind(context_id)
                .fetch_optional(pool)
                .await?;
            if found.is_none() {
                return Err(ApiError::NotFound);
            }
            Ok(CollaborationsContext {
                kind: ContextKind::Course,
                id: context_id,
                course_id: Some(context_id),
            })
        }
        Some("Group") => {
            let found: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT course_id FROM groups WHERE id = $1")
                    .bind(context_id)
                    .fetch_optional(pool)
                    .await?;
            match found {
                Some((course_id,)) => Ok(CollaborationsContext {
                    kind: ContextKind::Group,
                    id: context_id,
                    course_id,
                }),
                None => Err(ApiError::NotFound),
            }
        }
        _ => Err(ApiError::NotFound),
    }
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// star-lms models a Canvas site admin as `users.role = 'admin'`.
fn is_site_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas course `:read` — site admins and every enrolled user.
async fn can_read_course(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    if is_site_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 \
           AND workflow_state NOT IN ('deleted', 'rejected', 'inactive') \
         LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas course `:read_roster` — site admins and course admins (Teacher /
/// TA / Designer enrollments).
async fn can_read_roster_course(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if is_site_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' \
           AND workflow_state IN ('active', 'invited') \
         LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas group `:read` — site admins, the admins of the parent course and the
/// members of the group (`Group#has_member?` requires an accepted
/// membership).
async fn can_read_group(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: &CollaborationsContext,
) -> Result<bool, ApiError> {
    if is_site_admin(auth) {
        return Ok(true);
    }
    if let Some(course_id) = ctx.course_id {
        if can_read_roster_course(pool, auth, course_id).await? {
            return Ok(true);
        }
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM group_memberships \
         WHERE group_id = $1 AND user_id = $2 AND workflow_state = 'accepted' \
         LIMIT 1",
    )
    .bind(ctx.id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `authorized_action(@context, @current_user, :read)`.
async fn can_read_context(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: &CollaborationsContext,
) -> Result<bool, ApiError> {
    match ctx.kind {
        ContextKind::Course => can_read_course(pool, auth, ctx.id).await,
        ContextKind::Group => can_read_group(pool, auth, ctx).await,
    }
}

/// Canvas `authorized_action(@context, @current_user, :read_roster)`.
async fn can_read_roster_context(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: &CollaborationsContext,
) -> Result<bool, ApiError> {
    match ctx.kind {
        ContextKind::Course => can_read_roster_course(pool, auth, ctx.id).await,
        ContextKind::Group => can_read_group(pool, auth, ctx).await,
    }
}

/// Canvas `:manage_course_content_edit`, `:manage_course_content_delete` and
/// `:create_collaborations` on the context — in star-lms all three are granted
/// to the same role set (the admins of the course that owns the context).
async fn can_manage_context(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: &CollaborationsContext,
) -> Result<bool, ApiError> {
    match ctx.course_id {
        Some(course_id) => can_read_roster_course(pool, auth, course_id).await,
        None => Ok(is_site_admin(auth)),
    }
}

/// Canvas `Collaboration` policy — `:read` is granted to the author, to the
/// direct collaborators and to the members of a collaborating group.
async fn can_read_collaboration(
    pool: &PgPool,
    auth: &AuthUser,
    row: &CollaborationRow,
) -> Result<bool, ApiError> {
    if row.user_id == Some(auth.id) {
        return Ok(true);
    }
    let direct: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM collaborators \
         WHERE collaboration_id = $1 AND user_id = $2 \
         LIMIT 1",
    )
    .bind(row.id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if direct.is_some() {
        return Ok(true);
    }
    let group: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM collaborators col \
         JOIN group_memberships gm \
           ON gm.group_id = col.group_id AND gm.user_id = $2 \
          AND gm.workflow_state <> 'deleted' \
         WHERE col.collaboration_id = $1 \
         LIMIT 1",
    )
    .bind(row.id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    Ok(group.is_some())
}

/// Canvas `Collaboration` policy — `:update` and `:delete`.
///
/// `:manage_course_content_edit` (update) and `:manage_course_content_delete`
/// (delete) grant the right, and the author of the collaboration gets it as
/// well when the context grants `:create_collaborations`. star-lms maps all
/// three context permissions onto the same role set (the admins of the course
/// that owns the context), so the author branch adds nothing.
async fn can_change_collaboration(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: &CollaborationsContext,
) -> Result<bool, ApiError> {
    can_manage_context(pool, auth, ctx).await
}

// ---------------------------------------------------------------------------
// `require_collaborations_configured`
// ---------------------------------------------------------------------------

/// The account chain of a course (its account and every ancestor).
async fn course_account_ids(pool: &PgPool, course_id: i64) -> Result<Vec<i64>, ApiError> {
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE chain AS (
            SELECT a.id, a.parent_account_id
            FROM accounts a
            JOIN courses c ON c.account_id = a.id
            WHERE c.id = $1
            UNION ALL
            SELECT parent.id, parent.parent_account_id
            FROM accounts parent
            JOIN chain child ON parent.id = child.parent_account_id
        )
        SELECT id FROM chain
        "#,
    )
    .bind(course_id)
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

/// Canvas `Lti::ContextToolFinder.all_tools_scope_union(context, placements:
/// :collaboration).exists?` — the context has an LTI tool with the
/// `collaboration` placement.
async fn context_has_collaboration_tool(
    pool: &PgPool,
    ctx: &CollaborationsContext,
) -> Result<bool, ApiError> {
    let Some(course_id) = ctx.course_id else {
        return Ok(false);
    };
    let account_ids = course_account_ids(pool, course_id).await?;
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM context_external_tools tool
        JOIN context_external_tool_placements placement
          ON placement.context_external_tool_id = tool.id
        WHERE placement.placement_type = 'collaboration'
          AND (
                (tool.context_type = 'Course' AND tool.context_id = $1)
             OR (tool.context_type = 'Account' AND tool.context_id = ANY($2))
          )
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .bind(&account_ids)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `@domain_root_account.feature_enabled?(:new_collaborations)`.
async fn new_collaborations_enabled(
    pool: &PgPool,
    ctx: &CollaborationsContext,
) -> Result<bool, ApiError> {
    let course_id = ctx.course_id;
    let account_ids = match course_id {
        Some(course_id) => course_account_ids(pool, course_id).await?,
        None => Vec::new(),
    };
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM feature_flags
        WHERE feature = 'new_collaborations'
          AND state IN ('on', 'allowed', 'allowed_on')
          AND (
                (context_type = 'Course' AND context_id = $1)
             OR (context_type = 'Account' AND context_id = ANY($2))
          )
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .bind(&account_ids)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `require_collaborations_configured`.
///
/// `Collaboration.any_collaborations_configured?(context)` is true when the
/// context has an LTI tool with the `collaboration` placement; star-lms has
/// no Canvas plugin catalogue, so the plugin branch of the Ruby method is
/// always false.
async fn collaborations_configured(
    pool: &PgPool,
    ctx: &CollaborationsContext,
) -> Result<bool, ApiError> {
    if context_has_collaboration_tool(pool, ctx).await? {
        return Ok(true);
    }
    new_collaborations_enabled(pool, ctx).await
}

/// Canvas `require_collaborations_configured` failure.
///
/// Canvas redirects an HTML request to the context URL and sets the flash
/// error "Collaborations have not been enabled for this Canvas site". An API
/// request cannot redirect, so the workspace answers 403 with the same
/// message.
fn collaborations_not_enabled_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({ "errors": [COLLABORATIONS_NOT_ENABLED] })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Collaborations tab
// ---------------------------------------------------------------------------

/// Canvas `tab_enabled?(@context.class::TAB_COLLABORATIONS)`.
///
/// The collaborations tab is available to every reader of the context; it is
/// only dropped when the teacher hid it in the course tab configuration and
/// the caller is not a course admin (Canvas removes hidden tabs for
/// non-admins). Groups have no configurable tabs.
async fn collaborations_tab_enabled(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: &CollaborationsContext,
) -> Result<bool, ApiError> {
    if ctx.kind == ContextKind::Group {
        return Ok(true);
    }
    let hidden: Option<(bool,)> = sqlx::query_as(
        "SELECT hidden FROM course_tabs WHERE course_id = $1 AND tab_id = 'collaborations' LIMIT 1",
    )
    .bind(ctx.id)
    .fetch_optional(pool)
    .await?;
    match hidden {
        Some((true,)) => Ok(!can_manage_context(pool, auth, ctx).await?),
        _ => Ok(true),
    }
}

/// Canvas `render_tab_disabled` — a 404 with a `message` object.
fn tab_disabled_response(kind: ContextKind) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "message": kind.disabled_message() })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// The `update_url` of a collaboration — `ExternalToolCollaboration#update_url`
/// reads `data["updateUrl"]`.
fn update_url_of(row: &CollaborationRow) -> Option<String> {
    let raw = row.data.as_deref()?;
    let parsed: Value = serde_json::from_str(raw).ok()?;
    parsed
        .get("updateUrl")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

/// Canvas `collaboration_json`.
fn collaboration_json(
    row: &CollaborationRow,
    user_name: Option<&str>,
    can_update: bool,
    can_delete: bool,
) -> CollaborationJson {
    CollaborationJson {
        id: row.id,
        collaboration_type: row.collaboration_type.clone(),
        document_id: row.document_id.clone(),
        user_id: row.user_id,
        context_id: row.context_id,
        context_type: row.context_type.clone(),
        url: row.url.clone(),
        created_at: row.created_at,
        updated_at: row.updated_at,
        description: row.description.clone(),
        title: row.title.clone(),
        kind: row.kind.clone(),
        update_url: update_url_of(row),
        user_name: user_name.map(|name| name.to_string()),
        permissions: CollaborationPermissions {
            update: can_update,
            delete: can_delete,
        },
    }
}

/// The names of the authors of the listed collaborations, keyed by user id.
async fn author_names(
    pool: &PgPool,
    rows: &[CollaborationRow],
) -> Result<HashMap<i64, String>, ApiError> {
    let ids: Vec<i64> = rows
        .iter()
        .filter_map(|row| row.user_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let names: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM users WHERE id = ANY($1)")
        .bind(&ids)
        .fetch_all(pool)
        .await?;
    Ok(names.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Handlers — api_index
// ---------------------------------------------------------------------------

/// Canvas `api_index`'s collaboration query.
///
/// The list holds the `ExternalToolCollaboration` rows of the context. A
/// caller that does not manage the course content only sees the
/// collaborations it created or collaborates on (directly, or through one of
/// its course groups).
async fn load_index_collaborations(
    pool: &PgPool,
    ctx: &CollaborationsContext,
    auth: &AuthUser,
    limit: i64,
    offset: i64,
) -> Result<Vec<CollaborationRow>, ApiError> {
    let manages = can_manage_context(pool, auth, ctx).await?;
    let sql = format!(
        r#"
        SELECT {COLLABORATION_COLUMNS}
        FROM collaborations c
        WHERE c.context_type = $1
          AND c.context_id = $2
          AND c.workflow_state <> 'deleted'
          AND c.type = $3
          AND (
                $4
             OR c.user_id = $5
             OR EXISTS (
                    SELECT 1 FROM collaborators col
                    WHERE col.collaboration_id = c.id AND col.user_id = $5
                )
             OR (
                    $6
                AND EXISTS (
                    SELECT 1 FROM collaborators col
                    JOIN group_memberships gm ON gm.group_id = col.group_id AND gm.user_id = $5
                    WHERE col.collaboration_id = c.id
                      AND col.group_id IN (
                            SELECT g.id
                            FROM groups g
                            JOIN group_memberships gm2 ON gm2.group_id = g.id AND gm2.user_id = $5
                            WHERE g.course_id = $7
                        )
                )
             )
          )
        ORDER BY c.id ASC
        LIMIT $8 OFFSET $9
        "#
    );
    let rows: Vec<CollaborationRow> = sqlx::query_as(&sql)
        .bind(ctx.polymorphic_type())
        .bind(ctx.id)
        .bind(EXTERNAL_TOOL_COLLABORATION)
        .bind(manages)
        .bind(auth.id)
        .bind(ctx.kind == ContextKind::Course)
        .bind(ctx.course_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// The shared body of `api_index`.
async fn collaborations_index(
    state: &AppState,
    ctx: CollaborationsContext,
    params: &QueryParams,
    auth: &AuthUser,
) -> Result<Response, ApiError> {
    // `require_collaborations_configured`
    if !collaborations_configured(&state.db, &ctx).await? {
        return Ok(collaborations_not_enabled_response());
    }
    // `authorized_action(@context, @current_user, :read)`
    if !can_read_context(&state.db, auth, &ctx).await? {
        return Err(ApiError::Forbidden);
    }
    // `tab_enabled?(@context.class::TAB_COLLABORATIONS)`
    if !collaborations_tab_enabled(&state.db, auth, &ctx).await? {
        return Ok(tab_disabled_response(ctx.kind));
    }

    let per_page = per_page_of(params);
    let rows = load_index_collaborations(
        &state.db,
        &ctx,
        auth,
        per_page,
        (page_of(params) - 1) * per_page,
    )
    .await?;
    let names = author_names(&state.db, &rows).await?;
    let manages = can_manage_context(&state.db, auth, &ctx).await?;
    let items: Vec<CollaborationJson> = rows
        .iter()
        .map(|row| {
            let user_name = row
                .user_id
                .and_then(|id| names.get(&id))
                .map(|name| name.as_str());
            collaboration_json(row, user_name, manages, manages)
        })
        .collect();
    Ok(Json(items).into_response())
}

/// `GET /courses/{course_id}/collaborations` — Canvas `api_index`.
async fn course_collaborations_index(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    RawQuery(raw): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = resolve_course_context(&state.db, course_id).await?;
    let params = parse_query(raw.as_deref());
    collaborations_index(&state, ctx, &params, &auth).await
}

/// `GET /groups/{group_id}/collaborations` — Canvas `api_index`.
async fn group_collaborations_index(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    RawQuery(raw): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = resolve_group_context(&state.db, group_id).await?;
    let params = parse_query(raw.as_deref());
    collaborations_index(&state, ctx, &params, &auth).await
}

// ---------------------------------------------------------------------------
// Handlers — members
// ---------------------------------------------------------------------------

/// The collaborators of a collaboration.
async fn load_collaborators(
    pool: &PgPool,
    collaboration_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<CollaboratorRow>, ApiError> {
    let sql = r#"
        SELECT col.id,
               col.user_id,
               col.group_id,
               COALESCE(u.sortable_name, u.name) AS user_sortable_name,
               g.name AS group_name,
               profile.avatar_url,
               (u.id IS NOT NULL) AS user_found
        FROM collaborators col
        LEFT JOIN users u ON u.id = col.user_id
        LEFT JOIN groups g ON g.id = col.group_id
        LEFT JOIN user_profiles profile ON profile.user_id = col.user_id
        WHERE col.collaboration_id = $1
        ORDER BY col.id ASC
        LIMIT $2 OFFSET $3
    "#;
    let rows: Vec<CollaboratorRow> = sqlx::query_as(sql)
        .bind(collaboration_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// The past LTI id of a user in the given context
/// (`Lti::V1p1::Asset.old_id_for_user_in_context`).
async fn past_lti_id(
    pool: &PgPool,
    user_id: i64,
    ctx: &CollaborationsContext,
) -> Result<Option<String>, ApiError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT user_lti_context_id FROM user_past_lti_ids \
         WHERE user_id = $1 AND context_id = $2 AND context_type = $3 \
         LIMIT 1",
    )
    .bind(user_id)
    .bind(ctx.id)
    .bind(ctx.polymorphic_type())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(value,)| value))
}

/// `GET /collaborations/{id}/members` — Canvas `members`.
async fn collaboration_members(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    RawQuery(raw): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let params = parse_query(raw.as_deref());
    let includes = includes_of(&params);

    // `require_collaboration_and_context` — the members route has no context
    // segment, so Canvas looks the collaboration up globally.
    let sql = format!("SELECT {COLLABORATION_COLUMNS} FROM collaborations c WHERE c.id = $1");
    let row: Option<CollaborationRow> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };
    let ctx = resolve_collaboration_context(&state.db, &row).await?;

    // `require_collaborations_configured`
    if !collaborations_configured(&state.db, &ctx).await? {
        return Ok(collaborations_not_enabled_response());
    }
    // `authorized_action(@collaboration, @current_user, :read)`
    if !can_read_collaboration(&state.db, &auth, &row).await? {
        return Err(ApiError::Forbidden);
    }

    let per_page = per_page_of(&params);
    let rows = load_collaborators(
        &state.db,
        row.id,
        per_page,
        (page_of(&params) - 1) * per_page,
    )
    .await?;

    let mut items: Vec<CollaboratorJson> = Vec::with_capacity(rows.len());
    for collaborator in rows {
        let wants_lti_id = includes.iter().any(|value| value == "collaborator_lti_id");
        let wants_avatar = includes.iter().any(|value| value == "avatar_image_url");
        let lti_id = if wants_lti_id {
            let user_id = collaborator.user_id.filter(|_| collaborator.user_found);
            Some(match user_id {
                Some(user_id) => past_lti_id(&state.db, user_id, &ctx).await?,
                None => None,
            })
        } else {
            None
        };
        let avatar = if wants_avatar {
            Some(collaborator.avatar_url.clone())
        } else {
            None
        };
        let name = if collaborator.user_found {
            collaborator.user_sortable_name.clone()
        } else {
            collaborator.group_name.clone()
        };
        let collaborator_id = if collaborator.user_found {
            collaborator.user_id
        } else {
            collaborator.group_id
        };
        let Some(collaborator_id) = collaborator_id else {
            continue;
        };
        items.push(CollaboratorJson {
            id: collaborator.id,
            kind: if collaborator.group_id.is_some() {
                "group".to_string()
            } else {
                "user".to_string()
            },
            name,
            collaborator_id,
            collaborator_lti_id: lti_id,
            avatar_image_url: avatar,
        });
    }
    Ok(Json(items).into_response())
}

// ---------------------------------------------------------------------------
// Handlers — potential_collaborators
// ---------------------------------------------------------------------------

/// Canvas `Course#potential_collaborators_for(current_user)` —
/// `users_visible_to(current_user)`. The caller needs `:read_roster`, so the
/// roster is the set of users with a current enrollment in the course.
async fn load_course_potential_collaborators(
    pool: &PgPool,
    course_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<PotentialUserRow>, ApiError> {
    let sql = r#"
        SELECT u.id,
               u.name,
               u.created_at,
               COALESCE(u.sortable_name, u.name) AS sortable_name
        FROM users u
        JOIN enrollments e ON e.user_id = u.id
        WHERE e.course_id = $1
          AND e.workflow_state NOT IN ('rejected', 'completed', 'deleted', 'inactive')
        GROUP BY u.id, u.name, u.created_at, u.sortable_name
        ORDER BY sortable_name ASC
        LIMIT $2 OFFSET $3
    "#;
    let rows: Vec<PotentialUserRow> = sqlx::query_as(sql)
        .bind(course_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// Canvas `Group#potential_collaborators` — the members of the group that are
/// enrolled in the parent course, plus the admins of that course.
async fn load_group_potential_collaborators(
    pool: &PgPool,
    group_id: i64,
    course_id: Option<i64>,
    limit: i64,
    offset: i64,
) -> Result<Vec<PotentialUserRow>, ApiError> {
    let sql = r#"
        SELECT u.id,
               u.name,
               u.created_at,
               COALESCE(u.sortable_name, u.name) AS sortable_name
        FROM users u
        WHERE EXISTS (
                SELECT 1
                FROM group_memberships gm
                WHERE gm.group_id = $1
                  AND gm.user_id = u.id
                  AND gm.workflow_state = 'accepted'
                  AND (
                        $2::bigint IS NULL
                     OR EXISTS (
                            SELECT 1 FROM enrollments e
                            WHERE e.user_id = u.id
                              AND e.course_id = $2
                              AND e.workflow_state NOT IN ('rejected', 'completed', 'deleted', 'inactive')
                        )
                  )
            )
           OR EXISTS (
                SELECT 1 FROM enrollments e
                WHERE e.course_id = $2
                  AND e.user_id = u.id
                  AND e.role = 'teacher'
                  AND e.workflow_state = 'active'
            )
        ORDER BY sortable_name ASC
        LIMIT $3 OFFSET $4
    "#;
    let rows: Vec<PotentialUserRow> = sqlx::query_as(sql)
        .bind(group_id)
        .bind(course_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// The shared body of `potential_collaborators`.
async fn potential_collaborators(
    state: &AppState,
    ctx: CollaborationsContext,
    params: &QueryParams,
    auth: &AuthUser,
) -> Result<Response, ApiError> {
    // `require_collaborations_configured`
    if !collaborations_configured(&state.db, &ctx).await? {
        return Ok(collaborations_not_enabled_response());
    }
    // `authorized_action(@context, @current_user, :read_roster)`
    if !can_read_roster_context(&state.db, auth, &ctx).await? {
        return Err(ApiError::Forbidden);
    }

    let per_page = per_page_of(params);
    let offset = (page_of(params) - 1) * per_page;
    let rows = match ctx.kind {
        ContextKind::Course => {
            load_course_potential_collaborators(&state.db, ctx.id, per_page, offset).await?
        }
        ContextKind::Group => {
            load_group_potential_collaborators(&state.db, ctx.id, ctx.course_id, per_page, offset)
                .await?
        }
    };
    let items: Vec<UserJson> = rows
        .into_iter()
        .map(|row| UserJson {
            id: row.id,
            name: row.name,
            created_at: row.created_at,
            sortable_name: row.sortable_name,
            short_name: row.name,
        })
        .collect();
    Ok(Json(items).into_response())
}

/// `GET /courses/{course_id}/potential_collaborators` — Canvas
/// `potential_collaborators`.
async fn course_potential_collaborators(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    RawQuery(raw): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = resolve_course_context(&state.db, course_id).await?;
    let params = parse_query(raw.as_deref());
    potential_collaborators(&state, ctx, &params, &auth).await
}

/// `GET /groups/{group_id}/potential_collaborators` — Canvas
/// `potential_collaborators`.
async fn group_potential_collaborators(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    RawQuery(raw): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = resolve_group_context(&state.db, group_id).await?;
    let params = parse_query(raw.as_deref());
    potential_collaborators(&state, ctx, &params, &auth).await
}

// ---------------------------------------------------------------------------
// Handlers — destroy
// ---------------------------------------------------------------------------

/// Canvas `@context.collaborations.find(params[:id])` — the lookup is scoped
/// to the context, so an id of another context is a 404.
async fn load_context_collaboration(
    pool: &PgPool,
    ctx: &CollaborationsContext,
    id: i64,
) -> Result<CollaborationRow, ApiError> {
    let sql = format!(
        "SELECT {COLLABORATION_COLUMNS} FROM collaborations c \
         WHERE c.id = $1 AND c.context_type = $2 AND c.context_id = $3"
    );
    let row: Option<CollaborationRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(ctx.polymorphic_type())
        .bind(ctx.id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// Canvas `Collaboration#destroy` — the row is soft-deleted.
async fn soft_delete_collaboration(pool: &PgPool, id: i64) -> Result<CollaborationRow, ApiError> {
    let sql = format!(
        "UPDATE collaborations \
         SET workflow_state = 'deleted', \
             deleted_at = (now() AT TIME ZONE 'UTC'), \
             updated_at = (now() AT TIME ZONE 'UTC') \
         WHERE id = $1 \
         RETURNING {COLLABORATION_RETURNING}"
    );
    let row: Option<CollaborationRow> = sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
    row.ok_or(ApiError::NotFound)
}

/// The raw record hash Canvas answers with (`render json: @collaboration`):
/// every column except `uuid`, which `Collaboration.serialization_excludes`
/// removes.
fn raw_collaboration_json(row: &CollaborationRow) -> Value {
    json!({
        "id": row.id,
        "collaboration_type": row.collaboration_type,
        "document_id": row.document_id,
        "user_id": row.user_id,
        "context_id": row.context_id,
        "context_type": row.context_type,
        "url": row.url,
        "data": row.data,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "description": row.description,
        "title": row.title,
        "workflow_state": row.workflow_state,
        "deleted_at": row.deleted_at,
        "context_code": row.context_code,
        "type": row.kind,
        "resource_link_lookup_uuid": row.resource_link_lookup_uuid,
        "root_account_id": row.root_account_id,
    })
}

/// The shared body of `destroy`.
async fn collaboration_destroy(
    state: &AppState,
    ctx: CollaborationsContext,
    id: i64,
    auth: &AuthUser,
) -> Result<Response, ApiError> {
    // `require_collaborations_configured`
    if !collaborations_configured(&state.db, &ctx).await? {
        return Ok(collaborations_not_enabled_response());
    }
    // `@collaboration = @context.collaborations.find(params[:id])`
    let row = load_context_collaboration(&state.db, &ctx, id).await?;
    // `authorized_action(@collaboration, @current_user, :delete)`
    if !can_change_collaboration(&state.db, auth, &ctx).await? {
        return Err(ApiError::Forbidden);
    }
    // `delete_doc` would remove the file in the external document service;
    // star-lms has no such service, so the parameter is accepted and ignored.
    let deleted = soft_delete_collaboration(&state.db, row.id).await?;
    Ok(Json(raw_collaboration_json(&deleted)).into_response())
}

/// `DELETE /courses/{course_id}/collaborations/{id}` — Canvas `destroy`.
async fn course_collaboration_destroy(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = resolve_course_context(&state.db, course_id).await?;
    collaboration_destroy(&state, ctx, id, &auth).await
}

/// `DELETE /groups/{group_id}/collaborations/{id}` — Canvas `destroy`.
async fn group_collaboration_destroy(
    State(state): State<AppState>,
    Path((group_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = resolve_group_context(&state.db, group_id).await?;
    collaboration_destroy(&state, ctx, id, &auth).await
}
