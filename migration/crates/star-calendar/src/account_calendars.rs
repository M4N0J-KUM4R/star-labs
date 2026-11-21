//! account_calendars — Canvas `AccountCalendarsApiController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET /account_calendars` — the calendars available to the caller
//! - `GET /account_calendars/{accountId}` — one calendar
//! - `PUT /account_calendars/{accountId}` — update one calendar
//! - `PUT /accounts/{accountId}/account_calendars` — update many calendars
//! - `GET /accounts/{accountId}/account_calendars` — all the calendars of an account
//! - `GET /accounts/{accountId}/visible_calendars_count` — the number of visible calendars
//!
//! Canvas routes (config/routes.rb, the account API block):
//!   get "account_calendars", action: :index
//!   get "account_calendars/:account_id", action: :show
//!   put "account_calendars/:account_id", action: :update
//!   put "accounts/:account_id/account_calendars", action: :bulk_update
//!   get "accounts/:account_id/account_calendars", action: :all_calendars
//!   get "accounts/:account_id/visible_calendars_count", action: :visible_calendars_count
//!
//! Canvas controller (`app/controllers/account_calendars_api_controller.rb`):
//! the calendar of an account has to exist (404), the caller needs the
//! `view_account_calendar_details` right to read it and the
//! `manage_account_calendar_visibility` right to change it (403).
//!
//! The calendar JSON is the Canvas `Api::V1::AccountCalendar` shape: the
//! account attributes plus `visible`, `auto_subscribe`, `asset_string`,
//! `type` and the calendar-event URLs.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// An account with its calendar settings.
#[derive(Debug, sqlx::FromRow)]
struct AccountRow {
    id: i64,
    name: String,
    parent_account_id: Option<i64>,
    root_account_id: Option<i64>,
    account_calendar_visible: bool,
    account_calendar_subscription_type: String,
}

/// The query parameters of the listing.
#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    search_term: Option<String>,
    page: Option<String>,
    per_page: Option<String>,
}

/// The body of an update request.
#[derive(Debug, Default, Deserialize)]
struct CalendarRequest {
    visible: Option<bool>,
    auto_subscribe: Option<bool>,
}

/// The body of a bulk update — the `_json` array.
#[derive(Debug, Default, Deserialize)]
struct BulkRequest {
    #[serde(rename = "_json", default)]
    entries: Vec<BulkEntry>,
}

/// One entry of a bulk update.
#[derive(Debug, Default, Deserialize)]
struct BulkEntry {
    id: Option<i64>,
    visible: Option<bool>,
    auto_subscribe: Option<bool>,
}

/// Domain router for the Canvas account-calendar routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/account_calendars", get(index))
        .route("/account_calendars/{account_id}", get(show).put(update))
        .route(
            "/accounts/{account_id}/account_calendars",
            get(all_calendars).put(bulk_update),
        )
        .route(
            "/accounts/{account_id}/visible_calendars_count",
            get(visible_calendars_count),
        )
}

/// `GET /account_calendars` — account_calendars#index.
async fn index(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let per_page = params
        .per_page
        .as_deref()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(10)
        .min(100);
    let page = params
        .page
        .as_deref()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    let offset = (page - 1) * per_page;

    // Canvas lists the calendars the caller is associated with and that are
    // visible.
    let search_term = params
        .search_term
        .as_deref()
        .filter(|value| !value.is_empty());
    let mut sql = String::from(
        "SELECT a.id, a.name, a.parent_account_id, \
                (SELECT root.id FROM accounts root \
                   WHERE root.id = a.id OR root.id = a.parent_account_id) AS root_account_id, \
                a.account_calendar_visible, a.account_calendar_subscription_type \
         FROM accounts a \
         JOIN account_users au ON au.account_id = a.id \
         WHERE au.user_id = $1 AND a.workflow_state <> 'deleted' \
           AND a.account_calendar_visible = true",
    );
    if search_term.is_some() {
        sql.push_str(" AND a.name ILIKE $4");
    }
    sql.push_str(" ORDER BY a.name ASC LIMIT $2 OFFSET $3");

    let accounts: Vec<AccountRow> = sqlx::query_as(&sql)
        .bind(auth.id)
        .bind(per_page)
        .bind(offset)
        .bind(search_term.map(|term| format!("%{term}%")))
        .fetch_all(&state.db)
        .await?;

    // The total counts the whole set, not only the page.
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM accounts a \
         JOIN account_users au ON au.account_id = a.id \
         WHERE au.user_id = $1 AND a.workflow_state <> 'deleted' \
           AND a.account_calendar_visible = true",
    )
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    let mut calendars = Vec::with_capacity(accounts.len());
    for account in &accounts {
        calendars.push(calendar_json(account));
    }

    Ok(Json(json!({
        "account_calendars": calendars,
        "total_results": total,
    }))
    .into_response())
}

/// `GET /account_calendars/:account_id` — account_calendars#show.
async fn show(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let account = load_account(&state.db, account_id).await?;
    require_view(&state.db, &auth, &account).await?;
    Ok(Json(calendar_json(&account)).into_response())
}

/// `PUT /account_calendars/:account_id` — account_calendars#update.
async fn update(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<CalendarRequest>>,
) -> Result<Response, ApiError> {
    let account = load_account(&state.db, account_id).await?;
    require_manage(&state.db, &auth, &account).await?;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    apply_update(&state.db, &account, &body).await?;

    let account = load_account(&state.db, account_id).await?;
    Ok(Json(calendar_json(&account)).into_response())
}

/// `PUT /accounts/:account_id/account_calendars` — account_calendars#bulk_update.
async fn bulk_update(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<BulkRequest>>,
) -> Result<Response, ApiError> {
    let account = load_account(&state.db, account_id).await?;
    require_manage(&state.db, &auth, &account).await?;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    if body.entries.is_empty() {
        return Err(ApiError::BadRequest(
            "Expected array of objects".to_string(),
        ));
    }
    if body.entries.iter().any(|entry| entry.id.is_none()) {
        return Err(ApiError::BadRequest("Missing key(s)".to_string()));
    }
    let ids = body
        .entries
        .iter()
        .filter_map(|entry| entry.id)
        .collect::<Vec<i64>>();
    let unique = {
        let mut ids = ids.clone();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    };
    if unique != ids.len() {
        return Err(ApiError::BadRequest("Duplicate IDs".to_string()));
    }

    // Only the account itself and its sub-accounts may be changed.
    let allowed = sub_account_ids(&state.db, account.id).await?;
    if ids.iter().any(|id| !allowed.contains(id)) {
        return Err(ApiError::Unauthorized);
    }

    for entry in &body.entries {
        let Some(id) = entry.id else {
            continue;
        };
        let visible = entry.visible;
        let auto_subscribe = entry.auto_subscribe;
        sqlx::query(
            "UPDATE accounts SET \
                 account_calendar_visible = COALESCE($1, account_calendar_visible), \
                 account_calendar_subscription_type = CASE \
                     WHEN $2 IS TRUE THEN 'auto' \
                     WHEN $2 IS FALSE THEN 'manual' \
                     ELSE account_calendar_subscription_type END, \
                 updated_at = now() WHERE id = $3",
        )
        .bind(visible)
        .bind(auto_subscribe)
        .bind(id)
        .execute(&state.db)
        .await?;
    }

    Ok(Json(json!({})).into_response())
}

/// `GET /accounts/:account_id/account_calendars` — account_calendars#all_calendars.
async fn all_calendars(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let account = load_account(&state.db, account_id).await?;
    require_manage(&state.db, &auth, &account).await?;

    // Canvas answers the account and its sub-accounts.
    let ids = sub_account_ids(&state.db, account.id).await?;
    let mut calendars = Vec::with_capacity(ids.len());
    for id in ids {
        let account = load_account(&state.db, id).await?;
        calendars.push(calendar_json(&account));
    }
    Ok(Json(json!({ "account_calendars": calendars })).into_response())
}

/// `GET /accounts/:account_id/visible_calendars_count` —
/// account_calendars#visible_calendars_count.
async fn visible_calendars_count(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let account = load_account(&state.db, account_id).await?;
    require_manage(&state.db, &auth, &account).await?;

    // Canvas counts the visible calendars of the account and its
    // sub-accounts.
    let ids = sub_account_ids(&state.db, account.id).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM accounts \
         WHERE account_calendar_visible = true AND workflow_state <> 'deleted' \
           AND id = ANY($1)",
    )
    .bind(&ids)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({ "visible_calendars_count": count })).into_response())
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// The account of a route has to exist — 404.
async fn load_account(db: &PgPool, account_id: i64) -> Result<AccountRow, ApiError> {
    sqlx::query_as(
        "SELECT a.id, a.name, a.parent_account_id, \
                (SELECT root.id FROM accounts root \
                   WHERE root.id = a.id OR root.id = a.parent_account_id) AS root_account_id, \
                a.account_calendar_visible, a.account_calendar_subscription_type \
         FROM accounts a WHERE a.id = $1 AND a.workflow_state <> 'deleted'",
    )
    .bind(account_id)
    .fetch_optional(db)
    .await?
    .ok_or(ApiError::NotFound)
}

/// `view_account_calendar_details` — the caller is associated with the
/// account.
async fn require_view(db: &PgPool, auth: &AuthUser, account: &AccountRow) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(account.id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// `manage_account_calendar_visibility` — the caller manages the account.
async fn require_manage(
    db: &PgPool,
    auth: &AuthUser,
    account: &AccountRow,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM account_users \
         WHERE account_id = $1 AND user_id = $2 AND role <> 'student' LIMIT 1",
    )
    .bind(account.id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// The account and its sub-accounts.
async fn sub_account_ids(db: &PgPool, account_id: i64) -> Result<Vec<i64>, ApiError> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "WITH RECURSIVE down AS ( \
             SELECT id FROM accounts WHERE id = $1 \
             UNION ALL \
             SELECT a.id FROM accounts a JOIN down ON a.parent_account_id = down.id \
         ) SELECT id FROM down",
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;
    Ok(ids)
}

/// Apply the visibility and the subscription of an update request.
async fn apply_update(
    db: &PgPool,
    account: &AccountRow,
    body: &CalendarRequest,
) -> Result<(), ApiError> {
    let subscription = body
        .auto_subscribe
        .map(|value| if value { "auto" } else { "manual" }.to_string());
    sqlx::query(
        "UPDATE accounts SET \
             account_calendar_visible = COALESCE($1, account_calendar_visible), \
             account_calendar_subscription_type = COALESCE($2, account_calendar_subscription_type), \
             updated_at = now() WHERE id = $3",
    )
    .bind(body.visible)
    .bind(subscription)
    .bind(account.id)
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::AccountCalendar#account_calendar_json`.
fn calendar_json(account: &AccountRow) -> Value {
    json!({
        "id": account.id,
        "name": account.name,
        "parent_account_id": account.parent_account_id,
        "root_account_id": account.root_account_id,
        "visible": account.account_calendar_visible,
        "auto_subscribe": account.account_calendar_subscription_type == "auto",
        "asset_string": format!("account_{}", account.id),
        "type": "account",
        "calendar_event_url": format!("/accounts/{}/calendar_events/{{{{ id }}}}", account.id),
        "can_create_calendar_events": false,
        "create_calendar_event_url": format!("/accounts/{}/calendar_events", account.id),
        "new_calendar_event_url": format!("/accounts/{}/calendar_events/new", account.id),
    })
}
