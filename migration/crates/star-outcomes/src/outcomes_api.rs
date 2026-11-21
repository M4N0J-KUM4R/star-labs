//! Canvas Outcome Imports + Outcome Results API.
//!
//! Rust port of `app/controllers/outcome_imports_api_controller.rb` and
//! `app/controllers/outcome_results_controller.rb`.
//!
//! | Verb | Path | Ruby action |
//! |---|---|---|
//! | POST | `/accounts/{account_id}/outcome_imports` | `OutcomeImportsApiController#create` |
//! | POST | `/accounts/{account_id}/outcome_imports/group/{learning_outcome_group_id}` | `OutcomeImportsApiController#create` |
//! | POST | `/courses/{course_id}/outcome_imports` | `OutcomeImportsApiController#create` |
//! | POST | `/courses/{course_id}/outcome_imports/group/{learning_outcome_group_id}` | `OutcomeImportsApiController#create` |
//! | GET | `/accounts/{account_id}/outcome_imports/{id}` | `OutcomeImportsApiController#show` |
//! | GET | `/courses/{course_id}/outcome_imports/{id}` | `OutcomeImportsApiController#show` |
//! | GET | `/accounts/{account_id}/outcome_imports/{id}/created_group_ids` | `OutcomeImportsApiController#created_group_ids` |
//! | GET | `/courses/{course_id}/outcome_imports/{id}/created_group_ids` | `OutcomeImportsApiController#created_group_ids` |
//! | GET | `/courses/{course_id}/outcome_rollups` | `OutcomeResultsController#rollups` |
//! | GET | `/courses/{course_id}/outcome_results` | `OutcomeResultsController#index` |
//! | GET | `/courses/{course_id}/outcome_mastery_distribution` | `OutcomeResultsController#mastery_distribution` |
//! | GET | `/courses/{course_id}/outcomes/{outcome_id}/contributing_scores` | `OutcomeResultsController#contributing_scores` |
//! | POST | `/courses/{course_id}/assign_outcome_order` | `OutcomeResultsController#outcome_order` |
//! | POST | `/enqueue_outcome_rollup_calculation` | `OutcomeResultsController#enqueue_outcome_rollup_calculation` |
//!
//! # Required DDL
//!
//! The coordinator must add the following to a `star-db` migration. The tables
//! below already exist (`outcome_imports`, `outcome_groups`, `outcome_links`,
//! `outcome_alignments`, `learning_outcomes`, `learning_outcome_results`,
//! `attachments`, `jobs`, `users`, `pseudonyms`); only the listed columns and
//! the `outcome_import_errors` table are missing.
//!
//! ```sql
//! -- Canvas stores the rubric criterion / calculation settings on the outcome.
//! ALTER TABLE learning_outcomes
//!     ADD COLUMN IF NOT EXISTS data JSONB,
//!     ADD COLUMN IF NOT EXISTS calculation_method TEXT,
//!     ADD COLUMN IF NOT EXISTS calculation_int INTEGER;
//!
//! -- Canvas `LearningOutcomeResult` carries the context, the alignment link,
//! -- the rubric metadata and the hidden flag.
//! ALTER TABLE learning_outcome_results
//!     ALTER COLUMN score DROP NOT NULL,
//!     ADD COLUMN IF NOT EXISTS context_code TEXT,
//!     ADD COLUMN IF NOT EXISTS content_tag_id BIGINT,
//!     ADD COLUMN IF NOT EXISTS association_type TEXT,
//!     ADD COLUMN IF NOT EXISTS association_id BIGINT,
//!     ADD COLUMN IF NOT EXISTS hidden BOOLEAN NOT NULL DEFAULT FALSE,
//!     ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'active',
//!     ADD COLUMN IF NOT EXISTS possible DOUBLE PRECISION,
//!     ADD COLUMN IF NOT EXISTS percent DOUBLE PRECISION,
//!     ADD COLUMN IF NOT EXISTS mastery BOOLEAN,
//!     ADD COLUMN IF NOT EXISTS hide_points BOOLEAN NOT NULL DEFAULT FALSE,
//!     ADD COLUMN IF NOT EXISTS title TEXT,
//!     ADD COLUMN IF NOT EXISTS submitted_at TIMESTAMP,
//!     ADD COLUMN IF NOT EXISTS assessed_at TIMESTAMP;
//!
//! -- Which import created the group (`created_group_ids`).
//! ALTER TABLE outcome_groups
//!     ADD COLUMN IF NOT EXISTS outcome_import_id BIGINT;
//!
//! -- `GET .../outcome_imports/latest`.
//! ALTER TABLE courses
//!     ADD COLUMN IF NOT EXISTS latest_outcome_import_id BIGINT;
//! ALTER TABLE accounts
//!     ADD COLUMN IF NOT EXISTS latest_outcome_import_id BIGINT;
//!
//! -- `POST /enqueue_outcome_rollup_calculation` addresses a student by uuid.
//! ALTER TABLE users
//!     ADD COLUMN IF NOT EXISTS uuid TEXT;
//!
//! -- Canvas `OutcomeImportError`.
//! CREATE TABLE IF NOT EXISTS outcome_import_errors (
//!     id                BIGSERIAL PRIMARY KEY,
//!     outcome_import_id BIGINT NOT NULL REFERENCES outcome_imports(id) ON DELETE CASCADE,
//!     row               INTEGER,
//!     message           TEXT NOT NULL,
//!     failure           BOOLEAN NOT NULL DEFAULT FALSE,
//!     created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
//!     updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
//! );
//!
//! CREATE INDEX IF NOT EXISTS idx_learning_outcome_results_context
//!     ON learning_outcome_results (context_code, user_id);
//! CREATE INDEX IF NOT EXISTS idx_outcome_groups_import
//!     ON outcome_groups (outcome_import_id);
//! CREATE INDEX IF NOT EXISTS idx_outcome_imports_context
//!     ON outcome_imports (context_type, context_id);
//! ```

use std::collections::HashMap;

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{NaiveDateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Routes for the Canvas outcome import and outcome result APIs.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{account_id}/outcome_imports",
            post(create_account_outcome_import),
        )
        .route(
            "/accounts/{account_id}/outcome_imports/group/{learning_outcome_group_id}",
            post(create_account_outcome_import_in_group),
        )
        .route(
            "/accounts/{account_id}/outcome_imports/{id}",
            get(show_account_outcome_import),
        )
        .route(
            "/accounts/{account_id}/outcome_imports/{id}/created_group_ids",
            get(created_group_ids_for_account),
        )
        .route(
            "/courses/{course_id}/outcome_imports",
            post(create_course_outcome_import),
        )
        .route(
            "/courses/{course_id}/outcome_imports/group/{learning_outcome_group_id}",
            post(create_course_outcome_import_in_group),
        )
        .route(
            "/courses/{course_id}/outcome_imports/{id}",
            get(show_course_outcome_import),
        )
        .route(
            "/courses/{course_id}/outcome_imports/{id}/created_group_ids",
            get(created_group_ids_for_course),
        )
        .route("/courses/{course_id}/outcome_rollups", get(outcome_rollups))
        .route(
            "/courses/{course_id}/outcome_results",
            get(outcome_results_index),
        )
        .route(
            "/courses/{course_id}/outcome_mastery_distribution",
            get(outcome_mastery_distribution),
        )
        .route(
            "/courses/{course_id}/outcomes/{outcome_id}/contributing_scores",
            get(outcome_contributing_scores),
        )
        .route(
            "/courses/{course_id}/assign_outcome_order",
            post(assign_outcome_order),
        )
        .route(
            "/enqueue_outcome_rollup_calculation",
            post(enqueue_outcome_rollup_calculation),
        )
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Module local error type.
///
/// `ApiError` covers the generic failures; the Canvas specific bodies
/// (`reject!`, `render_json_unauthorized`, `RecordNotFound`) are modelled
/// explicitly so the JSON shape matches the Ruby controller byte for byte.
#[derive(Debug)]
enum OutcomeApiError {
    /// Generic `star_common::ApiError`.
    Api(ApiError),
    /// Canvas `reject!` — renders `RequestError#error_json`.
    Reject { status: StatusCode, message: String },
    /// Canvas `render_json_unauthorized` — 401 for anonymous, 403 otherwise.
    Unauthorized { authenticated: bool },
    /// Canvas `ActiveRecord::RecordNotFound` — `api_error_json`.
    NotFound,
    /// Canvas `InvalidContentType` (`OutcomeImportsApiController`).
    InvalidContentType,
    /// An uncaught Ruby `raise` inside the controller.
    Raised(String),
}

impl From<ApiError> for OutcomeApiError {
    fn from(err: ApiError) -> Self {
        OutcomeApiError::Api(err)
    }
}

impl From<sqlx::Error> for OutcomeApiError {
    fn from(err: sqlx::Error) -> Self {
        OutcomeApiError::Api(ApiError::from(err))
    }
}

/// `Rack::Utils::SYMBOL_TO_STATUS_CODE` key for a status code.
fn status_symbol(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "bad_request",
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::UNPROCESSABLE_ENTITY => "unprocessable_entity",
        StatusCode::INTERNAL_SERVER_ERROR => "internal_server_error",
        _ => "error",
    }
}

impl IntoResponse for OutcomeApiError {
    fn into_response(self) -> Response {
        match self {
            OutcomeApiError::Api(err) => err.into_response(),
            OutcomeApiError::Reject { status, message } => (
                status,
                Json(json!({ "status": status_symbol(status), "message": message })),
            )
                .into_response(),
            OutcomeApiError::Unauthorized {
                authenticated: true,
            } => (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "status": "unauthorized",
                    "errors": [{ "message": "user not authorized to perform that action" }]
                })),
            )
                .into_response(),
            OutcomeApiError::Unauthorized {
                authenticated: false,
            } => (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "status": "unauthenticated",
                    "errors": [{ "message": "user authorization required" }]
                })),
            )
                .into_response(),
            OutcomeApiError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "errors": [{ "message": "The specified resource does not exist." }]
                })),
            )
                .into_response(),
            OutcomeApiError::InvalidContentType => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid content type, UTF-8 required" })),
            )
                .into_response(),
            OutcomeApiError::Raised(message) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "errors": [{ "message": message }] })),
            )
                .into_response(),
        }
    }
}

/// Canvas `reject!(message)` — 400.
fn reject(message: impl Into<String>) -> OutcomeApiError {
    OutcomeApiError::Reject {
        status: StatusCode::BAD_REQUEST,
        message: message.into(),
    }
}

/// Canvas `reject!(message, :forbidden)` — 403.
fn reject_forbidden(message: impl Into<String>) -> OutcomeApiError {
    OutcomeApiError::Reject {
        status: StatusCode::FORBIDDEN,
        message: message.into(),
    }
}

/// Canvas `render json: { message: ... }, status: :not_found`.
fn not_found_with_message(message: impl Into<String>) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "message": message.into() })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Request parameters
// ---------------------------------------------------------------------------

/// Rails style request parameters (query string plus multipart form fields).
///
/// Rails merges the request body over the query string and the path
/// parameters over both, so `merge_multipart` overwrites the query values.
#[derive(Debug, Default, Clone)]
struct Params {
    values: HashMap<String, Vec<String>>,
}

impl Params {
    /// Parse a raw query string. `key[]=a&key[]=b` becomes an array, a bare
    /// `key=a` becomes a scalar (last one wins).
    fn from_query(raw: Option<&str>) -> Self {
        let mut values: HashMap<String, Vec<String>> = HashMap::new();
        let Some(raw) = raw else {
            return Self { values };
        };
        for pair in raw.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (key, value),
                None => (pair, ""),
            };
            let key = percent_decode(key);
            let value = percent_decode(value);
            if let Some(base) = key.strip_suffix("[]") {
                let base = base.to_string();
                values.entry(base).or_default().push(value);
            } else {
                values.insert(key, vec![value]);
            }
        }
        Self { values }
    }

    /// Merge multipart form fields over the query string values.
    fn merge_multipart(&mut self, fields: &[MultipartField]) {
        for field in fields {
            if field.filename.is_some() {
                // File uploads are not scalar parameters in Canvas.
                continue;
            }
            let text = String::from_utf8_lossy(&field.data).to_string();
            self.values.insert(field.name.clone(), vec![text]);
        }
    }

    /// Rails `params[:key]`.
    fn get(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .and_then(|v| v.last())
            .map(String::as_str)
    }

    /// Rails `params[:key].present?`.
    fn present(&self, key: &str) -> bool {
        self.get(key).is_some_and(|v| !v.is_empty())
    }

    /// Rails `Api.value_to_array(params[:key])`.
    fn array(&self, key: &str) -> Vec<String> {
        self.values.get(key).cloned().unwrap_or_default()
    }
}

/// Percent decode a query string token (`+` means space).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

// ---------------------------------------------------------------------------
// Multipart
// ---------------------------------------------------------------------------

/// One `multipart/form-data` part.
#[derive(Debug)]
struct MultipartField {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    data: Vec<u8>,
}

/// Extract the boundary from a `multipart/form-data` content type.
fn multipart_boundary(content_type: &str) -> Option<String> {
    if !content_type
        .to_ascii_lowercase()
        .starts_with("multipart/form-data")
    {
        return None;
    }
    content_type.split(';').skip(1).find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        if key.trim().eq_ignore_ascii_case("boundary") {
            Some(value.trim().trim_matches('"').to_string())
        } else {
            None
        }
    })
}

/// Parse a `multipart/form-data` body into its parts.
fn parse_multipart(body: &[u8], boundary: &str) -> Vec<MultipartField> {
    let delimiter = format!("--{boundary}");
    let delimiter = delimiter.as_bytes();
    let mut fields = Vec::new();
    let mut cursor = match find_bytes(body, delimiter, 0) {
        Some(pos) => pos + delimiter.len(),
        None => return fields,
    };
    loop {
        if body[cursor..].starts_with(b"--") {
            break;
        }
        if body[cursor..].starts_with(b"\r\n") {
            cursor += 2;
        } else if body[cursor..].starts_with(b"\n") {
            cursor += 1;
        }
        let Some(next) = find_bytes(body, delimiter, cursor) else {
            break;
        };
        let mut part = &body[cursor..next];
        if part.ends_with(b"\r\n") {
            part = &part[..part.len() - 2];
        } else if part.ends_with(b"\n") {
            part = &part[..part.len() - 1];
        }
        if let Some(field) = parse_multipart_part(part) {
            fields.push(field);
        }
        cursor = next + delimiter.len();
    }
    fields
}

/// Split one multipart part into its headers and body.
fn parse_multipart_part(part: &[u8]) -> Option<MultipartField> {
    let separator = find_bytes(part, b"\r\n\r\n", 0)
        .map(|pos| (pos, 4))
        .or_else(|| find_bytes(part, b"\n\n", 0).map(|pos| (pos, 2)))?;
    let headers = String::from_utf8_lossy(&part[..separator.0]).to_string();
    let data = part[separator.0 + separator.1..].to_vec();

    let mut name = None;
    let mut filename = None;
    let mut content_type = None;
    for line in headers.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-disposition:") {
            name = header_param(line, "name");
            filename = header_param(line, "filename");
        } else if lower.starts_with("content-type:") {
            content_type = line.split_once(':').map(|(_, v)| v.trim().to_string());
        }
    }

    Some(MultipartField {
        name: name?,
        filename,
        content_type,
        data,
    })
}

/// Read `key="value"` out of a header line.
fn header_param(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

/// Find `needle` in `haystack` at or after `from`.
fn find_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|pos| pos + from)
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Canvas `Account.site_admin` — the root account.
async fn site_admin_account_id(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' ORDER BY id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
}

/// Is the user an admin of the account or of one of its ancestors?
async fn is_account_admin(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        WITH RECURSIVE chain AS (
            SELECT id, parent_account_id FROM accounts WHERE id = $2
            UNION ALL
            SELECT a.id, a.parent_account_id
            FROM accounts a JOIN chain c ON a.id = c.parent_account_id
        )
        SELECT EXISTS (
            SELECT 1 FROM account_users au
            JOIN chain c ON c.id = au.account_id
            WHERE au.user_id = $1 AND au.role = 'admin'
        )
        "#,
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_one(pool)
    .await
}

/// Is the user an active teacher of the course?
async fn is_course_teacher(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM enrollments
            WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' AND workflow_state = 'active'
        )",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(pool)
    .await
}

/// The account a course belongs to.
async fn course_account_id(pool: &PgPool, course_id: i64) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT account_id FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(pool)
        .await
}

/// Canvas `grants_any_right?(:manage_grades, :view_all_grades)` on a course.
async fn can_manage_grades(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, sqlx::Error> {
    if auth.role == "admin" {
        return Ok(true);
    }
    if is_course_teacher(pool, auth.id, course_id).await? {
        return Ok(true);
    }
    if let Some(account_id) = course_account_id(pool, course_id).await? {
        if is_account_admin(pool, auth.id, account_id).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Canvas `grants_any_right?(:import_outcomes, :manage_outcomes)` on an account.
async fn can_manage_outcomes_on_account(
    pool: &PgPool,
    auth: &AuthUser,
    account_id: i64,
) -> Result<bool, sqlx::Error> {
    if auth.role == "admin" {
        return Ok(true);
    }
    is_account_admin(pool, auth.id, account_id).await
}

/// Canvas `grants_any_right?(:import_outcomes, :manage_outcomes)` on a course.
async fn can_manage_outcomes_on_course(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, sqlx::Error> {
    can_manage_grades(pool, auth, course_id).await
}

/// Is the `feature_flags` row for the account enabled?
async fn account_feature_enabled(
    pool: &PgPool,
    account_id: i64,
    feature: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM feature_flags
            WHERE context_type = 'Account' AND context_id = $1 AND feature = $2 AND state = 'enabled'
        )",
    )
    .bind(account_id)
    .bind(feature)
    .fetch_one(pool)
    .await
}

// ---------------------------------------------------------------------------
// Outcome imports
// ---------------------------------------------------------------------------

/// A row of the `outcome_imports` table.
#[derive(Debug, sqlx::FromRow)]
struct OutcomeImportRow {
    id: i64,
    workflow_state: String,
    user_id: Option<i64>,
    progress: Option<i64>,
    ended_at: Option<NaiveDateTime>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
    data: Option<Value>,
}

/// The context an outcome import belongs to.
#[derive(Debug, Clone, Copy)]
enum ImportContext {
    Account(i64),
    Course(i64),
}

impl ImportContext {
    fn id(self) -> i64 {
        match self {
            ImportContext::Account(id) | ImportContext::Course(id) => id,
        }
    }

    /// Canvas stores the context type on the polymorphic association.
    fn type_name(self) -> &'static str {
        match self {
            ImportContext::Account(_) => "Account",
            ImportContext::Course(_) => "Course",
        }
    }
}

/// `POST /accounts/{account_id}/outcome_imports`.
async fn create_account_outcome_import(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((account_id,)): Path<(i64,)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, OutcomeApiError> {
    create_outcome_import(
        &state,
        &auth,
        ImportContext::Account(account_id),
        None,
        raw,
        headers,
        body,
    )
    .await
}

/// `POST /accounts/{account_id}/outcome_imports/group/{learning_outcome_group_id}`.
async fn create_account_outcome_import_in_group(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((account_id, learning_outcome_group_id)): Path<(i64, i64)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, OutcomeApiError> {
    create_outcome_import(
        &state,
        &auth,
        ImportContext::Account(account_id),
        Some(learning_outcome_group_id),
        raw,
        headers,
        body,
    )
    .await
}

/// `POST /courses/{course_id}/outcome_imports`.
async fn create_course_outcome_import(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id,)): Path<(i64,)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, OutcomeApiError> {
    create_outcome_import(
        &state,
        &auth,
        ImportContext::Course(course_id),
        None,
        raw,
        headers,
        body,
    )
    .await
}

/// `POST /courses/{course_id}/outcome_imports/group/{learning_outcome_group_id}`.
async fn create_course_outcome_import_in_group(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, learning_outcome_group_id)): Path<(i64, i64)>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, OutcomeApiError> {
    create_outcome_import(
        &state,
        &auth,
        ImportContext::Course(course_id),
        Some(learning_outcome_group_id),
        raw,
        headers,
        body,
    )
    .await
}

/// `OutcomeImportsApiController#create`.
async fn create_outcome_import(
    state: &AppState,
    auth: &AuthUser,
    context: ImportContext,
    learning_outcome_group_id: Option<i64>,
    raw: Option<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, OutcomeApiError> {
    let pool = &state.db;

    // `before_action :get_context` — the context must exist.
    let context_exists = match context {
        ImportContext::Account(id) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted')",
        )
        .bind(id)
        .fetch_one(pool)
        .await?,
        ImportContext::Course(id) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
        )
        .bind(id)
        .fetch_one(pool)
        .await?,
    };
    if !context_exists {
        return Err(OutcomeApiError::NotFound);
    }

    // `authorized_action(@context, @current_user, :import_outcomes)`.
    let allowed = match context {
        ImportContext::Account(id) => can_manage_outcomes_on_account(pool, auth, id).await?,
        ImportContext::Course(id) => can_manage_outcomes_on_course(pool, auth, id).await?,
    };
    if !allowed {
        return Err(OutcomeApiError::Unauthorized {
            authenticated: true,
        });
    }

    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();

    let mut params = Params::from_query(raw.as_deref());
    let multipart = match multipart_boundary(&content_type) {
        Some(boundary) => parse_multipart(&body, &boundary),
        None => Vec::new(),
    };
    params.merge_multipart(&multipart);

    // `params[:import_type] ||= "instructure_csv"` then the validity check.
    let import_type = params
        .get("import_type")
        .unwrap_or("instructure_csv")
        .to_string();
    if import_type != "instructure_csv" {
        return Err(OutcomeApiError::Raised(
            "invalid import type parameter".to_string(),
        ));
    }

    let upload = resolve_upload(&params, &multipart, &body, &content_type)?;

    let now = Utc::now().naive_utc();
    let import_id: i64 = sqlx::query_scalar(
        "INSERT INTO outcome_imports
            (workflow_state, context_id, context_type, user_id, progress,
             created_at, updated_at, data, learning_outcome_group_id)
         VALUES ('initializing', $1, $2, $3, 0, $4, $4, $5::json, $6)
         RETURNING id",
    )
    .bind(context.id())
    .bind(context.type_name())
    .bind(auth.id)
    .bind(now)
    .bind(json!({ "import_type": import_type }))
    .bind(learning_outcome_group_id)
    .fetch_one(pool)
    .await?;

    // `Attachment.create_data_attachment(import, attachment, "outcome_upload_<global_id>.csv")`.
    let attachment_id: i64 = sqlx::query_scalar(
        "INSERT INTO attachments
            (context_id, context_type, size, content_type, filename, uuid, display_name,
             created_at, updated_at, workflow_state, user_id)
         VALUES ($1, 'OutcomeImport', $2, $3, $4, $5, $4, $6, $6, 'processed', $7)
         RETURNING id",
    )
    .bind(import_id)
    .bind(upload.size)
    .bind(&upload.content_type)
    .bind(&upload.filename)
    .bind(uuid_v4_like())
    .bind(now)
    .bind(auth.id)
    .fetch_one(pool)
    .await?;

    // `import.attachment = att; import.workflow_state = :created; import.save!`
    sqlx::query(
        "UPDATE outcome_imports SET attachment_id = $2, workflow_state = 'created', updated_at = $3 WHERE id = $1",
    )
    .bind(import_id)
    .bind(attachment_id)
    .bind(now)
    .execute(pool)
    .await?;

    // `import.schedule` — `delay(strand: "OutcomeImport::run::<root_account>").run`.
    enqueue_job(
        pool,
        "OutcomeImport::run",
        Some(auth.id),
        "OutcomeImport",
        import_id,
        Some(format!(
            "OutcomeImport::run::account_{}",
            site_admin_account_id(pool).await?.unwrap_or_default()
        )),
        None,
    )
    .await?;

    let row = fetch_outcome_import(pool, import_id).await?;
    let body = outcome_import_json(pool, &row, auth).await?;
    Ok(Json(body).into_response())
}

/// The file part of an import request.
struct ImportUpload {
    filename: String,
    content_type: Option<String>,
    size: i64,
}

/// Canvas `body_file` / `params[:attachment]`.
fn resolve_upload(
    params: &Params,
    multipart: &[MultipartField],
    body: &Bytes,
    content_type: &str,
) -> Result<ImportUpload, OutcomeApiError> {
    if let Some(field) = multipart.iter().find(|field| field.name == "attachment") {
        return Ok(ImportUpload {
            filename: field
                .filename
                .clone()
                .unwrap_or_else(|| "outcome_upload.csv".to_string()),
            content_type: field.content_type.clone(),
            size: field.data.len() as i64,
        });
    }

    // Raw body upload.
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let charset = content_type
        .split(';')
        .skip(1)
        .filter_map(|part| part.trim().split_once('='))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("charset"))
        .map(|(_, value)| value.trim().trim_matches('"').to_string());

    if let Some(charset) = charset {
        if !charset.eq_ignore_ascii_case("utf-8") {
            return Err(OutcomeApiError::InvalidContentType);
        }
    }

    if let Some(extension) = params.get("extension") {
        let filename = format!("outcome_import.{extension}");
        let mime = mimetype_for_extension(extension);
        return Ok(ImportUpload {
            filename,
            content_type: Some(mime.to_string()),
            size: body.len() as i64,
        });
    }

    let extension = match media_type.as_str() {
        "text/plain" | "text/csv" => "csv",
        _ => "csv",
    };
    Ok(ImportUpload {
        filename: format!("outcome_import.{extension}"),
        content_type: if media_type.is_empty() {
            None
        } else {
            Some(media_type)
        },
        size: body.len() as i64,
    })
}

/// Canvas `Attachment.mimetype` for the outcome import extensions.
fn mimetype_for_extension(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "csv" => "text/csv",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// A random uuid-ish token for the attachment row.
fn uuid_v4_like() -> String {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let pid = std::process::id() as u128;
    let mixed = (nanos as u128) ^ (pid << 64) ^ (Utc::now().timestamp() as u128);
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (mixed & 0xffff_ffff) as u32,
        ((mixed >> 32) & 0xffff) as u16,
        ((mixed >> 48) & 0xfff) as u16,
        ((mixed >> 60) & 0xfff) as u16,
        mixed & 0xffff_ffff_ffff
    )
}

/// Insert a `jobs` row (the workspace's durable stand-in for a delayed job).
async fn enqueue_job(
    pool: &PgPool,
    tag: &str,
    user_id: Option<i64>,
    context_type: &str,
    context_id: i64,
    strand: Option<String>,
    singleton: Option<String>,
) -> Result<i64, OutcomeApiError> {
    let now = Utc::now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO jobs
            (tag, workflow_state, progress, user_id, context_type, context_id,
             handler, strand, singleton, run_at, created_at, updated_at)
         VALUES ($1, 'queued', 0, $2, $3, $4, $1, $5, $6, $7, $7, $7)
         RETURNING id",
    )
    .bind(tag)
    .bind(user_id)
    .bind(context_type)
    .bind(context_id)
    .bind(strand)
    .bind(singleton)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Load an outcome import by id.
async fn fetch_outcome_import(pool: &PgPool, id: i64) -> Result<OutcomeImportRow, OutcomeApiError> {
    let row = sqlx::query_as::<_, OutcomeImportRow>(
        "SELECT id, workflow_state, user_id, progress, ended_at, created_at, updated_at, data
         FROM outcome_imports WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(OutcomeApiError::NotFound)
}

/// `Api::V1::OutcomeImport#outcome_import_json`.
async fn outcome_import_json(
    pool: &PgPool,
    import: &OutcomeImportRow,
    auth: &AuthUser,
) -> Result<Value, OutcomeApiError> {
    let mut body = Map::new();
    body.insert("id".to_string(), json!(import.id));
    body.insert(
        "created_at".to_string(),
        json!(iso8601(Some(import.created_at))),
    );
    body.insert("ended_at".to_string(), json!(iso8601(import.ended_at)));
    body.insert(
        "updated_at".to_string(),
        json!(iso8601(Some(import.updated_at))),
    );
    body.insert(
        "progress".to_string(),
        json!(import.progress.unwrap_or_default()),
    );
    body.insert("workflow_state".to_string(), json!(import.workflow_state));
    body.insert(
        "data".to_string(),
        import.data.clone().unwrap_or(Value::Null),
    );

    // `outcome_import_errors.order(:row).limit(25).pluck(:row, :message)`
    let errors = sqlx::query_as::<_, (Option<i32>, String)>(
        "SELECT row, message FROM outcome_import_errors
         WHERE outcome_import_id = $1 ORDER BY row ASC LIMIT 25",
    )
    .bind(import.id)
    .fetch_all(pool)
    .await?;
    let processing_errors: Vec<Value> = errors
        .into_iter()
        .map(|(row, message)| json!([row, message]))
        .collect();
    body.insert("processing_errors".to_string(), json!(processing_errors));

    if let Some(user_id) = import.user_id {
        body.insert("user".to_string(), user_json(pool, user_id, auth).await?);
    }

    Ok(Value::Object(body))
}

/// Format a timestamp the way Rails renders it in JSON.
fn iso8601(value: Option<NaiveDateTime>) -> Value {
    match value {
        Some(stamp) => json!(stamp.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()),
        None => Value::Null,
    }
}

/// Canvas `Api::V1::User#user_json` (the fields `outcome_import_json` uses).
async fn user_json(pool: &PgPool, user_id: i64, auth: &AuthUser) -> Result<Value, OutcomeApiError> {
    let row = sqlx::query_as::<_, (i64, String, NaiveDateTime)>(
        "SELECT id, name, created_at AT TIME ZONE 'UTC' AS created_at FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some((id, name, created_at)) = row else {
        return Ok(Value::Null);
    };
    let (first, last) = split_name(&name);
    let mut body = Map::new();
    body.insert("id".to_string(), json!(id));
    body.insert("name".to_string(), json!(name));
    body.insert("created_at".to_string(), json!(iso8601(Some(created_at))));
    body.insert(
        "sortable_name".to_string(),
        json!(sortable_name(&first, &last)),
    );
    body.insert("short_name".to_string(), json!(short_name(&first, &last)));

    // `sis_user_id` / `integration_id` are only readable by account admins.
    if auth.role == "admin" {
        let pseudonym = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT sis_user_id, integration_id FROM pseudonyms
             WHERE user_id = $1 AND workflow_state = 'active' LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
        if let Some((sis_user_id, integration_id)) = pseudonym {
            body.insert("sis_user_id".to_string(), json!(sis_user_id));
            body.insert("integration_id".to_string(), json!(integration_id));
        }
    }

    Ok(Value::Object(body))
}

/// Canvas `User#name_parts` — first name and last name.
fn split_name(name: &str) -> (String, String) {
    match name.split_once(' ') {
        Some((first, rest)) => (first.to_string(), rest.to_string()),
        None => (name.to_string(), String::new()),
    }
}

/// Canvas `User#sortable_name`.
fn sortable_name(first: &str, last: &str) -> String {
    if last.is_empty() {
        first.to_string()
    } else {
        format!("{last}, {first}")
    }
}

/// Canvas `User#short_name`.
fn short_name(first: &str, last: &str) -> String {
    if last.is_empty() {
        first.to_string()
    } else {
        let initial: String = last.chars().next().unwrap_or(' ').to_string();
        format!("{first} {initial}.")
    }
}

/// The `RecordNotFound` message for a missing outcome import.
fn outcome_import_not_found_message(id: &str) -> String {
    if id == "latest" {
        // `raise ActiveRecord::RecordNotFound` without a message.
        "Couldn't find OutcomeImport without ID".to_string()
    } else {
        format!("Couldn't find OutcomeImport with 'id'={id}")
    }
}

/// `GET /accounts/{account_id}/outcome_imports/{id}`.
async fn show_account_outcome_import(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((account_id, id)): Path<(i64, String)>,
) -> Result<Response, OutcomeApiError> {
    show_outcome_import(&state, &auth, ImportContext::Account(account_id), &id).await
}

/// `GET /courses/{course_id}/outcome_imports/{id}`.
async fn show_course_outcome_import(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, id)): Path<(i64, String)>,
) -> Result<Response, OutcomeApiError> {
    show_outcome_import(&state, &auth, ImportContext::Course(course_id), &id).await
}

/// `OutcomeImportsApiController#show`.
async fn show_outcome_import(
    state: &AppState,
    auth: &AuthUser,
    context: ImportContext,
    id: &str,
) -> Result<Response, OutcomeApiError> {
    let pool = &state.db;

    let context_exists = match context {
        ImportContext::Account(account_id) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted')",
        )
        .bind(account_id)
        .fetch_one(pool)
        .await?,
        ImportContext::Course(course_id) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
        )
        .bind(course_id)
        .fetch_one(pool)
        .await?,
    };
    if !context_exists {
        return Err(OutcomeApiError::NotFound);
    }

    let allowed = match context {
        ImportContext::Account(account_id) => {
            can_manage_outcomes_on_account(pool, auth, account_id).await?
        }
        ImportContext::Course(course_id) => {
            can_manage_outcomes_on_course(pool, auth, course_id).await?
        }
    };
    if !allowed {
        return Err(OutcomeApiError::Unauthorized {
            authenticated: true,
        });
    }

    let import = match find_outcome_import(pool, context, id).await? {
        Some(import) => import,
        None => {
            return Ok(not_found_with_message(outcome_import_not_found_message(id)));
        }
    };

    let body = outcome_import_json(pool, &import, auth).await?;
    Ok(Json(body).into_response())
}

/// `GET /accounts/{account_id}/outcome_imports/{id}/created_group_ids`.
async fn created_group_ids_for_account(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((account_id, id)): Path<(i64, String)>,
) -> Result<Response, OutcomeApiError> {
    created_group_ids(&state, &auth, ImportContext::Account(account_id), &id).await
}

/// `GET /courses/{course_id}/outcome_imports/{id}/created_group_ids`.
async fn created_group_ids_for_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, id)): Path<(i64, String)>,
) -> Result<Response, OutcomeApiError> {
    created_group_ids(&state, &auth, ImportContext::Course(course_id), &id).await
}

/// `OutcomeImportsApiController#created_group_ids`.
async fn created_group_ids(
    state: &AppState,
    auth: &AuthUser,
    context: ImportContext,
    id: &str,
) -> Result<Response, OutcomeApiError> {
    let pool = &state.db;

    let context_exists = match context {
        ImportContext::Account(account_id) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted')",
        )
        .bind(account_id)
        .fetch_one(pool)
        .await?,
        ImportContext::Course(course_id) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
        )
        .bind(course_id)
        .fetch_one(pool)
        .await?,
    };
    if !context_exists {
        return Err(OutcomeApiError::NotFound);
    }

    let allowed = match context {
        ImportContext::Account(account_id) => {
            can_manage_outcomes_on_account(pool, auth, account_id).await?
        }
        ImportContext::Course(course_id) => {
            can_manage_outcomes_on_course(pool, auth, course_id).await?
        }
    };
    if !allowed {
        return Err(OutcomeApiError::Unauthorized {
            authenticated: true,
        });
    }

    let Some(import) = find_outcome_import(pool, context, id).await? else {
        return Ok(not_found_with_message(outcome_import_not_found_message(id)));
    };

    if import.workflow_state == "failed" {
        return Ok(not_found_with_message("Import has failed"));
    }
    if import.workflow_state != "succeeded" {
        return Ok(not_found_with_message("Import is still being processed"));
    }

    let group_ids = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM outcome_groups WHERE outcome_import_id = $1 ORDER BY id ASC",
    )
    .bind(import.id)
    .fetch_all(pool)
    .await?;
    let ids: Vec<Value> = group_ids
        .into_iter()
        .map(|id| json!(id.to_string()))
        .collect();
    Ok(Json(json!(ids)).into_response())
}

/// Find an outcome import inside its context (`"latest"` returns the newest).
async fn find_outcome_import(
    pool: &PgPool,
    context: ImportContext,
    id: &str,
) -> Result<Option<OutcomeImportRow>, OutcomeApiError> {
    if id == "latest" {
        let latest = match context {
            ImportContext::Account(account_id) => {
                sqlx::query_scalar::<_, Option<i64>>(
                    "SELECT latest_outcome_import_id FROM accounts WHERE id = $1",
                )
                .bind(account_id)
                .fetch_one(pool)
                .await?
            }
            ImportContext::Course(course_id) => {
                sqlx::query_scalar::<_, Option<i64>>(
                    "SELECT latest_outcome_import_id FROM courses WHERE id = $1",
                )
                .bind(course_id)
                .fetch_one(pool)
                .await?
            }
        };
        let Some(latest_id) = latest else {
            return Ok(None);
        };
        let row = sqlx::query_as::<_, OutcomeImportRow>(
            "SELECT id, workflow_state, user_id, progress, ended_at, created_at, updated_at, data
             FROM outcome_imports WHERE id = $1",
        )
        .bind(latest_id)
        .fetch_optional(pool)
        .await?;
        return Ok(row);
    }

    let row = sqlx::query_as::<_, OutcomeImportRow>(
        "SELECT id, workflow_state, user_id, progress, ended_at, created_at, updated_at, data
         FROM outcome_imports
         WHERE id = $1 AND context_id = $2 AND context_type = $3",
    )
    .bind(id)
    .bind(context.id())
    .bind(context.type_name())
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Outcome results — shared context
// ---------------------------------------------------------------------------

/// A learning outcome row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct OutcomeRow {
    id: i64,
    title: String,
    data: Option<Value>,
    calculation_method: Option<String>,
    calculation_int: Option<i32>,
}

/// A user row (id + name).
#[derive(Debug, Clone, sqlx::FromRow)]
struct UserRow {
    id: i64,
    name: String,
}

/// The shared state of the `OutcomeResultsController` before_action chain.
struct OutcomeResultsContext {
    course_id: i64,
    account_id: Option<i64>,
    outcomes: Vec<OutcomeRow>,
    outcome_links: Vec<OutcomeLinkRow>,
    outcome_group_ids: Vec<i64>,
    users: Vec<UserRow>,
    all_users: Vec<UserRow>,
    section_id: Option<i64>,
    params: Params,
}

/// One `outcome_links` row (the outcome/group/content-tag association).
#[derive(Debug, Clone, sqlx::FromRow)]
struct OutcomeLinkRow {
    id: i64,
    outcome_id: i64,
    outcome_group_id: i64,
}

/// Run the `OutcomeResultsController` before_action chain.
///
/// The order matters and mirrors the Ruby `before_action` list exactly:
/// `require_context`, `require_outcome_context`,
/// `verify_aggregate_parameter`, `verify_aggregate_stat_parameter`,
/// `verify_sort_parameters`, `verify_include_parameter`, `require_outcomes`,
/// `require_users`.
async fn prepare_outcome_results(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    params: Params,
    rollup_action: bool,
    outcome_id_param: Option<i64>,
) -> Result<OutcomeResultsContext, OutcomeApiError> {
    // `require_context` — the course must exist.
    let course_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(course_id)
    .fetch_one(pool)
    .await?;
    if !course_exists {
        return Err(OutcomeApiError::NotFound);
    }
    let account_id = course_account_id(pool, course_id).await?;

    // `require_outcome_context`
    let manages_grades = can_manage_grades(pool, auth, course_id).await?;
    if !manages_grades {
        if params.get("user_ids").is_none() {
            return Err(reject_forbidden(
                "users not specified and no access to all grades",
            ));
        }
        let requested_values = params.array("user_ids");
        let requested: Vec<i64> = requested_values
            .iter()
            .filter_map(|v| v.parse::<i64>().ok())
            .collect();
        let enrolled = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT user_id) FROM enrollments
             WHERE course_id = $1 AND user_id = ANY($2) AND workflow_state = 'active'",
        )
        .bind(course_id)
        .bind(&requested)
        .fetch_one(pool)
        .await?;
        if enrolled as usize != requested.len() {
            return Err(reject("specified users not enrolled"));
        }
    }

    // `verify_aggregate_parameter`
    if rollup_action {
        if let Some(aggregate) = params.get("aggregate") {
            if aggregate != "course" {
                return Err(reject("invalid aggregate parameter value"));
            }
        }
    }

    // `verify_aggregate_stat_parameter`
    if rollup_action {
        if let Some(stat) = params.get("aggregate_stat") {
            if stat != "mean" && stat != "median" {
                return Err(reject("invalid aggregate_stat parameter value"));
            }
        }
    }

    // `verify_sort_parameters`
    if rollup_action {
        verify_sort_parameters(&params)?;
    }

    // `verify_include_parameter`
    verify_include_parameter(&params)?;

    // `require_outcomes`
    let outcome_group_ids = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM outcome_groups WHERE course_id = $1 AND workflow_state = 'active' ORDER BY id ASC",
    )
    .bind(course_id)
    .fetch_all(pool)
    .await?;

    if params.get("outcome_ids").is_some() && params.get("outcome_group_id").is_some() {
        return Err(reject(
            "can't filter by both outcome_ids and outcome_group_id",
        ));
    }

    let (outcome_links, outcomes) = if let Some(outcome_id) = outcome_id_param {
        let links = load_outcome_links(pool, course_id, Some(outcome_id)).await?;
        let outcomes = load_outcomes(pool, &links).await?;
        if outcomes.is_empty() {
            return Err(reject("outcome not found in this context"));
        }
        (links, outcomes)
    } else if params.get("outcome_group_id").is_some() {
        let group_id = params
            .get("outcome_group_id")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or_default();
        if !outcome_group_ids.contains(&group_id) {
            return Err(reject(
                "can only include an outcome group id in the outcome context",
            ));
        }
        let links = load_outcome_links_for_group(pool, group_id).await?;
        let outcomes = load_outcomes(pool, &links).await?;
        (links, outcomes)
    } else if params.get("outcome_ids").is_some() {
        let requested: Vec<i64> = params
            .array("outcome_ids")
            .iter()
            .filter_map(|value| value.parse::<i64>().ok())
            .collect();
        let mut unique = requested.clone();
        unique.sort_unstable();
        unique.dedup();
        let mut links = load_outcome_links(pool, course_id, None).await?;
        links.retain(|link| unique.contains(&link.outcome_id));
        let outcomes = load_outcomes(pool, &links).await?;
        if outcomes.len() != unique.len() {
            return Err(reject(
                "can only include id's of outcomes in the outcome context",
            ));
        }
        (links, outcomes)
    } else {
        let links = load_outcome_links(pool, course_id, None).await?;
        let outcomes = load_outcomes(pool, &links).await?;
        (links, outcomes)
    };

    // `require_users`
    let mut users: Vec<UserRow>;
    if params.get("user_ids").is_some() && params.get("section_id").is_some() {
        return Err(reject("cannot specify both user_ids and section_id"));
    }
    if params.get("user_ids").is_some() {
        let requested_values = params.array("user_ids");
        let requested_ids: Vec<i64> = requested_values
            .iter()
            .filter_map(|value| value.parse::<i64>().ok())
            .collect();
        let mut unique = requested_ids.clone();
        unique.sort_unstable();
        unique.dedup();
        users = load_course_students(pool, course_id).await?;
        users.retain(|user| unique.contains(&user.id));
        if users.len() != unique.len() {
            return Err(reject(
                "can only include id's of users in the outcome context",
            ));
        }
    } else if params.get("section_id").is_some() {
        let section_id = params
            .get("section_id")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or_default();
        let section_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM course_sections WHERE id = $1 AND course_id = $2)",
        )
        .bind(section_id)
        .bind(course_id)
        .fetch_one(pool)
        .await?;
        if !section_exists {
            return Err(reject("invalid section id"));
        }
        users = load_section_students(pool, course_id, section_id).await?;
    } else {
        users = load_course_students(pool, course_id).await?;
    }
    if params.get("sort_by").is_none() {
        users.sort_by_key(|user| user.id);
    }
    let all_users = users.clone();

    Ok(OutcomeResultsContext {
        course_id,
        account_id,
        outcomes,
        outcome_links,
        outcome_group_ids,
        users,
        all_users,
        section_id: params
            .get("section_id")
            .and_then(|value| value.parse::<i64>().ok()),
        params,
    })
}

/// Canvas `verify_sort_parameters`.
fn verify_sort_parameters(params: &Params) -> Result<(), OutcomeApiError> {
    let Some(sort_by) = params.get("sort_by") else {
        return Ok(());
    };
    const SORTABLE: [&str; 7] = [
        "student",
        "student_name",
        "student_sis_id",
        "student_integration_id",
        "student_login_id",
        "outcome",
        "contributing_score",
    ];
    if !SORTABLE.contains(&sort_by) {
        return Err(reject("invalid sort_by parameter value"));
    }
    if sort_by == "outcome" {
        let Some(outcome_id) = params.get("sort_outcome_id") else {
            return Err(reject("missing required sort_outcome_id parameter value"));
        };
        if outcome_id.is_empty() || !outcome_id.chars().all(|c| c.is_ascii_digit()) {
            return Err(reject("invalid sort_outcome_id parameter value"));
        }
    }
    if sort_by == "contributing_score" {
        let Some(alignment_id) = params.get("sort_alignment_id") else {
            return Err(reject("missing required sort_alignment_id parameter value"));
        };
        let valid = alignment_id
            .split('_')
            .next_back()
            .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
        if !valid {
            return Err(reject("invalid sort_alignment_id parameter value"));
        }
    }
    if let Some(sort_order) = params.get("sort_order") {
        if sort_order != "asc" && sort_order != "desc" {
            return Err(reject("invalid sort_order parameter value"));
        }
    }
    Ok(())
}

/// Canvas `verify_include_parameter`.
fn verify_include_parameter(params: &Params) -> Result<(), OutcomeApiError> {
    const VALID_INCLUDES: [&str; 9] = [
        "courses",
        "users",
        "alignments",
        "outcomes",
        "outcomes.alignments",
        "outcome_groups",
        "outcome_links",
        "outcome_paths",
        "alignment_distributions",
    ];
    for include in params.array("include") {
        match include.as_str() {
            "courses" => {
                if params.get("aggregate") != Some("course") {
                    return Err(reject("can't include courses unless aggregate is 'course'"));
                }
            }
            "users" => {
                if params.present("aggregate") {
                    return Err(reject("can't include users unless aggregate is not set"));
                }
            }
            "alignment_distributions" => {}
            other => {
                if !VALID_INCLUDES.contains(&other) {
                    return Err(reject(format!("invalid include: {other}")));
                }
            }
        }
    }
    Ok(())
}

/// Load the outcome links of a course (optionally for one outcome).
async fn load_outcome_links(
    pool: &PgPool,
    course_id: i64,
    outcome_id: Option<i64>,
) -> Result<Vec<OutcomeLinkRow>, OutcomeApiError> {
    let rows = match outcome_id {
        Some(outcome_id) => {
            sqlx::query_as::<_, OutcomeLinkRow>(
                "SELECT DISTINCT ON (ol.outcome_id) ol.id, ol.outcome_id, ol.outcome_group_id
                 FROM outcome_links ol
                 JOIN outcome_groups og ON og.id = ol.outcome_group_id
                 WHERE og.course_id = $1 AND ol.workflow_state = 'active'
                   AND ol.outcome_id = $2
                 ORDER BY ol.outcome_id, ol.id",
            )
            .bind(course_id)
            .bind(outcome_id)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, OutcomeLinkRow>(
                "SELECT DISTINCT ON (ol.outcome_id) ol.id, ol.outcome_id, ol.outcome_group_id
                 FROM outcome_links ol
                 JOIN outcome_groups og ON og.id = ol.outcome_group_id
                 WHERE og.course_id = $1 AND ol.workflow_state = 'active'
                 ORDER BY ol.outcome_id, ol.id",
            )
            .bind(course_id)
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

/// Load the outcome links of one outcome group.
async fn load_outcome_links_for_group(
    pool: &PgPool,
    group_id: i64,
) -> Result<Vec<OutcomeLinkRow>, OutcomeApiError> {
    let rows = sqlx::query_as::<_, OutcomeLinkRow>(
        "SELECT DISTINCT ON (ol.outcome_id) ol.id, ol.outcome_id, ol.outcome_group_id
         FROM outcome_links ol
         WHERE ol.outcome_group_id = $1 AND ol.workflow_state = 'active'
         ORDER BY ol.outcome_id, ol.id",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Load the outcome rows for a set of links.
async fn load_outcomes(
    pool: &PgPool,
    links: &[OutcomeLinkRow],
) -> Result<Vec<OutcomeRow>, OutcomeApiError> {
    let ids: Vec<i64> = links.iter().map(|link| link.outcome_id).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, OutcomeRow>(
        "SELECT id, title, data, calculation_method, calculation_int
         FROM learning_outcomes WHERE id = ANY($1) AND workflow_state = 'active'
         ORDER BY id ASC",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// The students of a course.
async fn load_course_students(
    pool: &PgPool,
    course_id: i64,
) -> Result<Vec<UserRow>, OutcomeApiError> {
    let rows = sqlx::query_as::<_, UserRow>(
        "SELECT DISTINCT u.id, u.name FROM users u
         JOIN enrollments e ON e.user_id = u.id
         WHERE e.course_id = $1 AND e.role = 'student' AND e.workflow_state = 'active'
         ORDER BY u.id ASC",
    )
    .bind(course_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// The students of a course section.
async fn load_section_students(
    pool: &PgPool,
    course_id: i64,
    section_id: i64,
) -> Result<Vec<UserRow>, OutcomeApiError> {
    let rows = sqlx::query_as::<_, UserRow>(
        "SELECT DISTINCT u.id, u.name FROM users u
         JOIN enrollments e ON e.user_id = u.id
         WHERE e.course_id = $1 AND e.course_section_id = $2
           AND e.role = 'student' AND e.workflow_state = 'active'
         ORDER BY u.id ASC",
    )
    .bind(course_id)
    .bind(section_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Outcome results — endpoints
// ---------------------------------------------------------------------------

/// `GET /courses/{course_id}/outcome_results`.
async fn outcome_results_index(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id,)): Path<(i64,)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, OutcomeApiError> {
    let pool = &state.db;
    let params = Params::from_query(raw.as_deref());
    let ctx = prepare_outcome_results(pool, &auth, course_id, params.clone(), false, None).await?;

    let include_hidden = value_to_boolean(ctx.params.get("include_hidden"));
    let results = load_results(pool, &ctx, &ctx.users, include_hidden).await?;

    let (page, per_page) = pagination(&ctx.params);
    let sliced = slice_page(&results, page, per_page);

    let mut body = Map::new();
    body.insert(
        "outcome_results".to_string(),
        json!(sliced
            .iter()
            .map(outcome_result_json)
            .collect::<Vec<Value>>()),
    );
    if ctx.params.present("include") {
        body.insert(
            "linked".to_string(),
            linked_include_collections(pool, &ctx, &results).await?,
        );
    }

    Ok(Json(Value::Object(body)).into_response())
}

/// `GET /courses/{course_id}/outcome_rollups`.
async fn outcome_rollups(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id,)): Path<(i64,)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, OutcomeApiError> {
    let pool = &state.db;
    let params = Params::from_query(raw.as_deref());
    let ctx = prepare_outcome_results(pool, &auth, course_id, params.clone(), true, None).await?;

    let mut response = Map::new();

    if ctx.params.get("aggregate") == Some("course") {
        // `aggregate_rollups_json`
        let results = load_results(pool, &ctx, &ctx.users, false).await?;
        let rollups = aggregate_rollups(pool, &ctx, &results).await?;
        response.insert(
            "rollups".to_string(),
            json!(rollups
                .iter()
                .map(|rollup| serialize_rollup(rollup, "course", course_id))
                .collect::<Vec<Value>>()),
        );
    } else {
        // `user_rollups_json`
        let results = load_results(pool, &ctx, &ctx.users, false).await?;
        let rollups = user_rollups(pool, &ctx, &results).await?;
        let (page, per_page) = pagination(&ctx.params);
        let total = ctx.users.len();
        let page_users: Vec<&UserRow> = ctx
            .users
            .iter()
            .skip((page - 1) * per_page)
            .take(per_page)
            .collect();
        let serialized: Vec<Value> = {
            let mut rows = Vec::new();
            for rollup in &rollups {
                if page_users.iter().any(|user| user.id == rollup.user_id) {
                    rows.push(serialize_user_rollup(pool, rollup, &ctx).await?);
                }
            }
            rows
        };
        response.insert("rollups".to_string(), json!(serialized));
        response.insert(
            "meta".to_string(),
            jsonapi_meta(
                &format!("/api/v1/courses/{course_id}/outcome_rollups"),
                raw.as_deref().unwrap_or(""),
                per_page,
                page,
                total,
            ),
        );
    }

    if ctx.params.present("include") {
        response.insert(
            "linked".to_string(),
            linked_include_collections(pool, &ctx, &[]).await?,
        );
    }

    Ok(Json(Value::Object(response)).into_response())
}

/// `GET /courses/{course_id}/outcome_mastery_distribution`.
async fn outcome_mastery_distribution(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id,)): Path<(i64,)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, OutcomeApiError> {
    let pool = &state.db;
    let params = Params::from_query(raw.as_deref());
    let ctx = prepare_outcome_results(pool, &auth, course_id, params.clone(), false, None).await?;

    // The action itself re-checks the grade permissions.
    if !can_manage_grades(pool, &auth, course_id).await? {
        return Err(reject_forbidden("insufficient permissions"));
    }

    let results = load_results(pool, &ctx, &ctx.all_users, false).await?;
    let mut rollups = user_rollups(pool, &ctx, &results).await?;

    // Optional `student_ids` filter.
    let mut all_users = ctx.all_users.clone();
    if ctx.params.present("student_ids") {
        let student_ids: Vec<i64> = ctx
            .params
            .array("student_ids")
            .iter()
            .filter_map(|value| value.parse::<i64>().ok())
            .collect();
        all_users.retain(|user| student_ids.contains(&user.id));
        rollups.retain(|rollup| student_ids.contains(&rollup.user_id));
    }

    let distributions = mastery_distributions(pool, &ctx, &rollups).await?;

    let mut body = Map::new();
    body.insert("outcome_distributions".to_string(), distributions);
    body.insert(
        "students".to_string(),
        json!(all_users
            .iter()
            .map(linked_user_json)
            .collect::<Vec<Value>>()),
    );

    Ok(Json(Value::Object(body)).into_response())
}

/// `GET /courses/{course_id}/outcomes/{outcome_id}/contributing_scores`.
async fn outcome_contributing_scores(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, outcome_id)): Path<(i64, i64)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, OutcomeApiError> {
    let pool = &state.db;
    let params = Params::from_query(raw.as_deref());
    let ctx = prepare_outcome_results(
        pool,
        &auth,
        course_id,
        params.clone(),
        false,
        Some(outcome_id),
    )
    .await?;

    if !can_manage_grades(pool, &auth, course_id).await? {
        return Err(reject_forbidden("insufficient permissions"));
    }

    let Some(outcome) = ctx.outcomes.first().cloned() else {
        return Err(reject("outcome not found"));
    };

    let alignments = load_alignments(pool, outcome.id, course_id).await?;
    let only_assignment_alignments = value_to_boolean(ctx.params.get("only_assignment_alignments"));
    let show_unpublished_assignments =
        value_to_boolean(ctx.params.get("show_unpublished_assignments"));

    let alignments: Vec<AlignmentRow> = alignments
        .into_iter()
        .filter(|alignment| show_unpublished_assignments || !alignment.unpublished)
        .collect();

    let results = load_results(pool, &ctx, &ctx.users, false).await?;

    let filtered: Vec<&AlignmentRow> = if only_assignment_alignments {
        alignments
            .iter()
            .filter(|alignment| alignment.content_type.as_deref() == Some("Assignment"))
            .collect()
    } else {
        alignments.iter().collect()
    };

    let mut body = Map::new();
    body.insert(
        "outcome".to_string(),
        json!({ "id": outcome.id.to_string(), "title": outcome.title }),
    );
    body.insert(
        "alignments".to_string(),
        json!(filtered
            .iter()
            .copied()
            .map(alignment_json)
            .collect::<Vec<Value>>()),
    );
    body.insert(
        "scores".to_string(),
        json!(contributing_scores_json(&filtered, &results)),
    );

    Ok(Json(Value::Object(body)).into_response())
}

/// `POST /courses/{course_id}/assign_outcome_order`.
async fn assign_outcome_order(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id,)): Path<(i64,)>,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Result<Response, OutcomeApiError> {
    let pool = &state.db;
    let params = Params::from_query(raw.as_deref());
    let ctx = prepare_outcome_results(pool, &auth, course_id, params.clone(), false, None).await?;

    // `JSON.parse(request.body.read)`
    let parsed: Value = serde_json::from_slice(&body).map_err(|_| reject("invalid JSON body"))?;
    let Some(entries) = parsed.as_array() else {
        return Err(reject("invalid JSON body"));
    };

    let outcome_ids: Vec<i64> = ctx.outcomes.iter().map(|outcome| outcome.id).collect();
    for entry in entries {
        let entry_id = entry.get("outcome_id").and_then(Value::as_i64);
        match entry_id {
            Some(id) if outcome_ids.contains(&id) => {}
            _ => return Err(reject("Outcomes do not belong to Course")),
        }
    }

    let root_id = match ctx.account_id {
        Some(account_id) => root_account_id(pool, account_id).await?,
        None => site_admin_account_id(pool).await?.unwrap_or_default(),
    };

    // `UserLmgbOutcomeOrderings.set_lmgb_outcome_ordering`
    let now = Utc::now().naive_utc();
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM user_lmgb_outcome_orderings WHERE user_id = $1 AND course_id = $2")
        .bind(auth.id)
        .bind(course_id)
        .execute(&mut *tx)
        .await?;
    for entry in entries {
        let Some(outcome_id) = entry.get("outcome_id").and_then(Value::as_i64) else {
            continue;
        };
        let position = entry.get("position").and_then(Value::as_i64).unwrap_or(0);
        sqlx::query(
            "INSERT INTO user_lmgb_outcome_orderings
                (root_account_id, user_id, course_id, learning_outcome_id, position, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $6)",
        )
        .bind(root_id)
        .bind(auth.id)
        .bind(course_id)
        .bind(outcome_id)
        .bind(position as i32)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    // The Ruby action renders nothing, so Rails answers `204 No Content`.
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /enqueue_outcome_rollup_calculation`.
async fn enqueue_outcome_rollup_calculation(
    State(state): State<AppState>,
    _auth: AuthUser,
    RawQuery(raw): RawQuery,
) -> Result<Response, OutcomeApiError> {
    let pool = &state.db;
    let params = Params::from_query(raw.as_deref());

    let site_admin = site_admin_account_id(pool).await?;
    let enabled = match site_admin {
        Some(account_id) => {
            account_feature_enabled(pool, account_id, "outcomes_rollup_propagation").await?
        }
        None => false,
    };
    if !enabled {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }

    let course_id_param = params.get("course_id").unwrap_or("").to_string();
    let student_uuid = params.get("student_uuid").unwrap_or("").to_string();

    if course_id_param.is_empty() || student_uuid.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "course id and student uuid are required" })),
        )
            .into_response());
    }

    // `Course.find(params[:course_id])` — a missing course is a 404.
    let Ok(course_id) = course_id_param.parse::<i64>() else {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Invalid course or student" })),
        )
            .into_response());
    };

    let course_exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1)")
            .bind(course_id)
            .fetch_one(pool)
            .await?;
    if !course_exists {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Invalid course or student" })),
        )
            .into_response());
    }

    let student_id = sqlx::query_scalar::<_, i64>(
        "SELECT u.id FROM users u
         JOIN enrollments e ON e.user_id = u.id
         WHERE e.course_id = $1 AND e.role = 'student' AND e.workflow_state = 'active'
           AND u.uuid = $2
         LIMIT 1",
    )
    .bind(course_id)
    .bind(&student_uuid)
    .fetch_optional(pool)
    .await?;
    let Some(student_id) = student_id else {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Invalid course or student" })),
        )
            .into_response());
    };

    // `Outcomes::StudentOutcomeRollupCalculationService.calculate_for_student`
    enqueue_job(
        pool,
        "Outcomes::StudentOutcomeRollupCalculationService",
        None,
        "Course",
        course_id,
        None,
        Some(format!(
            "calculate_for_student:{}:{}",
            course_id, student_id
        )),
    )
    .await?;

    Ok(Json(json!({
        "message": format!(
            "Rollup calculation enqueued for student {student_id} in course {course_id}"
        ),
        "type": "student"
    }))
    .into_response())
}

/// The root account of an account.
///
/// Canvas `Account#root_account_id` — the top of the account chain. The
/// recursive walk stops at the account without a parent, which is the root
/// account of the context.
async fn root_account_id(pool: &PgPool, account_id: i64) -> Result<i64, OutcomeApiError> {
    let root = sqlx::query_scalar::<_, i64>(
        r#"
        WITH RECURSIVE chain AS (
            SELECT id, parent_account_id FROM accounts WHERE id = $1
            UNION ALL
            SELECT a.id, a.parent_account_id
            FROM accounts a JOIN chain c ON a.id = c.parent_account_id
        )
        SELECT id FROM chain WHERE parent_account_id IS NULL LIMIT 1
        "#,
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    Ok(root.unwrap_or(account_id))
}

// ---------------------------------------------------------------------------
// Outcome results — helpers
// ---------------------------------------------------------------------------

/// Canvas `value_to_boolean`.
fn value_to_boolean(value: Option<&str>) -> bool {
    matches!(
        value,
        Some("true") | Some("1") | Some("t") | Some("T") | Some("yes")
    )
}

/// The `page` / `per_page` request parameters.
fn pagination(params: &Params) -> (usize, usize) {
    let page = params
        .get("page")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    let per_page = params
        .get("per_page")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(10);
    (page, per_page)
}

/// Slice a list the way `Api.paginate` does.
fn slice_page<T: Clone>(items: &[T], page: usize, per_page: usize) -> Vec<T> {
    items
        .iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .cloned()
        .collect()
}

/// Canvas `Api.jsonapi_meta`.
fn jsonapi_meta(base_url: &str, query: &str, per_page: usize, page: usize, total: usize) -> Value {
    let mut pagination = Map::new();
    pagination.insert("per_page".to_string(), json!(per_page));

    let page_count = if per_page == 0 {
        0
    } else {
        total.div_ceil(per_page)
    };
    let link = |page: usize| format!("{base_url}?{query}&page={page}&per_page={per_page}");

    if page < page_count {
        pagination.insert("next".to_string(), json!(link(page + 1)));
    }
    pagination.insert("last".to_string(), json!(link(page_count.max(1))));
    if page > 1 {
        pagination.insert("prev".to_string(), json!(link(page - 1)));
    }
    pagination.insert("current".to_string(), json!(link(page)));
    pagination.insert("first".to_string(), json!(link(1)));
    pagination.insert("page".to_string(), json!(page));
    pagination.insert(
        "template".to_string(),
        json!(format!(
            "{base_url}?{query}&page={{page}}&per_page={per_page}"
        )),
    );
    pagination.insert("count".to_string(), json!(total));
    pagination.insert("page_count".to_string(), json!(page_count));

    json!({ "pagination": Value::Object(pagination) })
}

/// A `learning_outcome_results` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ResultRow {
    id: i64,
    outcome_id: i64,
    user_id: i64,
    score: Option<f64>,
    content_tag_id: Option<i64>,
    association_type: Option<String>,
    association_id: Option<i64>,
    hidden: bool,
    possible: Option<f64>,
    percent: Option<f64>,
    mastery: Option<bool>,
    hide_points: bool,
    title: Option<String>,
    submitted_at: Option<NaiveDateTime>,
    assessed_at: Option<NaiveDateTime>,
    alignment_content_type: Option<String>,
    alignment_content_id: Option<i64>,
}

/// Load the outcome results for the given users and outcomes.
async fn load_results(
    pool: &PgPool,
    ctx: &OutcomeResultsContext,
    users: &[UserRow],
    include_hidden: bool,
) -> Result<Vec<ResultRow>, OutcomeApiError> {
    if users.is_empty() || ctx.outcomes.is_empty() {
        return Ok(Vec::new());
    }
    let user_ids: Vec<i64> = users.iter().map(|user| user.id).collect();
    let outcome_ids: Vec<i64> = ctx.outcomes.iter().map(|outcome| outcome.id).collect();
    let context_code = format!("course_{}", ctx.course_id);

    let rows = sqlx::query_as::<_, ResultRow>(
        "SELECT r.id, r.outcome_id, r.user_id, r.score, r.content_tag_id, r.association_type,
                r.association_id, r.hidden, r.possible, r.percent, r.mastery, r.hide_points,
                r.title, r.submitted_at, r.assessed_at,
                oa.content_type AS alignment_content_type,
                oa.content_id AS alignment_content_id
         FROM learning_outcome_results r
         LEFT JOIN outcome_alignments oa ON oa.id = r.content_tag_id
         WHERE r.workflow_state = 'active'
           AND (r.context_code IS NULL OR r.context_code = $1)
           AND r.user_id = ANY($2)
           AND r.outcome_id = ANY($3)
           AND ($4 OR r.hidden = FALSE)
         ORDER BY r.user_id ASC, r.outcome_id ASC, r.id ASC",
    )
    .bind(&context_code)
    .bind(&user_ids)
    .bind(&outcome_ids)
    .bind(include_hidden)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Canvas `Api::V1::OutcomeResults#outcome_result_json`.
fn outcome_result_json(result: &ResultRow) -> Value {
    json!({
        "id": result.id,
        "score": result.score,
        "mastery": result.mastery,
        "possible": result.possible,
        "percent": result.percent,
        "hide_points": result.hide_points,
        "hidden": result.hidden,
        "submitted_or_assessed_at": iso8601(result.submitted_at.or(result.assessed_at)),
        "links": {
            "user": result.user_id.to_string(),
            "learning_outcome": result.outcome_id.to_string(),
            "alignment": alignment_asset_string(result),
            "assignment": association_asset_string(result)
        }
    })
}

/// The `asset_string` of the alignment a result points at.
fn alignment_asset_string(result: &ResultRow) -> Value {
    match (
        result.alignment_content_type.as_deref(),
        result.alignment_content_id,
    ) {
        (Some(content_type), Some(content_id)) => {
            json!(format!("{}_{}", content_type.to_lowercase(), content_id))
        }
        _ => Value::Null,
    }
}

/// The `asset_string` of the assignment a result is associated with.
fn association_asset_string(result: &ResultRow) -> Value {
    match (result.association_type.as_deref(), result.association_id) {
        (Some("Assignment"), Some(id)) => json!(format!("assignment_{id}")),
        _ => Value::Null,
    }
}

/// An `outcome_alignments` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct AlignmentRow {
    id: i64,
    content_type: Option<String>,
    content_id: Option<i64>,
    content_title: Option<String>,
    unpublished: bool,
}

/// Load the alignments of an outcome inside a course.
async fn load_alignments(
    pool: &PgPool,
    outcome_id: i64,
    course_id: i64,
) -> Result<Vec<AlignmentRow>, OutcomeApiError> {
    let rows = sqlx::query_as::<_, AlignmentRow>(
        "SELECT oa.id, oa.content_type, oa.content_id, a.title AS content_title,
                COALESCE(a.workflow_state = 'unpublished', FALSE) AS unpublished
         FROM outcome_alignments oa
         LEFT JOIN assignments a ON oa.content_type = 'Assignment' AND a.id = oa.content_id
         WHERE oa.outcome_id = $1 AND oa.workflow_state = 'active'
           AND (oa.context_id = $2 OR oa.context_id IS NULL)
         ORDER BY oa.id ASC",
    )
    .bind(outcome_id)
    .bind(course_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Canvas `alignments_json` for the contributing scores endpoint.
fn alignment_json(alignment: &AlignmentRow) -> Value {
    json!({
        "alignment_id": format!("D_{}", alignment.id),
        "associated_asset_id": alignment.content_id.map(|id| id.to_string()).unwrap_or_default(),
        "associated_asset_name": alignment.content_title.clone().unwrap_or_default(),
        "associated_asset_type": alignment.content_type.clone().unwrap_or_default(),
        "html_url": ""
    })
}

/// Canvas `scores_json` for the contributing scores endpoint.
fn contributing_scores_json(alignments: &[&AlignmentRow], results: &[ResultRow]) -> Vec<Value> {
    let mut scores = Vec::new();
    for result in results {
        let Some(alignment) = alignments
            .iter()
            .find(|alignment| Some(alignment.id) == result.content_tag_id)
        else {
            continue;
        };
        scores.push(json!({
            "user_id": result.user_id.to_string(),
            "alignment_id": format!("D_{}", alignment.id),
            "score": result.score,
            "submitted_or_assessed_at": iso8601(result.submitted_at.or(result.assessed_at))
        }));
    }
    scores
}

// ---------------------------------------------------------------------------
// Rollups
// ---------------------------------------------------------------------------

/// A calculated rollup score for one outcome and one user.
#[derive(Debug, Clone)]
struct RollupScore {
    outcome_id: i64,
    score: Option<f64>,
    count: i64,
    title: Option<String>,
    submitted_at: Option<NaiveDateTime>,
    hide_points: bool,
}

/// A rollup for one user (or for the course when aggregating).
#[derive(Debug, Clone)]
struct Rollup {
    user_id: i64,
    scores: Vec<RollupScore>,
}

/// The rubric criterion of an outcome.
struct RubricCriterion {
    points_possible: Option<f64>,
    mastery_points: Option<f64>,
    ratings: Vec<RubricRating>,
}

/// One rubric rating.
struct RubricRating {
    description: Option<String>,
    points: f64,
    color: Option<String>,
}

/// Read `learning_outcomes.data->'rubric_criterion'`.
fn rubric_criterion(outcome: &OutcomeRow) -> RubricCriterion {
    let criterion = outcome
        .data
        .as_ref()
        .and_then(|data| data.get("rubric_criterion"))
        .cloned()
        .unwrap_or(Value::Null);

    let points_possible = criterion.get("points_possible").and_then(Value::as_f64);
    let mastery_points = criterion.get("mastery_points").and_then(Value::as_f64);
    let ratings = criterion
        .get("ratings")
        .and_then(Value::as_array)
        .map(|ratings| {
            ratings
                .iter()
                .filter_map(|rating| {
                    Some(RubricRating {
                        description: rating
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        points: rating.get("points").and_then(Value::as_f64)?,
                        color: rating
                            .get("color")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    RubricCriterion {
        points_possible,
        mastery_points,
        ratings,
    }
}

/// Round like Ruby's `Float#round(2)`.
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Canvas `RollupScore` — the score of one outcome for one user.
fn calculate_rollup_score(
    outcome: &OutcomeRow,
    results: &[&ResultRow],
    aggregate: bool,
    median: bool,
) -> RollupScore {
    let criterion = rubric_criterion(outcome);
    let points_possible = criterion.points_possible.unwrap_or_default();
    let mastery_points = criterion.mastery_points.unwrap_or_default();

    let calculation_method = outcome
        .calculation_method
        .clone()
        .filter(|method| !method.is_empty())
        .unwrap_or_else(|| "highest".to_string());
    let calculation_int = outcome.calculation_int;

    // The scoring methods that scale a result by the outcome's points.
    let scaled = matches!(
        calculation_method.as_str(),
        "decaying_average"
            | "latest"
            | "average"
            | "standard_decaying_average"
            | "weighted_average"
    );

    // `score_sets`
    let mut sets: Vec<(&ResultRow, Option<f64>)> = Vec::new();
    let selected: Vec<&ResultRow> = match calculation_method.as_str() {
        "decaying_average" | "standard_decaying_average" | "weighted_average" => {
            if aggregate {
                results.to_vec()
            } else {
                let mut sorted = results.to_vec();
                sorted.sort_by_key(|result| result_time(result));
                sorted
            }
        }
        "latest" => {
            if aggregate {
                results.to_vec()
            } else {
                let mut sorted = results.to_vec();
                sorted.sort_by_key(|result| result_time(result));
                sorted
                    .last()
                    .map(|result| vec![*result])
                    .unwrap_or_default()
            }
        }
        _ => results.to_vec(),
    };

    for result in selected {
        let score = if scaled {
            scaled_score(result, results, points_possible, mastery_points)
        } else {
            result_score(result, points_possible, mastery_points)
        };
        sets.push((result, score));
    }

    let count = sets.len() as i64;

    if aggregate {
        let scores: Vec<f64> = sets.iter().filter_map(|(_, score)| *score).collect();
        let score = if scores.is_empty() {
            None
        } else if median {
            let mut sorted = scores.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mid = sorted.len() / 2;
            if sorted.len().is_multiple_of(2) {
                Some((sorted[mid - 1] + sorted[mid]) / 2.0)
            } else {
                Some(sorted[mid])
            }
        } else {
            Some(round2(scores.iter().sum::<f64>() / scores.len() as f64))
        };
        let hide_points = !sets.is_empty() && sets.iter().all(|(result, _)| result.hide_points);
        return RollupScore {
            outcome_id: outcome.id,
            score,
            count,
            title: None,
            submitted_at: None,
            hide_points,
        };
    }

    let (score, used): (Option<f64>, Vec<&ResultRow>) = match calculation_method.as_str() {
        "latest" => match sets.first() {
            Some((result, score)) => (score.map(round2), vec![*result]),
            None => (None, Vec::new()),
        },
        "highest" => {
            let best = sets
                .iter()
                .filter_map(|(result, score)| score.map(|value| (*result, value)))
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            match best {
                Some((result, score)) => (Some(round2(score)), vec![result]),
                None => (None, Vec::new()),
            }
        }
        "average" => {
            let scores: Vec<f64> = sets.iter().filter_map(|(_, score)| *score).collect();
            if scores.is_empty() {
                (None, Vec::new())
            } else {
                (
                    Some(round2(scores.iter().sum::<f64>() / scores.len() as f64)),
                    sets.iter().map(|(result, _)| *result).collect(),
                )
            }
        }
        "n_mastery" => {
            let required = calculation_int.unwrap_or_default() as usize;
            let mastered: Vec<(&ResultRow, f64)> = sets
                .iter()
                .filter_map(|(result, score)| score.map(|value| (*result, value)))
                .filter(|(_, value)| *value >= mastery_points)
                .collect();
            if mastered.len() < required {
                (None, Vec::new())
            } else {
                let sum: f64 = mastered.iter().map(|(_, value)| value).sum();
                (
                    Some(round2(sum / mastered.len() as f64)),
                    mastered.into_iter().map(|(result, _)| result).collect(),
                )
            }
        }
        "standard_decaying_average" => {
            let decay_rate = calculation_int.unwrap_or(65) as f64;
            let remaining = 100.0 - decay_rate;
            let scores: Vec<f64> = sets.iter().filter_map(|(_, score)| *score).collect();
            if scores.len() == 1 {
                (
                    Some(round2(scores[0])),
                    sets.iter().map(|(r, _)| *r).collect(),
                )
            } else {
                let mut decay: Option<f64> = None;
                for pair in scores.windows(2) {
                    decay = Some(match decay {
                        None => pair[0] * (0.01 * remaining) + pair[1] * (0.01 * decay_rate),
                        Some(value) => value * (0.01 * remaining) + pair[1] * (0.01 * decay_rate),
                    });
                }
                (
                    decay.map(round2),
                    sets.iter().map(|(result, _)| *result).collect(),
                )
            }
        }
        "decaying_average" | "weighted_average" => {
            let weight = calculation_int.unwrap_or(65) as f64;
            let mut scores: Vec<f64> = sets.iter().filter_map(|(_, score)| *score).collect();
            let latest = scores.pop();
            match (latest, scores.is_empty()) {
                (None, _) => (None, Vec::new()),
                (Some(latest), true) => (
                    Some(round2(latest)),
                    sets.iter().map(|(result, _)| *result).collect(),
                ),
                (Some(latest), false) => {
                    let older: f64 = scores.iter().sum::<f64>() / scores.len() as f64;
                    let value = latest * (0.01 * weight) + older * (0.01 * (100.0 - weight));
                    (
                        Some(round2(value)),
                        sets.iter().map(|(result, _)| *result).collect(),
                    )
                }
            }
        }
        _ => (None, Vec::new()),
    };

    // `latest_result` — the most recent result of the set.
    let latest = used
        .iter()
        .copied()
        .max_by_key(|result| result_time(result));
    let (submitted_at, title) = match latest {
        Some(result) => {
            let stamp = result.submitted_at.or(result.assessed_at);
            let title = stamp
                .and(result.title.as_ref())
                .and_then(|value| value.split(", ").nth(1).map(str::to_string));
            (stamp, title)
        }
        None => (None, None),
    };

    let hide_points = !used.is_empty() && used.iter().all(|result| result.hide_points);

    RollupScore {
        outcome_id: outcome.id,
        score,
        count,
        title,
        submitted_at,
        hide_points,
    }
}

/// Canvas `RollupScoreAggregatorHelper#result_time`.
fn result_time(result: &ResultRow) -> i64 {
    result
        .submitted_at
        .or(result.assessed_at)
        .map(|stamp| stamp.and_utc().timestamp())
        .unwrap_or_default()
}

/// Canvas `RollupScoreAggregatorHelper#result_score`.
fn result_score(result: &ResultRow, points_possible: f64, mastery_points: f64) -> Option<f64> {
    let Some(percent) = result.percent else {
        return result.score;
    };
    if points_possible > 0.0 {
        Some(percent * points_possible)
    } else {
        Some(percent * mastery_points)
    }
}

/// Canvas `RollupScoreAggregatorHelper#scaled_score_from_result`.
fn scaled_score(
    result: &ResultRow,
    results: &[&ResultRow],
    points_possible: f64,
    mastery_points: f64,
) -> Option<f64> {
    let mut total = 0.0;
    let mut weighted = 0.0;
    for other in results {
        if other.association_id != result.association_id
            || other.outcome_id != result.outcome_id
            || other.association_type != result.association_type
        {
            continue;
        }
        let Some(possible) = other.possible else {
            continue;
        };
        total += possible;
        weighted += possible * other.percent.unwrap_or_default();
    }
    if total == 0.0 {
        return None;
    }
    let possible = if points_possible > 0.0 {
        points_possible
    } else {
        mastery_points
    };
    Some((weighted / total) * possible)
}

/// Canvas `Outcomes::ResultAnalytics#outcome_results_rollups`.
async fn user_rollups(
    pool: &PgPool,
    ctx: &OutcomeResultsContext,
    results: &[ResultRow],
) -> Result<Vec<Rollup>, OutcomeApiError> {
    let mut rollups: Vec<Rollup> = Vec::new();
    for user in &ctx.users {
        let user_results: Vec<&ResultRow> = results
            .iter()
            .filter(|result| result.user_id == user.id)
            .collect();
        let mut scores = Vec::new();
        for outcome in &ctx.outcomes {
            let outcome_results: Vec<&ResultRow> = user_results
                .iter()
                .copied()
                .filter(|result| result.outcome_id == outcome.id && result.score.is_some())
                .collect();
            if outcome_results.is_empty() {
                continue;
            }
            scores.push(calculate_rollup_score(
                outcome,
                &outcome_results,
                false,
                false,
            ));
        }
        rollups.push(Rollup {
            user_id: user.id,
            scores,
        });
    }
    let _ = pool;
    Ok(rollups)
}

/// Canvas `Outcomes::ResultAnalytics#aggregate_outcome_results_rollup`.
async fn aggregate_rollups(
    pool: &PgPool,
    ctx: &OutcomeResultsContext,
    results: &[ResultRow],
) -> Result<Vec<Rollup>, OutcomeApiError> {
    let user_rollups = user_rollups(pool, ctx, results).await?;
    let median = ctx.params.get("aggregate_stat") == Some("median");

    let mut scores = Vec::new();
    for outcome in &ctx.outcomes {
        let mut outcome_scores: Vec<&RollupScore> = Vec::new();
        for rollup in &user_rollups {
            for score in &rollup.scores {
                if score.outcome_id == outcome.id {
                    outcome_scores.push(score);
                }
            }
        }
        if outcome_scores.is_empty() {
            continue;
        }
        let mut aggregated = RollupScore {
            outcome_id: outcome.id,
            score: None,
            count: outcome_scores.iter().map(|score| score.count).sum(),
            title: None,
            submitted_at: None,
            hide_points: outcome_scores.iter().all(|score| score.hide_points),
        };
        // Aggregate the per-user scores for this outcome.
        let values: Vec<f64> = outcome_scores
            .iter()
            .filter_map(|score| score.score)
            .collect();
        aggregated.score = if values.is_empty() {
            None
        } else if median {
            let mut sorted = values.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mid = sorted.len() / 2;
            if sorted.len().is_multiple_of(2) {
                Some((sorted[mid - 1] + sorted[mid]) / 2.0)
            } else {
                Some(sorted[mid])
            }
        } else {
            Some(round2(values.iter().sum::<f64>() / values.len() as f64))
        };
        scores.push(aggregated);
    }

    Ok(vec![Rollup {
        user_id: ctx.course_id,
        scores,
    }])
}

/// Canvas `serialize_rollup`.
fn serialize_rollup(rollup: &Rollup, context_key: &str, context_id: i64) -> Value {
    let mut links = Map::new();
    links.insert(context_key.to_string(), json!(context_id.to_string()));
    json!({
        "scores": rollup
            .scores
            .iter()
            .map(serialize_rollup_score)
            .collect::<Vec<Value>>(),
        "links": Value::Object(links)
    })
}

/// Canvas `serialize_rollup_score`.
fn serialize_rollup_score(score: &RollupScore) -> Value {
    json!({
        "score": score.score,
        "title": score.title,
        "submitted_at": iso8601(score.submitted_at),
        "count": score.count,
        "hide_points": score.hide_points,
        "links": { "outcome": score.outcome_id.to_string() }
    })
}

/// Canvas `serialize_user_rollups` — adds the section links.
async fn serialize_user_rollup(
    pool: &PgPool,
    rollup: &Rollup,
    ctx: &OutcomeResultsContext,
) -> Result<Value, OutcomeApiError> {
    let mut body = serialize_rollup(rollup, "user", rollup.user_id);
    let sections = load_user_sections(pool, ctx.course_id, rollup.user_id, ctx.section_id).await?;
    let (section_id, status) = sections
        .first()
        .cloned()
        .unwrap_or((None, "active".to_string()));
    if let Some(links) = body.get_mut("links").and_then(Value::as_object_mut) {
        links.insert(
            "section".to_string(),
            json!(section_id.map(|id| id.to_string()).unwrap_or_default()),
        );
        links.insert("status".to_string(), json!(status));
    }
    Ok(body)
}

/// The section enrollments of a user in a course.
async fn load_user_sections(
    pool: &PgPool,
    course_id: i64,
    user_id: i64,
    section_id: Option<i64>,
) -> Result<Vec<(Option<i64>, String)>, OutcomeApiError> {
    let rows = sqlx::query_as::<_, (Option<i64>, String)>(
        "SELECT course_section_id, workflow_state FROM enrollments
         WHERE course_id = $1 AND user_id = $2 AND role = 'student'
           AND ($3::bigint IS NULL OR course_section_id = $3)
         ORDER BY id ASC",
    )
    .bind(course_id)
    .bind(user_id)
    .bind(section_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Mastery distribution
// ---------------------------------------------------------------------------

/// Canvas `Outcomes::ResultAnalytics#mastery_distributions`.
async fn mastery_distributions(
    pool: &PgPool,
    ctx: &OutcomeResultsContext,
    rollups: &[Rollup],
) -> Result<Value, OutcomeApiError> {
    let mut distributions = Map::new();
    for outcome in &ctx.outcomes {
        let mut ratings = rubric_criterion(outcome).ratings;
        if ratings.is_empty() {
            continue;
        }
        ratings.sort_by(|a, b| {
            b.points
                .partial_cmp(&a.points)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut counts: Vec<(RubricRating, Vec<String>)> = ratings
            .into_iter()
            .map(|rating| (rating, Vec::new()))
            .collect();

        let mut total_students = 0usize;
        for rollup in rollups {
            let Some(score) = rollup
                .scores
                .iter()
                .find(|score| score.outcome_id == outcome.id)
                .and_then(|score| score.score)
            else {
                continue;
            };
            total_students += 1;
            if let Some(index) = counts.iter().position(|(rating, _)| rating.points <= score) {
                counts[index].1.push(rollup.user_id.to_string());
            }
        }

        let rating_json: Vec<Value> = counts
            .iter()
            .map(|(rating, students)| {
                json!({
                    "description": rating.description,
                    "points": rating.points,
                    "color": rating.color.clone().unwrap_or_else(|| "#666666".to_string()),
                    "count": students.len(),
                    "student_ids": students
                })
            })
            .collect();

        distributions.insert(
            outcome.id.to_string(),
            json!({
                "outcome_id": outcome.id.to_string(),
                "ratings": rating_json,
                "total_students": total_students
            }),
        );
    }
    let _ = pool;
    Ok(Value::Object(distributions))
}

/// Canvas `outcome_results_linked_users_json` — the trimmed user payload.
fn linked_user_json(user: &UserRow) -> Value {
    let (first, last) = split_name(&user.name);
    json!({
        "id": user.id.to_string(),
        "name": user.name,
        "display_name": short_name(&first, &last),
        "sortable_name": sortable_name(&first, &last)
    })
}

/// Canvas `linked_include_collections`.
async fn linked_include_collections(
    pool: &PgPool,
    ctx: &OutcomeResultsContext,
    results: &[ResultRow],
) -> Result<Value, OutcomeApiError> {
    let mut linked = Map::new();
    let mut includes = ctx.params.array("include");
    includes.dedup();
    for include in includes {
        match include.as_str() {
            "courses" => {
                linked.insert(
                    "courses".to_string(),
                    json!([{ "id": ctx.course_id.to_string(), "name": course_name(pool, ctx.course_id).await? }]),
                );
            }
            "users" => {
                linked.insert(
                    "users".to_string(),
                    json!(ctx
                        .users
                        .iter()
                        .map(linked_user_json)
                        .collect::<Vec<Value>>()),
                );
            }
            "outcomes" => {
                linked.insert(
                    "outcomes".to_string(),
                    json!(ctx
                        .outcomes
                        .iter()
                        .map(|outcome| json!({
                            "id": outcome.id,
                            "title": outcome.title,
                            "alignments": [],
                            "group_id": ctx
                                .outcome_links
                                .iter()
                                .find(|link| link.outcome_id == outcome.id)
                                .map(|link| link.outcome_group_id.to_string())
                        }))
                        .collect::<Vec<Value>>()),
                );
            }
            "outcome_groups" => {
                let ids = &ctx.outcome_group_ids;
                let groups = sqlx::query_as::<_, (i64, String)>(
                    "SELECT id, title FROM outcome_groups WHERE id = ANY($1) AND workflow_state = 'active'",
                )
                .bind(ids)
                .fetch_all(pool)
                .await?;
                linked.insert(
                    "outcome_groups".to_string(),
                    json!(groups
                        .into_iter()
                        .map(|(id, title)| json!({ "id": id, "title": title }))
                        .collect::<Vec<Value>>()),
                );
            }
            "outcome_links" => {
                linked.insert(
                    "outcome_links".to_string(),
                    json!(ctx
                        .outcome_links
                        .iter()
                        .map(|link| json!({
                            "id": link.id,
                            "outcome_id": link.outcome_id,
                            "outcome_group_id": link.outcome_group_id
                        }))
                        .collect::<Vec<Value>>()),
                );
            }
            "outcome_paths" => {
                linked.insert(
                    "outcome_paths".to_string(),
                    json!(ctx
                        .outcomes
                        .iter()
                        .map(|outcome| json!({
                            "id": outcome.id,
                            "parts": [{ "name": outcome.title }]
                        }))
                        .collect::<Vec<Value>>()),
                );
            }
            "alignments" | "outcomes.alignments" | "alignment_distributions" => {
                linked.insert(
                    include.clone(),
                    json!(alignment_links_json(pool, ctx, results).await?),
                );
            }
            _ => {}
        }
    }
    Ok(Value::Object(linked))
}

/// The alignments referenced by the results (Canvas `include_alignments`).
async fn alignment_links_json(
    pool: &PgPool,
    ctx: &OutcomeResultsContext,
    results: &[ResultRow],
) -> Result<Vec<Value>, OutcomeApiError> {
    let mut alignments = Vec::new();
    for outcome in &ctx.outcomes {
        for alignment in load_alignments(pool, outcome.id, ctx.course_id).await? {
            alignments.push(json!({
                "id": format!("D_{}", alignment.id),
                "name": "",
                "html_url": ""
            }));
        }
    }
    let _ = results;
    Ok(alignments)
}

/// The name of a course.
async fn course_name(pool: &PgPool, course_id: i64) -> Result<String, OutcomeApiError> {
    let name = sqlx::query_scalar::<_, String>("SELECT name FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
    Ok(name.unwrap_or_default())
}
