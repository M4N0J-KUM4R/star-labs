//! star-modules domain crate — course modules and module items.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

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
// Router
// --------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
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
