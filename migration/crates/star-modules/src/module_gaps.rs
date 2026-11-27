//! Canvas parity gaps for the modules domain (PORT-B, 9 routes).
//!
//! Grouped by Canvas controller:
//! - context_module_items_api: GET module_item_redirect/:id,
//!   GET module_item_sequence, POST .../items/:id/mark_read and
//!   POST .../items/:id/select_mastery_path (conditional release is off
//!   here, so Canvas' feature-disabled 400 is returned).
//! - context_modules_api: PUT /modules (batch_update),
//!   POST /modules/:module_id/duplicate, PUT /modules/:id/relock.
//! - conditional_release stats: the two concrete mastery_paths/stats routes.
//!   Conditional release is disabled on this branch, so they answer Canvas'
//!   feature-off shapes ({"ranges": []} / {"students": []}); the
//!   mastery_paths/rules CRUD itself already lives in
//!   `conditional_release.rs` and is not duplicated here.
//!
//! Deviations from Canvas are documented per handler.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{
    module_item_json, require_course_read_access, require_done_access, require_teacher_or_admin,
    require_visible_module_in_course, Module, ModuleItemDetail,
};

// ---------------------------------------------------------------------------
// context_module_items_api — module_item_redirect
// ---------------------------------------------------------------------------

/// Canvas: context_module_items_api#redirect —
/// GET /api/v1/courses/:course_id/module_item_redirect/:id
///
/// Redirects (302) to the item's content location: ExternalUrl items go to
/// their stored `module_items.url`, every other type to the module item's
/// html page (`/courses/:course_id/modules/items/:id`), like Canvas'
/// `redirect_to(@tag.url) / course_context_modules_item_url` pair. An
/// ExternalUrl item without a usable url falls back to the html path.
async fn module_item_redirect(
    State(state): State<AppState>,
    Path((course_id, item_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;

    let item: Option<(String, Option<String>)> = sqlx::query_as(
        r#"
        SELECT mi.item_type, mi.url
        FROM module_items mi
        JOIN modules m ON m.id = mi.module_id
        WHERE mi.id = $1 AND m.course_id = $2 AND m.workflow_state <> 'deleted'
        "#,
    )
    .bind(item_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    let Some((item_type, url)) = item else {
        return Err(ApiError::NotFound);
    };

    let html_path = format!("/courses/{course_id}/modules/items/{item_id}");
    let location = if item_type == "external_url" {
        url.filter(|url| !url.is_empty()).unwrap_or(html_path)
    } else {
        html_path
    };

    Ok((StatusCode::FOUND, [(header::LOCATION, location)]).into_response())
}

// ---------------------------------------------------------------------------
// context_module_items_api — module_item_sequence
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ItemSequenceQuery {
    /// The module item to find the sequence around (Canvas: Required).
    pub asset_id: Option<i64>,
}

/// Canvas: context_module_items_api#item_sequence —
/// GET /api/v1/courses/:course_id/module_item_sequence?asset_id=n
///
/// Returns `{"items": {"current": ..., "prev": ..., "next": ...}}` by walking
/// the course's visible modules (position ASC, id ASC) and their items
/// (position ASC, id ASC) around `asset_id`; `prev`/`next` are null at the
/// edges, and an unknown asset (or a missing `asset_id`) is a 404.
/// Simplification: unpublished modules are walked too — Canvas'
/// `module_content_visible_to` filter is not reproduced.
async fn module_item_sequence(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<ItemSequenceQuery>,
) -> Result<Json<Value>, ApiError> {
    require_course_read_access(&state.db, auth.id, course_id, &auth).await?;

    let asset_id = params.asset_id.ok_or(ApiError::NotFound)?;

    let items: Vec<ModuleItemDetail> = sqlx::query_as(
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
        WHERE m.course_id = $1 AND m.workflow_state <> 'deleted'
        ORDER BY m.position ASC, m.id ASC, mi.position ASC, mi.id ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    let index = items
        .iter()
        .position(|item| item.id == asset_id)
        .ok_or(ApiError::NotFound)?;

    let prev = index
        .checked_sub(1)
        .map(|i| module_item_json(&items[i], false));
    let next = items
        .get(index + 1)
        .map(|item| module_item_json(item, false));

    Ok(Json(json!({
        "items": {
            "current": module_item_json(&items[index], false),
            "prev": prev,
            "next": next,
        }
    })))
}

// ---------------------------------------------------------------------------
// context_module_items_api — mark_read / select_mastery_path
// ---------------------------------------------------------------------------

/// Canvas: context_module_items_api#mark_item_read —
/// POST /api/v1/courses/:course_id/modules/:module_id/items/:id/mark_read
///
/// Marks the item read for the caller (204). Canvas records the read in a
/// separate ContentParticipation table; this repo reuses the
/// module_item_completions row (migration 0255 widened its state CHECK with
/// 'read'). A 'completed' row keeps its state — done dominates read — while
/// 'incomplete' rows flip to 'read'.
async fn mark_item_read(
    State(state): State<AppState>,
    Path((course_id, module_id, item_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_done_access(&state.db, course_id, module_id, item_id, &auth).await?;

    sqlx::query(
        r#"
        INSERT INTO module_item_completions (module_id, item_id, user_id, state)
        VALUES ($1, $2, $3, 'read')
        ON CONFLICT (user_id, item_id)
        DO UPDATE SET
            state = CASE WHEN module_item_completions.state = 'completed'
                         THEN module_item_completions.state
                         ELSE 'read' END,
            updated_at = now()
        "#,
    )
    .bind(module_id)
    .bind(item_id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Canvas: context_module_items_api#select_mastery_path —
/// POST /api/v1/courses/:course_id/modules/:module_id/items/:id/select_mastery_path
///
/// Canvas CYOE (choose-your-own-education) route. Conditional release is
/// disabled in this repo, so — like the other feature-off stances — the
/// route resolves the course/module/item and then rejects the selection with
/// 400 ("mastery paths not enabled").
async fn select_mastery_path(
    State(state): State<AppState>,
    Path((course_id, module_id, item_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_done_access(&state.db, course_id, module_id, item_id, &auth).await?;

    Err(ApiError::BadRequest("mastery paths not enabled".into()))
}

// ---------------------------------------------------------------------------
// context_modules_api — batch_update / duplicate / relock
// ---------------------------------------------------------------------------

/// One `course[modules][]` entry — only `id` is used; positions always
/// follow the array order (1..n).
#[derive(Debug, Deserialize)]
struct BatchModuleEntry {
    id: i64,
}

/// Batch update parameters, named after the documented `course[module_ids][]`
/// argument (JSON: `{"course": {"module_ids": [...]}}`); the
/// `course[modules][]` / `modules` (`[{"id": 1}, ...]`) variants carry the
/// same ids.
#[derive(Debug, Default, Deserialize)]
struct BatchUpdateCourseParams {
    module_ids: Option<Vec<i64>>,
    modules: Option<Vec<BatchModuleEntry>>,
}

/// Accepts both the Rails-style `{"course": {...}}` envelope and a flat body
/// (the same dual-shape convention as the crate's module item updates).
#[derive(Debug, Default, Deserialize)]
struct BatchUpdateModulesBody {
    course: Option<BatchUpdateCourseParams>,
    #[serde(flatten)]
    direct: BatchUpdateCourseParams,
}

/// Prefer the `{"course": {...}}` envelope when it carries any parameter,
/// else the flat body.
fn effective_batch_params(body: BatchUpdateModulesBody) -> BatchUpdateCourseParams {
    match body.course {
        Some(course) if course.module_ids.is_some() || course.modules.is_some() => course,
        _ => body.direct,
    }
}

/// Canvas: context_modules_api#batch_update —
/// PUT /api/v1/courses/:course_id/modules
///
/// Batch-reorders modules: positions are rewritten 1..n in the order the ids
/// are given, inside one transaction, and the updated modules are returned
/// in that order. An id that does not resolve to a visible module of the
/// course aborts the batch with 404 (Canvas hides unknown contexts).
/// Stance: modules not listed keep their stored positions.
async fn batch_update_modules(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<BatchUpdateModulesBody>>,
) -> Result<Json<Vec<Module>>, ApiError> {
    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    let params = effective_batch_params(body);
    let ids = params
        .module_ids
        .or_else(|| {
            params
                .modules
                .map(|modules| modules.into_iter().map(|module| module.id).collect())
        })
        .ok_or_else(|| ApiError::BadRequest("missing module_ids parameter".into()))?;

    let mut tx = state.db.begin().await?;
    let mut updated = Vec::with_capacity(ids.len());
    for (index, module_id) in ids.iter().enumerate() {
        let module: Option<Module> = sqlx::query_as(
            r#"
            UPDATE modules
            SET position = $1, updated_at = now()
            WHERE id = $2 AND course_id = $3 AND workflow_state <> 'deleted'
            RETURNING id, course_id, name, position, workflow_state, created_at, updated_at
            "#,
        )
        .bind(index as i32 + 1)
        .bind(module_id)
        .bind(course_id)
        .fetch_optional(&mut *tx)
        .await?;

        match module {
            Some(module) => updated.push(module),
            None => {
                tx.rollback().await?;
                return Err(ApiError::NotFound);
            }
        }
    }
    tx.commit().await?;

    Ok(Json(updated))
}

/// Canvas: context_modules_api#duplicate —
/// POST /api/v1/courses/:course_id/modules/:module_id/duplicate
///
/// Copies the module (Canvas keeps the same name) plus its module items.
/// The linked pages/assignments are shared, not cloned — Canvas' module
/// duplicate creates new ContentTags pointing at the same content, unlike
/// item-level duplicate which clones the content as well. The copy is
/// appended after the course's last visible module and keeps the source's
/// publish state.
async fn duplicate_module(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Module>), ApiError> {
    require_visible_module_in_course(&state.db, course_id, module_id).await?;
    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let mut tx = state.db.begin().await?;

    let copy: Module = sqlx::query_as(
        r#"
        INSERT INTO modules (course_id, name, position, workflow_state)
        SELECT course_id,
               name,
               COALESCE(
                   (SELECT MAX(position) FROM modules
                    WHERE course_id = $1 AND workflow_state <> 'deleted'),
                   0
               ) + 1,
               workflow_state
        FROM modules
        WHERE id = $2 AND course_id = $1
        RETURNING id, course_id, name, position, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(module_id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO module_items
            (module_id, item_type, item_id, position, title, indent, workflow_state, url)
        SELECT $1, item_type, item_id, position, title, indent, workflow_state, url
        FROM module_items
        WHERE module_id = $2
        "#,
    )
    .bind(copy.id)
    .bind(module_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok((StatusCode::OK, Json(copy)))
}

/// Canvas: context_modules_api#relock —
/// PUT /api/v1/courses/:course_id/modules/:id/relock
///
/// Canvas re-locks the module's sequential requirements, resetting student
/// progress. Stance: the module's completion rows are cleared (Canvas drops
/// the module's progression state), other modules keep theirs, and the
/// module JSON is returned unchanged.
async fn relock_module(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Module>, ApiError> {
    require_visible_module_in_course(&state.db, course_id, module_id).await?;
    require_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    sqlx::query("DELETE FROM module_item_completions WHERE module_id = $1")
        .bind(module_id)
        .execute(&state.db)
        .await?;

    let module: Module = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, workflow_state, created_at, updated_at
        FROM modules
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(module_id)
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(module))
}

// ---------------------------------------------------------------------------
// conditional_release — mastery paths stats (concrete feature-off routes)
// ---------------------------------------------------------------------------

/// Course existence probe — Canvas hides unknown courses (404) before the
/// rights check on the conditional_release routes.
async fn course_exists(db: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    let row: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

/// conditional_release gate: a teacher/ta enrollment in the course or the
/// global admin role.
async fn conditional_release_teacher(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let row: (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 \
           AND role IN ('teacher', 'ta') AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

/// Shared implementation of the two concrete mastery paths stats routes:
/// identical gating and feature-off shapes (conditional release is disabled
/// here, so the empty shapes are Canvas' own feature-off answer).
async fn mastery_paths_stat(
    state: &AppState,
    auth: &AuthUser,
    course_id: i64,
    stat: &str,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !conditional_release_teacher(&state.db, auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    match stat {
        "students_per_range" => Ok(Json(json!({ "ranges": [] }))),
        "student_details" => Ok(Json(json!({ "students": [] }))),
        _ => Err(ApiError::NotFound),
    }
}

/// Canvas: conditional_release/stats#students_per_range —
/// GET /api/v1/courses/:course_id/mastery_paths/stats/students_per_range
///
/// Concrete registration of the generic stats wildcard: the parity manifest
/// matches concrete paths only, which is why the route exists here.
async fn mastery_paths_students_per_range(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    mastery_paths_stat(&state, &auth, course_id, "students_per_range").await
}

/// Canvas: conditional_release/stats#student_details —
/// GET /api/v1/courses/:course_id/mastery_paths/stats/student_details
///
/// Concrete registration of the generic stats wildcard (see
/// mastery_paths_students_per_range).
async fn mastery_paths_student_details(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    mastery_paths_stat(&state, &auth, course_id, "student_details").await
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The nine PORT-B parity-gap routes for the modules domain.
pub fn module_gaps_routes() -> Router<AppState> {
    Router::new()
        // context_module_items_api
        .route(
            "/courses/{courseId}/module_item_redirect/{itemId}",
            get(module_item_redirect),
        )
        .route(
            "/courses/{courseId}/module_item_sequence",
            get(module_item_sequence),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}/items/{itemId}/mark_read",
            post(mark_item_read),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}/items/{itemId}/select_mastery_path",
            post(select_mastery_path),
        )
        // context_modules_api
        .route("/courses/{courseId}/modules", put(batch_update_modules))
        .route(
            "/courses/{courseId}/modules/{moduleId}/duplicate",
            post(duplicate_module),
        )
        .route(
            "/courses/{courseId}/modules/{moduleId}/relock",
            put(relock_module),
        )
        // conditional_release — concrete stats routes (feature-off shapes)
        .route(
            "/courses/{courseId}/mastery_paths/stats/students_per_range",
            get(mastery_paths_students_per_range),
        )
        .route(
            "/courses/{courseId}/mastery_paths/stats/student_details",
            get(mastery_paths_student_details),
        )
}

// ---------------------------------------------------------------------------
// Tests (DB-free unit tests; the HTTP behavior is pinned in
// tests/module_gaps_integration.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::module_gaps_routes;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    /// The gap router itself builds cleanly (route registration sanity).
    #[test]
    fn routes() {
        let _ = module_gaps_routes();
    }

    /// The concrete students_per_range/student_details routes must merge
    /// with any sibling wildcard router without panicking (star-http merges
    /// the crates' routers), and matchit must prefer the static segments.
    #[tokio::test]
    async fn concrete_stats_routes_take_priority_over_a_wildcard() {
        async fn wildcard() -> &'static str {
            "wildcard"
        }
        async fn concrete() -> &'static str {
            "concrete"
        }

        let other: Router<()> = Router::new().route(
            "/courses/{courseId}/mastery_paths/stats/{stat}",
            get(wildcard),
        );
        let mine: Router<()> = Router::new()
            .route(
                "/courses/{courseId}/mastery_paths/stats/students_per_range",
                get(concrete),
            )
            .route(
                "/courses/{courseId}/mastery_paths/stats/student_details",
                get(concrete),
            );

        let app = other.merge(mine);

        for path in [
            "/courses/1/mastery_paths/stats/students_per_range",
            "/courses/1/mastery_paths/stats/student_details",
        ] {
            let resp = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let text = std::str::from_utf8(&body).unwrap();
            assert_eq!(text, "concrete", "static route must win: {path}");
        }

        // The wildcard keeps serving every other stat name.
        let resp = app
            .oneshot(
                Request::get("/courses/1/mastery_paths/stats/other")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        assert_eq!(text, "wildcard");
    }
}
