//! star-modules domain crate — course modules and module items.
//!
//! Canvas module item detail routes (migration 0065):
//! - GET/PUT/DELETE /courses/{courseId}/modules/items/{itemId}
//! - POST /courses/{courseId}/modules/items/{itemId}/duplicate
//! - GET/PUT /courses/{courseId}/modules/{moduleId}/items/{itemId}
//!
//! The Phase 2 module CRUD, item add/remove and item done routes are kept
//! unchanged.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};

use star_common::{ApiError, AppState};

/// Authenticated user — role must be looked up from DB in handlers.
use star_auth::AuthUser;

// --------------------------------------------------------------------------
// Data types
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Module {
    pub id: i64,
    pub course_id: i64,
    pub name: String,
    pub position: i32,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ModuleItem {
    pub id: i64,
    pub module_id: i64,
    pub item_type: String,
    pub item_id: i64,
    pub position: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleWithItems {
    #[serde(flatten)]
    pub module: Module,
    pub items: Vec<ModuleItem>,
}

/// Per-user module item completion state (Canvas "mark item as done").
/// `state` is 'completed' after PUT .../done and 'incomplete' after
/// DELETE .../done — Canvas flips a ContextModuleProgression requirement
/// instead of dropping history, so rows are upserted, never removed.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ModuleItemCompletion {
    pub id: i64,
    pub module_id: i64,
    pub item_id: i64,
    pub user_id: i64,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateModule {
    pub name: String,
    pub position: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateModule {
    pub name: Option<String>,
    pub position: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct CreateModuleItem {
    pub item_type: String,
    pub item_id: i64,
    pub position: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct ListModulesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Course pace — mirrors Canvas CoursePace (app/models/course_pace.rb).
/// Workflow states: unpublished | active | deleted.
/// Context can be Course (primary pace, no section/user), CourseSection, or User.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CoursePace {
    pub id: i64,
    pub course_id: i64,
    pub course_section_id: Option<i64>,
    pub user_id: Option<i64>,
    pub root_account_id: i64,
    pub workflow_state: String,
    pub exclude_weekends: bool,
    pub selected_days_to_skip: Option<serde_json::Value>, // JSONB array e.g. ["sun","sat"]
    pub hard_end_dates: bool,
    pub end_date: Option<DateTime<Utc>>,
    pub published_at: Option<DateTime<Utc>>,
    pub assignments_weighting: Option<serde_json::Value>, // JSONB hash
    pub time_to_complete_calendar_days: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// course_pace_module_item — links a CoursePace to a content_tag (module_item_id).
/// Mirrors Canvas CoursePaceModuleItem (app/models/course_pace_module_item.rb).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CoursePaceModuleItem {
    pub id: i64,
    pub course_pace_id: i64,
    pub module_item_id: i64,
    pub root_account_id: i64,
    pub duration: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input for creating a course pace.
#[derive(Debug, Deserialize)]
pub struct CreateCoursePace {
    pub course_section_id: Option<i64>,
    pub user_id: Option<i64>,
    pub end_date: Option<DateTime<Utc>>,
    pub exclude_weekends: Option<bool>,
    pub selected_days_to_skip: Option<Vec<String>>,
    pub hard_end_dates: Option<bool>,
    pub workflow_state: Option<String>,
    pub time_to_complete_calendar_days: Option<i32>,
    pub assignments_weighting: Option<serde_json::Value>,
}

/// Input for updating a course pace (partial).
#[derive(Debug, Deserialize)]
pub struct UpdateCoursePace {
    pub end_date: Option<DateTime<Utc>>,
    pub exclude_weekends: Option<bool>,
    pub selected_days_to_skip: Option<Vec<String>>,
    pub hard_end_dates: Option<bool>,
    pub workflow_state: Option<String>,
    pub time_to_complete_calendar_days: Option<i32>,
    pub assignments_weighting: Option<serde_json::Value>,
}

/// Nested module item attributes inside course pace create/update bodies.
/// Mirrors Canvas `course_pace_module_items_attributes: %i[id duration module_item_id root_account_id]`.
#[derive(Debug, Deserialize)]
pub struct CoursePaceModuleItemAttrs {
    pub id: Option<i64>,
    pub duration: i32,
    pub module_item_id: i64,
    pub root_account_id: i64,
}

/// Wrapper body accepted by both create and update.
#[derive(Debug, Deserialize)]
pub struct CoursePaceBody {
    #[serde(flatten)]
    pub base: CreateCoursePace,
    pub course_pace_module_items_attributes: Option<Vec<CoursePaceModuleItemAttrs>>,
}

/// Mutable module item attributes accepted by the Canvas
/// `context_module_items_api#update` endpoints (Phase 3, migration 0065).
#[derive(Debug, Default, Deserialize)]
pub struct ModuleItemUpdate {
    pub title: Option<String>,
    pub indent: Option<i32>,
    pub position: Option<i32>,
    pub published: Option<bool>,
    /// Move the item into another module of the same course.
    pub module_id: Option<i64>,
}

impl ModuleItemUpdate {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.indent.is_none()
            && self.position.is_none()
            && self.published.is_none()
            && self.module_id.is_none()
    }
}

/// Canvas request envelope. Both the documented `{"module_item": {...}}`
/// shape and a flat `{"title": ..., "indent": ...}` body are accepted; flat
/// fields are the same ones the crate's other module routes take.
#[derive(Debug, Deserialize)]
pub struct UpdateModuleItemBody {
    pub module_item: Option<ModuleItemUpdate>,
    #[serde(flatten)]
    pub direct: ModuleItemUpdate,
}

/// Joined module item used by the Phase 3 detail/update/duplicate routes.
/// `content_title`/`page_url`/`points_possible`/`due_at` come from the linked
/// assignment or page so the response can mirror Canvas module_item_json.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ModuleItemDetail {
    pub id: i64,
    pub module_id: i64,
    pub course_id: i64,
    pub item_type: String,
    pub item_id: i64,
    pub position: i32,
    pub title: Option<String>,
    pub indent: i32,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub content_title: Option<String>,
    pub page_url: Option<String>,
    pub points_possible: Option<f64>,
    pub due_at: Option<DateTime<Utc>>,
}

/// `include[]=content_details` (Canvas accepts both the bracketed and plain
/// spellings; only the single-value form is meaningful for one item).
#[derive(Debug, Default, Deserialize)]
pub struct ItemDetailQuery {
    pub include: Option<String>,
    #[serde(rename = "include[]")]
    pub include_bracket: Option<String>,
}

impl ItemDetailQuery {
    fn wants_content_details(&self) -> bool {
        [&self.include, &self.include_bracket]
            .into_iter()
            .flatten()
            .any(|value| {
                value
                    .split(',')
                    .any(|include| include.trim() == "content_details")
            })
    }
}

// --------------------------------------------------------------------------
// Permission helpers
// --------------------------------------------------------------------------

fn is_teacher_or_admin(auth: &AuthUser) -> bool {
    auth.role == "teacher" || auth.role == "admin"
}

async fn require_teacher_or_admin(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    if is_teacher_or_admin(auth) {
        // For now, skip DB check if already admin via role
        return Ok(());
    }

    let row: Option<(String,)> =
        sqlx::query_as(r#"SELECT role FROM enrollments WHERE user_id = $1 AND course_id = $2"#)
            .bind(user_id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;

    match row {
        Some((role,)) if role == "teacher" || role == "admin" => Ok(()),
        _ => Err(ApiError::Forbidden),
    }
}

/// Course read access for module listing. Mirrors Canvas
/// ContextModulesApiController#index, which gates GET /courses/:course_id/modules
/// behind `authorized_action(@context, @current_user, :read)` — the course
/// `:read` right. Anonymous callers have no @current_user and are rejected 401
/// by the AuthUser extractor; admins (by session role) may read any course;
/// everyone else needs an active enrollment (enrollments.workflow_state =
/// 'active', per star-db migration 0010) in the course.
async fn require_course_read_access(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }

    let enrolled: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM enrollments
        WHERE user_id = $1 AND course_id = $2 AND workflow_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    match enrolled {
        Some(_) => Ok(()),
        None => Err(ApiError::Forbidden),
    }
}

// --------------------------------------------------------------------------
// Nested-route guards
// --------------------------------------------------------------------------

/// Canvas nested-alias guard: 404 unless the module belongs to the course.
/// Soft-deleted modules are left to the delegated handler, which either
/// filters them (item create) or intentionally does not (item delete).
async fn require_module_in_course(
    db: &PgPool,
    course_id: i64,
    module_id: i64,
) -> Result<(), ApiError> {
    let exists: Option<(i32,)> =
        sqlx::query_as(r#"SELECT 1 FROM modules WHERE id = $1 AND course_id = $2"#)
            .bind(module_id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;

    if exists.is_none() {
        return Err(ApiError::NotFound);
    }

    Ok(())
}

/// Canvas nested-alias guard for module detail/update/delete: 404 unless the
/// module belongs to the course and has not been soft-deleted.
async fn require_visible_module_in_course(
    db: &PgPool,
    course_id: i64,
    module_id: i64,
) -> Result<(), ApiError> {
    let exists: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM modules
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(module_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    if exists.is_none() {
        return Err(ApiError::NotFound);
    }

    Ok(())
}

/// Canvas context_module_items_api#get_module_item parity: 404 unless the
/// item exists inside the module named by the nested route.
async fn require_item_in_module(db: &PgPool, module_id: i64, item_id: i64) -> Result<(), ApiError> {
    let exists: Option<(i32,)> =
        sqlx::query_as(r#"SELECT 1 FROM module_items WHERE id = $1 AND module_id = $2"#)
            .bind(item_id)
            .bind(module_id)
            .fetch_optional(db)
            .await?;

    if exists.is_none() {
        return Err(ApiError::NotFound);
    }

    Ok(())
}

/// Shared guard for PUT/DELETE .../items/{itemId}/done. Canvas authorizes with
/// `authorized_action(@context, @current_user, :read)` before resolving the
/// module/item, so course read access (active enrollment, or admin) is checked
/// first, then the module and item must resolve inside the course.
async fn require_done_access(
    db: &PgPool,
    course_id: i64,
    module_id: i64,
    item_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    require_course_read_access(db, auth.id, course_id, auth).await?;
    require_visible_module_in_course(db, course_id, module_id).await?;
    require_item_in_module(db, module_id, item_id).await
}

// --------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------

/// GET /courses/{courseId}/modules
///
/// Authenticated course read access: admins may read any course; other users
/// must have an active enrollment in the course (see require_course_read_access).
async fn list_modules(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<ListModulesQuery>,
) -> Result<Json<Vec<Module>>, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;

    let limit = params.limit.unwrap_or(100).min(100);
    let offset = params.offset.unwrap_or(0);

    let modules = sqlx::query_as::<_, Module>(
        r#"
        SELECT id, course_id, name, position, workflow_state, created_at, updated_at
        FROM modules
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        ORDER BY position ASC, id ASC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(course_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(modules))
}

/// POST /courses/{courseId}/modules
async fn create_module(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<CreateModule>,
) -> Result<(StatusCode, Json<Module>), ApiError> {
    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let exists: Option<(i64,)> = sqlx::query_as(r#"SELECT id FROM courses WHERE id = $1"#)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;

    if exists.is_none() {
        return Err(ApiError::NotFound);
    }

    let module = sqlx::query_as::<_, Module>(
        r#"
        INSERT INTO modules (course_id, name, position, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id, course_id, name, position, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&body.name)
    .bind(body.position.unwrap_or(0))
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(module)))
}

/// GET /modules/{id}
async fn get_module(
    State(state): State<AppState>,
    Path(module_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<ModuleWithItems>, ApiError> {
    let module: Option<Module> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, workflow_state, created_at, updated_at
        FROM modules WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(module_id)
    .fetch_optional(&state.db)
    .await?;

    let module = module.ok_or(ApiError::NotFound)?;
    require_course_read_access(&state.db, auth.id, module.course_id, &auth).await?;

    let items = sqlx::query_as::<_, ModuleItem>(
        r#"SELECT id, module_id, item_type, item_id, position, created_at, updated_at
           FROM module_items WHERE module_id = $1 ORDER BY position ASC, id ASC"#,
    )
    .bind(module_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(ModuleWithItems { module, items }))
}

/// PUT /modules/{id}
async fn update_module(
    State(state): State<AppState>,
    Path(module_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<UpdateModule>,
) -> Result<Json<Module>, ApiError> {
    let module: Option<Module> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, workflow_state, created_at, updated_at
        FROM modules WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(module_id)
    .fetch_optional(&state.db)
    .await?;

    let module = module.ok_or(ApiError::NotFound)?;
    require_teacher_or_admin(&state.db, auth.id, module.course_id, &auth).await?;

    let updated: Module = sqlx::query_as(
        r#"
        UPDATE modules
        SET name = COALESCE($1, name), position = COALESCE($2, position)
        WHERE id = $3
        RETURNING id, course_id, name, position, workflow_state, created_at, updated_at
        "#,
    )
    .bind(&body.name)
    .bind(body.position)
    .bind(module_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /modules/{id}
async fn delete_module(
    State(state): State<AppState>,
    Path(module_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let module: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM modules WHERE id = $1"#)
        .bind(module_id)
        .fetch_optional(&state.db)
        .await?;

    let Some((course_id,)) = module else {
        return Err(ApiError::NotFound);
    };

    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    // Canvas parity: soft delete. wiki_page-style workflow_state transitions —
    // context_module.rb deletes mark the module 'deleted' rather than dropping
    // the row, so module_items and history survive.
    sqlx::query(r#"UPDATE modules SET workflow_state = 'deleted' WHERE id = $1"#)
        .bind(module_id)
        .execute(&state.db)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /modules/{id}/items
async fn create_or_update_item(
    State(state): State<AppState>,
    Path(module_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<CreateModuleItem>,
) -> Result<Json<ModuleItem>, ApiError> {
    if body.item_type != "page" && body.item_type != "assignment" {
        return Err(ApiError::Validation(
            "item_type must be 'page' or 'assignment'".into(),
        ));
    }

    let module: Option<Module> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, workflow_state, created_at, updated_at
        FROM modules WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(module_id)
    .fetch_optional(&state.db)
    .await?;

    let module = module.ok_or(ApiError::NotFound)?;
    require_teacher_or_admin(&state.db, auth.id, module.course_id, &auth).await?;

    let item = sqlx::query_as::<_, ModuleItem>(
        r#"INSERT INTO module_items (module_id, item_type, item_id, position)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (module_id, item_type, item_id)
           DO UPDATE SET position = EXCLUDED.position, updated_at = now()
           RETURNING id, module_id, item_type, item_id, position, created_at, updated_at"#,
    )
    .bind(module_id)
    .bind(&body.item_type)
    .bind(body.item_id)
    .bind(body.position.unwrap_or(0))
    .fetch_one(&state.db)
    .await?;

    Ok(Json(item))
}

/// DELETE /modules/{id}/items/{itemId}
async fn delete_module_item(
    State(state): State<AppState>,
    Path((module_id, item_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let module: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM modules WHERE id = $1"#)
        .bind(module_id)
        .fetch_optional(&state.db)
        .await?;

    let Some((course_id,)) = module else {
        return Err(ApiError::NotFound);
    };

    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let result = sqlx::query(r#"DELETE FROM module_items WHERE id = $1 AND module_id = $2"#)
        .bind(item_id)
        .bind(module_id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// GET /courses/{courseId}/modules/{moduleId}/items
async fn list_module_items(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<ModuleItem>>, ApiError> {
    let exists: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT id FROM modules
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(module_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    if exists.is_none() {
        return Err(ApiError::NotFound);
    }

    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;

    let items = sqlx::query_as::<_, ModuleItem>(
        r#"SELECT id, module_id, item_type, item_id, position, created_at, updated_at
           FROM module_items WHERE module_id = $1 ORDER BY position ASC, id ASC"#,
    )
    .bind(module_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(items))
}

// --------------------------------------------------------------------------
// Module item completion (Canvas mark-as-done)
// --------------------------------------------------------------------------

/// Upsert the caller's completion state for one module item. (user_id, item_id)
/// is unique, so repeated PUT/DELETE calls reuse the row and keep created_at
/// from the first completion.
async fn record_item_completion(
    db: &PgPool,
    module_id: i64,
    item_id: i64,
    user_id: i64,
    state: &str,
) -> Result<ModuleItemCompletion, ApiError> {
    let completion = sqlx::query_as::<_, ModuleItemCompletion>(
        r#"
        INSERT INTO module_item_completions (module_id, item_id, user_id, state)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (user_id, item_id)
        DO UPDATE SET state = EXCLUDED.state, updated_at = now()
        RETURNING id, module_id, item_id, user_id, state, created_at, updated_at
        "#,
    )
    .bind(module_id)
    .bind(item_id)
    .bind(user_id)
    .bind(state)
    .fetch_one(db)
    .await?;

    Ok(completion)
}

/// PUT /courses/{courseId}/modules/{moduleId}/items/{itemId}/done
///
/// Canvas context_module_items_api#mark_as_done. Any user with course read
/// access (i.e. an active enrollment, or an admin) may mark their own item
/// completion; no teacher role is required.
async fn mark_item_done(
    State(state): State<AppState>,
    Path((course_id, module_id, item_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<ModuleItemCompletion>, ApiError> {
    require_done_access(&state.db, course_id, module_id, item_id, &auth).await?;

    let completion =
        record_item_completion(&state.db, module_id, item_id, auth.id, "completed").await?;

    Ok(Json(completion))
}

/// DELETE /courses/{courseId}/modules/{moduleId}/items/{itemId}/done
///
/// Canvas context_module_items_api#mark_as_not_done: flips the caller's own
/// completion back to 'incomplete' (the row is kept, matching Canvas's
/// progression history semantics).
async fn mark_item_not_done(
    State(state): State<AppState>,
    Path((course_id, module_id, item_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<ModuleItemCompletion>, ApiError> {
    require_done_access(&state.db, course_id, module_id, item_id, &auth).await?;

    let completion =
        record_item_completion(&state.db, module_id, item_id, auth.id, "incomplete").await?;

    Ok(Json(completion))
}

// --------------------------------------------------------------------------
// Module item detail / update / duplicate (Canvas context_module_items_api)
// --------------------------------------------------------------------------

/// Canvas `Api::API_DATA_TYPE` for the item types this crate stores.
fn canvas_item_type(item_type: &str) -> &str {
    match item_type {
        "page" | "wiki_page" => "Page",
        "assignment" => "Assignment",
        "file" | "attachment" => "File",
        "quiz" => "Quiz",
        "topic" | "discussion" => "Discussion",
        "sub_header" | "subheader" => "SubHeader",
        "external_url" => "ExternalUrl",
        "external_tool" => "ExternalTool",
        other => other,
    }
}

/// Canvas-style page slug used as a fallback when `pages.url` is unset.
/// Mirrors star-pages' `generate_url_slug` (lowercase, separators collapse
/// to a single dash, punctuation is stripped).
fn page_slug(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_dash = true; // suppress leading dashes

    for ch in title.chars() {
        if ch.is_whitespace() || ch == '-' || ch == '_' {
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
    }

    if slug.ends_with('-') {
        slug.pop();
    }

    slug
}

/// Canvas module_item_json (lib/api/v1/context_module.rb) subset for the
/// types star-modules supports. Page items expose `page_url` instead of
/// `content_id`; assignment items expose `content_id` plus the assignment
/// API url. `content_details` is only emitted for `include[]=content_details`.
fn module_item_json(item: &ModuleItemDetail, include_content_details: bool) -> Value {
    let title = item
        .title
        .clone()
        .or_else(|| item.content_title.clone())
        .unwrap_or_default();

    let mut hash = serde_json::Map::new();
    hash.insert("id".into(), json!(item.id));
    hash.insert("position".into(), json!(item.position));
    hash.insert("title".into(), json!(title));
    hash.insert("indent".into(), json!(item.indent));
    hash.insert("type".into(), json!(canvas_item_type(&item.item_type)));
    hash.insert("module_id".into(), json!(item.module_id));
    hash.insert(
        "html_url".into(),
        json!(format!(
            "/courses/{}/modules/items/{}",
            item.course_id, item.id
        )),
    );

    match item.item_type.as_str() {
        "page" | "wiki_page" => {
            let slug = item.page_url.clone().unwrap_or_else(|| page_slug(&title));
            hash.insert("page_url".into(), json!(slug));
            hash.insert(
                "url".into(),
                json!(format!("/api/v1/courses/{}/pages/{}", item.course_id, slug)),
            );
        }
        "assignment" => {
            hash.insert("content_id".into(), json!(item.item_id));
            hash.insert(
                "url".into(),
                json!(format!(
                    "/api/v1/courses/{}/assignments/{}",
                    item.course_id, item.item_id
                )),
            );
        }
        _ => {
            hash.insert("content_id".into(), json!(item.item_id));
        }
    }

    hash.insert("published".into(), json!(item.workflow_state == "active"));

    if include_content_details {
        let mut details = serde_json::Map::new();
        if let Some(points_possible) = item.points_possible {
            details.insert("points_possible".into(), json!(points_possible));
        }
        if let Some(due_at) = item.due_at {
            details.insert("due_at".into(), json!(due_at));
        }
        hash.insert("content_details".into(), Value::Object(details));
    }

    Value::Object(hash)
}

/// Resolve the item, its owning course, and its linked content. `module_id`
/// constrains the lookup for the nested Canvas alias. Soft-deleted modules
/// hide their items (`context_module_tags.not_deleted` parity).
async fn load_module_item(
    db: &PgPool,
    course_id: i64,
    module_id: Option<i64>,
    item_id: i64,
) -> Result<ModuleItemDetail, ApiError> {
    let item: Option<ModuleItemDetail> = sqlx::query_as(
        r#"
        SELECT mi.id, mi.module_id, m.course_id, mi.item_type, mi.item_id,
               mi.position, mi.title, mi.indent, mi.workflow_state,
               mi.created_at, mi.updated_at,
               COALESCE(a.title, p.title) AS content_title,
               p.url AS page_url,
               a.points_possible::float8 AS points_possible,
               a.due_at AS due_at
        FROM module_items mi
        JOIN modules m ON m.id = mi.module_id
        LEFT JOIN assignments a
               ON mi.item_type = 'assignment' AND a.id = mi.item_id
        LEFT JOIN pages p
               ON mi.item_type = 'page' AND p.id = mi.item_id
        WHERE mi.id = $1
          AND ($2::bigint IS NULL OR mi.module_id = $2)
          AND m.course_id = $3
          AND m.workflow_state <> 'deleted'
        "#,
    )
    .bind(item_id)
    .bind(module_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    item.ok_or(ApiError::NotFound)
}

/// Item ids in Canvas list order (position ASC, id ASC).
async fn module_item_ids(conn: &mut PgConnection, module_id: i64) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM module_items WHERE module_id = $1 ORDER BY position ASC, id ASC",
    )
    .bind(module_id)
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Rewrite positions as a contiguous 1-based list, like Canvas ContentTag
/// `insert_at` does for a module's siblings.
async fn write_item_positions(conn: &mut PgConnection, ids: &[i64]) -> Result<(), ApiError> {
    for (index, id) in ids.iter().enumerate() {
        sqlx::query("UPDATE module_items SET position = $1, updated_at = now() WHERE id = $2")
            .bind(index as i32 + 1)
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }

    Ok(())
}

/// Move `item_id` to the requested 1-based position (Canvas `insert_at`);
/// a missing position appends the item at the end of the module.
async fn place_module_item(
    conn: &mut PgConnection,
    module_id: i64,
    item_id: i64,
    position: Option<i32>,
) -> Result<(), ApiError> {
    let mut ids = module_item_ids(&mut *conn, module_id).await?;
    ids.retain(|id| *id != item_id);

    let index = match position {
        Some(position) => ((position.max(1) - 1) as usize).min(ids.len()),
        None => ids.len(),
    };
    ids.insert(index, item_id);

    write_item_positions(&mut *conn, &ids).await
}

fn missing_module_item_parameter() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "message": "missing module item parameter" })),
    )
}

fn invalid_module_id() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "message": "invalid module_id" })),
    )
}

fn item_cannot_be_duplicated() -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "message": "Item cannot be duplicated" })),
    )
}

/// Persist one module item update. Position changes renumber the module's
/// items (Canvas `insert_at`); moving modules also closes the gap left in the
/// source module and appends to the target unless a position was given.
async fn write_module_item_update(
    db: &PgPool,
    item: &ModuleItemDetail,
    update: &ModuleItemUpdate,
) -> Result<ModuleItemDetail, ApiError> {
    let target_module = update.module_id.unwrap_or(item.module_id);
    let moving = target_module != item.module_id;
    let published_state = update
        .published
        .map(|published| if published { "active" } else { "unpublished" });

    let mut tx = db.begin().await?;

    sqlx::query(
        r#"
        UPDATE module_items
        SET title = COALESCE($1::text, title),
            indent = COALESCE($2::int, indent),
            workflow_state = COALESCE($3::text, workflow_state),
            module_id = $4,
            updated_at = now()
        WHERE id = $5
        "#,
    )
    .bind(update.title.as_deref())
    .bind(update.indent)
    .bind(published_state)
    .bind(target_module)
    .bind(item.id)
    .execute(&mut *tx)
    .await?;

    if moving {
        let remaining = module_item_ids(&mut tx, item.module_id).await?;
        write_item_positions(&mut tx, &remaining).await?;
        place_module_item(&mut tx, target_module, item.id, update.position).await?;
    } else if update.position.is_some() {
        place_module_item(&mut tx, item.module_id, item.id, update.position).await?;
    }

    tx.commit().await?;

    load_module_item(db, item.course_id, None, item.id).await
}

/// Shared body handling for both Canvas update routes. Canvas returns
/// `{"message": "missing module item parameter"}` (400) when no
/// `module_item` attributes are present and `{"message": "invalid module_id"}`
/// when a move targets a module outside the course.
async fn apply_module_item_update(
    state: &AppState,
    course_id: i64,
    item: ModuleItemDetail,
    body: UpdateModuleItemBody,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let update = match body.module_item {
        Some(update) => update,
        None if !body.direct.is_empty() => body.direct,
        None => return Ok(missing_module_item_parameter()),
    };

    if let Some(target_module) = update.module_id {
        if target_module != item.module_id {
            let exists: Option<(i32,)> = sqlx::query_as(
                r#"
                SELECT 1 FROM modules
                WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
                "#,
            )
            .bind(target_module)
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;

            if exists.is_none() {
                return Ok(invalid_module_id());
            }
        }
    }

    let updated = write_module_item_update(&state.db, &item, &update).await?;

    Ok((StatusCode::OK, Json(module_item_json(&updated, false))))
}

/// GET /courses/{courseId}/modules/items/{itemId}
///
/// Canvas show authorizes `:read` on the course before resolving the item, so
/// unauthorized callers get 403 even for a missing item.
async fn get_item_detail(
    State(state): State<AppState>,
    Path((course_id, item_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<ItemDetailQuery>,
) -> Result<Json<Value>, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;

    let item = load_module_item(&state.db, course_id, None, item_id).await?;

    Ok(Json(module_item_json(
        &item,
        params.wants_content_details(),
    )))
}

/// PUT /courses/{courseId}/modules/items/{itemId}
///
/// Canvas update resolves the item (`not_deleted`) before authorizing, then
/// requires `:update` on the owning module.
async fn update_item_detail(
    State(state): State<AppState>,
    Path((course_id, item_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<UpdateModuleItemBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let item = load_module_item(&state.db, course_id, None, item_id).await?;
    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    apply_module_item_update(&state, course_id, item, body).await
}

/// DELETE /courses/{courseId}/modules/items/{itemId}
///
/// Canvas destroy drops the ContentTag (the linked page/assignment survives)
/// and returns the deleted module item JSON with 200.
async fn delete_item_detail(
    State(state): State<AppState>,
    Path((course_id, item_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let item = load_module_item(&state.db, course_id, None, item_id).await?;
    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    sqlx::query("DELETE FROM module_items WHERE id = $1")
        .bind(item.id)
        .execute(&state.db)
        .await?;

    Ok((StatusCode::OK, Json(module_item_json(&item, false))))
}

/// POST /courses/{courseId}/modules/items/{itemId}/duplicate
///
/// Canvas duplicates the linked assignment/wiki page and inserts a new module
/// item right after the original (`insert_at(original.position + 1)`).
async fn duplicate_item(
    State(state): State<AppState>,
    Path((course_id, item_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let item = load_module_item(&state.db, course_id, None, item_id).await?;
    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let mut tx = state.db.begin().await?;

    // Normalize first so the original's stored position is its 1-based index.
    let ordered = module_item_ids(&mut tx, item.module_id).await?;
    write_item_positions(&mut tx, &ordered).await?;
    let original_index = ordered.iter().position(|id| *id == item.id).unwrap_or(0);

    // The (module_id, item_type, item_id) unique index forbids pointing the
    // copy at the original content, so duplicate the content row too.
    let new_content_id: Option<i64> = match item.item_type.as_str() {
        "page" => {
            sqlx::query_scalar::<_, i64>(
                r#"
                INSERT INTO pages (course_id, title, body)
                SELECT course_id, title || ' Copy', body
                FROM pages WHERE id = $1
                RETURNING id
                "#,
            )
            .bind(item.item_id)
            .fetch_optional(&mut *tx)
            .await?
        }
        "assignment" => {
            sqlx::query_scalar::<_, i64>(
                r#"
                INSERT INTO assignments (course_id, title, description, points_possible, due_at)
                SELECT course_id, title || ' Copy', description, points_possible, due_at
                FROM assignments WHERE id = $1
                RETURNING id
                "#,
            )
            .bind(item.item_id)
            .fetch_optional(&mut *tx)
            .await?
        }
        // Canvas duplicate_able? covers assignments, discussions and pages.
        _ => None,
    };

    let Some(new_content_id) = new_content_id else {
        tx.rollback().await?;
        return Ok(item_cannot_be_duplicated());
    };

    let new_item_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO module_items (module_id, item_type, item_id, position, indent)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(item.module_id)
    .bind(&item.item_type)
    .bind(new_content_id)
    .bind(ordered.len() as i32 + 1)
    .bind(item.indent)
    .fetch_one(&mut *tx)
    .await?;

    let mut reordered = ordered;
    let insert_at = (original_index + 1).min(reordered.len());
    reordered.insert(insert_at, new_item_id);
    write_item_positions(&mut tx, &reordered).await?;

    let new_positions: Vec<(i64, i32)> = sqlx::query_as(
        r#"
        SELECT id, position FROM module_items
        WHERE module_id = $1 ORDER BY position ASC, id ASC
        "#,
    )
    .bind(item.module_id)
    .fetch_all(&mut *tx)
    .await?;

    tx.commit().await?;

    let new_item = load_module_item(&state.db, course_id, None, new_item_id).await?;
    let mut value = module_item_json(&new_item, true);
    value["new_positions"] = json!(new_positions
        .into_iter()
        .map(|(id, position)| json!({ "id": id, "position": position }))
        .collect::<Vec<_>>());

    Ok((StatusCode::OK, Json(value)))
}

/// PUT /courses/{courseId}/modules/{moduleId}/items/{itemId}
///
/// Canvas module-scoped update. The item must belong to the module named in
/// the path (the crate's nested-alias ownership rule).
async fn update_module_scoped_item(
    State(state): State<AppState>,
    Path((course_id, module_id, item_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(body): Json<UpdateModuleItemBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let item = load_module_item(&state.db, course_id, Some(module_id), item_id).await?;
    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    apply_module_item_update(&state, course_id, item, body).await
}

/// GET /courses/{courseId}/modules/{moduleId}/items/{itemId}
///
/// Canvas `context_module_items_api#show` scoped by module (the canonical
/// documented path); the item must belong to the module in the path.
async fn get_module_scoped_item(
    State(state): State<AppState>,
    Path((course_id, module_id, item_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Query(params): Query<ItemDetailQuery>,
) -> Result<Json<Value>, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;

    let item = load_module_item(&state.db, course_id, Some(module_id), item_id).await?;

    Ok(Json(module_item_json(
        &item,
        params.wants_content_details(),
    )))
}

// --------------------------------------------------------------------------
// Canvas nested route aliases
// --------------------------------------------------------------------------

/// GET /courses/{courseId}/modules/{moduleId} — Canvas v1 alias for GET /modules/{id}.
async fn get_course_module(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<ModuleWithItems>, ApiError> {
    require_visible_module_in_course(&state.db, course_id, module_id).await?;

    get_module(State(state), Path(module_id), auth).await
}

/// PUT /courses/{courseId}/modules/{moduleId} — Canvas v1 alias for PUT /modules/{id}.
async fn update_course_module(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<UpdateModule>,
) -> Result<Json<Module>, ApiError> {
    require_visible_module_in_course(&state.db, course_id, module_id).await?;

    update_module(State(state), Path(module_id), auth, Json(body)).await
}

/// DELETE /courses/{courseId}/modules/{moduleId} — Canvas v1 alias for DELETE /modules/{id}.
async fn delete_course_module(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_visible_module_in_course(&state.db, course_id, module_id).await?;

    delete_module(State(state), Path(module_id), auth).await
}

/// POST /courses/{courseId}/modules/{moduleId}/items — Canvas v1 alias for
/// POST /modules/{id}/items.
async fn create_course_module_item(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<CreateModuleItem>,
) -> Result<Json<ModuleItem>, ApiError> {
    require_module_in_course(&state.db, course_id, module_id).await?;

    create_or_update_item(State(state), Path(module_id), auth, Json(body)).await
}

/// DELETE /courses/{courseId}/modules/{moduleId}/items/{itemId} — Canvas v1 alias
/// for DELETE /modules/{id}/items/{itemId}.
async fn delete_course_module_item(
    State(state): State<AppState>,
    Path((course_id, module_id, item_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_module_in_course(&state.db, course_id, module_id).await?;

    let item: Option<(i32,)> =
        sqlx::query_as(r#"SELECT 1 FROM module_items WHERE id = $1 AND module_id = $2"#)
            .bind(item_id)
            .bind(module_id)
            .fetch_optional(&state.db)
            .await?;

    if item.is_none() {
        return Err(ApiError::NotFound);
    }

    delete_module_item(State(state), Path((module_id, item_id)), auth).await
}

// --------------------------------------------------------------------------
// Course Pace handlers  (migration 0213)
// --------------------------------------------------------------------------

/// Guard: require teacher or admin on the course.
async fn require_pace_teacher_or_admin(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(String,)> =
        sqlx::query_as(r#"SELECT role FROM enrollments WHERE user_id = $1 AND course_id = $2"#)
            .bind(user_id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    match row {
        Some((role,)) if role == "teacher" || role == "admin" => Ok(()),
        _ => Err(ApiError::Forbidden),
    }
}

/// Load a course_pace by id, verifying it belongs to the given course.
async fn load_pace(db: &PgPool, course_id: i64, pace_id: i64) -> Result<CoursePace, ApiError> {
    let pace: Option<CoursePace> = sqlx::query_as(
        r#"
        SELECT id, course_id, course_section_id, user_id, root_account_id,
               workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates,
               end_date, published_at, assignments_weighting, time_to_complete_calendar_days,
               created_at, updated_at
        FROM course_paces
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(pace_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    pace.ok_or(ApiError::NotFound)
}

/// GET /courses/{courseId}/course_pacing
/// Returns the primary course pace (no section/user) for the course.
async fn get_course_pace(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<CoursePace>, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;

    let pace: Option<CoursePace> = sqlx::query_as(
        r#"
        SELECT id, course_id, course_section_id, user_id, root_account_id,
               workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates,
               end_date, published_at, assignments_weighting, time_to_complete_calendar_days,
               created_at, updated_at
        FROM course_paces
        WHERE course_id = $1 AND course_section_id IS NULL AND user_id IS NULL
          AND workflow_state <> 'deleted'
        ORDER BY id ASC LIMIT 1
        "#,
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    match pace {
        Some(p) => Ok(Json(p)),
        None => Err(ApiError::NotFound),
    }
}

/// POST /courses/{courseId}/course_pacing
async fn create_course_pace(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<CoursePaceBody>,
) -> Result<(StatusCode, Json<CoursePace>), ApiError> {
    require_pace_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    // Verify course exists
    let course_exists: Option<(i64,)> = sqlx::query_as(r#"SELECT id FROM courses WHERE id = $1"#)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;
    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Exactly one of section_id or user_id may be set
    let section_id = body.base.course_section_id;
    let user_id = body.base.user_id;
    if section_id.is_some() && user_id.is_some() {
        return Err(ApiError::Validation(
            "Only one of course_section_id and user_id may be set".into(),
        ));
    }

    // Look up root_account_id from the course
    let root_account_id: i64 =
        sqlx::query_scalar(r#"SELECT root_account_id FROM courses WHERE id = $1"#)
            .bind(course_id)
            .fetch_one(&state.db)
            .await
            .map_err(|_| ApiError::NotFound)?;

    let selected_days_json: Option<serde_json::Value> = body
        .base
        .selected_days_to_skip
        .as_ref()
        .map(|days| serde_json::to_value(days).unwrap_or(serde_json::Value::Null));

    let workflow_state = body.base.workflow_state.as_deref().unwrap_or("unpublished");

    let pace: CoursePace = sqlx::query_as(
        r#"
        INSERT INTO course_paces
          (course_id, course_section_id, user_id, root_account_id, workflow_state,
           exclude_weekends, selected_days_to_skip, hard_end_dates, end_date,
           assignments_weighting, time_to_complete_calendar_days)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, course_id, course_section_id, user_id, root_account_id,
                  workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates,
                  end_date, published_at, assignments_weighting, time_to_complete_calendar_days,
                  created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(section_id)
    .bind(user_id)
    .bind(root_account_id)
    .bind(workflow_state)
    .bind(body.base.exclude_weekends.unwrap_or(false))
    .bind(selected_days_json)
    .bind(body.base.hard_end_dates.unwrap_or(false))
    .bind(body.base.end_date)
    .bind(body.base.assignments_weighting.clone())
    .bind(body.base.time_to_complete_calendar_days)
    .fetch_one(&state.db)
    .await?;

    // Handle nested module items if provided
    if let Some(items) = &body.course_pace_module_items_attributes {
        for item in items {
            sqlx::query(
                r#"
                INSERT INTO course_pace_module_items
                  (course_pace_id, module_item_id, root_account_id, duration)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (course_pace_id, module_item_id) DO NOTHING
                "#,
            )
            .bind(pace.id)
            .bind(item.module_item_id)
            .bind(item.root_account_id)
            .bind(item.duration)
            .execute(&state.db)
            .await?;
        }
    }

    Ok((StatusCode::CREATED, Json(pace)))
}

/// GET /courses/{courseId}/course_pacing/{id}
async fn get_single_pace(
    State(state): State<AppState>,
    Path((course_id, pace_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<CoursePace>, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;
    let pace = load_pace(&state.db, course_id, pace_id).await?;
    Ok(Json(pace))
}

/// PUT /courses/{courseId}/course_pacing/{id}
async fn update_pace(
    State(state): State<AppState>,
    Path((course_id, pace_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<CoursePaceBody>,
) -> Result<Json<CoursePace>, ApiError> {
    require_pace_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let _ = load_pace(&state.db, course_id, pace_id).await?;

    let selected_days_json: Option<serde_json::Value> = body
        .base
        .selected_days_to_skip
        .as_ref()
        .map(|days| serde_json::to_value(days).unwrap_or(serde_json::Value::Null));

    let pace: CoursePace = sqlx::query_as(
        r#"
        UPDATE course_paces
        SET end_date               = COALESCE($1, end_date),
            exclude_weekends       = COALESCE($2, exclude_weekends),
            selected_days_to_skip  = COALESCE($3, selected_days_to_skip),
            hard_end_dates        = COALESCE($4, hard_end_dates),
            workflow_state        = COALESCE($5, workflow_state),
            time_to_complete_calendar_days = COALESCE($6, time_to_complete_calendar_days),
            assignments_weighting  = COALESCE($7, assignments_weighting),
            updated_at            = now()
        WHERE id = $8 AND course_id = $9 AND workflow_state <> 'deleted'
        RETURNING id, course_id, course_section_id, user_id, root_account_id,
                  workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates,
                  end_date, published_at, assignments_weighting, time_to_complete_calendar_days,
                  created_at, updated_at
        "#,
    )
    .bind(body.base.end_date)
    .bind(body.base.exclude_weekends)
    .bind(selected_days_json)
    .bind(body.base.hard_end_dates)
    .bind(body.base.workflow_state.as_deref())
    .bind(body.base.time_to_complete_calendar_days)
    .bind(body.base.assignments_weighting.as_ref())
    .bind(pace_id)
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;

    // Sync module items if provided
    if let Some(items) = &body.course_pace_module_items_attributes {
        // Delete existing and replace
        sqlx::query(r#"DELETE FROM course_pace_module_items WHERE course_pace_id = $1"#)
            .bind(pace_id)
            .execute(&state.db)
            .await?;
        for item in items {
            sqlx::query(
                r#"
                INSERT INTO course_pace_module_items
                  (course_pace_id, module_item_id, root_account_id, duration)
                VALUES ($1, $2, $3, $4)
                "#,
            )
            .bind(pace_id)
            .bind(item.module_item_id)
            .bind(item.root_account_id)
            .bind(item.duration)
            .execute(&state.db)
            .await?;
        }
    }

    Ok(Json(pace))
}

/// DELETE /courses/{courseId}/course_pacing/{id}
async fn delete_pace(
    State(state): State<AppState>,
    Path((course_id, pace_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_pace_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let pace = load_pace(&state.db, course_id, pace_id).await?;

    // Cannot delete the primary published pace
    if pace.course_section_id.is_none() && pace.user_id.is_none() && pace.workflow_state == "active"
    {
        return Err(ApiError::Validation(
            "Cannot delete the default course pace".into(),
        ));
    }

    // Soft delete
    sqlx::query(
        r#"UPDATE course_paces SET workflow_state = 'deleted', updated_at = now() WHERE id = $1"#,
    )
    .bind(pace_id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /courses/{courseId}/course_pacing/{id}/publish
/// Marks the pace as active (published).
async fn publish_pace(
    State(state): State<AppState>,
    Path((course_id, pace_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<CoursePace>, ApiError> {
    require_pace_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;
    let _ = load_pace(&state.db, course_id, pace_id).await?;

    let pace: CoursePace = sqlx::query_as(
        r#"
        UPDATE course_paces
        SET workflow_state = 'active', published_at = now(), updated_at = now()
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        RETURNING id, course_id, course_section_id, user_id, root_account_id,
                  workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates,
                  end_date, published_at, assignments_weighting, time_to_complete_calendar_days,
                  created_at, updated_at
        "#,
    )
    .bind(pace_id)
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(pace))
}

// --------------------------------------------------------------------------
// M1: Module item sequence  (item_sequence)
// GET /courses/{courseId}/module_item_sequence?asset_type=...&asset_id=...
// Returns previous/next module items around the given asset.
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ItemSequenceQuery {
    pub asset_type: String,
    pub asset_id: String,
}

/// Map Canvas API asset types to internal content type names.
fn canvas_asset_type(asset_type: &str) -> Option<&'static str> {
    match asset_type {
        "ModuleItem" => Some("ContentTag"),
        "File" => Some("Attachment"),
        "Page" => Some("WikiPage"),
        "Discussion" => Some("DiscussionTopic"),
        "Assignment" => Some("Assignment"),
        "Quiz" => Some("Quizzes::Quiz"),
        "ExternalTool" => Some("ExternalTool"),
        _ => None,
    }
}

/// Build the module-item-sequence response for an asset.
/// Returns { items: [{current, prev, next, mastery_path}] }.
async fn build_item_sequence(
    db: &PgPool,
    course_id: i64,
    asset_type: &str,
    asset_id: &str,
    user_id: i64,
) -> Result<Value, ApiError> {
    let canvas_type = canvas_asset_type(asset_type)
        .ok_or_else(|| ApiError::Validation("invalid asset_type".into()))?;

    // Fetch all module item ids for the course in module order.
    // Only visible (not deleted) items from published modules.
    let tag_rows: Vec<(i64,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT mi.id
        FROM module_items mi
        JOIN modules m ON m.id = mi.module_id
        WHERE m.course_id = $1 AND m.workflow_state <> 'deleted' AND mi.workflow_state <> 'deleted'
        ORDER BY m.position ASC, mi.position ASC, mi.id ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(db)
    .await?;

    let tag_ids: Vec<i64> = tag_rows.into_iter().map(|(id,)| id).collect();
    if tag_ids.is_empty() {
        return Ok(json!({ "items": [] }));
    }

    // Find matching tag indices
    let mut tag_indices: Vec<usize> = Vec::new();

    if canvas_type == "ContentTag" {
        let parsed_id = asset_id
            .parse::<i64>()
            .map_err(|_| ApiError::Validation("invalid asset_id".into()))?;
        for (ix, tid) in tag_ids.iter().enumerate() {
            if *tid == parsed_id {
                tag_indices.push(ix);
            }
        }
    } else {
        // Look up content_id match
        let matching_tags: Vec<(i64,)> = if canvas_type == "WikiPage" {
            // asset_id is a page URL slug; resolve to page id via wiki_pages
            let page_row: Option<(i64,)> = sqlx::query_as(
                r#"SELECT id FROM wiki_pages WHERE course_id = $1 AND url_suf = $2 LIMIT 1"#,
            )
            .bind(course_id)
            .bind(asset_id)
            .fetch_optional(db)
            .await?;
            if let Some((page_id,)) = page_row {
                sqlx::query_as(
                    r#"SELECT id FROM module_items WHERE module_id IN
                       (SELECT id FROM modules WHERE course_id = $1 AND workflow_state <> 'deleted')
                       AND item_type = 'page' AND item_id = $2"#,
                )
                .bind(course_id)
                .bind(page_id)
                .fetch_all(db)
                .await?
            } else {
                vec![]
            }
        } else {
            let content_id = asset_id
                .parse::<i64>()
                .map_err(|_| ApiError::Validation("invalid asset_id".into()))?;
            sqlx::query_as(
                r#"
                SELECT mi.id FROM module_items mi
                JOIN modules m ON m.id = mi.module_id
                WHERE m.course_id = $1 AND m.workflow_state <> 'deleted'
                  AND mi.workflow_state <> 'deleted'
                  AND mi.item_type = $2 AND mi.item_id = $3
                "#,
            )
            .bind(course_id)
            .bind(canvas_type)
            .bind(content_id)
            .fetch_all(db)
            .await?
        };

        for (ix, tid) in tag_ids.iter().enumerate() {
            if matching_tags.iter().any(|(mid,)| *mid == *tid) {
                tag_indices.push(ix);
            }
        }
    }

    if tag_indices.is_empty() {
        return Ok(json!({ "items": [] }));
    }

    // Cap at 3 (MAX_SEQUENCES in Canvas)
    tag_indices.truncate(3);

    // Load needed tags (current + prev + next for each)
    let mut needed_ids: Vec<i64> = Vec::new();
    for &ix in &tag_indices {
        needed_ids.push(tag_ids[ix]);
        if ix > 0 {
            needed_ids.push(tag_ids[ix - 1]);
        }
        if ix < tag_ids.len() - 1 {
            needed_ids.push(tag_ids[ix + 1]);
        }
    }
    needed_ids.sort();
    needed_ids.dedup();

    // Fetch module item details
    let items_map: std::collections::HashMap<i64, ModuleItemDetail> = {
        let rows: Vec<ModuleItemDetail> = sqlx::query_as(
            r#"
            SELECT mi.id, mi.module_id, m.course_id, mi.item_type, mi.item_id,
                   mi.position, mi.title, mi.indent, mi.workflow_state,
                   mi.created_at, mi.updated_at,
                   COALESCE(a.title, p.title) AS content_title,
                   p.url AS page_url,
                   a.points_possible::float8 AS points_possible,
                   a.due_at AS due_at
            FROM module_items mi
            JOIN modules m ON m.id = mi.module_id
            LEFT JOIN assignments a ON a.id = mi.item_id AND mi.item_type = 'assignment'
            LEFT JOIN wiki_pages p ON p.id = mi.item_id AND mi.item_type = 'page'
            WHERE mi.id = ANY($1)
            "#,
        )
        .bind(&needed_ids)
        .fetch_all(db)
        .await?;
        rows.into_iter().map(|item| (item.id, item)).collect()
    };

    let mut result_items: Vec<Value> = Vec::new();
    for &ix in &tag_indices {
        let cur_id = tag_ids[ix];
        let prev_id = if ix > 0 { Some(tag_ids[ix - 1]) } else { None };
        let next_id = if ix < tag_ids.len() - 1 {
            Some(tag_ids[ix + 1])
        } else {
            None
        };

        let make_item_json = |id: i64| -> Value {
            items_map
                .get(&id)
                .map(|item| module_item_json(item, false))
                .unwrap_or(Value::Null)
        };

        let mut item = serde_json::Map::new();
        item.insert("current".into(), make_item_json(cur_id));
        if let Some(pid) = prev_id {
            item.insert("prev".into(), make_item_json(pid));
        }
        if let Some(nid) = next_id {
            item.insert("next".into(), make_item_json(nid));
        }
        // mastery_path omitted (Conditional Release / cyoe not implemented)
        result_items.push(Value::Object(item));
    }

    Ok(json!({ "items": result_items }))
}

async fn get_item_sequence(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<ItemSequenceQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;
    let result = build_item_sequence(
        &state.db,
        course_id,
        &query.asset_type,
        &query.asset_id,
        auth.id,
    )
    .await?;
    Ok(Json(result))
}

// --------------------------------------------------------------------------
// M2: Show module item (module-scoped)
// GET /courses/{courseId}/modules/{moduleId}/items/{id}
// (Already registered — listed here for reference.)
// --------------------------------------------------------------------------

// --------------------------------------------------------------------------
// M3: Mark item read
// POST /courses/{courseId}/modules/{moduleId}/items/{id}/mark_read
// --------------------------------------------------------------------------

async fn mark_item_read(
    State(state): State<AppState>,
    Path((course_id, module_id, item_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;
    require_module_in_course(&state.db, course_id, module_id).await?;
    require_item_in_module(&state.db, module_id, item_id).await?;

    // Load the item to check if it's published/locked
    let item: Option<(String,)> = sqlx::query_as(
        r#"SELECT workflow_state FROM module_items WHERE id = $1 AND module_id = $2"#,
    )
    .bind(item_id)
    .bind(module_id)
    .fetch_optional(&state.db)
    .await?;

    match item {
        Some((ws,)) if ws == "active" => {
            // Record a "read" completion event in module_completions
            sqlx::query(
                r#"
                INSERT INTO module_completions (user_id, module_id, module_item_id, workflow_state, created_at, updated_at)
                VALUES ($1, $2, $3, 'completed', now(), now())
                ON CONFLICT (user_id, module_item_id)
                DO UPDATE SET workflow_state = 'completed', updated_at = now()
                "#,
            )
            .bind(auth.id)
            .bind(module_id)
            .bind(item_id)
            .execute(&state.db)
            .await?;
            Ok(Json(json!({ "message": "OK" })))
        }
        _ => Err(ApiError::Forbidden),
    }
}

// --------------------------------------------------------------------------
// M4: Select mastery path
// POST /courses/{courseId}/modules/{moduleId}/items/{id}/select_mastery_path
// assignment_set_id required.  Returns adjusted module items per the
// Conditional Release rules.  (Lightweight stub — full CYOE deferred.)
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SelectMasteryPathBody {
    pub assignment_set_id: i64,
}

async fn select_mastery_path(
    State(state): State<AppState>,
    Path((course_id, module_id, item_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(body): Json<SelectMasteryPathBody>,
) -> Result<Json<Value>, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;
    require_module_in_course(&state.db, course_id, module_id).await?;
    require_item_in_module(&state.db, module_id, item_id).await?;

    // Load the item — must be an assignment
    let item_detail: Option<ModuleItemDetail> = sqlx::query_as(
        r#"
        SELECT mi.id, mi.module_id, m.course_id, mi.item_type, mi.item_id,
               mi.position, mi.title, mi.indent, mi.workflow_state,
               mi.created_at, mi.updated_at,
               COALESCE(a.title, p.title) AS content_title,
               p.url AS page_url,
               a.points_possible::float8 AS points_possible,
               a.due_at AS due_at
        FROM module_items mi
        JOIN modules m ON m.id = mi.module_id
        LEFT JOIN assignments a ON a.id = mi.item_id AND mi.item_type = 'assignment'
        LEFT JOIN wiki_pages p ON p.id = mi.item_id AND mi.item_type = 'page'
        WHERE mi.id = $1 AND mi.module_id = $2 AND mi.item_type = 'assignment'
        "#,
    )
    .bind(item_id)
    .bind(module_id)
    .fetch_optional(&state.db)
    .await?;

    match item_detail {
        Some(item) if item.item_type == "assignment" => {
            // For now, return the item itself as the "mastery path" result.
            // Full Conditional Release OverrideHandler logic deferred.
            Ok(Json(json!({
                "meta": { "primaryCollection": "assignments" },
                "items": vec![module_item_json(&item, false)],
                "assignments": json!([{ "id": item.item_id, "title": item.content_title }])
            })))
        }
        _ => Err(ApiError::Validation("item is not an assignment".into())),
    }
}

// --------------------------------------------------------------------------
// Router
// --------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // ---- Course Paces (migration 0213) ----
        .route("/courses/{courseId}/course_pacing", get(get_course_pace))
        .route(
            "/courses/{courseId}/course_pacing",
            post(create_course_pace),
        )
        .route(
            "/courses/{courseId}/course_pacing/{paceId}",
            get(get_single_pace),
        )
        .route(
            "/courses/{courseId}/course_pacing/{paceId}",
            put(update_pace),
        )
        .route(
            "/courses/{courseId}/course_pacing/{paceId}",
            delete(delete_pace),
        )
        .route(
            "/courses/{courseId}/course_pacing/{paceId}/publish",
            post(publish_pace),
        )
        // ---- M1: Item sequence ----
        .route(
            "/courses/{courseId}/module_item_sequence",
            get(get_item_sequence),
        )
        // ---- M3: Mark item read ----
        .route(
            "/courses/{courseId}/modules/{moduleId}/items/{itemId}/mark_read",
            post(mark_item_read),
        )
        // ---- M4: Select mastery path ----
        .route(
            "/courses/{courseId}/modules/{moduleId}/items/{itemId}/select_mastery_path",
            post(select_mastery_path),
        )
        // ---- Modules ----
        .route("/courses/{courseId}/modules", get(list_modules))
        .route("/courses/{courseId}/modules", post(create_module))
        .route(
            "/courses/{courseId}/modules/{moduleId}/items",
            get(list_module_items),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}",
            get(get_course_module),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}",
            put(update_course_module),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}",
            delete(delete_course_module),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}/items",
            post(create_course_module_item),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}/items/{itemId}",
            delete(delete_course_module_item),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}/items/{itemId}/done",
            put(mark_item_done),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}/items/{itemId}/done",
            delete(mark_item_not_done),
        )
        .route(
            "/courses/{courseId}/modules/items/{itemId}",
            get(get_item_detail),
        )
        .route(
            "/courses/{courseId}/modules/items/{itemId}",
            put(update_item_detail),
        )
        .route(
            "/courses/{courseId}/modules/items/{itemId}",
            delete(delete_item_detail),
        )
        .route(
            "/courses/{courseId}/modules/items/{itemId}/duplicate",
            post(duplicate_item),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}/items/{itemId}",
            put(update_module_scoped_item),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}/items/{itemId}",
            get(get_module_scoped_item),
        )
        .route("/modules/{id}/items", post(create_or_update_item))
        .route("/modules/{id}/items/{itemId}", delete(delete_module_item))
        .route("/modules/{id}", get(get_module))
        .route("/modules/{id}", put(update_module))
        .route("/modules/{id}", delete(delete_module))
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Authorization regression tests for list_modules (GET /courses/{id}/modules).
    //!
    //! The router-level tests run against a pool that never connects: the
    //! AuthUser extractor rejects anonymous requests before any query runs.
    //! The DB-gated test covers the full enrollment workflow_state matrix
    //! against the real star-db schema and skips cleanly without DATABASE_URL
    //! (CLAUDE.md convention).

    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// PgPool pointed at a closed port. Connecting never happens unless a
    /// handler actually issues a query — the auth gate runs first.
    fn test_state() -> AppState {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(2))
            .connect_lazy("postgres://star_modules_tests:none@127.0.0.1:1/none")
            .expect("lazy pool");
        AppState {
            db: pool,
            jwt_secret: "test-secret".to_string(),
        }
    }

    /// Regression: an anonymous GET /courses/{courseId}/modules must not
    /// return modules. Previously list_modules took no AuthUser and served
    /// the module list to unauthenticated callers; now the AuthUser
    /// extractor rejects the request with 401 before any handler or DB work.
    #[tokio::test]
    async fn anonymous_list_modules_is_unauthorized() {
        let app = router().with_state(test_state());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/courses/1/modules")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "anonymous request must be rejected with 401, not served modules"
        );
    }

    /// Sanity: a valid bearer token gets past the auth gate. With the lazy
    /// pool the DB query then fails, so the response is a 500 (database
    /// error) — anything but 401 proves the route is protected, not closed.
    #[tokio::test]
    async fn authenticated_request_gets_past_auth_gate() {
        // Same secret derivation the AuthUser extractor uses.
        let token = star_auth::create_token(7, "student", "test-secret").expect("mint test token");

        let app = router().with_state(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/courses/1/modules")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "an authenticated request must not be stopped by the auth gate"
        );
    }

    /// Admin (by session role) reads any course — short-circuits before any
    /// DB access, so this passes against the never-connecting pool.
    #[tokio::test]
    async fn admin_role_reads_any_course_without_db() {
        let state = test_state();
        let auth = AuthUser {
            id: 1,
            role: "admin".to_string(),
        };

        require_course_read_access(&state.db, auth.id, 42, &auth)
            .await
            .expect("admin must read any course");
    }

    /// Full policy matrix against the real schema (star-db migrations).
    /// Verifies enrollments.workflow_state values from migration 0010:
    /// active | invited | completed | deleted | inactive — only 'active'
    /// grants module read access. Skips without DATABASE_URL.
    #[tokio::test]
    async fn enrollment_workflow_states_gate_module_read_access() {
        let Some(db_url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&db_url)
            .await
            .expect("connect DATABASE_URL");
        star_db::MIGRATOR.run(&pool).await.expect("run migrations");

        // Unique-per-run prefix so repeat runs never collide on UNIQUE keys.
        let prefix = format!(
            "mod-authz-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis()
        );

        let course_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id"#,
        )
        .bind(format!("{prefix} course"))
        .bind(&prefix)
        .fetch_one(&pool)
        .await
        .expect("seed course");

        async fn seed_user(pool: &PgPool, prefix: &str, name: &str) -> i64 {
            sqlx::query_scalar(
                r#"
                INSERT INTO users (name, email, password_hash, role)
                VALUES ($1, $2, $3, 'student')
                RETURNING id
                "#,
            )
            .bind(name)
            .bind(format!("{prefix}-{name}@test.local"))
            .bind("not-a-real-hash")
            .fetch_one(pool)
            .await
            .expect("seed user")
        }

        let active = seed_user(&pool, &prefix, "active").await;
        let invited = seed_user(&pool, &prefix, "invited").await;
        let completed = seed_user(&pool, &prefix, "completed").await;
        let inactive = seed_user(&pool, &prefix, "inactive").await;
        let deleted = seed_user(&pool, &prefix, "deleted").await;
        let outsider = seed_user(&pool, &prefix, "outsider").await;

        for (user_id, state) in [
            (active, "active"),
            (invited, "invited"),
            (completed, "completed"),
            (inactive, "inactive"),
            (deleted, "deleted"),
        ] {
            sqlx::query(
                r#"
                INSERT INTO enrollments (user_id, course_id, role, workflow_state)
                VALUES ($1, $2, 'student', $3)
                "#,
            )
            .bind(user_id)
            .bind(course_id)
            .bind(state)
            .execute(&pool)
            .await
            .expect("seed enrollment");
        }

        fn auth(id: i64, role: &str) -> AuthUser {
            AuthUser {
                id,
                role: role.to_string(),
            }
        }

        // Active enrollment (any role) -> allowed.
        require_course_read_access(&pool, active, course_id, &auth(active, "student"))
            .await
            .expect("active enrollment must grant module read access");

        // Every non-active workflow_state -> forbidden.
        for (user_id, state) in [
            (invited, "invited"),
            (completed, "completed"),
            (inactive, "inactive"),
            (deleted, "deleted"),
        ] {
            let err =
                require_course_read_access(&pool, user_id, course_id, &auth(user_id, "student"))
                    .await
                    .expect_err("non-active enrollment must be forbidden");
            assert!(
                matches!(err, ApiError::Forbidden),
                "workflow_state={state} must be forbidden, got {err:?}"
            );
        }

        // No enrollment at all -> forbidden.
        let err =
            require_course_read_access(&pool, outsider, course_id, &auth(outsider, "student"))
                .await
                .expect_err("no enrollment must be forbidden");
        assert!(matches!(err, ApiError::Forbidden));

        // Teacher role claim without an enrollment -> forbidden: read access
        // comes from the enrollment row, not the session role.
        let err =
            require_course_read_access(&pool, outsider, course_id, &auth(outsider, "teacher"))
                .await
                .expect_err("teacher claim without enrollment must be forbidden");
        assert!(matches!(err, ApiError::Forbidden));

        // Admin claim -> allowed without any enrollment.
        require_course_read_access(&pool, outsider, course_id, &auth(outsider, "admin"))
            .await
            .expect("admin must read any course");

        // Teardown (best effort — safe to run against a shared dev DB).
        let _ = sqlx::query(r#"DELETE FROM enrollments WHERE course_id = $1"#)
            .bind(course_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query(r#"DELETE FROM users WHERE email LIKE $1"#)
            .bind(format!("{prefix}%@test.local"))
            .execute(&pool)
            .await;
        let _ = sqlx::query(r#"DELETE FROM courses WHERE id = $1"#)
            .bind(course_id)
            .execute(&pool)
            .await;
    }
}
