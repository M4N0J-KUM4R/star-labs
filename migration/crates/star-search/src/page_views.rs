//! Canvas page views API — port of `PageViewsController`
//! (`canvas-src/app/controllers/page_views_controller.rb`).
//!
//! Routes served here (the coordinator mounts this router under `/api/v1`):
//!
//! | Method | Path                                                   | Canvas action      |
//! |--------|--------------------------------------------------------|--------------------|
//! | GET    | `/users/{user_id}/page_views`                         | `index`            |
//! | POST   | `/users/{user_id}/page_views/query`                   | `query`            |
//! | GET    | `/users/{user_id}/page_views/query/{query_id}`        | `poll_query`       |
//! | GET    | `/users/{user_id}/page_views/query/{query_id}/results`| `query_results`    |
//! | POST   | `/users/page_views/query`                             | `batch_query`      |
//! | GET    | `/users/page_views/query/{query_id}`                  | `poll_batch_query` |
//! | GET    | `/users/page_views/query/{query_id}/results`          | `batch_query_results` |
//!
//! Faithfulness notes:
//!
//! * `index` returns a bare JSON array of page view hashes, newest first, in
//!   the shape `Api::V1::PageView#page_view_json` builds: `id` is the
//!   `request_id`, the id columns move into `links`, `contributed` is always
//!   `false` and `app_name` is the developer key name.
//! * The authorization is Canvas' `authorized_action(@user, @current_user,
//!   :view_statistics)`: the user themself, a site admin, or an admin of an
//!   account the user belongs to. A failure is a 403.
//! * `start_time` in the future and `end_time` before `start_time` are the
//!   two 400s of `index`.
//! * The query routes store the query (`page_view_queries`) and the results
//!   route reads the stored result back. Canvas delegates this to the
//!   external PV5 service; star-lms has no such service, so the result is
//!   produced from the `page_views` table when the query is enqueued and the
//!   row is marked `finished` right away.

use std::collections::HashMap;

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `PageViewsController::PAGE_VIEWS_MAX_PER_PAGE`.
const PAGE_VIEWS_MAX_PER_PAGE: i64 = 200;

/// Canvas `PageViewsController::BATCH_QUERY_MAX_USER_IDS`.
const BATCH_QUERY_MAX_USER_IDS: usize = 10;

/// Canvas `PageViews::Common::FORMATS`.
const FORMATS: [&str; 2] = ["csv", "jsonl"];

/// Canvas `PageView::EXPORTED_COLUMNS` — the columns of the CSV export.
const EXPORTED_COLUMNS: [&str; 19] = [
    "request_id",
    "user_id",
    "url",
    "context_id",
    "context_type",
    "asset_id",
    "asset_type",
    "controller",
    "action",
    "interaction_seconds",
    "created_at",
    "user_request",
    "render_time",
    "user_agent",
    "participated",
    "account_id",
    "real_user_id",
    "http_method",
    "remote_ip",
];

/// Domain router for the Canvas page views routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users/{user_id}/page_views", get(index))
        .route("/users/{user_id}/page_views/query", post(query))
        .route(
            "/users/{user_id}/page_views/query/{query_id}",
            get(poll_query),
        )
        .route(
            "/users/{user_id}/page_views/query/{query_id}/results",
            get(query_results),
        )
        .route("/users/page_views/query", post(batch_query))
        .route("/users/page_views/query/{query_id}", get(poll_batch_query))
        .route(
            "/users/page_views/query/{query_id}/results",
            get(batch_query_results),
        )
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One `page_views` row plus the joined developer key name.
#[derive(Debug, sqlx::FromRow)]
struct PageViewRow {
    request_id: String,
    session_id: Option<String>,
    user_id: i64,
    url: Option<String>,
    context_id: Option<i64>,
    context_type: Option<String>,
    asset_id: Option<i64>,
    asset_type: Option<String>,
    controller: Option<String>,
    action: Option<String>,
    interaction_seconds: Option<f64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    developer_key_id: Option<i64>,
    developer_key_name: Option<String>,
    user_request: Option<bool>,
    render_time: Option<f64>,
    user_agent: Option<String>,
    asset_user_access_id: Option<i64>,
    participated: Option<bool>,
    summarized: Option<bool>,
    account_id: Option<i64>,
    real_user_id: Option<i64>,
    http_method: Option<String>,
    remote_ip: Option<String>,
}

/// One `page_view_queries` row.
#[derive(Debug, sqlx::FromRow)]
struct QueryRow {
    id: String,
    user_id: Option<i64>,
    requestor_id: i64,
    results_format: String,
    status: String,
    error_code: Option<String>,
    content: Option<Vec<u8>>,
    filename: Option<String>,
    content_type: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Canvas `api_find(User, params[:user_id])`: `self` is the current user, an
/// unknown id is a 404.
fn resolve_user_segment(raw: &str, self_id: i64) -> Result<i64, ApiError> {
    if raw == "self" {
        return Ok(self_id);
    }
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or(ApiError::NotFound)
}

/// 404 when the user does not exist (Canvas `api_find`).
async fn ensure_user(pool: &PgPool, user_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// star-lms models a Canvas site admin as `users.role = 'admin'`.
fn is_site_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// The accounts the caller administrates, including the sub-accounts the
/// account admin rights are inherited into.
async fn administered_account_ids(pool: &PgPool, user_id: i64) -> Result<Vec<i64>, ApiError> {
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE admin_accounts AS (
            SELECT au.account_id
            FROM account_users au
            WHERE au.user_id = $1
              AND au.role = 'admin'
              AND au.workflow_state <> 'deleted'
            UNION
            SELECT a.id
            FROM accounts a
            JOIN admin_accounts p ON a.parent_account_id = p.account_id
        )
        SELECT account_id FROM admin_accounts
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

/// The accounts the given user belongs to (account membership and the
/// accounts of the courses the user is enrolled in).
async fn user_account_ids(pool: &PgPool, user_id: i64) -> Result<Vec<i64>, ApiError> {
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE member_accounts AS (
            SELECT au.account_id FROM account_users au WHERE au.user_id = $1
            UNION
            SELECT c.account_id
            FROM enrollments e
            JOIN courses c ON c.id = e.course_id
            WHERE e.user_id = $1
        )
        SELECT DISTINCT account_id FROM member_accounts WHERE account_id IS NOT NULL
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

/// Canvas `authorized_action(@user, @current_user, :view_statistics)`.
///
/// star-lms has no `view_statistics` permission, so the right is modelled as
/// the user themself, a site admin, or an admin of an account the user belongs
/// to. A failure is a 403.
async fn authorize_view_statistics(
    pool: &PgPool,
    auth: &AuthUser,
    target_user_id: i64,
) -> Result<(), ApiError> {
    if target_user_id == auth.id || is_site_admin(auth) {
        return Ok(());
    }
    let admin_accounts = administered_account_ids(pool, auth.id).await?;
    if admin_accounts.is_empty() {
        return Err(ApiError::Forbidden);
    }
    let target_accounts = user_account_ids(pool, target_user_id).await?;
    if target_accounts.is_empty() {
        return Err(ApiError::Forbidden);
    }
    if admin_accounts
        .iter()
        .any(|account| target_accounts.contains(account))
    {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Canvas `validate_query_id!` — the query id has to be a lower case UUID.
fn is_uuid(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let widths = [8usize, 4, 4, 4, 12];
    parts.iter().zip(widths.iter()).all(|(part, width)| {
        part.len() == *width
            && part
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    })
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

/// Canvas `CanvasTime.try_parse` — nil when the value cannot be parsed.
fn parse_canvas_time(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?));
    }
    None
}

/// A Canvas style error body: `{"error": "..."}`.
fn canvas_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// The error type of the handlers.
///
/// Canvas answers the specific failures with a JSON body
/// (`{"error": "..."}`), the house `ApiError` answers everything else with
/// its plain text body.
enum PvError {
    Api(ApiError),
}

impl From<ApiError> for PvError {
    fn from(error: ApiError) -> Self {
        PvError::Api(error)
    }
}

impl From<sqlx::Error> for PvError {
    fn from(error: sqlx::Error) -> Self {
        PvError::Api(ApiError::Db(error.to_string()))
    }
}

impl IntoResponse for PvError {
    fn into_response(self) -> Response {
        match self {
            PvError::Api(ApiError::BadRequest(message)) => {
                canvas_error(StatusCode::BAD_REQUEST, &message)
            }
            PvError::Api(ApiError::Validation(message)) => {
                canvas_error(StatusCode::UNPROCESSABLE_ENTITY, &message)
            }
            PvError::Api(other) => other.into_response(),
        }
    }
}

/// The JSON body of one page view — Canvas
/// `Api::V1::PageView#page_view_json`.
fn page_view_json(row: &PageViewRow) -> Value {
    let app_name = match row.developer_key_id {
        Some(id) => match &row.developer_key_name {
            Some(name) => Value::String(name.clone()),
            None => Value::String(id.to_string()),
        },
        None => Value::Null,
    };
    json!({
        "id": row.request_id,
        "session_id": row.session_id,
        "url": row.url,
        "context_type": row.context_type,
        "asset_type": row.asset_type,
        "controller": row.controller,
        "action": row.action,
        "interaction_seconds": row.interaction_seconds,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "developer_key_id": row.developer_key_id,
        "user_request": row.user_request,
        "render_time": row.render_time,
        "user_agent": row.user_agent,
        "asset_user_access_id": row.asset_user_access_id,
        "participated": row.participated,
        "summarized": row.summarized,
        "http_method": row.http_method,
        "remote_ip": row.remote_ip,
        "contributed": false,
        "app_name": app_name,
        "links": {
            "user": row.user_id,
            "context": row.context_id,
            "asset": row.asset_id,
            "real_user": row.real_user_id,
            "account": row.account_id,
        }
    })
}

/// The CSV export row of one page view — Canvas `PageView#to_row`
/// (`EXPORTED_COLUMNS`, `nil` becomes an empty field).
fn page_view_csv_row(row: &PageViewRow) -> Vec<String> {
    vec![
        row.request_id.clone(),
        row.user_id.to_string(),
        row.url.clone().unwrap_or_default(),
        row.context_id.map(|v| v.to_string()).unwrap_or_default(),
        row.context_type.clone().unwrap_or_default(),
        row.asset_id.map(|v| v.to_string()).unwrap_or_default(),
        row.asset_type.clone().unwrap_or_default(),
        row.controller.clone().unwrap_or_default(),
        row.action.clone().unwrap_or_default(),
        row.interaction_seconds
            .map(|v| v.to_string())
            .unwrap_or_default(),
        row.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        row.user_request.map(|v| v.to_string()).unwrap_or_default(),
        row.render_time.map(|v| v.to_string()).unwrap_or_default(),
        row.user_agent.clone().unwrap_or_default(),
        row.participated.map(|v| v.to_string()).unwrap_or_default(),
        row.account_id.map(|v| v.to_string()).unwrap_or_default(),
        row.real_user_id.map(|v| v.to_string()).unwrap_or_default(),
        row.http_method.clone().unwrap_or_default(),
        row.remote_ip.clone().unwrap_or_default(),
    ]
}

/// RFC 4180 quoting, like Ruby's `Array#to_csv`.
fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// The page views of a user in a month range, as CSV or as JSON lines.
fn build_result_content(rows: &[PageViewRow], format: &str) -> Vec<u8> {
    match format {
        "jsonl" => {
            let mut out = String::new();
            for row in rows {
                out.push_str(&page_view_json(row).to_string());
                out.push('\n');
            }
            out.into_bytes()
        }
        _ => {
            let mut out = String::new();
            out.push_str(
                &EXPORTED_COLUMNS
                    .iter()
                    .map(|column| csv_escape(column))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push('\n');
            for row in rows {
                out.push_str(
                    &page_view_csv_row(row)
                        .iter()
                        .map(|field| csv_escape(field))
                        .collect::<Vec<_>>()
                        .join(","),
                );
                out.push('\n');
            }
            out.into_bytes()
        }
    }
}

/// The content type of a result file — Canvas
/// `PageViews::Common::CONTENT_TYPE_MAPPINGS`.
fn content_type_for(format: &str) -> &'static str {
    match format {
        "jsonl" => "application/jsonl",
        _ => "text/csv",
    }
}

/// The file extension of a result file.
fn extension_for(format: &str) -> &'static str {
    match format {
        "jsonl" => "jsonl",
        _ => "csv",
    }
}

/// The page views of a user inside a month range (the end month is included,
/// so the range covers whole months). `account_ids` limits the rows to the
/// accounts the viewer may see; `None` means no restriction.
async fn fetch_page_views_for_range(
    pool: &PgPool,
    user_id: i64,
    start_date: NaiveDate,
    end_date: NaiveDate,
    account_ids: Option<&[i64]>,
) -> Result<Vec<PageViewRow>, ApiError> {
    let rows: Vec<PageViewRow> = sqlx::query_as(
        r#"
        SELECT pv.request_id, pv.session_id, pv.user_id, pv.url, pv.context_id,
               pv.context_type, pv.asset_id, pv.asset_type, pv.controller, pv.action,
               pv.interaction_seconds, pv.created_at, pv.updated_at,
               pv.developer_key_id, dk.name AS developer_key_name, pv.user_request,
               pv.render_time, pv.user_agent, pv.asset_user_access_id, pv.participated,
               pv.summarized, pv.account_id, pv.real_user_id, pv.http_method, pv.remote_ip
        FROM page_views pv
        LEFT JOIN developer_keys dk ON dk.id = pv.developer_key_id
        WHERE pv.user_id = $1
          AND pv.created_at >= $2::timestamptz
          AND pv.created_at < ($3::timestamptz + INTERVAL '1 month')
          AND ($4::bigint[] IS NULL OR pv.account_id IS NULL OR pv.account_id = ANY($4))
        ORDER BY pv.created_at DESC
        "#,
    )
    .bind(user_id)
    .bind(Utc.from_utc_datetime(&start_date.and_hms_opt(0, 0, 0).expect("midnight")))
    .bind(Utc.from_utc_datetime(&end_date.and_hms_opt(0, 0, 0).expect("midnight")))
    .bind(account_ids.map(|ids| ids.to_vec()))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Store an enqueued query together with its result.
///
/// The query id is a uuid, the file name of the result is the query id plus
/// the extension, exactly like the Canvas service names its result files.
async fn insert_query(
    pool: &PgPool,
    query_id: &str,
    requestor_id: i64,
    user_id: Option<i64>,
    user_ids: Vec<i64>,
    start_date: NaiveDate,
    end_date: NaiveDate,
    format: &str,
    content: Vec<u8>,
    filename: String,
    content_type: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO page_view_queries (
            id, requestor_id, user_id, user_ids, start_date, end_date,
            results_format, status, content, filename, content_type, completed_at
        )
        VALUES ($1::uuid, $2, $3, $4, $5, $6, $7, 'finished', $8, $9, $10, now())
        "#,
    )
    .bind(query_id)
    .bind(requestor_id)
    .bind(user_id)
    .bind(user_ids)
    .bind(start_date)
    .bind(end_date)
    .bind(format)
    .bind(content)
    .bind(filename)
    .bind(content_type)
    .execute(pool)
    .await?;
    Ok(())
}

/// Load one query by its id.
async fn find_query(pool: &PgPool, query_id: &str) -> Result<Option<QueryRow>, ApiError> {
    let row: Option<QueryRow> = sqlx::query_as(
        r#"
        SELECT id::text AS id, user_id, requestor_id, results_format, status, error_code,
               content, filename, content_type
        FROM page_view_queries
        WHERE id = $1::uuid
        "#,
    )
    .bind(query_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// The result file of a finished query.
fn result_response(row: &QueryRow) -> Result<Response, PvError> {
    if row.status != "finished" {
        // Canvas `PageViews::Common::InvalidResultError`.
        return Err(ApiError::BadRequest(
            "Query results are not in a valid state for download".to_string(),
        )
        .into());
    }
    let content = row.content.clone().unwrap_or_default();
    if content.is_empty() {
        // Canvas `PageViews::Common::NoContentError` -> `head :no_content`.
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let mut response = Response::new(Body::from(content));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(
            row.content_type
                .as_deref()
                .unwrap_or_else(|| content_type_for(&row.results_format)),
        )
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    let filename = row
        .filename
        .clone()
        .unwrap_or_else(|| format!("{}.{}", row.id, extension_for(&row.results_format)));
    let disposition = format!("attachment; filename=\"{filename}\"");
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}

/// The status body of a poll — Canvas `poll_query` / `poll_batch_query`.
fn poll_json(query_id: &str, row: &QueryRow, results_url: Option<String>) -> Value {
    json!({
        "query_id": query_id,
        "status": row.status,
        "format": row.results_format,
        "results_url": results_url,
        "error_code": row.error_code,
    })
}

// ---------------------------------------------------------------------------
// GET /users/{user_id}/page_views
// ---------------------------------------------------------------------------

/// Canvas `PageViewsController#index`.
async fn index(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Response, PvError> {
    let target_user_id = resolve_user_segment(&user_id, auth.id)?;
    ensure_user(&state.db, target_user_id).await?;
    authorize_view_statistics(&state.db, &auth, target_user_id).await?;

    // Canvas parses the times with `CanvasTime.try_parse`; an unparsable value
    // is ignored (the range stays open on that side).
    let start_time = params
        .get("start_time")
        .and_then(|value| parse_canvas_time(value));
    let end_time = params
        .get("end_time")
        .and_then(|value| parse_canvas_time(value));

    if let Some(start) = start_time {
        if start > Utc::now() {
            return Err(
                ApiError::BadRequest("start_time cannot be in the future".to_string()).into(),
            );
        }
    }
    if let (Some(start), Some(end)) = (start_time, end_time) {
        if end < start {
            return Err(
                ApiError::BadRequest("end_time must be after start_time".to_string()).into(),
            );
        }
    }

    // Canvas: `per_page = (params[:per_page] || 10).to_i.clamp(1, 200)`.
    let per_page = ruby_to_i(params.get("per_page").map(|s| s.as_str()).unwrap_or("10"))
        .clamp(1, PAGE_VIEWS_MAX_PER_PAGE);
    let page = ruby_to_i(params.get("page").map(|s| s.as_str()).unwrap_or("1")).max(1);
    let offset = (page - 1) * per_page;

    // Canvas filters the rows down to the accounts the viewer may see
    // (`PageView.for_user` / `AccountFilter.filter`); a site admin and the
    // user themself see everything.
    let account_ids = if target_user_id == auth.id || is_site_admin(&auth) {
        None
    } else {
        Some(administered_account_ids(&state.db, auth.id).await?)
    };

    let rows: Vec<PageViewRow> = sqlx::query_as(
        r#"
        SELECT pv.request_id, pv.session_id, pv.user_id, pv.url, pv.context_id,
               pv.context_type, pv.asset_id, pv.asset_type, pv.controller, pv.action,
               pv.interaction_seconds, pv.created_at, pv.updated_at,
               pv.developer_key_id, dk.name AS developer_key_name, pv.user_request,
               pv.render_time, pv.user_agent, pv.asset_user_access_id, pv.participated,
               pv.summarized, pv.account_id, pv.real_user_id, pv.http_method, pv.remote_ip
        FROM page_views pv
        LEFT JOIN developer_keys dk ON dk.id = pv.developer_key_id
        WHERE pv.user_id = $1
          AND ($2::timestamptz IS NULL OR pv.created_at >= $2)
          AND ($3::timestamptz IS NULL OR pv.created_at <= $3)
          AND ($4::bigint[] IS NULL OR pv.account_id IS NULL OR pv.account_id = ANY($4))
        ORDER BY pv.created_at DESC
        LIMIT $5 OFFSET $6
        "#,
    )
    .bind(target_user_id)
    .bind(start_time)
    .bind(end_time)
    .bind(account_ids)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let views: Vec<Value> = rows.iter().map(page_view_json).collect();
    Ok(Json(Value::Array(views)).into_response())
}

// ---------------------------------------------------------------------------
// POST /users/{user_id}/page_views/query
// ---------------------------------------------------------------------------

/// Canvas `PageViewsController#query`.
async fn query(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, PvError> {
    let body = parse_body(&body)?;

    // Canvas: `params.require(%i[user_id start_date end_date results_format])`.
    let start_date = require_param(&body, "start_date")?;
    let end_date = require_param(&body, "end_date")?;
    let results_format = require_param(&body, "results_format")?;

    let target_user_id = resolve_user_segment(&user_id, auth.id)?;
    ensure_user(&state.db, target_user_id).await?;
    authorize_view_statistics(&state.db, &auth, target_user_id).await?;

    // Canvas: the enqueue service validates the request and raises an
    // `ArgumentError`, which the controller answers with a generic 400.
    let (start, end) =
        validate_query_request(&start_date, &end_date, &results_format).map_err(|_| {
            ApiError::BadRequest("Page Views received an invalid or malformed request.".to_string())
        })?;

    let account_ids = if target_user_id == auth.id || is_site_admin(&auth) {
        None
    } else {
        Some(administered_account_ids(&state.db, auth.id).await?)
    };
    let rows = fetch_page_views_for_range(
        &state.db,
        target_user_id,
        start,
        end,
        account_ids.as_deref(),
    )
    .await?;
    let content = build_result_content(&rows, &results_format);

    let query_id = uuid::Uuid::new_v4().to_string();
    insert_query(
        &state.db,
        &query_id,
        auth.id,
        Some(target_user_id),
        Vec::new(),
        start,
        end,
        &results_format,
        content,
        format!("{}.{}", query_id, extension_for(&results_format)),
        content_type_for(&results_format),
    )
    .await?;

    let poll_url = format!("/api/v1/users/{target_user_id}/page_views/query/{query_id}");
    Ok((StatusCode::CREATED, Json(json!({ "poll_url": poll_url }))).into_response())
}

// ---------------------------------------------------------------------------
// GET /users/{user_id}/page_views/query/{query_id}
// ---------------------------------------------------------------------------

/// Canvas `PageViewsController#poll_query`.
async fn poll_query(
    State(state): State<AppState>,
    Path((user_id, query_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Response, PvError> {
    if !is_uuid(&query_id) {
        return Ok(canvas_error(StatusCode::BAD_REQUEST, "Invalid query ID"));
    }
    let target_user_id = resolve_user_segment(&user_id, auth.id)?;
    ensure_user(&state.db, target_user_id).await?;
    authorize_view_statistics(&state.db, &auth, target_user_id).await?;

    let row = match find_query(&state.db, &query_id).await? {
        Some(row) if row.user_id == Some(target_user_id) => row,
        _ => {
            return Ok(canvas_error(
                StatusCode::NOT_FOUND,
                "The query was not found.",
            ))
        }
    };

    let results_url = if row.status == "finished" {
        Some(format!(
            "/api/v1/users/{target_user_id}/page_views/query/{query_id}/results"
        ))
    } else {
        None
    };
    Ok(Json(poll_json(&query_id, &row, results_url)).into_response())
}

// ---------------------------------------------------------------------------
// GET /users/{user_id}/page_views/query/{query_id}/results
// ---------------------------------------------------------------------------

/// Canvas `PageViewsController#query_results`.
async fn query_results(
    State(state): State<AppState>,
    Path((user_id, query_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Response, PvError> {
    if !is_uuid(&query_id) {
        return Ok(canvas_error(StatusCode::BAD_REQUEST, "Invalid query ID"));
    }
    let target_user_id = resolve_user_segment(&user_id, auth.id)?;
    ensure_user(&state.db, target_user_id).await?;
    authorize_view_statistics(&state.db, &auth, target_user_id).await?;

    let row = match find_query(&state.db, &query_id).await? {
        Some(row) if row.user_id == Some(target_user_id) => row,
        _ => {
            return Ok(canvas_error(
                StatusCode::NOT_FOUND,
                "The query was not found.",
            ))
        }
    };
    Ok(result_response(&row)?)
}

// ---------------------------------------------------------------------------
// POST /users/page_views/query
// ---------------------------------------------------------------------------

/// Canvas `PageViewsController#batch_query`.
async fn batch_query(
    State(state): State<AppState>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, PvError> {
    let body = parse_body(&body)?;

    // Canvas: `params.require(%i[user_ids start_date end_date results_format])`.
    let user_ids = require_param_list(&body, "user_ids")?;
    let start_date = require_param(&body, "start_date")?;
    let end_date = require_param(&body, "end_date")?;
    let results_format = require_param(&body, "results_format")?;

    if user_ids.len() > BATCH_QUERY_MAX_USER_IDS {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "Too many user IDs. Maximum: {BATCH_QUERY_MAX_USER_IDS}, provided: {}",
                user_ids.len()
            ),
        ));
    }
    for (index, id) in user_ids.iter().enumerate() {
        if user_ids[..index].contains(id) {
            return Ok(canvas_error(
                StatusCode::BAD_REQUEST,
                &format!("Duplicate user ID found: {id}"),
            ));
        }
    }

    // Canvas authorizes every user of the batch before the query is enqueued.
    for user_id in &user_ids {
        ensure_user(&state.db, *user_id).await?;
        authorize_view_statistics(&state.db, &auth, *user_id).await?;
    }

    // Canvas: the enqueue service raises an `ArgumentError`, which the batch
    // route answers with the message of the error.
    let (start, end) = validate_batch_request(&start_date, &end_date, &results_format, &user_ids)
        .map_err(ApiError::BadRequest)?;

    let mut rows: Vec<PageViewRow> = Vec::new();
    for user_id in &user_ids {
        let account_ids = if *user_id == auth.id || is_site_admin(&auth) {
            None
        } else {
            Some(administered_account_ids(&state.db, auth.id).await?)
        };
        let mut user_rows =
            fetch_page_views_for_range(&state.db, *user_id, start, end, account_ids.as_deref())
                .await?;
        rows.append(&mut user_rows);
    }

    let content = build_result_content(&rows, &results_format);
    let query_id = uuid::Uuid::new_v4().to_string();
    insert_query(
        &state.db,
        &query_id,
        auth.id,
        None,
        user_ids,
        start,
        end,
        &results_format,
        content,
        format!("{}.{}", query_id, extension_for(&results_format)),
        content_type_for(&results_format),
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "poll_url": format!("/api/v1/users/page_views/query/{query_id}") })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// GET /users/page_views/query/{query_id}
// ---------------------------------------------------------------------------

/// Canvas `PageViewsController#poll_batch_query`.
async fn poll_batch_query(
    State(state): State<AppState>,
    Path(query_id): Path<String>,
    auth: AuthUser,
) -> Result<Response, PvError> {
    if !is_uuid(&query_id) {
        return Ok(canvas_error(StatusCode::BAD_REQUEST, "Invalid query ID"));
    }
    let row = match find_query(&state.db, &query_id).await? {
        Some(row) => row,
        None => {
            return Ok(canvas_error(
                StatusCode::NOT_FOUND,
                "The query was not found.",
            ))
        }
    };
    // The batch query has no user in the path, so only the requestor of the
    // query (or a site admin) may read it.
    if row.requestor_id != auth.id && !is_site_admin(&auth) {
        return Err(ApiError::Forbidden.into());
    }

    let results_url = if row.status == "finished" {
        Some(format!("/api/v1/users/page_views/query/{query_id}/results"))
    } else {
        None
    };
    Ok(Json(poll_json(&query_id, &row, results_url)).into_response())
}

// ---------------------------------------------------------------------------
// GET /users/page_views/query/{query_id}/results
// ---------------------------------------------------------------------------

/// Canvas `PageViewsController#batch_query_results`.
async fn batch_query_results(
    State(state): State<AppState>,
    Path(query_id): Path<String>,
    auth: AuthUser,
) -> Result<Response, PvError> {
    if !is_uuid(&query_id) {
        return Ok(canvas_error(StatusCode::BAD_REQUEST, "Invalid query ID"));
    }
    let row = match find_query(&state.db, &query_id).await? {
        Some(row) => row,
        None => {
            return Ok(canvas_error(
                StatusCode::NOT_FOUND,
                "The query was not found.",
            ))
        }
    };
    if row.requestor_id != auth.id && !is_site_admin(&auth) {
        return Err(ApiError::Forbidden.into());
    }
    Ok(result_response(&row)?)
}

// ---------------------------------------------------------------------------
// Body helpers
// ---------------------------------------------------------------------------

/// The JSON body of a request. An empty body is an empty object, so the
/// missing parameter is reported by the `require_param` helpers.
fn parse_body(body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|_| {
        ApiError::BadRequest("Page Views received an invalid or malformed request.".to_string())
    })
}

/// Canvas `params.require(:name)` — a missing parameter is a 400.
fn require_param(body: &Value, name: &str) -> Result<String, ApiError> {
    match body.get(name).and_then(|value| value.as_str()) {
        Some(value) => Ok(value.to_string()),
        None => Err(ApiError::BadRequest(format!(
            "Parameter {name} is missing."
        ))),
    }
}

/// Canvas `params.require(:user_ids)` — the ids may be numbers or strings.
fn require_param_list(body: &Value, name: &str) -> Result<Vec<i64>, ApiError> {
    let missing = || ApiError::BadRequest(format!("Parameter {name} is missing."));
    let items = body
        .get(name)
        .and_then(|value| value.as_array())
        .ok_or_else(missing)?;
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let id = match item {
            Value::Number(number) => number.as_i64(),
            Value::String(text) => text.trim().parse::<i64>().ok(),
            _ => None,
        };
        ids.push(id.ok_or_else(missing)?);
    }
    Ok(ids)
}

/// The month range of a query request.
///
/// Canvas `PageViews::EnqueueQueryService#parse_date_only`: the dates are
/// `YYYY-MM-DD` and have to be real calendar dates.
fn parse_month_range(start_date: &str, end_date: &str) -> Result<(NaiveDate, NaiveDate), String> {
    let start = parse_date_only(start_date)?;
    let end = parse_date_only(end_date)?;
    Ok((start, end))
}

/// Canvas `PageViews::Common::AsyncQueryRequest#validate!`.
///
/// The order of the checks is the order of the Ruby struct: the dates, the
/// month interval and last the format.
fn validate_query_request(
    start_date: &str,
    end_date: &str,
    format: &str,
) -> Result<(NaiveDate, NaiveDate), String> {
    let (start, end) = parse_month_range(start_date, end_date)?;
    if start > end {
        return Err("Start date must be before or equal to end date".to_string());
    }
    if start.day() != 1 || end.day() != 1 {
        return Err("Only full-month intervals are supported".to_string());
    }
    if !FORMATS.contains(&format) {
        return Err(format!("Format must be one of {}", FORMATS.join(", ")));
    }
    Ok((start, end))
}

/// Canvas `PageViews::Common::AsyncBatchQueryRequest#validate!` — the dates
/// are parsed first (the service does that before the request is built), then
/// the format is checked, then the interval and the user ids.
fn validate_batch_request(
    start_date: &str,
    end_date: &str,
    format: &str,
    user_ids: &[i64],
) -> Result<(NaiveDate, NaiveDate), String> {
    let (start, end) = parse_month_range(start_date, end_date)?;
    if !FORMATS.contains(&format) {
        return Err(format!("Format must be one of {}", FORMATS.join(", ")));
    }
    if start > end {
        return Err("Start date must be before or equal to end date".to_string());
    }
    if start.day() != 1 || end.day() != 1 {
        return Err("Only full-month intervals are supported".to_string());
    }
    if user_ids.is_empty() {
        return Err("User IDs cannot be empty".to_string());
    }
    if user_ids.iter().any(|id| *id <= 0) {
        return Err("All user IDs must be positive numbers".to_string());
    }
    Ok((start, end))
}

/// Canvas `parse_date_only`: `YYYY-MM-DD` only, and a real calendar date.
fn parse_date_only(value: &str) -> Result<NaiveDate, String> {
    let bytes = value.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(|b| b.is_ascii_digit())
        && bytes[5..7].iter().all(|b| b.is_ascii_digit())
        && bytes[8..].iter().all(|b| b.is_ascii_digit());
    if !shaped {
        return Err("Date must be in YYYY-MM-DD format".to_string());
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| "invalid date".to_string())
}
