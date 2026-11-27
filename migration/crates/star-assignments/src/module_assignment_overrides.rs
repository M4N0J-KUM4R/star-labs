//! Module assignment overrides — Canvas `ModuleAssignmentOverridesController`
//! (parity gap).
//!
//! Routes:
//!   - GET /courses/{courseId}/modules/{contextModuleId}/assignment_overrides (index)
//!   - PUT /courses/{courseId}/modules/{contextModuleId}/assignment_overrides (bulk_update)
//!   - PUT /courses/{courseId}/modules/{contextModuleId}/assignment_overrides/
//!     convert_tag_overrides (convert_tag_overrides_to_adhoc_overrides)
//!
//! Canvas semantics:
//!   - require_context + check_authorized_action (manage content → teacher
//!     or admin); require_context_module (404 unknown module).
//!   - index: the module's ACTIVE overrides, paginated.
//!   - bulk_update: a list under `overrides`; each entry needs one of
//!     id / student_ids / course_section_id (400 with the Canvas message
//!     otherwise); course_section_id + student_ids together → 400;
//!     group_id is rejected (400 "group_id is not allowed as an override")
//!     — differentiation tags are off in this workspace. 204 on success.
//!   - convert_tag_overrides: expands the module's group (tag) overrides
//!     into adhoc student_ids overrides, deactivating the group row —
//!     with tags off this is a 204 no-op that keeps parity of shape.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::AssignmentOverride;

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct BulkUpdatePayload {
    #[serde(default)]
    pub overrides: Vec<BulkOverride>,
}

#[derive(Debug, Default, Deserialize)]
pub struct BulkOverride {
    pub id: Option<i64>,
    pub student_ids: Option<Vec<i64>>,
    pub course_section_id: Option<i64>,
    pub group_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct IndexQuery {
    pub per_page: Option<i64>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// require_context + require_context_module: module exists in the course.
async fn ensure_module_in_course(
    pool: &PgPool,
    course_id: i64,
    module_id: i64,
) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM context_modules \
         WHERE id = $1 AND course_id = $2)",
    )
    .bind(module_id)
    .bind(course_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// check_authorized_action: manage content → teacher/ta or admin.
async fn check_module_manage(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
           AND workflow_state <> 'deleted')",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/modules/{contextModuleId}/assignment_overrides
async fn index_module_overrides(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    Query(query): Query<IndexQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_module_in_course(&state.db, course_id, module_id).await?;
    if !check_module_manage(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let rows: Vec<AssignmentOverride> = sqlx::query_as(
        r#"
        SELECT id, assignment_id, title, due_at, unlock_at, lock_at,
               student_ids, course_section_id, group_id, workflow_state
        FROM assignment_overrides
        WHERE context_module_id = $1 AND workflow_state = 'active'
        ORDER BY id ASC
        LIMIT $2
        "#,
    )
    .bind(module_id)
    .bind(per_page)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(Value::Array(
        rows.into_iter()
            .map(|o| serde_json::to_value(o).unwrap_or(Value::Null))
            .collect(),
    )))
}

/// PUT /courses/{courseId}/modules/{contextModuleId}/assignment_overrides
async fn bulk_update_overrides(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<BulkUpdatePayload>,
) -> Result<StatusCode, ApiError> {
    ensure_module_in_course(&state.db, course_id, module_id).await?;
    if !check_module_manage(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas validation order: array shape, group_id rules, then the
    // required-any-of rule, then the section+students clash.
    for o in &payload.overrides {
        if o.group_id.is_some() {
            return Err(ApiError::BadRequest(
                "group_id is not allowed as an override".into(),
            ));
        }
        if o.id.is_none() && o.student_ids.is_none() && o.course_section_id.is_none() {
            return Err(ApiError::BadRequest(
                "id, student_ids, or course_section_id required with each override".into(),
            ));
        }
        if o.course_section_id.is_some() && o.student_ids.is_some() {
            return Err(ApiError::BadRequest(
                "cannot provide course_section_id and student_ids on the same override".into(),
            ));
        }
    }

    for o in &payload.overrides {
        match o.id {
            Some(id) => {
                // Update the existing override inside this module.
                let owned: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM assignment_overrides \
                     WHERE id = $1 AND context_module_id = $2)",
                )
                .bind(id)
                .bind(module_id)
                .fetch_one(&state.db)
                .await?;
                if !owned {
                    return Err(ApiError::NotFound);
                }
                sqlx::query(
                    "UPDATE assignment_overrides SET \
                        student_ids = COALESCE($2, student_ids), \
                        course_section_id = COALESCE($3, course_section_id), \
                        updated_at = now() \
                     WHERE id = $1",
                )
                .bind(id)
                .bind(o.student_ids.as_deref().map(<[i64]>::to_vec))
                .bind(o.course_section_id)
                .execute(&state.db)
                .await?;
            }
            None => {
                // Create a new module-scoped override for each assignment
                // that has none yet? Canvas's bulk_update only updates
                // existing overrides by id; entries without id but with
                // student_ids/section create adhoc rows tied to the module.
                sqlx::query(
                    "INSERT INTO assignment_overrides \
                        (assignment_id, title, student_ids, course_section_id, \
                         context_module_id, workflow_state) \
                     VALUES ( \
                        COALESCE((SELECT assignment_id FROM assignment_overrides \
                                  WHERE context_module_id = $1 \
                                    AND workflow_state = 'active' LIMIT 1), 0), \
                        ' differentiated', $2, $3, $1, 'active')",
                )
                .bind(module_id)
                .bind(o.student_ids.clone().unwrap_or_default())
                .bind(o.course_section_id)
                .execute(&state.db)
                .await?;
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// PUT .../assignment_overrides/convert_tag_overrides
async fn convert_tag_overrides(
    State(state): State<AppState>,
    Path((course_id, module_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_module_in_course(&state.db, course_id, module_id).await?;
    if !check_module_manage(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    // Differentiation tags are disabled in this workspace, so there are no
    // tag overrides to convert: report an empty conversion (Canvas returns
    // the conversion counts; zero conversions is the honest result).
    Ok(Json(json!({ "converted": 0 })))
}

/// Mount the module-override routes.
pub fn module_assignment_overrides_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/modules/{contextModuleId}/assignment_overrides",
            get(index_module_overrides).put(bulk_update_overrides),
        )
        .route(
            "/courses/{courseId}/modules/{contextModuleId}/assignment_overrides/convert_tag_overrides",
            axum::routing::put(convert_tag_overrides),
        )
}
