//! Canvas SIS imports API — port of `SisImportsApiController`
//! (`canvas-src/app/controllers/sis_imports_api_controller.rb`).
//!
//! Routes served here (the coordinator mounts this router under `/api/v1`):
//!
//! | Method | Path                                                      | Canvas action       |
//! |--------|-----------------------------------------------------------|---------------------|
//! | GET    | `/accounts/{account_id}/sis_imports`                     | `index`             |
//! | POST   | `/accounts/{account_id}/sis_imports`                     | `create`            |
//! | GET    | `/accounts/{account_id}/sis_imports/importing`           | `importing`         |
//! | GET    | `/accounts/{account_id}/sis_imports/{id}`                | `show`              |
//! | PUT    | `/accounts/{account_id}/sis_imports/{id}/abort`          | `abort`             |
//! | PUT    | `/accounts/{account_id}/sis_imports/{id}/restore_states` | `restore_states`    |
//! | PUT    | `/accounts/{account_id}/sis_imports/abort_all_pending`  | `abort_all_pending` |
//!
//! Faithfulness notes:
//!
//! * The controller level `before_action` chain is reproduced in order:
//!   `get_context` (an unknown account is a 404), `check_account` (a
//!   sub-account is a 400, an account without SIS imports is a 403) and only
//!   then the `authorized_action` of the action itself (a 403). The account
//!   guards therefore always win over the permission check.
//! * `index` answers `{"sis_imports": [...]}` and is paginated the way
//!   `Api.paginate` paginates (`per_page` defaults to 10, is capped at 100,
//!   `page` defaults to 1). `importing` answers the same envelope but is not
//!   paginated, exactly like Canvas.
//! * `show` answers the bare import object (no envelope).
//! * `abort_all_pending` answers `{"aborted": true}`.
//! * `restore_states` answers a bare `Progress` object
//!   (`Api::V1::Progress#progress_json`).
//! * Canvas stores `sis_batches.data`, `sis_batches.options`,
//!   `processing_errors` and `processing_warnings` as serialized Ruby hashes.
//!   star-lms keeps the `text` columns and writes/reads JSON in them; a column
//!   that does not hold JSON is read as `{}` / `[]`.
//! * Canvas renders the account guards with `{"errors": ["..."]}`, the
//!   permission failure with
//!   `{"status": "unauthorized", "errors": [{"message": "user not authorized to perform that action"}]}`
//!   and a missing record with
//!   `{"errors": [{"message": "The specified resource does not exist."}]}`.
//!   Those bodies are reproduced verbatim.
//! * Canvas distinguishes `:import_sis` (create) from `:manage_sis`
//!   (restore_states); both are account level rights that star-lms models with
//!   the same account admin check, so the two routes share one guard.
//! * star-lms has no SIS import worker. `create` therefore stores the batch
//!   (and its attachment) but does not run the CSV parser; the batch stays in
//!   the `created` state, exactly like a Canvas batch whose job has not been
//!   picked up yet.
//! * The `pre_attachment` upload flow needs Canvas's file upload subsystem
//!   (an S3 presigned post), which star-lms does not have. The branch is
//!   reproduced (the batch is created in the `initializing` state and the
//!   response carries a `pre_attachment` object) but `upload_url` is `null`.
//!
//! Required DDL — the coordinator has to add this to a migration:
//!
//! ```sql
//! ALTER TABLE accounts
//!     ADD COLUMN IF NOT EXISTS allow_sis_import BOOLEAN NOT NULL DEFAULT false;
//! UPDATE accounts SET allow_sis_import = true WHERE parent_account_id IS NULL;
//! ```
//!
//! Canvas enables SIS imports on every root account it creates
//! (`Account#enable_sis_imports`, `app/models/account.rb`), so the root account
//! of an existing installation has to be switched on as well.
//!
//! `sis_batches`, `sis_batch_errors`, `sis_batch_roll_back_data`, `attachments`,
//! `enrollment_terms`, `pseudonyms`, `progress` and `jobs` already exist
//! (`star-db/migrations/0223_canvas_schema_tables_a.sql`,
//! `0224_canvas_schema_tables_b.sql`, `0225_canvas_schema_tables_c.sql`,
//! `0127_progress.sql`, `0033_jobs.sql`).
//!
//! Required crate dependencies — the coordinator has to add these to
//! `crates/star-sis/Cargo.toml` (and refresh `Cargo.lock`, CI lints with
//! `--locked`):
//!
//! ```toml
//! star-auth = { path = "../star-auth" }
//! sqlx = { workspace = true }
//! serde = { workspace = true }
//! serde_json = { workspace = true }
//! chrono = { workspace = true }
//! ```

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, RawQuery, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `Api::PER_PAGE`.
const PER_PAGE: i64 = 10;

/// Canvas `Api::MAX_PER_PAGE`.
const MAX_PER_PAGE: i64 = 100;

/// The `workflow_state` values `index` accepts as a filter (the list of the
/// Canvas controller).
const VALID_WORKFLOW_STATES: [&str; 12] = [
    "initializing",
    "created",
    "importing",
    "cleanup_batch",
    "imported",
    "imported_with_messages",
    "aborted",
    "failed",
    "failed_with_messages",
    "restoring",
    "partially_restored",
    "restored",
];

/// Canvas `SIS::CSV::DiffGenerator::VALID_ENROLLMENT_DROP_STATUS`.
const VALID_ENROLLMENT_DROP_STATUS: [&str; 4] =
    ["deleted", "inactive", "completed", "deleted_last_completed"];

/// Canvas `SIS::CSV::DiffGenerator::VALID_USER_REMOVE_STATUS`.
const VALID_USER_REMOVE_STATUS: [&str; 2] = ["deleted", "suspended"];

/// The query parameters Canvas leaves out of the pagination links.
const EXCLUDED_IN_PAGINATION_LINKS: [&str; 4] = ["page", "per_page", "access_token", "api_key"];

/// The columns of a `sis_batches` row the API renders.
const BATCH_COLUMNS: &str = "id, account_id, workflow_state, data, created_at, updated_at, \
     started_at, ended_at, progress, batch_mode, batch_mode_term_id, options, user_id, \
     diffing_data_set_identifier, diffing_remaster, errors_attachment_id, change_threshold, \
     diffing_threshold_exceeded, processing_errors, processing_warnings";

/// Domain router for the Canvas SIS imports routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{account_id}/sis_imports",
            get(index).post(create),
        )
        .route(
            "/accounts/{account_id}/sis_imports/importing",
            get(importing),
        )
        .route(
            "/accounts/{account_id}/sis_imports/abort_all_pending",
            put(abort_all_pending),
        )
        .route("/accounts/{account_id}/sis_imports/{id}", get(show))
        .route("/accounts/{account_id}/sis_imports/{id}/abort", put(abort))
        .route(
            "/accounts/{account_id}/sis_imports/{id}/restore_states",
            put(restore_states),
        )
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A Canvas shaped error response.
struct SisError {
    status: StatusCode,
    body: Value,
}

impl SisError {
    /// Canvas `api_error_json` for `ActiveRecord::RecordNotFound`.
    fn not_found() -> Self {
        SisError {
            status: StatusCode::NOT_FOUND,
            body: json!({ "errors": [{ "message": "The specified resource does not exist." }] }),
        }
    }

    /// Canvas `render_json_unauthorized` for a signed in user without the right.
    fn forbidden() -> Self {
        SisError {
            status: StatusCode::FORBIDDEN,
            body: json!({
                "status": "unauthorized",
                "errors": [{ "message": "user not authorized to perform that action" }]
            }),
        }
    }

    /// Canvas `render_json_unauthorized` for an anonymous caller.
    fn unauthorized() -> Self {
        SisError {
            status: StatusCode::UNAUTHORIZED,
            body: json!({
                "status": "unauthenticated",
                "errors": [{ "message": "user authorization required" }]
            }),
        }
    }

    /// Canvas `api_error_json` for an unhandled exception.
    fn internal() -> Self {
        SisError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: json!({
                "errors": [{
                    "message": "An error occurred.",
                    "error_code": "internal_server_error"
                }]
            }),
        }
    }

    /// A 400 with the given Canvas body.
    fn bad_request(body: Value) -> Self {
        SisError {
            status: StatusCode::BAD_REQUEST,
            body,
        }
    }

    /// A 403 with the given Canvas body (`check_account`).
    fn forbidden_body(body: Value) -> Self {
        SisError {
            status: StatusCode::FORBIDDEN,
            body,
        }
    }

    /// A 422 with the given Canvas body (`Api::Errors::Reporter.to_hash`).
    fn unprocessable(body: Value) -> Self {
        SisError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            body,
        }
    }
}

impl IntoResponse for SisError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<ApiError> for SisError {
    fn from(error: ApiError) -> Self {
        match error {
            ApiError::NotFound => SisError::not_found(),
            ApiError::Forbidden => SisError::forbidden(),
            ApiError::Unauthorized => SisError::unauthorized(),
            _ => SisError::internal(),
        }
    }
}

impl From<sqlx::Error> for SisError {
    fn from(error: sqlx::Error) -> Self {
        eprintln!("database error: {error}");
        SisError::internal()
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One `sis_batches` row.
#[derive(Debug, sqlx::FromRow)]
struct SisBatchRow {
    id: i64,
    account_id: i64,
    workflow_state: String,
    data: Option<String>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
    started_at: Option<NaiveDateTime>,
    ended_at: Option<NaiveDateTime>,
    progress: Option<i64>,
    batch_mode: Option<bool>,
    batch_mode_term_id: Option<i64>,
    options: Option<String>,
    user_id: Option<i64>,
    diffing_data_set_identifier: Option<String>,
    diffing_remaster: Option<bool>,
    errors_attachment_id: Option<i64>,
    change_threshold: Option<i64>,
    diffing_threshold_exceeded: bool,
    processing_errors: Option<String>,
    processing_warnings: Option<String>,
}

/// One `attachments` row.
#[derive(Debug, sqlx::FromRow)]
struct AttachmentRow {
    id: i64,
    folder_id: Option<i64>,
    display_name: Option<String>,
    filename: Option<String>,
    uuid: Option<String>,
    content_type: Option<String>,
    size: Option<i64>,
    created_at: Option<NaiveDateTime>,
    updated_at: Option<NaiveDateTime>,
    unlock_at: Option<NaiveDateTime>,
    locked: Option<bool>,
    lock_at: Option<NaiveDateTime>,
    modified_at: Option<NaiveDateTime>,
    media_entry_id: Option<String>,
    category: String,
}

/// The batch user with the pseudonym of the root account.
#[derive(Debug, sqlx::FromRow)]
struct SisUserRow {
    id: i64,
    name: String,
    created_at: DateTime<Utc>,
    sortable_name: Option<String>,
    sis_user_id: Option<String>,
    integration_id: Option<String>,
    sis_batch_id: Option<i64>,
    unique_id: Option<String>,
}

/// One `progress` row.
#[derive(Debug, sqlx::FromRow)]
struct ProgressRow {
    id: i64,
    context_id: Option<i64>,
    context_type: Option<String>,
    user_id: Option<i64>,
    tag: String,
    completion: i64,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    message: Option<String>,
}

/// The account the request runs against.
#[derive(Debug, sqlx::FromRow)]
struct AccountRow {
    id: i64,
    parent_account_id: Option<i64>,
    allow_sis_import: bool,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Canvas `SisImportsApiController#index`.
async fn index(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Response, SisError> {
    let account_id = parse_id(&account_id)?;
    let account = load_account(&state.db, account_id).await?;
    check_account(&account)?;
    authorize_sis(&state.db, &auth, account_id).await?;

    let params = Params::parse(raw_query.as_deref());
    let created_since = params.get("created_since").and_then(parse_canvas_time);
    let created_before = params.get("created_before").and_then(parse_canvas_time);
    let requested_states = params.get_all("workflow_state");
    let states: Vec<String> = requested_states
        .into_iter()
        .filter(|state| VALID_WORKFLOW_STATES.contains(&state.as_str()))
        .collect();
    let states_filter = if states.is_empty() {
        None
    } else {
        Some(states)
    };

    let per_page = match params.get("per_page") {
        Some(raw) => ruby_to_i(raw).clamp(1, MAX_PER_PAGE),
        None => PER_PAGE,
    };
    let page = params.get("page").map(ruby_to_i).unwrap_or(0).max(1);
    let offset = (page - 1) * per_page;

    let batches = fetch_batches(
        &state.db,
        account_id,
        created_since,
        created_before,
        states_filter.clone(),
        per_page,
        offset,
    )
    .await?;

    let mut imports = Vec::with_capacity(batches.len());
    for batch in &batches {
        imports.push(sis_import_json(&state.db, batch, None).await?);
    }

    let total = count_batches(
        &state.db,
        account_id,
        created_since,
        created_before,
        states_filter,
    )
    .await?;

    let body = json!({ "sis_imports": imports });
    let link = pagination_link(
        &format!("/api/v1/accounts/{account_id}/sis_imports"),
        &params,
        page,
        per_page,
        total,
    );
    Ok(json_response(body, link))
}

/// Canvas `SisImportsApiController#importing`.
async fn importing(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    auth: AuthUser,
) -> Result<Response, SisError> {
    let account_id = parse_id(&account_id)?;
    let account = load_account(&state.db, account_id).await?;
    check_account(&account)?;
    authorize_sis(&state.db, &auth, account_id).await?;

    let batches = sqlx::query_as::<_, SisBatchRow>(&format!(
        "SELECT {BATCH_COLUMNS} FROM sis_batches \
         WHERE account_id = $1 AND workflow_state IN ('importing', 'cleanup_batch')"
    ))
    .bind(account_id)
    .fetch_all(&state.db)
    .await?;

    let mut imports = Vec::with_capacity(batches.len());
    for batch in &batches {
        imports.push(sis_import_json(&state.db, batch, None).await?);
    }

    Ok(json_response(json!({ "sis_imports": imports }), None))
}

/// Canvas `SisImportsApiController#show`.
async fn show(
    State(state): State<AppState>,
    Path((account_id, batch_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Response, SisError> {
    let account_id = parse_id(&account_id)?;
    let batch_id = parse_id(&batch_id)?;
    let account = load_account(&state.db, account_id).await?;
    check_account(&account)?;
    authorize_sis(&state.db, &auth, account_id).await?;

    let batch = find_batch(&state.db, account_id, batch_id).await?;
    let body = sis_import_json(&state.db, &batch, None).await?;
    Ok(json_response(body, None))
}

/// Canvas `SisImportsApiController#abort`.
async fn abort(
    State(state): State<AppState>,
    Path((account_id, batch_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Response, SisError> {
    let account_id = parse_id(&account_id)?;
    let batch_id = parse_id(&batch_id)?;
    let account = load_account(&state.db, account_id).await?;
    check_account(&account)?;
    authorize_sis(&state.db, &auth, account_id).await?;

    let mut tx = state.db.begin().await?;
    let batch = sqlx::query_as::<_, SisBatchRow>(&format!(
        "SELECT {BATCH_COLUMNS} FROM sis_batches \
         WHERE id = $1 AND account_id = $2 \
         AND workflow_state IN ('initializing', 'created', 'importing', 'cleanup_batch') \
         FOR UPDATE"
    ))
    .bind(batch_id)
    .bind(account_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(SisError::not_found)?;

    // Canvas `SisBatch#abort_batch`: the state is flipped with an `update_all`
    // that leaves the progress untouched.
    sqlx::query(
        "UPDATE sis_batches SET workflow_state = 'aborted', updated_at = now() \
         WHERE id = $1 AND workflow_state IN ('initializing', 'created', 'importing', 'cleanup_batch')",
    )
    .bind(batch.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let batch = find_batch(&state.db, account_id, batch.id).await?;
    let body = sis_import_json(&state.db, &batch, None).await?;
    Ok(json_response(body, None))
}

/// Canvas `SisImportsApiController#abort_all_pending`.
async fn abort_all_pending(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    auth: AuthUser,
) -> Result<Response, SisError> {
    let account_id = parse_id(&account_id)?;
    let account = load_account(&state.db, account_id).await?;
    check_account(&account)?;
    authorize_sis(&state.db, &auth, account_id).await?;

    // Canvas `SisBatch.abort_all_pending_for_account`: the `update_all` does
    // not touch `updated_at`.
    sqlx::query(
        "UPDATE sis_batches SET workflow_state = 'aborted', progress = 100 \
         WHERE account_id = $1 AND workflow_state IN ('initializing', 'created')",
    )
    .bind(account_id)
    .execute(&state.db)
    .await?;

    Ok(json_response(json!({ "aborted": true }), None))
}

/// Canvas `SisImportsApiController#restore_states`.
async fn restore_states(
    State(state): State<AppState>,
    Path((account_id, batch_id)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Response, SisError> {
    let account_id = parse_id(&account_id)?;
    let batch_id = parse_id(&batch_id)?;
    let account = load_account(&state.db, account_id).await?;
    check_account(&account)?;
    authorize_sis(&state.db, &auth, account_id).await?;

    let batch = find_batch(&state.db, account_id, batch_id).await?;

    // Canvas `@batch.roll_back_data.not_expired.exists?`
    let has_roll_back: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sis_batch_roll_back_data \
         WHERE sis_batch_id = $1 AND created_at >= now() - interval '30 days')",
    )
    .bind(batch.id)
    .fetch_one(&state.db)
    .await?;
    if !has_roll_back {
        return Err(SisError::bad_request(
            json!({ "message": "restore data unavailable" }),
        ));
    }

    let params = Params::parse(raw_query.as_deref());
    let undelete_only = value_to_boolean(params.get("undelete_only"));
    let unconclude_only = value_to_boolean(params.get("unconclude_only"));
    if undelete_only && unconclude_only {
        return Err(SisError::bad_request(
            json!({ "message": "cannot set both undelete_only and unconclude_only" }),
        ));
    }

    // Canvas `SisBatch#restore_states_later` creates the progress row; the
    // restore itself runs in the background job.
    let progress = sqlx::query_as::<_, ProgressRow>(
        "INSERT INTO progress (tag, workflow_state, completion, context_type, context_id, \
         created_at, updated_at) \
         VALUES ('sis_batch_state_restore', 'queued', 0, 'SisBatch', $1, now(), now()) \
         RETURNING id, context_id, context_type, user_id, tag, completion, workflow_state, \
         created_at, updated_at, message",
    )
    .bind(batch.id)
    .fetch_one(&state.db)
    .await?;

    Ok(json_response(progress_json(&progress), None))
}

/// Canvas `SisImportsApiController#create`.
async fn create(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, SisError> {
    let account_id = parse_id(&account_id)?;
    let account = load_account(&state.db, account_id).await?;
    check_account(&account)?;
    authorize_sis(&state.db, &auth, account_id).await?;

    let content_type_header = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let (media_type, charset) = split_content_type(&content_type_header);
    let is_multipart = media_type == "multipart/form-data";

    let mut params = Params::default();
    let mut uploaded: Option<UploadedFile> = None;
    if is_multipart {
        let boundary = content_type_header
            .split(';')
            .filter_map(|part| part.trim().strip_prefix("boundary="))
            .map(|value| value.trim_matches('"').to_string())
            .next();
        let Some(boundary) = boundary else {
            return Err(SisError::bad_request(json!({
                "error": true,
                "error_message": "Missing attachment"
            })));
        };
        for part in parse_multipart(&body, &boundary) {
            if part.filename.is_some() {
                if part.name == "attachment" {
                    uploaded = Some(UploadedFile {
                        filename: part.filename.clone().unwrap_or_default(),
                        content_type: part.content_type.clone(),
                        data: part.data,
                    });
                }
                continue;
            }
            params.push(&part.name, &String::from_utf8_lossy(&part.data));
        }
    }
    // Rails merges the query parameters over the request body parameters.
    for (key, value) in Params::parse(raw_query.as_deref()).pairs {
        params.push(&key, &value);
    }

    // Canvas: `params[:import_type] ||= "instructure_csv"`.
    let import_type = match params.get("import_type") {
        Some(value) => value.to_string(),
        None => "instructure_csv".to_string(),
    };
    if import_type != "instructure_csv" {
        // Canvas raises `invalid import type parameter`, which the API
        // renders as a 500.
        return Err(SisError::internal());
    }

    // Canvas resolves the uploaded file before it creates the batch.
    let has_pre_attachment = params.has("pre_attachment");
    let has_attachment = params.has("attachment") || uploaded.is_some();
    let mut file: Option<FileSpec> = None;
    let mut pre_attachment: Option<Value> = None;
    if has_pre_attachment {
        if has_attachment {
            return Err(SisError::bad_request(json!({
                "error": true,
                "error_message": "Cannot specify both attachment and pre_attachment"
            })));
        }
        pre_attachment = Some(json!({
            "upload_url": Value::Null,
            "upload_params": json!({}),
            "file_param": "file"
        }));
    } else if has_attachment {
        let Some(upload) = uploaded else {
            // Canvas would call `original_filename` on a plain parameter and
            // blow up with a 500.
            return Err(SisError::internal());
        };
        file = Some(FileSpec {
            filename: upload.filename,
            content_type: upload.content_type,
            size: upload.data.len() as i64,
        });
    } else if is_multipart {
        return Err(SisError::bad_request(json!({
            "error": true,
            "error_message": "Missing attachment"
        })));
    } else {
        // A raw post: the body is the SIS data.
        let (filename, file_content_type) = match params.get("extension") {
            Some(extension) => {
                let extension = extension.trim();
                (
                    format!("sis_import.{extension}"),
                    mimetype_for_extension(extension),
                )
            }
            None => {
                if let Some(charset) = &charset {
                    if !charset.eq_ignore_ascii_case("utf-8") {
                        return Err(SisError::bad_request(json!({
                            "error": "Invalid content type, UTF-8 required"
                        })));
                    }
                }
                let extension = match media_type.as_str() {
                    "application/zip" => "zip",
                    "text/xml" => "xml",
                    "text/plain" => "csv",
                    "text/csv" => "csv",
                    _ => "zip",
                };
                (format!("sis_import.{extension}"), Some(media_type.clone()))
            }
        };
        file = Some(FileSpec {
            filename,
            content_type: file_content_type,
            size: body.len() as i64,
        });
    }

    // Canvas looks the batch mode term up before it creates the batch.
    let mut batch_mode_term = None;
    if value_to_boolean(params.get("batch_mode")) {
        if params.present("batch_mode_term_id") {
            batch_mode_term = find_enrollment_term(
                &state.db,
                account_id,
                params.get("batch_mode_term_id").unwrap_or_default(),
            )
            .await?;
        }
        if batch_mode_term.is_none() && !params.has("multi_term_batch_mode") {
            return Err(SisError::bad_request(json!({
                "message": "Batch mode specified, but the given batch_mode_term_id cannot be found."
            })));
        }
    }

    // Canvas `SisBatch.create_with_attachment` saves the batch before the
    // block runs, so a validation failure inside the block leaves the batch
    // behind in the `initializing` state.
    let data = json!({ "import_type": import_type });
    let batch_id: i64 = sqlx::query_scalar(
        "INSERT INTO sis_batches (account_id, workflow_state, data, progress, user_id, \
         created_at, updated_at) \
         VALUES ($1, 'initializing', $2, 0, $3, now(), now()) RETURNING id",
    )
    .bind(account_id)
    .bind(serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string()))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    let mut attachment_id = None;
    if let Some(file) = file {
        let id = create_attachment(
            &state.db,
            batch_id,
            account.id,
            auth.id,
            &file.filename,
            file.content_type.as_deref(),
            file.size,
        )
        .await?;
        sqlx::query("UPDATE sis_batches SET attachment_id = $1 WHERE id = $2")
            .bind(id)
            .bind(batch_id)
            .execute(&state.db)
            .await?;
        attachment_id = Some(id);
    }

    let mut options = Map::new();
    let change_threshold = match params.get("change_threshold") {
        Some(raw) if !raw.trim().is_empty() => Some(ruby_to_i(raw)),
        _ => None,
    };
    if let Some(raw) = params.get("diff_row_count_threshold") {
        let threshold = ruby_to_i(raw);
        if threshold > 0 {
            options.insert("diff_row_count_threshold".to_string(), json!(threshold));
        }
    }

    let mut batch_mode = None;
    let mut batch_mode_term_id = None;
    let mut diffing_data_set_identifier = None;
    let mut diffing_remaster = None;

    if let Some(term_id) = batch_mode_term {
        batch_mode = Some(true);
        batch_mode_term_id = Some(term_id);
    } else if params.has("multi_term_batch_mode") {
        batch_mode = Some(true);
        options.insert(
            "multi_term_batch_mode".to_string(),
            json!(value_to_boolean(params.get("multi_term_batch_mode"))),
        );
        if change_threshold.is_none() {
            return Err(SisError::bad_request(json!({
                "message": "change_threshold is required to use multi term_batch mode."
            })));
        }
    } else if params.present("diffing_data_set_identifier") {
        // Canvas `SisBatch#enable_diffing` only diffs `instructure_csv`.
        diffing_data_set_identifier = Some(
            params
                .get("diffing_data_set_identifier")
                .unwrap_or_default()
                .to_string(),
        );
        diffing_remaster = Some(value_to_boolean(params.get("diffing_remaster_data_set")));
    }

    options.insert(
        "skip_deletes".to_string(),
        json!(value_to_boolean(params.get("skip_deletes"))),
    );
    options.insert(
        "update_sis_id_if_login_claimed".to_string(),
        json!(value_to_boolean(
            params.get("update_sis_id_if_login_claimed")
        )),
    );
    if value_to_boolean(params.get("override_sis_stickiness")) {
        options.insert("override_sis_stickiness".to_string(), json!(true));
        if value_to_boolean(params.get("add_sis_stickiness")) {
            options.insert("add_sis_stickiness".to_string(), json!(true));
        }
        if value_to_boolean(params.get("clear_sis_stickiness")) {
            options.insert("clear_sis_stickiness".to_string(), json!(true));
        }
    }
    if params.present("diffing_drop_status") {
        match first_valid(
            params.get_all("diffing_drop_status"),
            &VALID_ENROLLMENT_DROP_STATUS,
        ) {
            Some(value) => {
                options.insert("diffing_drop_status".to_string(), json!(value));
            }
            None => {
                return Err(SisError::bad_request(
                    json!({ "message": "Invalid diffing_drop_status" }),
                ));
            }
        }
    }
    if params.present("diffing_user_remove_status") {
        match first_valid(
            params.get_all("diffing_user_remove_status"),
            &VALID_USER_REMOVE_STATUS,
        ) {
            Some(value) => {
                options.insert("diffing_user_remove_status".to_string(), json!(value));
            }
            None => {
                return Err(SisError::bad_request(
                    json!({ "message": "Invalid diffing_user_remove_status" }),
                ));
            }
        }
    }
    if params.present("batch_mode_enrollment_drop_status") {
        match first_valid(
            params.get_all("batch_mode_enrollment_drop_status"),
            &VALID_ENROLLMENT_DROP_STATUS,
        ) {
            Some(value) => {
                options.insert(
                    "batch_mode_enrollment_drop_status".to_string(),
                    json!(value),
                );
            }
            None => {
                return Err(SisError::bad_request(
                    json!({ "message": "Invalid batch_mode_enrollment_drop_status" }),
                ));
            }
        }
    }

    // Canvas validates the length of `diffing_data_set_identifier` when the
    // batch is saved (`save!`), which raises a 422.
    if let Some(identifier) = &diffing_data_set_identifier {
        if identifier.chars().count() > 128 {
            return Err(SisError::unprocessable(json!({
                "errors": [{
                    "field": "diffing_data_set_identifier",
                    "message": "is too long (maximum is 128 characters)",
                    "error_code": "too_long"
                }]
            })));
        }
    }

    let options_text =
        serde_json::to_string(&Value::Object(options)).unwrap_or_else(|_| "{}".to_string());
    let workflow_state = if attachment_id.is_some() {
        "created"
    } else {
        "initializing"
    };
    sqlx::query(
        "UPDATE sis_batches SET options = $1, batch_mode = $2, batch_mode_term_id = $3, \
         change_threshold = $4, diffing_data_set_identifier = $5, diffing_remaster = $6, \
         workflow_state = $7 WHERE id = $8",
    )
    .bind(options_text)
    .bind(batch_mode)
    .bind(batch_mode_term_id)
    .bind(change_threshold)
    .bind(&diffing_data_set_identifier)
    .bind(diffing_remaster)
    .bind(workflow_state)
    .bind(batch_id)
    .execute(&state.db)
    .await?;

    if attachment_id.is_some() {
        // Canvas enqueues the import job here. star-lms has no SIS import
        // worker, so the job row is the only trace of it.
        let _ = sqlx::query(
            "INSERT INTO jobs (tag, workflow_state, progress, user_id, context_type, context_id) \
             VALUES ('SisBatch#process_instructure_csv_zip', 'queued', 0, $1, 'SisBatch', $2)",
        )
        .bind(auth.id)
        .bind(batch_id)
        .execute(&state.db)
        .await;
    }

    let batch = find_batch(&state.db, account_id, batch_id).await?;
    let body = sis_import_json(&state.db, &batch, pre_attachment).await?;
    Ok(json_response(body, None))
}

// ---------------------------------------------------------------------------
// Permissions and account guards
// ---------------------------------------------------------------------------

/// Canvas `api_find(Account.active, params[:account_id])`.
///
/// Canvas casts the parameter to an integer, so an id that does not exist (or
/// is not a number at all) is a 404.
async fn load_account(pool: &PgPool, account_id: i64) -> Result<AccountRow, SisError> {
    sqlx::query_as::<_, AccountRow>(
        "SELECT id, parent_account_id, allow_sis_import FROM accounts \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(SisError::not_found)
}

/// Canvas `SisImportsApiController#check_account`.
fn check_account(account: &AccountRow) -> Result<(), SisError> {
    if account.parent_account_id.is_some() {
        return Err(SisError::bad_request(
            json!({ "errors": ["SIS imports can only be executed on root accounts"] }),
        ));
    }
    if !account.allow_sis_import {
        return Err(SisError::forbidden_body(json!({
            "errors": ["SIS imports are not enabled for this account"]
        })));
    }
    Ok(())
}

/// Canvas `authorized_action(@account, @current_user, [:import_sis, :manage_sis])`.
///
/// `:import_sis` and `:manage_sis` are account rights that only account admins
/// (and site admins) hold, so one check covers every route of this controller.
async fn authorize_sis(pool: &PgPool, auth: &AuthUser, account_id: i64) -> Result<(), SisError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let is_admin: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM account_users \
         WHERE account_id = $1 AND user_id = $2 AND role = 'admin' \
         AND workflow_state <> 'deleted')",
    )
    .bind(account_id)
    .bind(auth.id)
    .fetch_one(pool)
    .await?;
    if is_admin {
        Ok(())
    } else {
        Err(SisError::forbidden())
    }
}

// ---------------------------------------------------------------------------
// Database access
// ---------------------------------------------------------------------------

/// Canvas `@account.sis_batches.find(params[:id])`.
async fn find_batch(
    pool: &PgPool,
    account_id: i64,
    batch_id: i64,
) -> Result<SisBatchRow, SisError> {
    sqlx::query_as::<_, SisBatchRow>(&format!(
        "SELECT {BATCH_COLUMNS} FROM sis_batches WHERE id = $1 AND account_id = $2"
    ))
    .bind(batch_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(SisError::not_found)
}

/// Canvas `api_find(@account.enrollment_terms.active, params[:batch_mode_term_id])`.
async fn find_enrollment_term(
    pool: &PgPool,
    account_id: i64,
    raw_id: &str,
) -> Result<Option<i64>, SisError> {
    let Ok(term_id) = raw_id.parse::<i64>() else {
        return Ok(None);
    };
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM enrollment_terms \
         WHERE id = $1 AND root_account_id = $2 AND workflow_state = 'active'",
    )
    .bind(term_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    Ok(found)
}

/// The `index` query: the batches of the account, newest first.
async fn fetch_batches(
    pool: &PgPool,
    account_id: i64,
    created_since: Option<NaiveDateTime>,
    created_before: Option<NaiveDateTime>,
    states: Option<Vec<String>>,
    per_page: i64,
    offset: i64,
) -> Result<Vec<SisBatchRow>, SisError> {
    sqlx::query_as::<_, SisBatchRow>(&format!(
        "SELECT {BATCH_COLUMNS} FROM sis_batches \
         WHERE account_id = $1 \
         AND ($2::timestamp IS NULL OR created_at > $2) \
         AND ($3::timestamp IS NULL OR created_at < $3) \
         AND ($4::text[] IS NULL OR workflow_state = ANY($4)) \
         ORDER BY created_at DESC LIMIT $5 OFFSET $6"
    ))
    .bind(account_id)
    .bind(created_since)
    .bind(created_before)
    .bind(states)
    .bind(per_page)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(SisError::from)
}

/// The number of batches the `index` query matches.
async fn count_batches(
    pool: &PgPool,
    account_id: i64,
    created_since: Option<NaiveDateTime>,
    created_before: Option<NaiveDateTime>,
    states: Option<Vec<String>>,
) -> Result<i64, SisError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sis_batches \
         WHERE account_id = $1 \
         AND ($2::timestamp IS NULL OR created_at > $2) \
         AND ($3::timestamp IS NULL OR created_at < $3) \
         AND ($4::text[] IS NULL OR workflow_state = ANY($4))",
    )
    .bind(account_id)
    .bind(created_since)
    .bind(created_before)
    .bind(states)
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Creates the `attachments` row of an uploaded SIS file.
///
/// Canvas `Attachment.create_data_attachment(batch, file_obj, filename)` hangs
/// the attachment on the batch.
async fn create_attachment(
    pool: &PgPool,
    batch_id: i64,
    root_account_id: i64,
    user_id: i64,
    filename: &str,
    content_type: Option<&str>,
    size: i64,
) -> Result<i64, SisError> {
    let uuid = pseudo_uuid();
    let attachment_id: i64 = sqlx::query_scalar(
        "INSERT INTO attachments (context_id, context_type, size, content_type, filename, uuid, \
         display_name, created_at, updated_at, modified_at, workflow_state, file_state, \
         user_id, root_account_id, category) \
         VALUES ($1, 'SisBatch', $2, $3, $4, $5, $4, now(), now(), now(), 'available', \
         'available', $6, $7, 'uncategorized') RETURNING id",
    )
    .bind(batch_id)
    .bind(size)
    .bind(content_type)
    .bind(filename)
    .bind(uuid)
    .bind(user_id)
    .bind(root_account_id)
    .fetch_one(pool)
    .await?;
    Ok(attachment_id)
}

/// The `attachments` rows of a batch that are listed in
/// `data[:downloadable_attachment_ids]`.
async fn downloadable_attachments(
    pool: &PgPool,
    batch: &SisBatchRow,
    ids: &[i64],
) -> Result<Vec<AttachmentRow>, SisError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as::<_, AttachmentRow>(
        "SELECT id, folder_id, display_name, filename, uuid, content_type, size, created_at, \
         updated_at, unlock_at, locked, lock_at, modified_at, media_entry_id, category \
         FROM attachments WHERE id = ANY($1) AND context_type = 'SisBatch' AND context_id = $2",
    )
    .bind(ids.to_vec())
    .bind(batch.id)
    .fetch_all(pool)
    .await
    .map_err(SisError::from)
}

/// The user of the batch with the pseudonym of the root account.
async fn load_batch_user(
    pool: &PgPool,
    account_id: i64,
    user_id: i64,
) -> Result<Option<SisUserRow>, SisError> {
    let row = sqlx::query_as::<_, SisUserRow>(
        "SELECT u.id, u.name, u.created_at, u.sortable_name, p.sis_user_id, p.integration_id, \
         p.sis_batch_id, p.unique_id \
         FROM users u \
         LEFT JOIN LATERAL (SELECT * FROM pseudonyms WHERE user_id = u.id AND account_id = $2 \
         ORDER BY id LIMIT 1) p ON TRUE \
         WHERE u.id = $1",
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// One `attachments` row by id.
async fn load_attachment(
    pool: &PgPool,
    attachment_id: i64,
) -> Result<Option<AttachmentRow>, SisError> {
    let row = sqlx::query_as::<_, AttachmentRow>(
        "SELECT id, folder_id, display_name, filename, uuid, content_type, size, created_at, \
         updated_at, unlock_at, locked, lock_at, modified_at, media_entry_id, category \
         FROM attachments WHERE id = $1",
    )
    .bind(attachment_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::SisImport#sis_import_json`.
async fn sis_import_json(
    pool: &PgPool,
    batch: &SisBatchRow,
    pre_attachment: Option<Value>,
) -> Result<Value, SisError> {
    let data = parse_json_object(batch.data.as_deref());
    let options = parse_json_object(batch.options.as_deref());
    let downloadable_ids = id_list(data.get("downloadable_attachment_ids"));
    let diffed_ids = id_list(data.get("diffed_attachment_ids"));

    let mut json = Map::new();
    json.insert("id".to_string(), json!(batch.id));
    json.insert("created_at".to_string(), json!(iso8601(&batch.created_at)));
    json.insert(
        "started_at".to_string(),
        json!(batch.started_at.map(|value| iso8601(&value))),
    );
    json.insert(
        "ended_at".to_string(),
        json!(batch.ended_at.map(|value| iso8601(&value))),
    );
    json.insert("updated_at".to_string(), json!(iso8601(&batch.updated_at)));
    json.insert(
        "progress".to_string(),
        json!(batch.progress.map(|value| value as f64)),
    );
    json.insert("workflow_state".to_string(), json!(batch.workflow_state));
    json.insert("data".to_string(), Value::Object(data));
    json.insert("batch_mode".to_string(), json!(batch.batch_mode));
    json.insert(
        "batch_mode_term_id".to_string(),
        json!(batch.batch_mode_term_id),
    );
    json.insert(
        "multi_term_batch_mode".to_string(),
        options
            .get("multi_term_batch_mode")
            .cloned()
            .unwrap_or(Value::Null),
    );
    json.insert(
        "override_sis_stickiness".to_string(),
        options
            .get("override_sis_stickiness")
            .cloned()
            .unwrap_or(Value::Null),
    );
    json.insert(
        "add_sis_stickiness".to_string(),
        options
            .get("add_sis_stickiness")
            .cloned()
            .unwrap_or(Value::Null),
    );
    json.insert(
        "update_sis_id_if_login_claimed".to_string(),
        options
            .get("update_sis_id_if_login_claimed")
            .cloned()
            .unwrap_or(Value::Null),
    );
    json.insert(
        "clear_sis_stickiness".to_string(),
        options
            .get("clear_sis_stickiness")
            .cloned()
            .unwrap_or(Value::Null),
    );
    json.insert(
        "diffing_data_set_identifier".to_string(),
        json!(batch.diffing_data_set_identifier),
    );
    json.insert(
        "diffing_remaster".to_string(),
        json!(batch.diffing_remaster),
    );
    json.insert(
        "diffed_against_import_id".to_string(),
        options
            .get("diffed_against_sis_batch_id")
            .cloned()
            .unwrap_or(Value::Null),
    );
    json.insert(
        "diffing_drop_status".to_string(),
        options
            .get("diffing_drop_status")
            .cloned()
            .unwrap_or(Value::Null),
    );
    json.insert(
        "diffing_user_remove_status".to_string(),
        options
            .get("diffing_user_remove_status")
            .cloned()
            .unwrap_or(Value::Null),
    );
    json.insert(
        "skip_deletes".to_string(),
        options.get("skip_deletes").cloned().unwrap_or(Value::Null),
    );
    json.insert(
        "change_threshold".to_string(),
        json!(batch.change_threshold),
    );
    json.insert(
        "diff_row_count_threshold".to_string(),
        options
            .get("diff_row_count_threshold")
            .cloned()
            .unwrap_or(Value::Null),
    );

    if let Some(errors) = parse_json_array(batch.processing_errors.as_deref()) {
        if !errors.is_empty() {
            json.insert("processing_errors".to_string(), Value::Array(errors));
        }
    }
    if let Some(warnings) = parse_json_array(batch.processing_warnings.as_deref()) {
        if !warnings.is_empty() {
            json.insert("processing_warnings".to_string(), Value::Array(warnings));
        }
    }
    if batch.diffing_data_set_identifier.is_some() {
        json.insert(
            "diffing_threshold_exceeded".to_string(),
            json!(batch.diffing_threshold_exceeded),
        );
    }

    if let Some(errors_attachment_id) = batch.errors_attachment_id {
        if let Some(attachment) = load_attachment(pool, errors_attachment_id).await? {
            json.insert(
                "errors_attachment".to_string(),
                attachment_json(&attachment),
            );
        }
    }

    if let Some(user_id) = batch.user_id {
        if let Some(user) = load_batch_user(pool, batch.account_id, user_id).await? {
            json.insert("user".to_string(), user_json(&user));
        }
    }

    let attachments = downloadable_attachments(pool, batch, &downloadable_ids).await?;
    let uploaded: Vec<&AttachmentRow> = attachments
        .iter()
        .filter(|attachment| !diffed_ids.contains(&attachment.id))
        .collect();
    let diffed: Vec<&AttachmentRow> = attachments
        .iter()
        .filter(|attachment| diffed_ids.contains(&attachment.id))
        .collect();
    if !uploaded.is_empty() {
        json.insert(
            "csv_attachments".to_string(),
            Value::Array(uploaded.iter().map(|item| attachment_json(item)).collect()),
        );
    }
    if !diffed.is_empty() {
        json.insert(
            "diffed_csv_attachments".to_string(),
            Value::Array(diffed.iter().map(|item| attachment_json(item)).collect()),
        );
    }

    if let Some(pre_attachment) = pre_attachment {
        json.insert("pre_attachment".to_string(), pre_attachment);
    }

    Ok(Value::Object(json))
}

/// Canvas `Api::V1::User#user_json` for the default includes and excludes.
///
/// The caller of a SIS import route is always an account admin, so the SIS
/// fields of the user are part of the answer.
fn user_json(user: &SisUserRow) -> Value {
    let mut json = Map::new();
    json.insert("id".to_string(), json!(user.id));
    json.insert("name".to_string(), json!(user.name));
    json.insert(
        "created_at".to_string(),
        json!(iso8601_utc(&user.created_at)),
    );
    json.insert("sortable_name".to_string(), json!(user.sortable_name));
    // Canvas renders `short_name`; star-lms does not model the column.
    json.insert("short_name".to_string(), Value::Null);
    json.insert("sis_user_id".to_string(), json!(user.sis_user_id));
    json.insert("integration_id".to_string(), json!(user.integration_id));
    json.insert("sis_import_id".to_string(), json!(user.sis_batch_id));
    json.insert("login_id".to_string(), json!(user.unique_id));
    Value::Object(json)
}

/// Canvas `Api::V1::Attachment#attachment_json`.
fn attachment_json(attachment: &AttachmentRow) -> Value {
    let url = format!(
        "/files/{}/download?download=1&download_frd=1&verifier={}",
        attachment.id,
        attachment.uuid.as_deref().unwrap_or_default()
    );
    let mut json = Map::new();
    json.insert("id".to_string(), json!(attachment.id));
    json.insert("folder_id".to_string(), json!(attachment.folder_id));
    json.insert("display_name".to_string(), json!(attachment.display_name));
    json.insert("filename".to_string(), json!(attachment.filename));
    json.insert("uuid".to_string(), json!(attachment.uuid));
    json.insert("upload_status".to_string(), json!("success"));
    json.insert("content-type".to_string(), json!(attachment.content_type));
    json.insert("url".to_string(), json!(url));
    json.insert("size".to_string(), json!(attachment.size));
    json.insert(
        "created_at".to_string(),
        json!(attachment.created_at.map(|value| iso8601(&value))),
    );
    json.insert(
        "updated_at".to_string(),
        json!(attachment.updated_at.map(|value| iso8601(&value))),
    );
    json.insert(
        "unlock_at".to_string(),
        json!(attachment.unlock_at.map(|value| iso8601(&value))),
    );
    json.insert(
        "locked".to_string(),
        json!(attachment.locked.unwrap_or(false)),
    );
    // Canvas tracks `hidden` on the attachment; star-lms does not model it.
    json.insert("hidden".to_string(), json!(false));
    json.insert(
        "lock_at".to_string(),
        json!(attachment.lock_at.map(|value| iso8601(&value))),
    );
    json.insert("hidden_for_user".to_string(), json!(false));
    json.insert("thumbnail_url".to_string(), json!(""));
    json.insert(
        "modified_at".to_string(),
        json!(attachment
            .modified_at
            .or(attachment.updated_at)
            .map(|value| iso8601(&value))),
    );
    json.insert(
        "mime_class".to_string(),
        json!(mime_class(attachment.content_type.as_deref())),
    );
    json.insert(
        "media_entry_id".to_string(),
        json!(attachment.media_entry_id),
    );
    json.insert("category".to_string(), json!(attachment.category));
    json.insert("locked_for_user".to_string(), json!(false));
    Value::Object(json)
}

/// Canvas `Api::V1::Progress#progress_json`.
fn progress_json(progress: &ProgressRow) -> Value {
    let mut json = Map::new();
    json.insert("id".to_string(), json!(progress.id));
    json.insert("context_id".to_string(), json!(progress.context_id));
    json.insert("context_type".to_string(), json!(progress.context_type));
    json.insert("user_id".to_string(), json!(progress.user_id));
    json.insert("tag".to_string(), json!(progress.tag));
    json.insert("completion".to_string(), json!(progress.completion as f64));
    json.insert("workflow_state".to_string(), json!(progress.workflow_state));
    json.insert(
        "created_at".to_string(),
        json!(iso8601_utc(&progress.created_at)),
    );
    json.insert(
        "updated_at".to_string(),
        json!(iso8601_utc(&progress.updated_at)),
    );
    json.insert("message".to_string(), json!(progress.message));
    json.insert(
        "url".to_string(),
        json!(format!("/api/v1/progress/{}", progress.id)),
    );
    Value::Object(json)
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// A JSON response with an optional `Link` header.
fn json_response(body: Value, link: Option<String>) -> Response {
    let mut response = (StatusCode::OK, Json(body)).into_response();
    if let Some(link) = link {
        if let Ok(value) = HeaderValue::from_str(&link) {
            response.headers_mut().insert(header::LINK, value);
        }
    }
    response
}

/// The `Link` header Canvas builds in `Api.build_links_from_hash`.
fn pagination_link(
    base_url: &str,
    params: &Params,
    page: i64,
    per_page: i64,
    total: i64,
) -> Option<String> {
    let last_page = if per_page > 0 {
        ((total + per_page - 1) / per_page).max(1)
    } else {
        1
    };
    let mut base = base_url.to_string();
    base.push('?');
    let mut kept: Vec<&(String, String)> = params
        .pairs
        .iter()
        .filter(|(key, _)| !EXCLUDED_IN_PAGINATION_LINKS.contains(&key.as_str()))
        .collect();
    kept.sort_by(|left, right| left.0.cmp(&right.0));
    for (key, value) in kept {
        base.push_str(&url_encode(key));
        base.push('=');
        base.push_str(&url_encode(value));
        base.push('&');
    }
    let link = |page: i64| -> String { format!("{base}page={page}&per_page={per_page}") };

    let mut links: Vec<String> = Vec::new();
    links.push(format!("<{}>; rel=\"current\"", link(page)));
    if page < last_page {
        links.push(format!("<{}>; rel=\"next\"", link(page + 1)));
    }
    if page > 1 {
        links.push(format!("<{}>; rel=\"prev\"", link(page - 1)));
    }
    links.push(format!("<{}>; rel=\"first\"", link(1)));
    links.push(format!("<{}>; rel=\"last\"", link(last_page)));
    Some(links.join(","))
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// The request parameters of a route.
///
/// Canvas reads them from the query string and, for a multipart post, from the
/// form fields as well.
#[derive(Debug, Default)]
struct Params {
    pairs: Vec<(String, String)>,
}

impl Params {
    /// Parses a query string.
    fn parse(raw: Option<&str>) -> Self {
        let mut params = Params::default();
        let Some(raw) = raw else { return params };
        for pair in raw.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (url_decode(key), url_decode(value)),
                None => (url_decode(pair), String::new()),
            };
            params.pairs.push((key, value));
        }
        params
    }

    /// Adds a parameter.
    fn push(&mut self, key: &str, value: &str) {
        self.pairs.push((key.to_string(), value.to_string()));
    }

    /// The last value of a parameter.
    fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .rev()
            .find(|(name, _)| base_key(name) == key)
            .map(|(_, value)| value.as_str())
    }

    /// All values of a parameter (`params[:key]` may be an array).
    fn get_all(&self, key: &str) -> Vec<String> {
        self.pairs
            .iter()
            .filter(|(name, _)| base_key(name) == key)
            .map(|(_, value)| value.clone())
            .collect()
    }

    /// Whether the parameter was sent at all.
    fn has(&self, key: &str) -> bool {
        self.pairs.iter().any(|(name, _)| base_key(name) == key)
    }

    /// Whether the parameter was sent with a non blank value
    /// (Rails `params[:key].present?`).
    fn present(&self, key: &str) -> bool {
        self.get(key).is_some_and(|value| !value.trim().is_empty())
    }
}

/// The parameter name without the Rails array marker.
fn base_key(key: &str) -> &str {
    key.strip_suffix("[]").unwrap_or(key)
}

/// The first value that is in the list of valid values (Ruby `Array & list`).
fn first_valid(values: Vec<String>, valid: &[&str]) -> Option<String> {
    values
        .into_iter()
        .find(|value| valid.contains(&value.as_str()))
}

// ---------------------------------------------------------------------------
// Multipart
// ---------------------------------------------------------------------------

/// The file of a SIS import.
struct UploadedFile {
    filename: String,
    content_type: Option<String>,
    data: Vec<u8>,
}

/// The file a SIS import is created with.
struct FileSpec {
    filename: String,
    content_type: Option<String>,
    size: i64,
}

/// One part of a `multipart/form-data` body.
struct MultipartPart {
    name: String,
    filename: Option<String>,
    content_type: Option<String>,
    data: Vec<u8>,
}

/// A minimal `multipart/form-data` parser.
fn parse_multipart(body: &[u8], boundary: &str) -> Vec<MultipartPart> {
    let mut parts = Vec::new();
    let delimiter = format!("--{boundary}");
    let next_delimiter = format!("\r\n--{boundary}");
    let Some(start) = find_bytes(body, delimiter.as_bytes(), 0) else {
        return parts;
    };
    let mut cursor = start + delimiter.len();
    loop {
        // "--" marks the end of the body.
        if body.len() >= cursor + 2 && &body[cursor..cursor + 2] == b"--" {
            break;
        }
        if body.len() >= cursor + 2 && &body[cursor..cursor + 2] == b"\r\n" {
            cursor += 2;
        } else if body.len() > cursor && body[cursor] == b'\n' {
            cursor += 1;
        } else {
            break;
        }
        let Some(header_end) = find_bytes(body, b"\r\n\r\n", cursor) else {
            break;
        };
        let headers = String::from_utf8_lossy(&body[cursor..header_end]).to_string();
        let content_start = header_end + 4;
        let Some(next) = find_bytes(body, next_delimiter.as_bytes(), content_start) else {
            break;
        };
        let mut content_end = next;
        if content_end >= content_start + 2 && &body[content_end - 2..content_end] == b"\r\n" {
            content_end -= 2;
        }
        parts.push(build_multipart_part(
            &headers,
            &body[content_start..content_end],
        ));
        cursor = next + next_delimiter.len();
    }
    parts
}

/// Builds a part from its headers and its content.
fn build_multipart_part(headers: &str, data: &[u8]) -> MultipartPart {
    let mut name = String::new();
    let mut filename = None;
    let mut content_type = None;
    for line in headers.split("\r\n") {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if key == "content-disposition" {
            for parameter in value.split(';').skip(1) {
                let parameter = parameter.trim();
                let Some((pkey, pvalue)) = parameter.split_once('=') else {
                    continue;
                };
                let pvalue = pvalue.trim().trim_matches('"');
                match pkey.trim() {
                    "name" => name = pvalue.to_string(),
                    "filename" => filename = Some(pvalue.to_string()),
                    _ => {}
                }
            }
        } else if key == "content-type" {
            content_type = Some(value.to_string());
        }
    }
    MultipartPart {
        name,
        filename,
        content_type,
        data: data.to_vec(),
    }
}

/// The offset of the first occurrence of `needle` in `haystack`.
fn find_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() || haystack.len() - from < needle.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| offset + from)
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Canvas casts an id parameter to an integer; a value that is not a positive
/// number does not exist and is a 404.
fn parse_id(raw: &str) -> Result<i64, SisError> {
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(SisError::not_found)
}

/// Ruby's `String#to_i`: the leading integer of the string, 0 when there is
/// none.
fn ruby_to_i(value: &str) -> i64 {
    let trimmed = value.trim_start();
    let mut end = 0;
    for (index, character) in trimmed.char_indices() {
        if character.is_ascii_digit() || (index == 0 && (character == '-' || character == '+')) {
            end = index + character.len_utf8();
        } else {
            break;
        }
    }
    trimmed[..end].parse::<i64>().unwrap_or(0)
}

/// Canvas `Canvas::Plugin.value_to_boolean`.
fn value_to_boolean(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "y" | "true" | "t" | "on" | "1" => true,
        "no" | "n" | "false" | "f" | "off" | "0" => false,
        other => ruby_to_i(other) != 0,
    }
}

/// Canvas `CanvasTime.try_parse`.
fn parse_canvas_time(value: &str) -> Option<NaiveDateTime> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.naive_utc());
    }
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(value, format) {
            return Some(naive);
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return date.and_hms_opt(0, 0, 0);
    }
    None
}

/// Splits a `Content-Type` header into the media type and the charset.
fn split_content_type(value: &str) -> (String, Option<String>) {
    let mut parts = value.split(';');
    let media_type = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let mut charset = None;
    for parameter in parts {
        let parameter = parameter.trim();
        if let Some(value) = parameter.strip_prefix("charset=") {
            charset = Some(value.trim().trim_matches('"').to_string());
        }
    }
    (media_type, charset)
}

/// Canvas `Attachment.mimetype` for the extensions the SIS import accepts.
fn mimetype_for_extension(extension: &str) -> Option<String> {
    match extension.to_ascii_lowercase().as_str() {
        "zip" => Some("application/zip".to_string()),
        "csv" => Some("text/csv".to_string()),
        "xml" => Some("text/xml".to_string()),
        "txt" => Some("text/plain".to_string()),
        _ => None,
    }
}

/// The `mime_class` of a content type (Ruby `MIME::Type#simplified`).
fn mime_class(content_type: Option<&str>) -> Value {
    let Some(content_type) = content_type else {
        return Value::Null;
    };
    let simplified = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if simplified.contains('/') {
        Value::String(simplified)
    } else {
        Value::Null
    }
}

/// A pseudo unique attachment uuid.
fn pseudo_uuid() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0);
    let mixed = (nanos as u128) ^ ((std::process::id() as u128) << 64);
    format!("{mixed:032x}")
}

/// Rails renders a timestamp as an ISO8601 string with three fraction digits.
fn iso8601(value: &NaiveDateTime) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Rails renders a timestamp as an ISO8601 string with three fraction digits.
fn iso8601_utc(value: &DateTime<Utc>) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Reads a JSON object out of a text column.
fn parse_json_object(raw: Option<&str>) -> Map<String, Value> {
    raw.and_then(|value| serde_json::from_str::<Value>(value).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

/// Reads a JSON array out of a text column.
fn parse_json_array(raw: Option<&str>) -> Option<Vec<Value>> {
    raw.and_then(|value| serde_json::from_str::<Value>(value).ok())
        .and_then(|value| value.as_array().cloned())
}

/// The ids of a JSON array of ids.
fn id_list(value: Option<&Value>) -> Vec<i64> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_i64())
                .collect::<Vec<i64>>()
        })
        .unwrap_or_default()
}

/// Decodes a percent encoded value.
fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
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
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Encodes a value for the pagination links (Ruby `to_query`).
fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
