//! shared_brand_configs — Canvas `SharedBrandConfigsController` parity.
//!
//! Routes:
//! - POST   /accounts/{accountId}/shared_brand_configs — share a brand config
//! - PUT    /accounts/{accountId}/shared_brand_configs/{id} — update a share
//! - DELETE /shared_brand_configs/{id} — un-share a brand config
//!
//! Canvas behavior mirrored here:
//! - the create and update routes read the `shared_brand_config` envelope
//!   with the `name` and the `brand_config_md5` of the brand config
//!   (`params.require(:shared_brand_config).permit(:brand_config_md5, :name)`);
//! - a share belongs to an account, and the caller needs the `:create`,
//!   `:update` and `:delete` right on it — the account admin right of the port;
//! - the destroy route finds the share by its id alone, so it is not scoped to
//!   an account;
//! - the body of every route is the model attributes of the share.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, post, put},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// The request envelope of Canvas `shared_brand_config_params`.
#[derive(Debug, serde::Deserialize)]
struct SharedBrandConfigPayload {
    name: Option<String>,
    brand_config_md5: Option<String>,
}

impl SharedBrandConfigPayload {
    /// Canvas `params.require(:shared_brand_config).permit(:brand_config_md5, :name)`.
    fn into_params(self) -> Result<(String, String), ApiError> {
        let name = self
            .name
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| ApiError::Validation("name is required".into()))?;
        let md5 = self
            .brand_config_md5
            .map(|md5| md5.trim().to_string())
            .filter(|md5| !md5.is_empty())
            .ok_or_else(|| ApiError::Validation("brand_config_md5 is required".into()))?;
        Ok((name, md5))
    }
}

/// A shared brand config row.
#[derive(Debug, sqlx::FromRow)]
struct SharedBrandConfigRow {
    id: i64,
    account_id: i64,
    name: String,
    brand_config_md5: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl SharedBrandConfigRow {
    /// The model attributes of the share.
    fn to_json(&self) -> Value {
        fn ts(value: &DateTime<Utc>) -> String {
            value.to_rfc3339_opts(SecondsFormat::Secs, true)
        }

        json!({
            "id": self.id,
            "account_id": self.account_id,
            "name": self.name,
            "brand_config_md5": self.brand_config_md5,
            "created_at": ts(&self.created_at),
            "updated_at": ts(&self.updated_at),
        })
    }
}

/// Canvas `authorized_action(@shared_brand_config, @current_user, :create)`:
/// the account admin right of the port.
async fn can_manage_account(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// 404 unless the account exists.
async fn ensure_account(pool: &PgPool, account_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// POST /accounts/{accountId}/shared_brand_configs — Canvas `create`.
async fn create_shared_brand_config(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<SharedBrandConfigPayload>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let payload = body.map(|Json(value)| value).unwrap_or(SharedBrandConfigPayload {
        name: None,
        brand_config_md5: None,
    });
    let (name, md5) = payload.into_params()?;

    let row: SharedBrandConfigRow = sqlx::query_as(
        "INSERT INTO shared_brand_configs (account_id, name, brand_config_md5) \
         VALUES ($1, $2, $3) \
         RETURNING id, account_id, name, brand_config_md5, created_at, updated_at",
    )
    .bind(account_id)
    .bind(&name)
    .bind(&md5)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row.to_json())))
}

/// PUT /accounts/{accountId}/shared_brand_configs/{id} — Canvas `update`.
async fn update_shared_brand_config(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Option<Json<SharedBrandConfigPayload>>,
) -> Result<Json<Value>, ApiError> {
    ensure_account(&state.db, account_id).await?;

    let row = find_shared_brand_config(&state.db, id).await?;
    if row.account_id != account_id {
        return Err(ApiError::NotFound);
    }
    if !can_manage_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let payload = body.map(|Json(value)| value).unwrap_or(SharedBrandConfigPayload {
        name: None,
        brand_config_md5: None,
    });
    let (name, md5) = payload.into_params()?;

    let updated: SharedBrandConfigRow = sqlx::query_as(
        "UPDATE shared_brand_configs \
         SET name = $2, brand_config_md5 = $3, updated_at = now() \
         WHERE id = $1 \
         RETURNING id, account_id, name, brand_config_md5, created_at, updated_at",
    )
    .bind(id)
    .bind(&name)
    .bind(&md5)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated.to_json()))
}

/// DELETE /shared_brand_configs/{id} — Canvas `destroy`.
async fn destroy_shared_brand_config(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let row = find_shared_brand_config(&state.db, id).await?;
    if !can_manage_account(&state.db, auth.id, row.account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    sqlx::query("DELETE FROM shared_brand_configs WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;

    Ok(Json(row.to_json()))
}

/// Load one shared brand config.
async fn find_shared_brand_config(
    pool: &PgPool,
    id: i64,
) -> Result<SharedBrandConfigRow, ApiError> {
    sqlx::query_as(
        "SELECT id, account_id, name, brand_config_md5, created_at, updated_at \
         FROM shared_brand_configs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

/// Build the shared brand configs router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{accountId}/shared_brand_configs",
            post(create_shared_brand_config),
        )
        .route(
            "/accounts/{accountId}/shared_brand_configs/{id}",
            put(update_shared_brand_config),
        )
        .route(
            "/shared_brand_configs/{id}",
            delete(destroy_shared_brand_config),
        )
}
