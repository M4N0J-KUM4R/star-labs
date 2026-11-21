//! Canvas account calendars API — a faithful port of
//! `AccountCalendarsApiController`
//! (`canvas-src/app/controllers/account_calendars_api_controller.rb`) and of
//! the serializer `Api::V1::AccountCalendar`
//! (`canvas-src/lib/api/v1/account_calendar.rb`).
//!
//! Routes served here (the crate router mounts this module under `/api/v1`):
//!
//! | Method | Path                                             | Canvas action             |
//! |--------|--------------------------------------------------|---------------------------|
//! | GET    | `/account_calendars`                            | `index`                   |
//! | GET    | `/account_calendars/{account_id}`                | `show`                    |
//! | PUT    | `/account_calendars/{account_id}`                | `update`                  |
//! | PUT    | `/accounts/{account_id}/account_calendars`       | `bulk_update`             |
//! | GET    | `/accounts/{account_id}/account_calendars`       | `all_calendars`           |
//! | GET    | `/accounts/{account_id}/visible_calendars_count` | `visible_calendars_count` |
//!
//! Canvas routes (`config/routes.rb`, the `account_calendars_api` scope):
//!
//! ```text
//! get "account_calendars",                             action: :index
//! get "account_calendars/:account_id",                 action: :show
//! put "account_calendars/:account_id",                 action: :update
//! put "accounts/:account_id/account_calendars",        action: :bulk_update
//! get "accounts/:account_id/account_calendars",        action: :all_calendars
//! get "accounts/:account_id/visible_calendars_count",  action: :visible_calendars_count
//! ```
//!
//! ## Schema
//!
//! The two calendar columns of the account already exist (migration
//! `0236_account_calendars.sql`); no new DDL is needed:
//!
//! ```sql
//! ALTER TABLE accounts
//!     ADD COLUMN IF NOT EXISTS account_calendar_visible BOOLEAN NOT NULL DEFAULT false,
//!     ADD COLUMN IF NOT EXISTS account_calendar_subscription_type TEXT NOT NULL DEFAULT 'manual';
//! ```
//!
//! ## Faithfulness notes
//!
//! * `index` answers `{"account_calendars": [...], "total_results": n}` — the
//!   wrapper is part of the controller, `total_results` counts the whole
//!   (searched) set, not the page.
//! * `all_calendars` answers a **bare array** (no wrapper) and adds
//!   `sub_account_count` to every entry.
//! * `visible_calendars_count` answers `{"count": n}`.
//! * `bulk_update` answers `{"message": "Updated 1 account"}` /
//!   `{"message": "Updated 3 accounts"}`.
//! * The order of the checks is Canvas' order: the account is looked up first
//!   (404 for a missing or inactive account), the permission is checked
//!   afterwards (403), the body of `bulk_update` is validated last (400).
//! * `api_find` maps the account ids `self` and `default` to the root
//!   account; every other unknown id is a 404.
//! * `search_term` needs at least two characters, otherwise Canvas answers
//!   `400` with `{"errors":[{"field":"search_term","code":"invalid",
//!   "message":"2 or more characters is required"}]}`.
//! * `filter` has to be `visible` or `hidden`, otherwise Canvas answers `400`
//!   with `{"errors":"Expected filter param to be one of: visible, hidden"}`.
//! * `value_to_boolean` is Canvas' `Canvas::Plugin.value_to_boolean`
//!   (`yes/y/true/t/on/1` are true, `no/n/false/f/off/0` are false, anything
//!   else falls back to `String#to_i`), so `visible=false` really hides the
//!   calendar and `auto_subscribe=0` really unsubscribes it.
//! * The permission model is the account admin of the account chain
//!   (`AccountAdmin`), the way `star-accounts` models it: `manage_account_calendar_visibility`
//!   and `manage_account_calendar_events` are `true_for: %w[AccountAdmin]`,
//!   `view_account_calendar_details` additionally needs the calendar to be
//!   visible unless the caller manages the account.

use std::collections::HashSet;

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `Api::PER_PAGE`.
const PER_PAGE_DEFAULT: i64 = 10;
/// Canvas `Api::MAX_PER_PAGE`.
const PER_PAGE_MAX: i64 = 100;
/// Canvas `SearchTermHelper::MIN_SEARCH_TERM_LENGTH`.
const MIN_SEARCH_TERM_LENGTH: usize = 2;

/// The recursive CTE that resolves the root account of every account.
///
/// Canvas keeps `root_account_id` in a column, star-lms derives it from the
/// `parent_account_id` chain.
const ACCOUNT_TREE_CTE: &str = r#"
    account_tree AS (
        SELECT a.id, a.id AS root_account_id, a.parent_account_id, 0 AS depth
          FROM accounts a
         WHERE a.parent_account_id IS NULL
        UNION ALL
        SELECT c.id, t.root_account_id, c.parent_account_id, t.depth + 1
          FROM accounts c
          JOIN account_tree t ON c.parent_account_id = t.id
         WHERE t.depth < 64
    )
"#;

/// The CTE of the accounts the caller is associated with.
///
/// Canvas stores the association in `user_account_associations` (the account
/// chain of every account the user belongs to); star-lms keeps the membership
/// in `account_users` and the enrolled courses in `enrollments`, so the
/// association is rebuilt from those two.
const ASSOC_START_CTE: &str = r#"
    assoc_start AS (
        SELECT DISTINCT au.account_id AS id
          FROM account_users au
         WHERE au.user_id = $1
           AND au.workflow_state = 'active'
        UNION
        SELECT c.account_id
          FROM enrollments e
          JOIN courses c ON c.id = e.course_id
         WHERE e.user_id = $1
           AND e.workflow_state = 'active'
           AND c.account_id IS NOT NULL
    ),
    assoc_chain AS (
        SELECT a.id, a.parent_account_id
          FROM accounts a
         WHERE a.id IN (SELECT id FROM assoc_start)
        UNION
        SELECT p.id, p.parent_account_id
          FROM accounts p
          JOIN assoc_chain c ON p.id = c.parent_account_id
    )
"#;

/// The CTE of the accounts the caller administrates.
///
/// Canvas' `cached_account_users_for` looks at the account chain of the
/// account, so an account admin of a parent account administrates the
/// sub-accounts as well.
const ADMIN_ACCOUNTS_CTE: &str = r#"
    admin_accounts AS (
        SELECT DISTINCT au.account_id AS id
          FROM account_users au
         WHERE au.user_id = $1
           AND au.workflow_state = 'active'
           AND au.role IN ('admin', 'AccountAdmin')
    ),
    managed AS (
        SELECT a.id, a.parent_account_id
          FROM accounts a
          JOIN admin_accounts p ON a.parent_account_id = p.id
        UNION
        SELECT a.id, a.parent_account_id
          FROM accounts a
          JOIN managed m ON a.parent_account_id = m.id
    )
"#;

/// The CTE of the active sub-accounts of an account.
///
/// Canvas `Account.sub_account_ids_recursive` only walks the active accounts.
const SUB_ACCOUNTS_CTE: &str = r#"
    sub_accounts AS (
        SELECT a.id, a.parent_account_id
          FROM accounts a
         WHERE a.parent_account_id = $1
           AND a.workflow_state = 'active'
        UNION
        SELECT c.id, c.parent_account_id
          FROM accounts c
          JOIN sub_accounts s ON c.parent_account_id = s.id
         WHERE c.workflow_state = 'active'
    )
"#;

/// One account with its calendar settings.
#[derive(Debug, sqlx::FromRow)]
struct AccountCalendarRow {
    id: i64,
    name: String,
    parent_account_id: Option<i64>,
    root_account_id: i64,
    account_calendar_visible: bool,
    account_calendar_subscription_type: String,
    /// Only the `all_calendars` route reports it (`include: ["sub_account_count"]`).
    sub_account_count: Option<i64>,
}

/// Domain router for the Canvas account calendar routes.
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

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /account_calendars` — `account_calendars#index`.
///
/// The calendars the caller is associated with and that are visible.
async fn index(
    State(state): State<AppState>,
    Query(query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Value>, CalendarError> {
    let search_term = param_value(&query, "search_term").filter(|term| !term.trim().is_empty());
    if let Some(term) = search_term {
        validate_search_term(term)?;
    }
    let (per_page, offset) = page_params(&query);
    let pattern = search_term.map(like_pattern);

    let rows: Vec<AccountCalendarRow> = sqlx::query_as::<_, AccountCalendarRow>(&index_sql())
        .bind(auth.id)
        .bind(pattern.clone())
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    // `total_results` counts the whole set, not only the page.
    let total: i64 = sqlx::query_scalar(&index_count_sql())
        .bind(auth.id)
        .bind(pattern)
        .fetch_one(&state.db)
        .await?;

    let managed = administrated_account_ids(&state.db, auth.id).await?;
    let calendars = rows
        .iter()
        .map(|row| account_calendar_json(row, can_create_events(&managed, row.id), false))
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "account_calendars": calendars,
        "total_results": total,
    })))
}

/// `GET /account_calendars/:account_id` — `account_calendars#show`.
async fn show(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, CalendarError> {
    let account = find_account(&state.db, &account_id).await?;
    authorize_view_details(&state.db, &auth, &account).await?;

    let managed = administrated_account_ids(&state.db, auth.id).await?;
    Ok(Json(account_calendar_json(
        &account,
        can_create_events(&managed, account.id),
        false,
    )))
}

/// `PUT /account_calendars/:account_id` — `account_calendars#update`.
///
/// `visible` and `auto_subscribe` are read from the request parameters, so
/// they may arrive in the query string, in a form encoded body or in a JSON
/// body — exactly like the Rails parameters of the Canvas controller.
async fn update(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<Value>, CalendarError> {
    let mut account = find_account(&state.db, &account_id).await?;
    authorize_manage_visibility(&state.db, &auth, &account).await?;

    let params = merge_params(&query, &headers, &body);

    let mut visible = account.account_calendar_visible;
    let mut subscription = account.account_calendar_subscription_type.clone();
    if let Some(value) = find_param(&params, "visible") {
        visible = value_to_boolean(value);
    }
    if let Some(value) = find_param(&params, "auto_subscribe") {
        subscription = if value_to_boolean(value) {
            "auto".to_string()
        } else {
            "manual".to_string()
        };
    }

    // Canvas saves the account only when it changed.
    if visible != account.account_calendar_visible
        || subscription != account.account_calendar_subscription_type
    {
        sqlx::query(
            "UPDATE accounts \
             SET account_calendar_visible = $2, \
                 account_calendar_subscription_type = $3, \
                 updated_at = now() \
             WHERE id = $1",
        )
        .bind(account.id)
        .bind(visible)
        .bind(&subscription)
        .execute(&state.db)
        .await?;
    }
    account.account_calendar_visible = visible;
    account.account_calendar_subscription_type = subscription;

    let managed = administrated_account_ids(&state.db, auth.id).await?;
    Ok(Json(account_calendar_json(
        &account,
        can_create_events(&managed, account.id),
        false,
    )))
}

/// `PUT /accounts/:account_id/account_calendars` — `account_calendars#bulk_update`.
///
/// The body is the JSON array of the accounts to change, exactly like the
/// `curl --data '[{"id": 1, "visible": true}]'` of the Canvas documentation.
async fn bulk_update(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<Value>, CalendarError> {
    let account = find_account(&state.db, &account_id).await?;
    authorize_manage_visibility(&state.db, &auth, &account).await?;

    // Rails wraps a JSON array into `params[:_json]` and `permit` drops the
    // entries that are not objects.
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let entries: Vec<&Map<String, Value>> = match parsed {
        Value::Array(ref items) => items.iter().filter_map(|item| item.as_object()).collect(),
        _ => Vec::new(),
    };
    if entries.is_empty() {
        return Err(canvas_bad_request("Expected array of objects"));
    }
    if entries.iter().any(|entry| !entry.contains_key("id")) {
        return Err(canvas_bad_request("Missing key(s)"));
    }

    let ids: Vec<&Value> = entries
        .iter()
        .map(|entry| entry.get("id").expect("checked above"))
        .collect();
    let mut seen: Vec<&Value> = Vec::new();
    for id in ids.iter() {
        if seen.contains(id) {
            return Err(canvas_bad_request("Duplicate IDs"));
        }
        seen.push(*id);
    }
    let account_ids: Vec<i64> = ids.iter().map(|id| ruby_to_i_value(id)).collect();

    // Only the account itself and its sub-accounts may be changed.
    let mut allowed = vec![account.id];
    allowed.extend(sub_account_ids(&state.db, account.id).await?);
    if account_ids.iter().any(|id| !allowed.contains(id)) {
        return Err(CalendarError::Api(ApiError::Forbidden));
    }

    let mut enable_visible: Vec<i64> = Vec::new();
    let mut disable_visible: Vec<i64> = Vec::new();
    let mut enable_auto: Vec<i64> = Vec::new();
    let mut disable_auto: Vec<i64> = Vec::new();
    for (entry, id) in entries.iter().zip(account_ids.iter()) {
        match entry.get("visible") {
            Some(Value::Null) | None => {}
            Some(value) => {
                if value_to_boolean(value) {
                    enable_visible.push(*id);
                } else {
                    disable_visible.push(*id);
                }
            }
        }
        match entry.get("auto_subscribe") {
            Some(Value::Null) | None => {}
            Some(value) => {
                if value_to_boolean(value) {
                    enable_auto.push(*id);
                } else {
                    disable_auto.push(*id);
                }
            }
        }
    }

    if !enable_visible.is_empty() {
        sqlx::query(
            "UPDATE accounts SET account_calendar_visible = $2, updated_at = now() \
             WHERE id = ANY($1) AND workflow_state = 'active'",
        )
        .bind(&enable_visible)
        .bind(true)
        .execute(&state.db)
        .await?;
    }
    if !disable_visible.is_empty() {
        sqlx::query(
            "UPDATE accounts SET account_calendar_visible = $2, updated_at = now() \
             WHERE id = ANY($1) AND workflow_state = 'active'",
        )
        .bind(&disable_visible)
        .bind(false)
        .execute(&state.db)
        .await?;
    }
    if !enable_auto.is_empty() {
        sqlx::query(
            "UPDATE accounts SET account_calendar_subscription_type = $2, updated_at = now() \
             WHERE id = ANY($1) AND workflow_state = 'active'",
        )
        .bind(&enable_auto)
        .bind("auto")
        .execute(&state.db)
        .await?;
    }
    if !disable_auto.is_empty() {
        sqlx::query(
            "UPDATE accounts SET account_calendar_subscription_type = $2, updated_at = now() \
             WHERE id = ANY($1) AND workflow_state = 'active'",
        )
        .bind(&disable_auto)
        .bind("manual")
        .execute(&state.db)
        .await?;
    }

    let mut unique_ids = account_ids.clone();
    unique_ids.sort_unstable();
    unique_ids.dedup();
    let message = if unique_ids.len() == 1 {
        "Updated 1 account".to_string()
    } else {
        format!("Updated {} accounts", unique_ids.len())
    };

    Ok(Json(json!({ "message": message })))
}

/// `GET /accounts/:account_id/account_calendars` — `account_calendars#all_calendars`.
///
/// Without a `search_term` and without a `filter` the answer holds the
/// account and its first level of sub-accounts, otherwise the whole subtree.
async fn all_calendars(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Value>, CalendarError> {
    let account = find_account(&state.db, &account_id).await?;
    authorize_manage_visibility(&state.db, &auth, &account).await?;

    let filter = param_value(&query, "filter").filter(|value| !value.trim().is_empty());
    if let Some(value) = filter {
        if value != "visible" && value != "hidden" {
            return Err(canvas_bad_request(
                "Expected filter param to be one of: visible, hidden",
            ));
        }
    }
    let search_term = param_value(&query, "search_term").filter(|term| !term.trim().is_empty());
    if let Some(term) = search_term {
        validate_search_term(term)?;
    }
    let (per_page, offset) = page_params(&query);

    let managed = administrated_account_ids(&state.db, auth.id).await?;
    let rows: Vec<AccountCalendarRow> = if search_term.is_some() || filter.is_some() {
        let pattern = search_term.map(like_pattern);
        let visible = filter.map(|value| value == "visible");
        sqlx::query_as::<_, AccountCalendarRow>(&all_calendars_search_sql())
            .bind(account.id)
            .bind(pattern)
            .bind(visible)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
    } else {
        sqlx::query_as::<_, AccountCalendarRow>(&all_calendars_direct_sql())
            .bind(account.id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
    };

    let calendars = rows
        .iter()
        .map(|row| account_calendar_json(row, can_create_events(&managed, row.id), true))
        .collect::<Vec<_>>();

    Ok(Json(Value::Array(calendars)))
}

/// `GET /accounts/:account_id/visible_calendars_count` —
/// `account_calendars#visible_calendars_count`.
async fn visible_calendars_count(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, CalendarError> {
    let account = find_account(&state.db, &account_id).await?;
    authorize_manage_visibility(&state.db, &auth, &account).await?;

    let count: i64 = sqlx::query_scalar(&visible_count_sql())
        .bind(account.id)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(json!({ "count": count })))
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// The listing of `index` — the visible calendars of the associated accounts.
fn index_sql() -> String {
    format!(
        "WITH RECURSIVE {account_tree}, {assoc_start}, {admin_accounts}, \
         visible_admin_accounts AS ( \
             SELECT a.id FROM accounts a JOIN admin_accounts aa ON aa.id = a.id \
              WHERE a.workflow_state = 'active' AND a.account_calendar_visible = true \
         ), \
         admin_descendants AS ( \
             SELECT a.id, a.parent_account_id \
               FROM accounts a JOIN visible_admin_accounts v ON a.parent_account_id = v.id \
             UNION \
             SELECT a.id, a.parent_account_id \
               FROM accounts a JOIN admin_descendants d ON a.parent_account_id = d.id \
         ) \
         SELECT a.id, a.name, a.parent_account_id, \
                COALESCE(t.root_account_id, a.id) AS root_account_id, \
                a.account_calendar_visible, a.account_calendar_subscription_type, \
                NULL::bigint AS sub_account_count \
           FROM accounts a \
           LEFT JOIN account_tree t ON t.id = a.id \
          WHERE a.workflow_state = 'active' \
            AND a.account_calendar_visible = true \
            AND (a.id IN (SELECT id FROM assoc_chain) \
                 OR a.id IN (SELECT id FROM admin_descendants)) \
            AND ($2::text IS NULL OR a.name ILIKE $2) \
          ORDER BY a.name ASC, a.id ASC \
          LIMIT $3 OFFSET $4",
        account_tree = ACCOUNT_TREE_CTE,
        assoc_start = ASSOC_START_CTE,
        admin_accounts = ADMIN_ACCOUNTS_CTE,
    )
}

/// The total of `index` — the same set, counted.
fn index_count_sql() -> String {
    format!(
        "WITH RECURSIVE {assoc_start}, {admin_accounts}, \
         visible_admin_accounts AS ( \
             SELECT a.id FROM accounts a JOIN admin_accounts aa ON aa.id = a.id \
              WHERE a.workflow_state = 'active' AND a.account_calendar_visible = true \
         ), \
         admin_descendants AS ( \
             SELECT a.id, a.parent_account_id \
               FROM accounts a JOIN visible_admin_accounts v ON a.parent_account_id = v.id \
             UNION \
             SELECT a.id, a.parent_account_id \
               FROM accounts a JOIN admin_descendants d ON a.parent_account_id = d.id \
         ) \
         SELECT count(*) FROM accounts a \
          WHERE a.workflow_state = 'active' \
            AND a.account_calendar_visible = true \
            AND (a.id IN (SELECT id FROM assoc_chain) \
                 OR a.id IN (SELECT id FROM admin_descendants)) \
            AND ($2::text IS NULL OR a.name ILIKE $2)",
        assoc_start = ASSOC_START_CTE,
        admin_accounts = ADMIN_ACCOUNTS_CTE,
    )
}

/// The listing of `all_calendars` with a search term or a filter — the whole
/// subtree of the account.
fn all_calendars_search_sql() -> String {
    format!(
        "WITH RECURSIVE {account_tree}, {sub_accounts} \
         SELECT a.id, a.name, a.parent_account_id, \
                COALESCE(t.root_account_id, a.id) AS root_account_id, \
                a.account_calendar_visible, a.account_calendar_subscription_type, \
                (SELECT count(*) FROM accounts sa \
                  WHERE sa.parent_account_id = a.id \
                    AND sa.workflow_state <> 'deleted') AS sub_account_count \
           FROM accounts a \
           LEFT JOIN account_tree t ON t.id = a.id \
          WHERE a.workflow_state = 'active' \
            AND (a.id = $1 OR a.id IN (SELECT id FROM sub_accounts)) \
            AND ($2::text IS NULL OR a.name ILIKE $2) \
            AND ($3::boolean IS NULL OR a.account_calendar_visible = $3) \
          ORDER BY a.name ASC, a.id ASC \
          LIMIT $4 OFFSET $5",
        account_tree = ACCOUNT_TREE_CTE,
        sub_accounts = SUB_ACCOUNTS_CTE,
    )
}

/// The listing of `all_calendars` without a search term and without a filter —
/// the account and its first level of sub-accounts.
fn all_calendars_direct_sql() -> String {
    format!(
        "WITH RECURSIVE {account_tree} \
         SELECT a.id, a.name, a.parent_account_id, \
                COALESCE(t.root_account_id, a.id) AS root_account_id, \
                a.account_calendar_visible, a.account_calendar_subscription_type, \
                (SELECT count(*) FROM accounts sa \
                  WHERE sa.parent_account_id = a.id \
                    AND sa.workflow_state <> 'deleted') AS sub_account_count \
           FROM accounts a \
           LEFT JOIN account_tree t ON t.id = a.id \
          WHERE a.id = $1 \
             OR (a.parent_account_id = $1 AND a.workflow_state <> 'deleted') \
          ORDER BY (a.id = $1) DESC, a.name ASC, a.id ASC \
          LIMIT $2 OFFSET $3",
        account_tree = ACCOUNT_TREE_CTE,
    )
}

/// The number of the visible calendars of the account and its sub-accounts.
fn visible_count_sql() -> String {
    format!(
        "WITH RECURSIVE {sub_accounts} \
         SELECT count(*) FROM accounts a \
          WHERE a.workflow_state = 'active' \
            AND a.account_calendar_visible = true \
            AND (a.id = $1 OR a.id IN (SELECT id FROM sub_accounts))",
        sub_accounts = SUB_ACCOUNTS_CTE,
    )
}

/// One account of the caller.
async fn find_account(pool: &PgPool, raw_id: &str) -> Result<AccountCalendarRow, CalendarError> {
    let id = resolve_account_id(pool, raw_id).await?;
    let sql = format!(
        "WITH RECURSIVE {account_tree} \
         SELECT a.id, a.name, a.parent_account_id, \
                COALESCE(t.root_account_id, a.id) AS root_account_id, \
                a.account_calendar_visible, a.account_calendar_subscription_type, \
                NULL::bigint AS sub_account_count \
           FROM accounts a \
           LEFT JOIN account_tree t ON t.id = a.id \
          WHERE a.id = $1 AND a.workflow_state = 'active'",
        account_tree = ACCOUNT_TREE_CTE,
    );
    let row = sqlx::query_as::<_, AccountCalendarRow>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(CalendarError::Api(ApiError::NotFound))
}

/// Canvas `Api#api_find` for accounts: `self` and `default` are the root
/// account of the domain, every other unknown id is a 404.
async fn resolve_account_id(pool: &PgPool, raw_id: &str) -> Result<i64, CalendarError> {
    if let Ok(id) = raw_id.trim().parse::<i64>() {
        return Ok(id);
    }
    match raw_id.trim() {
        "self" | "default" => {
            let id: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id LIMIT 1",
            )
            .fetch_optional(pool)
            .await?;
            id.ok_or(CalendarError::Api(ApiError::NotFound))
        }
        _ => Err(CalendarError::Api(ApiError::NotFound)),
    }
}

/// The active sub-accounts of an account (Canvas
/// `Account.sub_account_ids_recursive`).
async fn sub_account_ids(pool: &PgPool, account_id: i64) -> Result<Vec<i64>, CalendarError> {
    let sql = format!(
        "WITH RECURSIVE {sub_accounts} SELECT id FROM sub_accounts",
        sub_accounts = SUB_ACCOUNTS_CTE,
    );
    let ids: Vec<i64> = sqlx::query_scalar(&sql)
        .bind(account_id)
        .fetch_all(pool)
        .await?;
    Ok(ids)
}

/// The accounts the caller administrates (Canvas `cached_account_users_for`).
async fn administrated_account_ids(
    pool: &PgPool,
    user_id: i64,
) -> Result<HashSet<i64>, CalendarError> {
    let sql = format!(
        "WITH RECURSIVE {admin_accounts} \
         SELECT id FROM admin_accounts UNION SELECT id FROM managed",
        admin_accounts = ADMIN_ACCOUNTS_CTE,
    );
    let ids: Vec<i64> = sqlx::query_scalar(&sql)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    Ok(ids.into_iter().collect())
}

/// The accounts the caller is associated with (Canvas `user_account_associations`).
async fn associated_account_ids(
    pool: &PgPool,
    user_id: i64,
) -> Result<HashSet<i64>, CalendarError> {
    let sql = format!(
        "WITH RECURSIVE {assoc_start} SELECT id FROM assoc_chain",
        assoc_start = ASSOC_START_CTE,
    );
    let ids: Vec<i64> = sqlx::query_scalar(&sql)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    Ok(ids.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// star-lms models a Canvas site admin as `users.role = 'admin'`.
fn is_site_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas `authorized_action(account, user, :manage_account_calendar_visibility)`.
///
/// The right is `true_for: %w[AccountAdmin]`, so the caller has to be an
/// account admin of the account or of one of its parents.
async fn authorize_manage_visibility(
    pool: &PgPool,
    auth: &AuthUser,
    account: &AccountCalendarRow,
) -> Result<(), CalendarError> {
    if is_site_admin(auth) {
        return Ok(());
    }
    let managed = administrated_account_ids(pool, auth.id).await?;
    if managed.contains(&account.id) {
        Ok(())
    } else {
        Err(CalendarError::Api(ApiError::Forbidden))
    }
}

/// Canvas `authorized_action(account, user, :view_account_calendar_details)`.
///
/// The account grants the right when the caller is associated with it (or
/// administrates it) and when the calendar is visible (or the caller
/// administrates the account).
async fn authorize_view_details(
    pool: &PgPool,
    auth: &AuthUser,
    account: &AccountCalendarRow,
) -> Result<(), CalendarError> {
    if is_site_admin(auth) {
        return Ok(());
    }
    let managed = administrated_account_ids(pool, auth.id).await?;
    if managed.contains(&account.id) {
        return Ok(());
    }
    let associated = associated_account_ids(pool, auth.id).await?;
    if !associated.contains(&account.id) {
        return Err(CalendarError::Api(ApiError::Forbidden));
    }
    if !account.account_calendar_visible {
        return Err(CalendarError::Api(ApiError::Forbidden));
    }
    Ok(())
}

/// Canvas `CalendarEvent#grants_right?(user, :create)` for an account context
/// — the `manage_account_calendar_events` right.
fn can_create_events(managed: &HashSet<i64>, account_id: i64) -> bool {
    managed.contains(&account_id)
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// The value of a query parameter.
fn param_value<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// The `page` and `per_page` of a request (Canvas `Api.paginate`).
fn page_params(params: &[(String, String)]) -> (i64, i64) {
    let number =
        |key: &str| param_value(params, key).and_then(|value| value.trim().parse::<i64>().ok());
    let page = number("page").unwrap_or(1).max(1);
    let per_page = number("per_page")
        .unwrap_or(PER_PAGE_DEFAULT)
        .clamp(1, PER_PAGE_MAX);
    (per_page, (page - 1) * per_page)
}

/// Canvas `SearchTermHelper.validate_search_term` — at least two characters.
fn validate_search_term(term: &str) -> Result<(), CalendarError> {
    if term.chars().count() >= MIN_SEARCH_TERM_LENGTH {
        return Ok(());
    }
    Err(CalendarError::Canvas(
        StatusCode::BAD_REQUEST,
        json!({
            "errors": [{
                "field": "search_term",
                "code": "invalid",
                "message": format!("{MIN_SEARCH_TERM_LENGTH} or more characters is required"),
            }]
        }),
    ))
}

/// The `LIKE` pattern of a search term (Canvas `ActiveRecord::Base.wildcard`).
///
/// The wildcards of the term are escaped, the backslash is the escape
/// character of PostgreSQL.
fn like_pattern(term: &str) -> String {
    let mut pattern = String::with_capacity(term.len() + 2);
    pattern.push('%');
    for ch in term.chars() {
        match ch {
            '\\' => pattern.push_str("\\\\"),
            '%' => pattern.push_str("\\%"),
            '_' => pattern.push_str("\\_"),
            other => pattern.push(other),
        }
    }
    pattern.push('%');
    pattern
}

/// The request parameters of a PUT — the query string plus the body.
///
/// Rails merges the query string and the body parameters, the body wins.
fn merge_params(
    query: &[(String, String)],
    headers: &HeaderMap,
    body: &Bytes,
) -> Vec<(String, Value)> {
    let mut params: Vec<(String, Value)> = query
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    if body.is_empty() {
        return params;
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let text = String::from_utf8_lossy(body);
    let looks_like_json = {
        let trimmed = text.trim_start();
        trimmed.starts_with('{') || trimmed.starts_with('[')
    };
    if content_type.contains("json") || (content_type.is_empty() && looks_like_json) {
        if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) {
            for (key, value) in map {
                set_param(&mut params, key, value);
            }
        }
    } else {
        for (key, value) in parse_form_body(body) {
            set_param(&mut params, key, Value::String(value));
        }
    }
    params
}

/// Set a parameter, replacing the value of an equal key.
fn set_param(params: &mut Vec<(String, Value)>, key: String, value: Value) {
    if let Some(entry) = params.iter_mut().find(|(name, _)| *name == key) {
        entry.1 = value;
    } else {
        params.push((key, value));
    }
}

/// The value of a parameter.
fn find_param<'a>(params: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    params
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

/// The pairs of a form encoded body.
fn parse_form_body(body: &Bytes) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(body);
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (percent_decode(key), percent_decode(value)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

/// Decode a form value (percent escapes and `+` as a space).
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
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        out.push((high * 16 + low) as u8);
                        index += 3;
                    }
                    _ => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Canvas `Canvas::Plugin.value_to_boolean`.
fn value_to_boolean(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => string_to_boolean(text),
        Value::Number(number) => number
            .as_f64()
            .map(|value| value.trunc() != 0.0)
            .unwrap_or(false),
        _ => false,
    }
}

/// Canvas `Canvas::Plugin.value_to_boolean` for a string.
fn string_to_boolean(raw: &str) -> bool {
    match raw.to_lowercase().as_str() {
        "yes" | "y" | "true" | "t" | "on" | "1" => true,
        "no" | "n" | "false" | "f" | "off" | "0" => false,
        other => ruby_to_i(other) != 0,
    }
}

/// Ruby's `String#to_i` — the leading integer of the string, 0 when there is
/// none.
fn ruby_to_i(value: &str) -> i64 {
    let trimmed = value.trim_start();
    let mut end = 0;
    for (index, ch) in trimmed.char_indices() {
        if ch.is_ascii_digit() || (index == 0 && (ch == '-' || ch == '+')) {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    trimmed[..end].parse::<i64>().unwrap_or(0)
}

/// Ruby's `to_i` of a JSON value.
fn ruby_to_i_value(value: &Value) -> i64 {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value.trunc() as i64))
            .unwrap_or(0),
        Value::String(text) => ruby_to_i(text),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::AccountCalendar#account_calendar_json`.
fn account_calendar_json(
    row: &AccountCalendarRow,
    can_create_calendar_events: bool,
    include_sub_account_count: bool,
) -> Value {
    let mut json = Map::new();
    json.insert("id".to_string(), Value::from(row.id));
    json.insert("name".to_string(), Value::from(row.name.clone()));
    json.insert(
        "parent_account_id".to_string(),
        match row.parent_account_id {
            Some(id) => Value::from(id),
            None => Value::Null,
        },
    );
    json.insert(
        "root_account_id".to_string(),
        Value::from(row.root_account_id),
    );
    json.insert(
        "visible".to_string(),
        Value::from(row.account_calendar_visible),
    );
    json.insert(
        "auto_subscribe".to_string(),
        Value::from(row.account_calendar_subscription_type == "auto"),
    );
    json.insert(
        "asset_string".to_string(),
        Value::from(format!("account_{}", row.id)),
    );
    json.insert("type".to_string(), Value::from("account"));
    if include_sub_account_count {
        json.insert(
            "sub_account_count".to_string(),
            Value::from(row.sub_account_count.unwrap_or(0)),
        );
    }
    json.insert(
        "calendar_event_url".to_string(),
        Value::from(format!("/accounts/{}/calendar_events/{{{{ id }}}}", row.id)),
    );
    json.insert(
        "can_create_calendar_events".to_string(),
        Value::from(can_create_calendar_events),
    );
    json.insert(
        "create_calendar_event_url".to_string(),
        Value::from(format!("/accounts/{}/calendar_events", row.id)),
    );
    json.insert(
        "new_calendar_event_url".to_string(),
        Value::from(format!("/accounts/{}/calendar_events/new", row.id)),
    );
    Value::Object(json)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The error type of the handlers.
///
/// Canvas answers with JSON bodies: `api_find` raises
/// `ActiveRecord::RecordNotFound` (a 404 with
/// `{"errors":[{"message":"The specified resource does not exist."}]}`), a
/// refused `authorized_action` renders `render_json_unauthorized` (a 403 for a
/// signed in caller, a 401 for an anonymous one) and the controller renders
/// its own 400 bodies.
#[derive(Debug)]
enum CalendarError {
    Api(ApiError),
    Canvas(StatusCode, Value),
}

impl From<ApiError> for CalendarError {
    fn from(error: ApiError) -> Self {
        CalendarError::Api(error)
    }
}

impl From<sqlx::Error> for CalendarError {
    fn from(error: sqlx::Error) -> Self {
        CalendarError::Api(ApiError::Db(error.to_string()))
    }
}

impl IntoResponse for CalendarError {
    fn into_response(self) -> Response {
        match self {
            CalendarError::Api(ApiError::NotFound) => (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "errors": [{ "message": "The specified resource does not exist." }]
                })),
            )
                .into_response(),
            CalendarError::Api(ApiError::Unauthorized) => (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "status": "unauthenticated",
                    "errors": [{ "message": "user authorization required" }]
                })),
            )
                .into_response(),
            CalendarError::Api(ApiError::Forbidden) => (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "status": "unauthorized",
                    "errors": [{ "message": "user not authorized to perform that action" }]
                })),
            )
                .into_response(),
            CalendarError::Api(ApiError::BadRequest(message)) => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "errors": [{ "message": message }] })),
            )
                .into_response(),
            CalendarError::Api(ApiError::Validation(message)) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "errors": [{ "message": message }] })),
            )
                .into_response(),
            CalendarError::Api(ApiError::Conflict(message)) => (
                StatusCode::CONFLICT,
                Json(json!({ "errors": [{ "message": message }] })),
            )
                .into_response(),
            CalendarError::Api(ApiError::Db(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "errors": [{
                        "message": "An error occurred.",
                        "error_code": "internal_server_error"
                    }]
                })),
            )
                .into_response(),
            CalendarError::Canvas(status, body) => (status, Json(body)).into_response(),
        }
    }
}

/// A 400 with the body of the Canvas controller.
fn canvas_bad_request(message: &str) -> CalendarError {
    CalendarError::Canvas(StatusCode::BAD_REQUEST, json!({ "errors": message }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn value_to_boolean_follows_canvas() {
        assert!(value_to_boolean(&json!(true)));
        assert!(!value_to_boolean(&json!(false)));
        assert!(value_to_boolean(&json!("true")));
        assert!(!value_to_boolean(&json!("false")));
        assert!(value_to_boolean(&json!("yes")));
        assert!(!value_to_boolean(&json!("no")));
        assert!(value_to_boolean(&json!("1")));
        assert!(!value_to_boolean(&json!("0")));
        assert!(!value_to_boolean(&json!(null)));
        assert!(!value_to_boolean(&json!("nonsense")));
        // Anything else falls back to `String#to_i`.
        assert!(value_to_boolean(&json!("12")));
    }

    #[test]
    fn ruby_to_i_reads_the_leading_integer() {
        assert_eq!(ruby_to_i("13"), 13);
        assert_eq!(ruby_to_i("13abc"), 13);
        assert_eq!(ruby_to_i("abc"), 0);
        assert_eq!(ruby_to_i(""), 0);
        assert_eq!(ruby_to_i("-4"), -4);
        assert_eq!(ruby_to_i_value(&json!(13)), 13);
        assert_eq!(ruby_to_i_value(&json!("13")), 13);
        assert_eq!(ruby_to_i_value(&json!(null)), 0);
    }

    #[test]
    fn like_pattern_escapes_the_wildcards() {
        assert_eq!(like_pattern("chem"), "%chem%");
        assert_eq!(like_pattern("50%"), "%50\\%%");
        assert_eq!(like_pattern("a_b"), "%a\\_b%");
    }

    #[test]
    fn percent_decode_reads_the_form_escapes() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
    }

    #[test]
    fn a_short_search_term_is_a_bad_request() {
        assert!(validate_search_term("ch").is_ok());
        assert!(validate_search_term("c").is_err());
    }

    #[test]
    fn the_pagination_follows_canvas() {
        let query = vec![
            ("page".to_string(), "3".to_string()),
            ("per_page".to_string(), "5".to_string()),
        ];
        assert_eq!(page_params(&query), (5, 10));
        assert_eq!(page_params(&[]), (PER_PAGE_DEFAULT, 0));
        let huge = vec![("per_page".to_string(), "1000".to_string())];
        assert_eq!(page_params(&huge).0, PER_PAGE_MAX);
    }
}
