//! Account-level parity gaps — P5 port.
//!
//! Three small Canvas controllers, batched:
//!   - `developer_keys` (DeveloperKeysController): the account-scoped
//!     developer key listing/create plus the universal-tool-id lookup.
//!   - `release_notes` (ReleaseNotesController): publish / unpublish.
//!   - `authentication_providers` (AuthenticationProvidersController#restore):
//!     restoring a soft-deleted provider (the table and the sibling CRUD
//!     handlers live in star-auth; this route queries the same table so there
//!     is a single source of truth).
//!
//! Schema note (adapted from the backup port): this branch's `developer_keys`
//! table (migration 0032) has no `account_id` column — Canvas' real schema
//! associates keys with accounts through `developer_key_account_bindings`
//! (migration 0224, Canvas' own join model). The account-scoped listing
//! therefore joins the bindings table, and the create handler inserts a
//! binding next to the key.
//!
//! Shared stances:
//!   - the Canvas gating chain is 401 (no/invalid token) -> 404 (unknown
//!     context) -> 403 (known context, insufficient rights);
//!   - "admin" is the global admin role (`AuthUser.role == "admin"`),
//!     matching the crate's `is_admin` helper;
//!   - `release_notes` has no `published_at` column here (migration 0097),
//!     so publish/unpublish only flip `published` and touch `updated_at`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// developer_keys (account-scoped)
// ---------------------------------------------------------------------------

/// Developer key row subset for the account-scoped listing (the table is
/// owned by migration 0032; the account association lives in
/// `developer_key_account_bindings`, migration 0224).
#[derive(Debug, sqlx::FromRow)]
struct DeveloperKeyRow {
    id: i64,
    name: Option<String>,
    icon_url: Option<String>,
    redirect_uris: Vec<String>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const DEVELOPER_KEY_COLUMNS: &str =
    "id, name, icon_url, redirect_uris, workflow_state, created_at, updated_at";

/// The same projection, prefixed for the bindings join in the index query.
const DEVELOPER_KEY_COLUMNS_PREFIXED: &str =
    "k.id, k.name, k.icon_url, k.redirect_uris, k.workflow_state, k.created_at, k.updated_at";

/// Canvas `DeveloperKey` JSON subset for this batch: the deprecated singular
/// `redirect_uri` is the first registered uri (null when none), and
/// `account_id` echoes the account the listing is scoped to.
fn developer_key_json(row: &DeveloperKeyRow, account_id: i64) -> Value {
    json!({
        "id": row.id,
        "name": row.name,
        "icon_url": row.icon_url,
        "redirect_uri": row.redirect_uris.first().map(String::as_str),
        "account_id": account_id,
        "workflow_state": row.workflow_state,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    })
}

/// The root account of an account chain (Canvas `Account#root_account`);
/// a parentless account is its own root.
async fn root_account_id(pool: &PgPool, account_id: i64) -> Result<i64, ApiError> {
    let (root,): (i64,) = sqlx::query_as(
        r#"
        WITH RECURSIVE up AS (
            SELECT id, parent_account_id FROM accounts WHERE id = $1
            UNION ALL
            SELECT a.id, a.parent_account_id
            FROM accounts a JOIN up ON a.id = up.parent_account_id
        )
        SELECT id FROM up WHERE parent_account_id IS NULL
        ORDER BY id ASC
        LIMIT 1
        "#,
    )
    .bind(account_id)
    .fetch_one(pool)
    .await?;
    Ok(root)
}

/// GET /accounts/{accountId}/developer_keys — Canvas:
/// developer_keys_controller#index —
/// GET /api/v1/accounts/:account_id/developer_keys.
///
/// Admin only. Lists the account's non-deleted developer keys — the keys
/// bound to the account through `developer_key_account_bindings` (the
/// account association model of this schema).
async fn developer_keys_index(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    Query(pag): Query<crate::Pagination>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    if !crate::account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !crate::is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let (_, per_page, offset) = pag.resolve();
    let rows: Vec<DeveloperKeyRow> = sqlx::query_as(&format!(
        r#"
        SELECT {DEVELOPER_KEY_COLUMNS_PREFIXED}
        FROM developer_keys k
        JOIN developer_key_account_bindings b ON b.developer_key_id = k.id
        WHERE b.account_id = $1 AND k.workflow_state <> 'deleted'
        ORDER BY k.id ASC
        LIMIT $2 OFFSET $3
        "#
    ))
    .bind(account_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.iter()
            .map(|row| developer_key_json(row, account_id))
            .collect(),
    ))
}

/// Body for POST /accounts/{accountId}/developer_keys — Canvas nests under
/// `developer_key`; a flat body is accepted too.
#[derive(Debug, Default, Deserialize)]
pub struct DeveloperKeyEnvelope {
    pub developer_key: Option<DeveloperKeyParams>,
    #[serde(flatten)]
    pub flat: Option<DeveloperKeyParams>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeveloperKeyParams {
    pub name: Option<String>,
    pub icon_url: Option<String>,
    pub redirect_uri: Option<String>,
    pub vendor_code: Option<String>,
}

impl DeveloperKeyEnvelope {
    fn params(&self) -> Option<&DeveloperKeyParams> {
        self.developer_key.as_ref().or(self.flat.as_ref())
    }
}

/// POST /accounts/{accountId}/developer_keys — Canvas:
/// developer_keys_controller#create —
/// POST /api/v1/accounts/:account_id/developer_keys.
///
/// Admin only. `name` is required (422 otherwise). The key secret is
/// generated server-side (Canvas `SecureRandom`); here a DB-side md5 token
/// stands in. The new key is bound to the account through
/// `developer_key_account_bindings` (this schema's account association).
async fn developer_keys_create(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<DeveloperKeyEnvelope>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if !crate::account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !crate::is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let params = payload
        .and_then(|Json(body)| body.params().cloned())
        .unwrap_or_default();
    let name = params.name.as_deref().map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err(ApiError::Validation("name can't be blank".into()));
    }
    let redirect_uris: Vec<String> = params
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
        .map(|uri| vec![uri.to_string()])
        .unwrap_or_default();

    let mut tx = state.db.begin().await.map_err(ApiError::from)?;

    let row: DeveloperKeyRow = sqlx::query_as(&format!(
        r#"
        INSERT INTO developer_keys
            (name, icon_url, api_key, redirect_uris, vendor_code, user_id, workflow_state)
        VALUES ($1, $2, md5(random()::text || clock_timestamp()::text), $3, $4, $5, 'active')
        RETURNING {DEVELOPER_KEY_COLUMNS}
        "#
    ))
    .bind(name)
    .bind(&params.icon_url)
    .bind(&redirect_uris)
    .bind(&params.vendor_code)
    .bind(auth.id)
    .fetch_one(&mut *tx)
    .await?;

    let root = root_account_id(&state.db, account_id).await?;
    sqlx::query(
        r#"
        INSERT INTO developer_key_account_bindings
            (account_id, developer_key_id, workflow_state, root_account_id)
        VALUES ($1, $2, 'on', $3)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(account_id)
    .bind(row.id)
    .bind(root)
    .execute(&mut *tx)
    .await?;
    tx.commit().await.map_err(ApiError::from)?;

    Ok((
        StatusCode::CREATED,
        Json(developer_key_json(&row, account_id)),
    ))
}

/// GET /accounts/{accountId}/developer_keys/lookup_utids — Canvas:
/// developer_keys_controller#lookup_utids —
/// GET /api/v1/accounts/:account_id/developer_keys/lookup_utids.
///
/// Resolves universal tool ids for the account. No universal tool ids are
/// registered in this workspace, so Canvas' empty answer is 204 No Content
/// (any authenticated caller; the account must exist).
async fn developer_keys_lookup_utids(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if !crate::account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// release_notes publish / unpublish
// ---------------------------------------------------------------------------

/// Release note row subset (the table is owned by migration 0097 /
/// star-dashboard; queried here so there is one source of truth).
#[derive(Debug, sqlx::FromRow)]
struct ReleaseNoteRow {
    id: i64,
    published: bool,
    target_roles: Vec<String>,
    show_at_production: Option<DateTime<Utc>>,
    show_at_beta: Option<DateTime<Utc>>,
    show_at_test: Option<DateTime<Utc>>,
}

/// Canvas `ReleaseNote#to_json` (matches star-dashboard's `ReleaseNoteJson`):
/// id, show_ats, target_roles, published.
fn release_note_json(row: &ReleaseNoteRow) -> Value {
    let mut show_ats = serde_json::Map::new();
    if let Some(at) = row.show_at_production {
        show_ats.insert("production".into(), json!(at));
    }
    if let Some(at) = row.show_at_beta {
        show_ats.insert("beta".into(), json!(at));
    }
    if let Some(at) = row.show_at_test {
        show_ats.insert("test".into(), json!(at));
    }
    json!({
        "id": row.id,
        "show_ats": show_ats,
        "target_roles": row.target_roles,
        "published": row.published,
    })
}

/// PUT /release_notes/{id}/published — Canvas: release_notes_controller#publish —
/// PUT /api/v1/release_notes/:id/published.
///
/// Admin only. Publishes the note (`published = true`; this schema has no
/// `published_at` column, so `updated_at` is the witness) and renders it;
/// 404 for an unknown id.
async fn publish_release_note(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !crate::is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let row: Option<ReleaseNoteRow> = sqlx::query_as(
        r#"
        UPDATE release_notes
        SET published = TRUE, updated_at = now()
        WHERE id = $1
        RETURNING id, published, target_roles, show_at_production, show_at_beta, show_at_test
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let row = row.ok_or(ApiError::NotFound)?;
    Ok(Json(release_note_json(&row)))
}

/// DELETE /release_notes/{id}/published — Canvas:
/// release_notes_controller#unpublish —
/// DELETE /api/v1/release_notes/:id/published.
///
/// Admin only. Unpublishes the note (`published = false`) and renders it;
/// 404 for an unknown id.
async fn unpublish_release_note(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !crate::is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let row: Option<ReleaseNoteRow> = sqlx::query_as(
        r#"
        UPDATE release_notes
        SET published = FALSE, updated_at = now()
        WHERE id = $1
        RETURNING id, published, target_roles, show_at_production, show_at_beta, show_at_test
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let row = row.ok_or(ApiError::NotFound)?;
    Ok(Json(release_note_json(&row)))
}

// ---------------------------------------------------------------------------
// authentication_providers restore
// ---------------------------------------------------------------------------

/// Authentication provider row (the table and the sibling CRUD handlers are
/// owned by star-auth, migration 0103; the restore route queries the same
/// table so there is one source of truth).
#[derive(Debug, sqlx::FromRow)]
struct AuthenticationProviderRow {
    id: i64,
    account_id: i64,
    auth_type: String,
    position: Option<i32>,
    settings: Value,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Canvas `AuthenticationProvider` JSON — flat provider object with the
/// JSONB settings merged at the top level (matches star-auth's
/// `authentication_provider_json`).
fn authentication_provider_json(row: &AuthenticationProviderRow) -> Value {
    let mut obj = json!({
        "id": row.id,
        "account_id": row.account_id,
        "auth_type": row.auth_type,
        "position": row.position,
        "workflow_state": row.workflow_state,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    });
    if let Value::Object(map) = &obj {
        let mut merged = map.clone();
        if let Value::Object(settings) = &row.settings {
            for (key, value) in settings {
                merged.insert(key.clone(), value.clone());
            }
        }
        obj = Value::Object(merged);
    }
    obj
}

/// PUT /accounts/{accountId}/authentication_providers/{id}/restore — Canvas:
/// authentication_providers_controller#restore —
/// PUT /api/v1/accounts/:account_id/authentication_providers/:id/restore.
///
/// Admin only. Restores a soft-deleted provider (`workflow_state` back to
/// `active`) and renders it; restoring an already-active provider is a no-op
/// that still renders it. 404 for an unknown id (or an id belonging to a
/// different account — Canvas scopes the find to the account).
async fn restore_authentication_provider(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !crate::account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !crate::is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let row: Option<AuthenticationProviderRow> = sqlx::query_as(
        r#"
        UPDATE authentication_providers
        SET workflow_state = 'active', updated_at = now()
        WHERE id = $2 AND account_id = $1
        RETURNING id, account_id, auth_type, position, settings,
                  workflow_state, created_at, updated_at
        "#,
    )
    .bind(account_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let row = row.ok_or(ApiError::NotFound)?;
    Ok(Json(authentication_provider_json(&row)))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Mount the P5 account gap routes (merged into the crate router in
/// `lib.rs`).
pub fn account_gaps_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{accountId}/developer_keys",
            get(developer_keys_index).post(developer_keys_create),
        )
        .route(
            "/accounts/{accountId}/developer_keys/lookup_utids",
            get(developer_keys_lookup_utids),
        )
        .route(
            "/release_notes/{id}/published",
            put(publish_release_note).delete(unpublish_release_note),
        )
        .route(
            "/accounts/{accountId}/authentication_providers/{id}/restore",
            put(restore_authentication_provider),
        )
}
