//! Canvas ePortfolios API v1 — port of `EportfoliosApiController`
//! (`canvas-src/app/controllers/eportfolios_api_controller.rb`).
//!
//! Routes served here (the coordinator mounts this router under `/api/v1`):
//!
//! | Method | Path                                     | Canvas action  |
//! |--------|------------------------------------------|----------------|
//! | GET    | `/users/{user_id}/eportfolios`           | `index`        |
//! | PUT    | `/users/{user_id}/eportfolios`           | `moderate_all` |
//! | GET    | `/eportfolios/{id}`                      | `show`         |
//! | DELETE | `/eportfolios/{id}`                      | `delete`       |
//! | GET    | `/eportfolios/{eportfolio_id}/pages`     | `pages`        |
//! | PUT    | `/eportfolios/{eportfolio_id}/moderate`  | `moderate`     |
//! | PUT    | `/eportfolios/{eportfolio_id}/restore`   | `restore`      |
//!
//! ## Schema
//!
//! The three Canvas tables this controller touches already exist in
//! `star-db` (migration `0224_canvas_schema_tables_b.sql`, indexes in
//! `0226`, foreign keys in `0227`). **No new migration is required.** The
//! statements are repeated here for the record:
//!
//! ```sql
//! CREATE TABLE eportfolio_categories (
//!     id                               BIGSERIAL PRIMARY KEY,
//!     eportfolio_id                    bigint NOT NULL,
//!     name                             varchar(255),
//!     position                         integer,
//!     slug                             varchar(255),
//!     created_at                       timestamp NOT NULL,
//!     updated_at                       timestamp NOT NULL
//! );
//!
//! CREATE TABLE eportfolio_entries (
//!     id                               BIGSERIAL PRIMARY KEY,
//!     eportfolio_id                    bigint NOT NULL,
//!     eportfolio_category_id           bigint NOT NULL,
//!     position                         integer,
//!     name                             varchar(255),
//!     allow_comments                   boolean,
//!     show_comments                    boolean,
//!     slug                             varchar(255),
//!     content                          text,
//!     created_at                       timestamp NOT NULL,
//!     updated_at                       timestamp NOT NULL
//! );
//!
//! CREATE TABLE eportfolios (
//!     id                               BIGSERIAL PRIMARY KEY,
//!     user_id                          bigint NOT NULL,
//!     name                             varchar(255),
//!     public                           boolean,
//!     created_at                       timestamp NOT NULL,
//!     updated_at                       timestamp NOT NULL,
//!     uuid                             varchar(255),
//!     workflow_state                   varchar(255) NOT NULL DEFAULT 'active',
//!     deleted_at                       timestamp,
//!     spam_status                      varchar
//! );
//! ```
//!
//! ## Faithfulness notes
//!
//! * The listings answer a **bare JSON array**, exactly like
//!   `render json: portfolios.map { |e| eportfolio_json(e, ...) }`. `Api.paginate`
//!   only adds the `Link` header.
//! * The lookup order of every action is Canvas' `before_action` chain:
//!   `reject_student_view_student` → `verified_user_check` → the action's own
//!   `Eportfolio.find` (404) → the policy check (403). A missing resource is
//!   therefore a 404 *before* a missing permission is a 403.
//! * `verified_user_check` is Canvas `Account#user_needs_verification?`:
//!   `require_confirmed_email? && user.cached_active_emails.none?`. The
//!   setting lives in `accounts.settings`; the "active emails" are the
//!   `communication_channels` rows with `path_type = 'email'` and
//!   `workflow_state = 'active'`.
//! * `reject_student_view_student` needs a Canvas student-view enrollment
//!   (`StudentViewEnrollment`); star-lms has no student-view impersonation,
//!   so the guard is a no-op here.
//! * The rights on a portfolio are Canvas' `Eportfolio` policy:
//!   the owner may `:read`/`:delete` their own **active** portfolio, everybody
//!   may `:read` an **active**, **public**, **not spam** portfolio, and an
//!   account administrator with `:moderate_user_content` on the owner may
//!   `:read`, `:moderate`, `:delete` and `:restore`.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Canvas `Api::PER_PAGE` — the default page size of `Api.paginate`.
const PER_PAGE: i64 = 10;

/// Canvas `Api::MAX_PER_PAGE`.
const MAX_PER_PAGE: i64 = 100;

/// Canvas `Eportfolio::SPAM_MODERATIONS` — the spam statuses an administrator
/// may set through the moderation routes.
const SPAM_MODERATIONS: [&str; 2] = ["marked_as_safe", "marked_as_spam"];

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Domain router for the Canvas ePortfolio routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users/{user_id}/eportfolios", get(index).put(moderate_all))
        .route("/eportfolios/{id}", get(show).delete(delete))
        .route("/eportfolios/{eportfolio_id}/pages", get(pages))
        .route("/eportfolios/{eportfolio_id}/moderate", put(moderate))
        .route("/eportfolios/{eportfolio_id}/restore", put(restore))
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One `eportfolios` row, in the shape of Canvas'
/// `Api::V1::Eportfolio::EPORTFOLIO_ATTRIBUTES`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct PortfolioRow {
    id: i64,
    user_id: i64,
    name: Option<String>,
    public: Option<bool>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    workflow_state: String,
    deleted_at: Option<DateTime<Utc>>,
    spam_status: Option<String>,
}

impl PortfolioRow {
    /// Canvas `Eportfolio#active?` — the `active` scope is
    /// `where("eportfolios.workflow_state<>'deleted'")`.
    fn is_active(&self) -> bool {
        self.workflow_state != "deleted"
    }

    /// Canvas `Eportfolio#spam?` — `marked_as_spam` or
    /// `flagged_as_possible_spam`.
    fn is_spam(&self) -> bool {
        matches!(
            self.spam_status.as_deref(),
            Some("marked_as_spam") | Some("flagged_as_possible_spam")
        )
    }
}

/// One `eportfolio_entries` row, in the shape of Canvas'
/// `Api::V1::Eportfolio::ENTRY_ATTRIBUTES`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct EntryRow {
    id: i64,
    eportfolio_id: i64,
    position: Option<i32>,
    name: Option<String>,
    content: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// The error type of the handlers.
///
/// Canvas answers the specific failures with a JSON body; the house
/// `ApiError` answers everything else with its plain text body.
enum PortfolioError {
    Api(ApiError),
    Canvas { status: StatusCode, body: Value },
}

impl From<ApiError> for PortfolioError {
    fn from(error: ApiError) -> Self {
        PortfolioError::Api(error)
    }
}

impl From<sqlx::Error> for PortfolioError {
    fn from(error: sqlx::Error) -> Self {
        PortfolioError::Api(ApiError::from(error))
    }
}

impl IntoResponse for PortfolioError {
    fn into_response(self) -> Response {
        match self {
            // Canvas `render_json_unauthorized`: an authenticated caller
            // without the right gets 403 and this body.
            PortfolioError::Api(ApiError::Forbidden) => (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "status": "unauthorized",
                    "errors": [{ "message": "user not authorized to perform that action" }],
                })),
            )
                .into_response(),
            // Canvas `render json: { error: "..." }, status: :bad_request`.
            PortfolioError::Api(ApiError::BadRequest(message)) => {
                (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
            }
            PortfolioError::Canvas { status, body } => (status, Json(body)).into_response(),
            PortfolioError::Api(other) => other.into_response(),
        }
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Rails serializes a `timestamp` column with `ActiveSupport::JSON` and the
/// default precision of three fractional digits.
fn iso8601(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Canvas `Api::V1::Eportfolio#eportfolio_json`.
fn portfolio_json(row: &PortfolioRow) -> Value {
    json!({
        "id": row.id,
        "user_id": row.user_id,
        "name": row.name,
        // Canvas: `hash["public"] = !!hash["public"]`.
        "public": row.public.unwrap_or(false),
        "created_at": iso8601(row.created_at),
        "updated_at": iso8601(row.updated_at),
        "workflow_state": row.workflow_state,
        "deleted_at": row.deleted_at.map(iso8601),
        "spam_status": row.spam_status,
    })
}

/// Canvas `Api::V1::Eportfolio#eportfolio_entry_json`.
///
/// `EportfolioEntry` declares `serialize :content`, so the column holds the
/// serialized sections. A column that still holds a JSON array or object is
/// answered as such, everything else stays the plain string it is.
fn entry_json(row: &EntryRow) -> Value {
    json!({
        "id": row.id,
        "eportfolio_id": row.eportfolio_id,
        "position": row.position,
        "name": row.name,
        "content": entry_content(row.content.as_deref()),
        "created_at": iso8601(row.created_at),
        "updated_at": iso8601(row.updated_at),
    })
}

/// The deserialized value of an `EportfolioEntry#content` column.
fn entry_content(raw: Option<&str>) -> Value {
    let Some(raw) = raw else {
        return Value::Null;
    };
    match serde_json::from_str::<Value>(raw) {
        Ok(value) => match value {
            Value::Array(_) | Value::Object(_) => value,
            _ => Value::String(raw.to_string()),
        },
        Err(_) => Value::String(raw.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Canvas `api_find(User, params[:user_id])`: the literal `self` resolves to
/// the current user, an unknown id is a 404.
fn resolve_user_segment(raw: &str, self_id: i64) -> Result<i64, PortfolioError> {
    if raw == "self" {
        return Ok(self_id);
    }
    match raw.parse::<i64>() {
        Ok(id) if id > 0 => Ok(id),
        _ => Err(ApiError::NotFound.into()),
    }
}

/// Canvas `Eportfolio.find(params[:id])` — a non numeric id is a
/// `RecordNotFound`, so a 404.
fn resolve_portfolio_id(raw: &str) -> Result<i64, PortfolioError> {
    match raw.parse::<i64>() {
        Ok(id) => Ok(id),
        Err(_) => Err(ApiError::NotFound.into()),
    }
}

/// The values of one query string (or form body) parameter.
fn query_values(raw: Option<&str>, key: &str) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let name = percent_decode(parts.next().unwrap_or_default());
            let value = percent_decode(parts.next().unwrap_or_default());
            if name == key {
                Some(value)
            } else {
                None
            }
        })
        .collect()
}

/// The percent decoding of a query string value (`+` is a space).
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Ruby's `String#to_i`: the leading integer of the string, 0 when there is
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

/// Canvas `Api.per_page_for`: `params[:per_page] || PER_PAGE`, clamped to
/// `1..MAX_PER_PAGE`.
fn per_page_for(raw_query: Option<&str>) -> i64 {
    match query_values(raw_query, "per_page").into_iter().next() {
        Some(value) => ruby_to_i(&value).clamp(1, MAX_PER_PAGE),
        None => PER_PAGE,
    }
}

/// The requested page, starting at 1.
fn page_number(raw_query: Option<&str>) -> i64 {
    match query_values(raw_query, "page").into_iter().next() {
        Some(value) => ruby_to_i(&value).max(1),
        None => 1,
    }
}

/// Canvas `params[:spam_status]`.
///
/// Rails merges the query string, a form encoded body and a JSON body into
/// `params`, so all three are read here.
fn spam_status_param(headers: &HeaderMap, body: &Bytes, raw_query: Option<&str>) -> Option<String> {
    if let Some(value) = query_values(raw_query, "spam_status").into_iter().next() {
        return Some(value);
    }
    if body.is_empty() {
        return None;
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let is_json = content_type.starts_with("application/json") || body.starts_with(b"{");
    if is_json {
        let Ok(value) = serde_json::from_slice::<Value>(body) else {
            return None;
        };
        return value
            .get("spam_status")
            .and_then(|status| status.as_str())
            .map(|status| status.to_string());
    }
    let text = String::from_utf8_lossy(body);
    query_values(Some(text.as_ref()), "spam_status")
        .into_iter()
        .next()
}

/// The error body of Canvas' `Eportfolio::SPAM_MODERATIONS.exclude?` branch.
fn spam_moderations_error() -> String {
    let list = SPAM_MODERATIONS
        .iter()
        .map(|status| format!("\"{status}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("spam_status must be one of [{list}]")
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The scheme and host of the request, like Canvas' `url_for`.
fn base_url(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(',').next().unwrap_or("http").trim().to_string())
        .unwrap_or_else(|| "http".to_string());
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "localhost".to_string());
    format!("{scheme}://{host}")
}

/// The `Link` header Canvas builds in `Api.build_links_from_hash`.
fn pagination_links(
    base: &str,
    query: &str,
    current: i64,
    per_page: i64,
    total: i64,
) -> Option<String> {
    let last_page = if per_page > 0 {
        ((total + per_page - 1) / per_page).max(1)
    } else {
        1
    };
    let link = |page: i64| -> String {
        let mut url = format!("{base}?");
        if !query.is_empty() {
            url.push_str(query);
            url.push('&');
        }
        url.push_str(&format!("page={page}&per_page={per_page}"));
        url
    };

    let mut links: Vec<String> = Vec::new();
    links.push(format!("<{}>; rel=\"current\"", link(current)));
    if current < last_page {
        links.push(format!("<{}>; rel=\"next\"", link(current + 1)));
    }
    if current > 1 {
        links.push(format!("<{}>; rel=\"prev\"", link(current - 1)));
    }
    links.push(format!("<{}>; rel=\"first\"", link(1)));
    links.push(format!("<{}>; rel=\"last\"", link(last_page)));
    Some(links.join(","))
}

/// The query string of the `index` pagination links.
fn index_query_string(include_deleted: bool, per_page: i64, page: i64) -> String {
    let mut parts: Vec<String> = Vec::new();
    if include_deleted {
        parts.push("include[]=deleted".to_string());
    }
    parts.push(format!("per_page={per_page}"));
    parts.push(format!("page={page}"));
    parts.join("&")
}

/// The query string of the `pages` pagination links.
fn pages_query_string(per_page: i64, page: i64) -> String {
    format!("per_page={per_page}&page={page}")
}

/// Attach the `Link` header to a paginated response.
fn with_pagination_links(mut response: Response, links: Option<String>) -> Response {
    if let Some(links) = links {
        if let Ok(value) = header::HeaderValue::from_str(&links) {
            response.headers_mut().insert(header::LINK, value);
        }
    }
    response
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// 404 when the user does not exist (Canvas `api_find(User, ...)`).
async fn ensure_user(pool: &PgPool, user_id: i64) -> Result<(), PortfolioError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    match row {
        Some(_) => Ok(()),
        None => Err(ApiError::NotFound.into()),
    }
}

/// Canvas `User#grants_right?(actor, :moderate_user_content)`.
///
/// The right is granted by the accounts the owner belongs to: the actor has
/// to administrate one of them. A site administrator holds every right.
async fn can_moderate_user_content(
    pool: &PgPool,
    auth: &AuthUser,
    owner_id: i64,
) -> Result<bool, PortfolioError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM account_users actor
        JOIN account_users member ON member.account_id = actor.account_id
        WHERE actor.user_id = $1 AND actor.role = 'admin' AND member.user_id = $2
        UNION ALL
        SELECT 1
        FROM account_users actor
        JOIN courses c ON c.account_id = actor.account_id
        JOIN enrollments e ON e.course_id = c.id
        WHERE actor.user_id = $1 AND actor.role = 'admin' AND e.user_id = $2
        LIMIT 1
        "#,
    )
    .bind(auth.id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `User#eportfolios_enabled?`.
///
/// `associated_root_accounts.empty? || (associated_root_accounts.include?(root)
/// && root.settings[:enable_eportfolios] != false)`.
async fn eportfolios_enabled(pool: &PgPool, user_id: i64) -> Result<bool, PortfolioError> {
    let sql = r#"
        WITH RECURSIVE user_accounts AS (
            SELECT a.id AS id, a.parent_account_id AS parent_account_id
            FROM accounts a
            JOIN account_users au ON au.account_id = a.id
            WHERE au.user_id = $1
            UNION
            SELECT acc.id, acc.parent_account_id
            FROM enrollments e
            JOIN courses c ON c.id = e.course_id
            JOIN accounts acc ON acc.id = c.account_id
            WHERE e.user_id = $1
        ), chains AS (
            SELECT id, parent_account_id, 0 AS depth FROM user_accounts
            UNION ALL
            SELECT parent.id, parent.parent_account_id, chain.depth + 1
            FROM accounts parent
            JOIN chains chain ON parent.id = chain.parent_account_id
            WHERE chain.depth < 10
        )
        SELECT DISTINCT id FROM chains WHERE parent_account_id IS NULL
    "#;
    let roots: Vec<i64> = sqlx::query_scalar(sql)
        .bind(user_id)
        .fetch_all(pool)
        .await?;
    if roots.is_empty() {
        return Ok(true);
    }

    // The domain root account of star-lms is the site root account.
    let root: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT id, settings->>'enable_eportfolios' FROM accounts \
         WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some((root_id, enabled)) = root else {
        return Ok(true);
    };
    if !roots.contains(&root_id) {
        // The user belongs to another root account than the one that serves
        // the request.
        return Ok(false);
    }
    Ok(enabled.as_deref() != Some("false"))
}

/// Canvas `Eportfolio` policy — `:read`.
///
/// * the owner of an active portfolio (eportfolios enabled),
/// * everybody for an active, public and not spam portfolio,
/// * an account administrator with `:moderate_user_content` for an active
///   portfolio.
async fn can_read_portfolio(
    pool: &PgPool,
    auth: &AuthUser,
    portfolio: &PortfolioRow,
) -> Result<bool, PortfolioError> {
    if !portfolio.is_active() {
        return Ok(false);
    }
    let is_owner = portfolio.user_id == auth.id;
    if is_owner && eportfolios_enabled(pool, portfolio.user_id).await? {
        return Ok(true);
    }
    if portfolio.public.unwrap_or(false)
        && !portfolio.is_spam()
        && eportfolios_enabled(pool, portfolio.user_id).await?
    {
        return Ok(true);
    }
    if !is_owner && can_moderate_user_content(pool, auth, portfolio.user_id).await? {
        return Ok(true);
    }
    Ok(false)
}

/// Canvas `Eportfolio` policy — `:delete`.
async fn can_delete_portfolio(
    pool: &PgPool,
    auth: &AuthUser,
    portfolio: &PortfolioRow,
) -> Result<bool, PortfolioError> {
    if !portfolio.is_active() {
        return Ok(false);
    }
    let is_owner = portfolio.user_id == auth.id;
    if is_owner && eportfolios_enabled(pool, portfolio.user_id).await? {
        return Ok(true);
    }
    if !is_owner && can_moderate_user_content(pool, auth, portfolio.user_id).await? {
        return Ok(true);
    }
    Ok(false)
}

/// Canvas `Eportfolio` policy — `:moderate`.
async fn can_moderate_portfolio(
    pool: &PgPool,
    auth: &AuthUser,
    portfolio: &PortfolioRow,
) -> Result<bool, PortfolioError> {
    Ok(portfolio.is_active()
        && portfolio.user_id != auth.id
        && can_moderate_user_content(pool, auth, portfolio.user_id).await?)
}

/// Canvas `Eportfolio` policy — `:restore`.
///
/// The policy grants the right to a moderator of the owner for an active
/// portfolio (first rule) and for a deleted one (last rule), so both states
/// restore.
async fn can_restore_portfolio(
    pool: &PgPool,
    auth: &AuthUser,
    portfolio: &PortfolioRow,
) -> Result<bool, PortfolioError> {
    Ok(portfolio.user_id != auth.id
        && can_moderate_user_content(pool, auth, portfolio.user_id).await?)
}

/// Canvas `verified_user_check` — `Account#user_needs_verification?`.
///
/// The root account decides with its `require_confirmed_email` setting; an
/// unverified caller gets Canvas' `render_unverified_error` body.
async fn verified_user_check(pool: &PgPool, auth: &AuthUser) -> Result<(), PortfolioError> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT settings->>'require_confirmed_email' FROM accounts \
         WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some((required,)) = row else {
        return Ok(());
    };
    if required.as_deref() != Some("true") {
        return Ok(());
    }
    let channel: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM communication_channels \
         WHERE user_id = $1 AND path_type = 'email' AND workflow_state = 'active' LIMIT 1",
    )
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if channel.is_some() {
        return Ok(());
    }
    Err(PortfolioError::Canvas {
        status: StatusCode::UNAUTHORIZED,
        body: json!({
            "status": "unverified",
            "errors": [{
                "message": "user not authorized to perform that action until verifying email"
            }],
        }),
    })
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// The `eportfolios` columns of the serializer.
const PORTFOLIO_COLUMNS: &str = "id, user_id, name, public, \
     created_at AT TIME ZONE 'UTC' AS created_at, \
     updated_at AT TIME ZONE 'UTC' AS updated_at, \
     workflow_state, \
     deleted_at AT TIME ZONE 'UTC' AS deleted_at, \
     spam_status";

/// Canvas `Eportfolio.find(params[:id])`.
async fn find_portfolio(pool: &PgPool, id: i64) -> Result<PortfolioRow, PortfolioError> {
    let sql = format!("SELECT {PORTFOLIO_COLUMNS} FROM eportfolios WHERE id = $1");
    let row: Option<PortfolioRow> = sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
    match row {
        Some(row) => Ok(row),
        None => Err(ApiError::NotFound.into()),
    }
}

/// The portfolios of a user, ordered by `updated_at` like Canvas'
/// `scope.order(:updated_at)`.
async fn fetch_portfolios(
    pool: &PgPool,
    user_id: i64,
    active_only: bool,
    per_page: i64,
    page: i64,
) -> Result<Vec<PortfolioRow>, PortfolioError> {
    let offset = (page - 1) * per_page;
    let sql = format!(
        "SELECT {PORTFOLIO_COLUMNS} FROM eportfolios \
         WHERE user_id = $1 AND (NOT $2::boolean OR workflow_state <> 'deleted') \
         ORDER BY updated_at ASC LIMIT $3 OFFSET $4"
    );
    let rows: Vec<PortfolioRow> = sqlx::query_as(&sql)
        .bind(user_id)
        .bind(active_only)
        .bind(per_page)
        .bind(offset)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// The number of portfolios of the paginated scope.
async fn count_portfolios(
    pool: &PgPool,
    user_id: i64,
    active_only: bool,
) -> Result<i64, PortfolioError> {
    let (total,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM eportfolios \
         WHERE user_id = $1 AND (NOT $2::boolean OR workflow_state <> 'deleted')",
    )
    .bind(user_id)
    .bind(active_only)
    .fetch_one(pool)
    .await?;
    Ok(total)
}

/// The pages of a portfolio, ordered by `position` like Canvas'
/// `portfolio.eportfolio_entries.order(:position)`.
async fn fetch_entries(
    pool: &PgPool,
    eportfolio_id: i64,
    per_page: i64,
    page: i64,
) -> Result<Vec<EntryRow>, PortfolioError> {
    let offset = (page - 1) * per_page;
    let rows: Vec<EntryRow> = sqlx::query_as(
        "SELECT id, eportfolio_id, position, name, content, \
                created_at AT TIME ZONE 'UTC' AS created_at, \
                updated_at AT TIME ZONE 'UTC' AS updated_at \
         FROM eportfolio_entries WHERE eportfolio_id = $1 \
         ORDER BY position ASC LIMIT $2 OFFSET $3",
    )
    .bind(eportfolio_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// The number of pages of the paginated scope.
async fn count_entries(pool: &PgPool, eportfolio_id: i64) -> Result<i64, PortfolioError> {
    let (total,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM eportfolio_entries WHERE eportfolio_id = $1")
            .bind(eportfolio_id)
            .fetch_one(pool)
            .await?;
    Ok(total)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/v1/users/:user_id/eportfolios` — Canvas
/// `EportfoliosApiController#index`.
async fn index(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, PortfolioError> {
    verified_user_check(&state.db, &auth).await?;

    let owner_id = resolve_user_segment(&user_id, auth.id)?;
    ensure_user(&state.db, owner_id).await?;

    let is_owner = owner_id == auth.id;
    if !is_owner && !can_moderate_user_content(&state.db, &auth, owner_id).await? {
        return Err(ApiError::Forbidden.into());
    }

    // Canvas: `params[:include] ||= []` and
    // `if user == @current_user || params[:include].exclude?("deleted")`.
    let include_deleted = query_values(raw_query.as_deref(), "include[]")
        .iter()
        .any(|value| value == "deleted");
    let active_only = is_owner || !include_deleted;

    let per_page = per_page_for(raw_query.as_deref());
    let page = page_number(raw_query.as_deref());

    let rows = fetch_portfolios(&state.db, owner_id, active_only, per_page, page).await?;
    let total = count_portfolios(&state.db, owner_id, active_only).await?;
    let payload: Vec<Value> = rows.iter().map(portfolio_json).collect();

    let base = format!("{}/api/v1/users/{owner_id}/eportfolios", base_url(&headers));
    let links = pagination_links(
        &base,
        &index_query_string(include_deleted, per_page, page),
        page,
        per_page,
        total,
    );
    Ok(with_pagination_links(Json(payload).into_response(), links))
}

/// `PUT /api/v1/users/:user_id/eportfolios` — Canvas
/// `EportfoliosApiController#moderate_all`.
async fn moderate_all(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, PortfolioError> {
    verified_user_check(&state.db, &auth).await?;

    let owner_id = resolve_user_segment(&user_id, auth.id)?;
    ensure_user(&state.db, owner_id).await?;

    if !can_moderate_user_content(&state.db, &auth, owner_id).await? {
        return Err(ApiError::Forbidden.into());
    }

    let spam_status = match spam_status_param(&headers, &body, raw_query.as_deref()) {
        Some(value) if SPAM_MODERATIONS.contains(&value.as_str()) => value,
        _ => return Err(ApiError::BadRequest(spam_moderations_error()).into()),
    };

    // Canvas: `user.eportfolios.active.update_all(spam_status: ..., updated_at: Time.now.utc)`.
    sqlx::query(
        "UPDATE eportfolios SET spam_status = $1, updated_at = now() AT TIME ZONE 'UTC' \
         WHERE user_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(&spam_status)
    .bind(owner_id)
    .execute(&state.db)
    .await?;

    // Canvas `render json: :success`.
    Ok(Json(json!({ "success": true })).into_response())
}

/// `GET /api/v1/eportfolios/:id` — Canvas `EportfoliosApiController#show`.
async fn show(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth: AuthUser,
) -> Result<Response, PortfolioError> {
    verified_user_check(&state.db, &auth).await?;

    let portfolio = find_portfolio(&state.db, resolve_portfolio_id(&id)?).await?;
    if !can_read_portfolio(&state.db, &auth, &portfolio).await? {
        return Err(ApiError::Forbidden.into());
    }
    Ok(Json(portfolio_json(&portfolio)).into_response())
}

/// `DELETE /api/v1/eportfolios/:id` — Canvas `EportfoliosApiController#delete`.
async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth: AuthUser,
) -> Result<Response, PortfolioError> {
    verified_user_check(&state.db, &auth).await?;

    let portfolio = find_portfolio(&state.db, resolve_portfolio_id(&id)?).await?;
    if !can_delete_portfolio(&state.db, &auth, &portfolio).await? {
        return Err(ApiError::Forbidden.into());
    }

    // Canvas `Eportfolio#destroy`: the portfolio is soft deleted.
    let updated = sqlx::query(
        "UPDATE eportfolios SET workflow_state = 'deleted', \
                deleted_at = now() AT TIME ZONE 'UTC', \
                updated_at = now() AT TIME ZONE 'UTC' \
         WHERE id = $1",
    )
    .bind(portfolio.id)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(
            ApiError::BadRequest("There was an error destroying the ePortfolio".into()).into(),
        );
    }

    let portfolio = find_portfolio(&state.db, portfolio.id).await?;
    Ok(Json(portfolio_json(&portfolio)).into_response())
}

/// `GET /api/v1/eportfolios/:eportfolio_id/pages` — Canvas
/// `EportfoliosApiController#pages`.
async fn pages(
    State(state): State<AppState>,
    Path(eportfolio_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, PortfolioError> {
    verified_user_check(&state.db, &auth).await?;

    let portfolio = find_portfolio(&state.db, resolve_portfolio_id(&eportfolio_id)?).await?;
    if !can_read_portfolio(&state.db, &auth, &portfolio).await? {
        return Err(ApiError::Forbidden.into());
    }

    let per_page = per_page_for(raw_query.as_deref());
    let page = page_number(raw_query.as_deref());

    let rows = fetch_entries(&state.db, portfolio.id, per_page, page).await?;
    let total = count_entries(&state.db, portfolio.id).await?;
    let payload: Vec<Value> = rows.iter().map(entry_json).collect();

    let base = format!(
        "{}/api/v1/eportfolios/{}/pages",
        base_url(&headers),
        portfolio.id
    );
    let links = pagination_links(
        &base,
        &pages_query_string(per_page, page),
        page,
        per_page,
        total,
    );
    Ok(with_pagination_links(Json(payload).into_response(), links))
}

/// `PUT /api/v1/eportfolios/:eportfolio_id/moderate` — Canvas
/// `EportfoliosApiController#moderate`.
async fn moderate(
    State(state): State<AppState>,
    Path(eportfolio_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, PortfolioError> {
    verified_user_check(&state.db, &auth).await?;

    let portfolio = find_portfolio(&state.db, resolve_portfolio_id(&eportfolio_id)?).await?;
    if !can_moderate_portfolio(&state.db, &auth, &portfolio).await? {
        return Err(ApiError::Forbidden.into());
    }

    let spam_status = match spam_status_param(&headers, &body, raw_query.as_deref()) {
        Some(value) if SPAM_MODERATIONS.contains(&value.as_str()) => value,
        _ => return Err(ApiError::BadRequest(spam_moderations_error()).into()),
    };

    let updated = sqlx::query(
        "UPDATE eportfolios SET spam_status = $1, updated_at = now() AT TIME ZONE 'UTC' \
         WHERE id = $2",
    )
    .bind(&spam_status)
    .bind(portfolio.id)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(
            ApiError::BadRequest("There was an error updating the ePortfolio".into()).into(),
        );
    }

    let portfolio = find_portfolio(&state.db, portfolio.id).await?;
    Ok(Json(portfolio_json(&portfolio)).into_response())
}

/// `PUT /api/v1/eportfolios/:eportfolio_id/restore` — Canvas
/// `EportfoliosApiController#restore`.
async fn restore(
    State(state): State<AppState>,
    Path(eportfolio_id): Path<String>,
    auth: AuthUser,
) -> Result<Response, PortfolioError> {
    verified_user_check(&state.db, &auth).await?;

    let portfolio = find_portfolio(&state.db, resolve_portfolio_id(&eportfolio_id)?).await?;
    if !can_restore_portfolio(&state.db, &auth, &portfolio).await? {
        return Err(ApiError::Forbidden.into());
    }

    // Canvas `Eportfolio#restore`.
    let updated = sqlx::query(
        "UPDATE eportfolios SET workflow_state = 'active', deleted_at = NULL, \
                updated_at = now() AT TIME ZONE 'UTC' \
         WHERE id = $1",
    )
    .bind(portfolio.id)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(
            ApiError::BadRequest("There was an error restoring the ePortfolio".into()).into(),
        );
    }

    let portfolio = find_portfolio(&state.db, portfolio.id).await?;
    Ok(Json(portfolio_json(&portfolio)).into_response())
}
