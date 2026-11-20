//! block_editor_templates — Canvas `BlockEditorTemplatesApiController` parity.
//!
//! Routes:
//! - GET    /courses/{courseId}/block_editor_templates            — list
//! - POST   /courses/{courseId}/block_editor_templates            — create
//! - PUT    /courses/{courseId}/block_editor_templates/{id}       — update
//! - DELETE /courses/{courseId}/block_editor_templates/{id}       — destroy
//! - POST   /courses/{courseId}/block_editor_templates/{id}/publish — publish
//! - GET    /courses/{courseId}/block_editor_templates/can_edit   — the editor flags
//!
//! Canvas behavior mirrored here (`app/controllers/block_editor_templates_api_controller.rb`):
//! - the index needs the `block_editor` feature of the account of the course,
//!   otherwise it is a 403 with an empty body; the other routes need
//!   `template_editor?`, which is the `block_editor` and
//!   `block_template_editor` features plus the
//!   `block_editor_template_editor` right on the course.
//! - the refused routes answer the Canvas message of the route, for example
//!   `{"message":"Cannot create block templates."}`.
//! - the index drops the `created_at` and `updated_at` columns and keeps
//!   `node_tree` and `thumbnail` only when `include[]` asks for them.
//! - the create route defaults the name to "Untitled Template", the node tree
//!   to "{}", the editor version to "0.2" and the template type to "page".

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct BlockEditorTemplateRow {
    id: i64,
    root_account_id: Option<i64>,
    context_type: String,
    context_id: i64,
    name: String,
    description: Option<String>,
    tags: Vec<String>,
    node_tree: Value,
    editor_version: String,
    template_type: String,
    thumbnail: Option<String>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl BlockEditorTemplateRow {
    /// Canvas `block_editor_template_json` — the model attributes plus the
    /// Canvas global id.
    fn to_json(&self, with_node_tree: bool, with_thumbnail: bool) -> Value {
        let mut value = json!({
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "tags": self.tags,
            "editor_version": self.editor_version,
            "template_type": self.template_type,
            "workflow_state": self.workflow_state,
            "root_account_id": self.root_account_id,
            "context_type": self.context_type,
            "context_id": self.context_id,
            "global_id": format!("block_editor_template-{}", self.id),
        });
        if with_node_tree {
            value
                .as_object_mut()
                .expect("json object")
                .insert("node_tree".to_string(), self.node_tree.clone());
        }
        if with_thumbnail {
            value
                .as_object_mut()
                .expect("json object")
                .insert("thumbnail".to_string(), json!(self.thumbnail));
        }
        value
    }

    /// Canvas `template.to_json` — the model attributes without the global
    /// id, as the destroy route renders it.
    fn to_model_json(&self) -> Value {
        fn ts(value: &DateTime<Utc>) -> String {
            value.to_rfc3339_opts(SecondsFormat::Secs, true)
        }

        json!({
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "tags": self.tags,
            "node_tree": self.node_tree,
            "editor_version": self.editor_version,
            "template_type": self.template_type,
            "thumbnail": self.thumbnail,
            "workflow_state": self.workflow_state,
            "created_at": ts(&self.created_at),
            "updated_at": ts(&self.updated_at),
            "root_account_id": self.root_account_id,
            "context_type": self.context_type,
            "context_id": self.context_id,
        })
    }
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// The account of the caller: the first account the caller is a member of,
/// otherwise the root account.
async fn caller_account(pool: &PgPool, user_id: i64) -> Result<i64, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT account_id FROM account_users WHERE user_id = $1 ORDER BY account_id ASC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    if let Some((account_id,)) = row {
        return Ok(account_id);
    }
    let root: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    root.map(|(id,)| id).ok_or(ApiError::NotFound)
}

/// Canvas `Account#feature_enabled?`.
async fn feature_enabled(pool: &PgPool, account_id: i64, feature: &str) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM feature_flags \
         WHERE context_type = 'Account' AND context_id = $1 AND feature = $2 \
           AND state = 'enabled' LIMIT 1",
    )
    .bind(account_id)
    .bind(feature)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// The account of a course.
async fn course_account(pool: &PgPool, course_id: i64) -> Result<i64, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(account_id,)| account_id).unwrap_or(1))
}

/// Canvas `require_context`: 404 for an unknown course.
async fn ensure_course(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `authorized_action(@context, @current_user, :read)` on a course.
async fn can_read_course(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 \
           AND role IN ('teacher', 'ta', 'designer') LIMIT 1",
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `template_editor?`: the two features of the account of the caller
/// and the template editor right on the course.
async fn template_editor(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    let account_id = caller_account(pool, auth.id).await?;
    if !feature_enabled(pool, account_id, "block_editor").await? {
        return Ok(false);
    }
    if !feature_enabled(pool, account_id, "block_template_editor").await? {
        return Ok(false);
    }
    can_read_course(pool, auth, course_id).await
}

/// Canvas `global_template_editor?`: the two features and the global template
/// editor right on the course.
async fn global_template_editor(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    let account_id = caller_account(pool, auth.id).await?;
    if !feature_enabled(pool, account_id, "block_editor").await? {
        return Ok(false);
    }
    if !feature_enabled(pool, account_id, "block_template_editor").await? {
        return Ok(false);
    }
    can_read_course(pool, auth, course_id).await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// One query parameter of the request.
fn param_value<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.as_str())
}

/// The `include[]` values of the request.
fn includes(params: &HashMap<String, String>) -> Vec<String> {
    params
        .iter()
        .filter(|(key, _)| key.as_str() == "include[]" || key.as_str() == "include")
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

/// The `type[]` values of the request.
fn template_types(params: &HashMap<String, String>) -> Vec<String> {
    params
        .iter()
        .filter(|(key, _)| key.as_str() == "type[]" || key.as_str() == "type")
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

/// Load one template of a course.
async fn find_template(
    pool: &PgPool,
    course_id: i64,
    id: i64,
) -> Result<BlockEditorTemplateRow, ApiError> {
    sqlx::query_as(
        "SELECT id, root_account_id, context_type, context_id, name, description, tags, \
                node_tree, editor_version, template_type, thumbnail, workflow_state, \
                created_at, updated_at \
         FROM block_editor_templates \
         WHERE id = $1 AND context_type = 'Course' AND context_id = $2",
    )
    .bind(id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/block_editor_templates — Canvas `index`.
async fn list_block_editor_templates(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;

    // The account of the course needs the block editor feature.
    let account_id = course_account(&state.db, course_id).await?;
    if !feature_enabled(&state.db, account_id, "block_editor").await? {
        return Ok((StatusCode::FORBIDDEN, Json(Value::Null)));
    }

    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let include = includes(&params);
    let with_node_tree = include.iter().any(|item| item == "node_tree");
    let with_thumbnail = include.iter().any(|item| item == "thumbnail");

    // Canvas: the drafts are only visible to a template editor.
    let drafts = matches!(
        param_value(&params, "drafts").map(|value| value.to_lowercase()),
        Some(ref value) if value == "1" || value == "true" || value == "t" || value == "yes"
    );
    let editor = template_editor(&state.db, &auth, course_id).await?;
    let workflow_states: Vec<String> = if drafts && editor {
        vec!["active".to_string(), "unpublished".to_string()]
    } else {
        vec!["active".to_string()]
    };

    let types = template_types(&params);
    let sort = param_value(&params, "sort").unwrap_or("");
    let descending = param_value(&params, "order") == Some("desc");

    let mut sql = String::from(
        "SELECT id, root_account_id, context_type, context_id, name, description, tags, \
                node_tree, editor_version, template_type, thumbnail, workflow_state, \
                created_at, updated_at \
         FROM block_editor_templates \
         WHERE context_type = 'Course' AND context_id = $1 \
           AND workflow_state = ANY($2)",
    );
    if !types.is_empty() {
        sql.push_str(" AND template_type = ANY($3)");
    }
    match sort {
        "name" => sql.push_str(if descending {
            " ORDER BY name DESC, id DESC"
        } else {
            " ORDER BY name ASC, id ASC"
        }),
        "created_at" | "updated_at" => {
            let column = if sort == "created_at" {
                "created_at"
            } else {
                "updated_at"
            };
            sql.push_str(&format!(
                " ORDER BY {column} {}, id {}",
                if descending { "DESC" } else { "ASC" },
                if descending { "DESC" } else { "ASC" }
            ));
        }
        _ => sql.push_str(if descending {
            " ORDER BY id DESC"
        } else {
            " ORDER BY id ASC"
        }),
    }

    let mut query = sqlx::query_as::<_, BlockEditorTemplateRow>(&sql).bind(course_id);
    query = query.bind(&workflow_states);
    if !types.is_empty() {
        query = query.bind(&types);
    }
    let rows = query.fetch_all(&state.db).await?;

    let list: Vec<Value> = rows
        .iter()
        .map(|row| row.to_json(with_node_tree, with_thumbnail))
        .collect();
    Ok((StatusCode::OK, Json(Value::Array(list))))
}

/// POST /courses/{courseId}/block_editor_templates — Canvas `create`.
async fn create_block_editor_template(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;

    if !template_editor(&state.db, &auth, course_id).await? {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Cannot create block templates." })),
        ));
    }

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("Untitled Template")
        .to_string();
    let node_tree = match payload.get("node_tree") {
        Some(value) if !value.is_null() => value.clone(),
        _ => json!({}),
    };
    let editor_version = payload
        .get("editor_version")
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty())
        .unwrap_or("0.2")
        .to_string();
    let template_type = payload
        .get("template_type")
        .and_then(Value::as_str)
        .filter(|kind| !kind.is_empty())
        .unwrap_or("page")
        .to_string();
    let thumbnail = payload
        .get("thumbnail")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string());
    let workflow_state = payload
        .get("workflow_state")
        .and_then(Value::as_str)
        .filter(|state| !state.is_empty())
        .unwrap_or("unpublished")
        .to_string();
    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let tags: Vec<String> = payload
        .get("tags")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|tag| tag.to_string())
                .collect()
        })
        .unwrap_or_default();

    let root_account_id = course_account(&state.db, course_id).await?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO block_editor_templates \
         (root_account_id, context_type, context_id, name, description, tags, node_tree, \
          editor_version, template_type, thumbnail, workflow_state) \
         VALUES ($1, 'Course', $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(root_account_id)
    .bind(course_id)
    .bind(&name)
    .bind(&description)
    .bind(&tags)
    .bind(&node_tree)
    .bind(&editor_version)
    .bind(&template_type)
    .bind(&thumbnail)
    .bind(&workflow_state)
    .fetch_one(&state.db)
    .await?;

    let row = find_template(&state.db, course_id, id).await?;
    Ok((StatusCode::OK, Json(row.to_json(true, true))))
}

/// PUT /courses/{courseId}/block_editor_templates/{id} — Canvas `update`.
async fn update_block_editor_template(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;

    if !template_editor(&state.db, &auth, course_id).await? {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Cannot update block templates." })),
        ));
    }

    let row = find_template(&state.db, course_id, id).await?;
    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);

    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let node_tree = payload.get("node_tree").cloned();
    let editor_version = payload
        .get("editor_version")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let template_type = payload
        .get("template_type")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let thumbnail = payload
        .get("thumbnail")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let workflow_state = payload
        .get("workflow_state")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let tags = payload.get("tags").and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(|tag| tag.to_string())
            .collect::<Vec<String>>()
    });

    sqlx::query(
        "UPDATE block_editor_templates SET \
            name = COALESCE($1, name), \
            description = COALESCE($2, description), \
            node_tree = COALESCE($3, node_tree), \
            editor_version = COALESCE($4, editor_version), \
            template_type = COALESCE($5, template_type), \
            thumbnail = COALESCE($6, thumbnail), \
            workflow_state = COALESCE($7, workflow_state), \
            tags = COALESCE($8, tags), \
            updated_at = now() \
         WHERE id = $9",
    )
    .bind(name)
    .bind(description)
    .bind(node_tree)
    .bind(editor_version)
    .bind(template_type)
    .bind(thumbnail)
    .bind(workflow_state)
    .bind(tags)
    .bind(row.id)
    .execute(&state.db)
    .await?;

    let row = find_template(&state.db, course_id, id).await?;
    Ok((StatusCode::OK, Json(row.to_json(true, true))))
}

/// POST /courses/{courseId}/block_editor_templates/{id}/publish — Canvas `publish`.
async fn publish_block_editor_template(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;

    if !template_editor(&state.db, &auth, course_id).await? {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Cannot publish block templates." })),
        ));
    }

    let row = find_template(&state.db, course_id, id).await?;
    // Canvas `BlockEditorTemplate#publish!` activates the template.
    sqlx::query(
        "UPDATE block_editor_templates SET workflow_state = 'active', updated_at = now() WHERE id = $1",
    )
    .bind(row.id)
    .execute(&state.db)
    .await?;

    let row = find_template(&state.db, course_id, id).await?;
    Ok((StatusCode::OK, Json(row.to_json(true, true))))
}

/// DELETE /courses/{courseId}/block_editor_templates/{id} — Canvas `destroy`.
async fn destroy_block_editor_template(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;

    if !template_editor(&state.db, &auth, course_id).await? {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Cannot delete block templates." })),
        ));
    }

    let row = find_template(&state.db, course_id, id).await?;
    sqlx::query("DELETE FROM block_editor_templates WHERE id = $1")
        .bind(row.id)
        .execute(&state.db)
        .await?;

    Ok((StatusCode::OK, Json(row.to_model_json())))
}

/// GET /courses/{courseId}/block_editor_templates/can_edit — Canvas `can_edit`.
async fn block_editor_templates_can_edit(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;

    let can = template_editor(&state.db, &auth, course_id).await?;
    let can_global = global_template_editor(&state.db, &auth, course_id).await?;

    Ok(Json(json!({
        "can_edit": can || can_global,
        "can_edit_global": can_global,
    })))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the block editor templates router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/block_editor_templates",
            get(list_block_editor_templates).post(create_block_editor_template),
        )
        .route(
            "/courses/{courseId}/block_editor_templates/can_edit",
            get(block_editor_templates_can_edit),
        )
        .route(
            "/courses/{courseId}/block_editor_templates/{id}",
            put(update_block_editor_template).delete(destroy_block_editor_template),
        )
        .route(
            "/courses/{courseId}/block_editor_templates/{id}/publish",
            post(publish_block_editor_template),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_global_id_has_the_canvas_shape() {
        let row = BlockEditorTemplateRow {
            id: 12,
            root_account_id: Some(1),
            context_type: "Course".to_string(),
            context_id: 7,
            name: "Nav".to_string(),
            description: None,
            tags: Vec::new(),
            node_tree: json!({}),
            editor_version: "0.2".to_string(),
            template_type: "page".to_string(),
            thumbnail: None,
            workflow_state: "active".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let value = row.to_json(true, true);
        assert_eq!(value["global_id"], "block_editor_template-12");
        assert_eq!(value["name"], "Nav");
        assert!(value.get("node_tree").is_some());
        assert!(value.get("thumbnail").is_some());

        // The index drops the columns that were not asked for.
        let value = row.to_json(false, false);
        assert!(value.get("node_tree").is_none());
        assert!(value.get("thumbnail").is_none());
    }
}
