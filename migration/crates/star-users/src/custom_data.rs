//! Custom data — Canvas `CustomDataController` (parity gap).
//!
//! Routes (canvas_manifest.json carries the scope-less paths; the
//! `(*/*scope)` glob variants are not manifest routes):
//!   - GET    /users/{userId}/custom_data  (get_data)
//!   - PUT    /users/{userId}/custom_data  (set_data)
//!   - DELETE /users/{userId}/custom_data  (delete_data)
//!
//! Canvas semantics (`custom_data` table, one row per user+namespace):
//!   - `ns` query param is REQUIRED (`{message: "invalid namespace"}` 400
//!     when blank); `scope` selects a path inside the stored `d/<scope>`
//!     JSON tree.
//!   - get_data: `read` on the user (self or admin); missing row → 400
//!     `{message: "no data for scope"}`; non-hash traversal → 400.
//!   - set_data: `manage`/`manage_user_details`; no `data` param → 400
//!     `{message: "no data specified"}`; first write to a scope → 201,
//!     overwrite → 200; scalar-in-the-way → 409 conflict.
//!   - delete_data: manage; unknown scope key → 400 `{message: "invalid
//!     scope for hash"}`.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Params / rows
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CustomDataQuery {
    /// Canvas `ns`: the namespace. Required, non-blank.
    pub ns: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct CustomDataRow {
    id: i64,
    data_json: Value,
}

#[derive(Debug, Deserialize)]
pub struct SetDataPayload {
    pub data: Option<Value>,
}

// ---------------------------------------------------------------------------
// Scope tree helpers (CustomData hash_data_from_scope / set_ / delete_)
// ---------------------------------------------------------------------------

/// Walk `keys` through nested objects; None when any step misses or hits a
/// scalar (Canvas raises ArgumentError → 400 for the read path).
fn hash_from_scope<'a>(hash: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut current = hash;
    for k in keys {
        let obj = current.as_object()?;
        current = obj.get(*k)?;
    }
    Some(current)
}

/// Errors mirroring the two Canvas failure modes.
enum ScopeError {
    /// ArgumentError: "invalid scope for hash"
    InvalidScope,
    /// CustomData::WriteConflict (scalar in the path)
    Conflict,
}

fn set_hash_data_from_scope(
    hash: &mut Value,
    keys: &[&str],
    data: Value,
) -> Result<bool, ScopeError> {
    let (last, parents) = keys.split_last().ok_or(ScopeError::InvalidScope)?;
    let mut current = hash;
    for k in parents {
        let obj = current.as_object_mut().ok_or(ScopeError::Conflict)?;
        let entry = obj
            .entry((*k).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            return Err(ScopeError::Conflict);
        }
        current = entry;
    }
    let obj = current.as_object_mut().ok_or(ScopeError::Conflict)?;
    let overwrite = obj.contains_key(*last);
    obj.insert((*last).to_string(), data);
    Ok(overwrite)
}

fn delete_hash_data_from_scope(hash: &mut Value, keys: &[&str]) -> Result<Value, ScopeError> {
    let (last, parents) = keys.split_last().ok_or(ScopeError::InvalidScope)?;
    let mut current: &mut Value = hash;
    for k in parents {
        let obj = current.as_object_mut().ok_or(ScopeError::InvalidScope)?;
        current = obj.get_mut(*k).ok_or(ScopeError::InvalidScope)?;
    }
    let obj = current.as_object_mut().ok_or(ScopeError::InvalidScope)?;
    if !obj.contains_key(*last) {
        return Err(ScopeError::InvalidScope);
    }
    Ok(obj.remove(*last).expect("checked"))
}

/// Prune now-empty parent objects along the path (Canvas hash2.delete k if
/// hash3.empty?). Best-effort: no error when the shape changed.
fn prune_empty(hash: &mut Value, keys: &[&str]) {
    let Some((last, parents)) = keys.split_last() else {
        return;
    };
    let mut current: &mut Value = hash;
    for k in parents {
        let Some(obj) = current.as_object_mut() else {
            return;
        };
        current = match obj.get_mut(*k) {
            Some(v) => v,
            None => return,
        };
    }
    if let Some(obj) = current.as_object_mut() {
        if obj.get(*last).is_none() {
            obj.remove(*last);
        }
    }
    // Second pass for empty parents (single level keeps parity with the
    // Canvas recursive delete; deeper trees rarely nest empties).
    if let Some(obj) = hash.as_object_mut() {
        if parents.len() == 1 {
            let k = parents[0];
            if let Some(Value::Object(child)) = obj.get_mut(k) {
                if child.is_empty() {
                    obj.remove(k);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

/// `get_context`: the target user must exist (404 otherwise).
async fn load_user(pool: &PgPool, user_id: i64) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// `authorized_action(user, :read|:manage)`: self, or a global admin.
fn authorized(user_id: i64, auth: &AuthUser) -> bool {
    auth.id == user_id || auth.role == "admin"
}

async fn load_row(
    pool: &PgPool,
    user_id: i64,
    ns: &str,
) -> Result<Option<CustomDataRow>, ApiError> {
    sqlx::query_as("SELECT id, data_json FROM custom_data WHERE user_id = $1 AND namespace = $2")
        .bind(user_id)
        .bind(ns)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

async fn custom_data(
    State(state): State<AppState>,
    Path((user_id, scope)): Path<(i64, String)>,
    Query(query): Query<CustomDataQuery>,
    method: axum::http::Method,
    auth: AuthUser,
    body: Option<Json<SetDataPayload>>,
) -> Result<axum::response::Response, ApiError> {
    custom_data_with_scope(State(state), user_id, scope, Query(query), method, auth, body).await
}

async fn custom_data_with_scope(
    state: State<AppState>,
    user_id: i64,
    scope: String,
    query: Query<CustomDataQuery>,
    method: axum::http::Method,
    auth: AuthUser,
    body: Option<Json<SetDataPayload>>,
) -> Result<axum::response::Response, ApiError> {
    load_user(&state.db, user_id).await?;

    // require_namespace: blank ns → 400 {message}.
    let ns = query
        .ns
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("invalid namespace".into()))?;

    // The scope path: `d/<scope>` inside data_json (Canvas prefix "d/").
    let scope_keys: Vec<String> = scope
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let full_path = std::iter::once("d".to_string())
        .chain(scope_keys.iter().cloned())
        .collect::<Vec<_>>();
    let keys: Vec<&str> = full_path.iter().map(String::as_str).collect();

    match method {
        axum::http::Method::GET => {
            if !authorized(user_id, &auth) {
                return Err(ApiError::Forbidden);
            }
            let row = load_row(&state.db, user_id, ns)
                .await?
                .ok_or_else(|| ApiError::BadRequest("no data for scope".into()))?;
            match hash_from_scope(&row.data_json, &keys) {
                Some(value) => {
                    use axum::response::IntoResponse;
                    Ok(axum::Json(json!({ "data": value })).into_response())
                }
                None => Err(ApiError::BadRequest("invalid scope for hash".into())),
            }
        }
        axum::http::Method::PUT => {
            if !authorized(user_id, &auth) {
                return Err(ApiError::Forbidden);
            }
            let Json(payload) =
                body.ok_or_else(|| ApiError::BadRequest("no data specified".into()))?;
            let data = payload
                .data
                .ok_or_else(|| ApiError::BadRequest("no data specified".into()))?;

            // First-or-create the row, then merge under the row lock
            // (unique (user_id, namespace), Canvas first_or_create!).
            sqlx::query(
                "INSERT INTO custom_data (user_id, namespace, data_json, created_at, updated_at) \
                 VALUES ($1, $2, '{}'::jsonb, now(), now()) \
                 ON CONFLICT (user_id, namespace) DO NOTHING",
            )
            .bind(user_id)
            .bind(ns)
            .execute(&state.db)
            .await?;
            let row = load_row(&state.db, user_id, ns)
                .await?
                .ok_or(ApiError::NotFound)?;

            let mut tree = row.data_json.clone();
            let overwrite = match set_hash_data_from_scope(&mut tree, &keys, data) {
                Ok(o) => o,
                Err(ScopeError::Conflict) => {
                    return Err(ApiError::Conflict("conflicting scope path".into()));
                }
                Err(ScopeError::InvalidScope) => {
                    return Err(ApiError::BadRequest("invalid scope for hash".into()));
                }
            };
            sqlx::query("UPDATE custom_data SET data_json = $2, updated_at = now() WHERE id = $1")
                .bind(row.id)
                .bind(tree)
                .execute(&state.db)
                .await?;
            let saved = load_row(&state.db, user_id, ns)
                .await?
                .ok_or(ApiError::NotFound)?;
            let value = hash_from_scope(&saved.data_json, &keys)
                .cloned()
                .unwrap_or(Value::Null);
            let status = if overwrite {
                axum::http::StatusCode::OK
            } else {
                axum::http::StatusCode::CREATED
            };
            use axum::response::IntoResponse;
            let resp = axum::Json(json!({ "data": value })).into_response();
            Ok((status, resp).into_response())
        }
        axum::http::Method::DELETE => {
            if !authorized(user_id, &auth) {
                return Err(ApiError::Forbidden);
            }
            let row = load_row(&state.db, user_id, ns)
                .await?
                .ok_or_else(|| ApiError::BadRequest("no data for scope".into()))?;
            let mut tree = row.data_json.clone();
            let removed = delete_hash_data_from_scope(&mut tree, &keys)
                .map_err(|_| ApiError::BadRequest("invalid scope for hash".into()))?;
            prune_empty(&mut tree, &keys);
            sqlx::query("UPDATE custom_data SET data_json = $2, updated_at = now() WHERE id = $1")
                .bind(row.id)
                .bind(tree)
                .execute(&state.db)
                .await?;
            use axum::response::IntoResponse;
            Ok(axum::Json(json!({ "data": removed })).into_response())
        }
        _ => Err(ApiError::BadRequest("method not allowed".into())),
    }
}

/// Mount the custom-data routes (wildcard scope; the bare path is also
/// accepted with an empty scope — Canvas routes it the same way).
pub fn custom_data_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/users/{userId}/custom_data",
            get(custom_data_root).put(custom_data_root).delete(custom_data_root),
        )
        .route(
            "/users/{userId}/custom_data/{*scope}",
            get(custom_data).put(custom_data).delete(custom_data),
        )
}

/// `/users/{userId}/custom_data` with no `(/:scope)` suffix — Canvas treats a
/// missing scope as the empty scope path (same handler, empty key list).
async fn custom_data_root(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(query): Query<CustomDataQuery>,
    method: axum::http::Method,
    auth: AuthUser,
    body: Option<Json<SetDataPayload>>,
) -> Result<axum::response::Response, ApiError> {
    custom_data_with_scope(State(state), user_id, String::new(), Query(query), method, auth, body).await
}
