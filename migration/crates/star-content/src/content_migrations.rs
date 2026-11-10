//! content_migrations — Canvas `content_migrations_controller` parity.
//!
//! Routes (Canvas `content_migrations` + `available_migrators` actions):
//! - GET  /courses/{courseId}/content_migrations            — index
//! - POST /courses/{courseId}/content_migrations            — create
//! - GET  /courses/{courseId}/content_migrations/{id}       — show
//! - GET  /courses/{courseId}/content_migrations/migrators  — available_migrators
//! - GET  /content_migrations/{id}                          — flat show alias
//! - GET  /content_migrations/migrators                     — flat migrators alias
//!
//! Canvas behavior mirrored here (`lib/api/v1/content_migration.rb`):
//! - the migration JSON is `id, user_id, workflow_state, created_at,
//!   started_at, finished_at, migration_type, migration_type_title,
//!   migration_issues_url, migration_issues_count` plus `settings` (source
//!   course info for course copies) and `audit_info` for admins / `?include=`
//! - internal workflow states map to the documented API states:
//!   created → pre_processing, pre_process_error → failed,
//!   exporting/importing → running, imported → completed,
//!   exported → waiting_for_select unless `import_immediately`
//! - POST validates `migration_type` against the course migrators, the
//!   course-copy source course, the .zip destination folder, and the
//!   file-upload requirement (`pre_attachment`, `settings[file_url]` or
//!   `settings[content_export_id]`)
//! - a create without an upload is queued (workflow_state `exporting`, API
//!   `running`); `do_not_run` keeps it in `created`; a selective course copy
//!   stops in `exported` → `waiting_for_select`. No worker runs in this
//!   workspace, so migrations stay in those states until star-jobs advances
//!   them.
//! - pre_processing uploads older than one hour are failed with a migration
//!   issue on index/show (`check_for_pre_processing_timeout`)
//!
//! Attachment binaries are out of scope: `pre_attachment` name/size are stored
//! as metadata only, so no `pre_attachment` upload envelope or `attachment`
//! JSON is returned.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `ContentMigration::UPLOAD_TIMEOUT` — pre_processing uploads older
/// than this are failed on the next index/show.
const UPLOAD_TIMEOUT_HOURS: i64 = 1;

/// `date_shift_options` keys Canvas `set_date_shift_options` persists.
const DATE_SHIFT_KEYS: [&str; 8] = [
    "shift_dates",
    "remove_dates",
    "old_start_date",
    "old_end_date",
    "new_start_date",
    "new_end_date",
    "day_substitutions",
    "time_zone",
];

/// A Canvas export_system migrator valid for Course contexts
/// (`lib/canvas/plugins/default_plugins.rb` + bundled plugin engines, sorted by
/// plugin name like `Canvas::Plugin.all_for_tag`).
struct Migrator {
    migration_type: &'static str,
    /// Canvas `settings[:requires_file_upload]`.
    requires_file_upload: bool,
    /// Canvas `select_text` — the label shown in the migration picker.
    name: &'static str,
    /// Canvas `display_name` (falling back to `name`) — `migration_type_title`.
    title: &'static str,
    /// Canvas `settings[:required_settings]`.
    required_settings: &'static [&'static str],
}

const COURSE_MIGRATORS: [Migrator; 6] = [
    Migrator {
        migration_type: "canvas_cartridge_importer",
        requires_file_upload: true,
        name: "Canvas Course Export Package",
        title: "Canvas Common Cartridge",
        required_settings: &[],
    },
    Migrator {
        migration_type: "common_cartridge_importer",
        requires_file_upload: true,
        name: "Common Cartridge 1.x Package",
        title: "Common Cartridge",
        required_settings: &[],
    },
    Migrator {
        migration_type: "course_copy_importer",
        requires_file_upload: false,
        name: "Copy a Canvas Course",
        title: "Course Copy",
        required_settings: &["source_course_id"],
    },
    Migrator {
        migration_type: "moodle_converter",
        // Canvas does not set requires_file_upload for the Moodle importer.
        requires_file_upload: false,
        name: "Moodle 1.9/2.x",
        title: "Moodle Importer",
        required_settings: &[],
    },
    Migrator {
        migration_type: "qti_converter",
        requires_file_upload: true,
        name: "QTI .zip file",
        title: "QTI",
        required_settings: &[],
    },
    Migrator {
        migration_type: "zip_file_importer",
        requires_file_upload: true,
        name: "Unzip .zip file into folder",
        title: "File Import",
        required_settings: &["source_folder_id"],
    },
];

/// Columns returned for a content migration row. The issue count is computed
/// in SQL so index/show stay a single round trip per page.
const MIGRATION_SELECT: &str = "SELECT m.id, m.course_id, m.user_id, \
        m.migration_type, m.workflow_state, m.migration_settings, \
        m.source_course_id, sc.name AS source_course_name, \
        m.started_at, m.finished_at, m.created_at, m.updated_at, \
        (SELECT count(*) FROM migration_issues i \
          WHERE i.content_migration_id = m.id) AS migration_issues_count \
     FROM content_migrations m \
     LEFT JOIN courses sc ON sc.id = m.source_course_id";

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct ContentMigrationRow {
    id: i64,
    course_id: Option<i64>,
    user_id: Option<i64>,
    migration_type: String,
    workflow_state: String,
    migration_settings: String,
    source_course_id: Option<i64>,
    source_course_name: Option<String>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    migration_issues_count: i64,
}

// ---------------------------------------------------------------------------
// Canvas JSON helpers
// ---------------------------------------------------------------------------

fn ts(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn opt_ts(value: &Option<DateTime<Utc>>) -> Option<String> {
    value.as_ref().map(ts)
}

fn parse_settings(raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ Value::Object(_)) => value,
        _ => json!({}),
    }
}

/// Canvas `Canvas::Plugin.value_to_boolean`.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64().map(|v| v != 0.0).unwrap_or(false),
        Some(Value::String(text)) => {
            let text = text.trim().to_ascii_lowercase();
            matches!(text.as_str(), "yes" | "y" | "true" | "t" | "on" | "1")
                || text.parse::<f64>().map(|v| v != 0.0).unwrap_or(false)
        }
        _ => false,
    }
}

/// Canvas `ContentMigration` state → documented API workflow_state.
fn api_workflow_state<'a>(state: &'a str, import_immediately: bool) -> &'a str {
    match state {
        "created" => "pre_processing",
        "pre_process_error" => "failed",
        "exported" if !import_immediately => "waiting_for_select",
        "exporting" | "importing" | "exported" => "running",
        "imported" => "completed",
        other => other,
    }
}

fn migrator(migration_type: &str) -> Option<&'static Migrator> {
    COURSE_MIGRATORS
        .iter()
        .find(|migrator| migrator.migration_type == migration_type)
}

fn migration_type_title(migration_type: &str) -> Option<&'static str> {
    migrator(migration_type).map(|migrator| migrator.title)
}

fn migrator_json(migrator: &Migrator) -> Value {
    json!({
        "type": migrator.migration_type,
        "requires_file_upload": migrator.requires_file_upload,
        "name": migrator.name,
        "required_settings": migrator.required_settings,
    })
}

fn migrators_json() -> Value {
    Value::Array(COURSE_MIGRATORS.iter().map(migrator_json).collect())
}

// ---------------------------------------------------------------------------
// Request-body helpers (JSON bodies; no serde derives in this crate)
// ---------------------------------------------------------------------------

fn json_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn json_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn object_i64(settings: Option<&Map<String, Value>>, key: &str) -> Option<i64> {
    settings
        .and_then(|settings| settings.get(key))
        .and_then(json_i64)
}

/// Canvas `params.dig(:settings, key).present?`.
fn object_present(settings: Option<&Map<String, Value>>, key: &str) -> bool {
    match settings.and_then(|settings| settings.get(key)) {
        None | Some(Value::Null) => false,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(_) => true,
    }
}

fn bad_request(message: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "message": message })))
}

fn includes_audit_info(params: &HashMap<String, String>) -> bool {
    ["include", "include[]"].iter().any(|key| {
        params
            .get(*key)
            .map(|value| value.split(',').any(|item| item.trim() == "audit_info"))
            .unwrap_or(false)
    })
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// 404 unless the course exists (Canvas `require_context`).
async fn ensure_course(db: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `manage_course_content`: teachers/designers/TAs and global admins.
async fn manages_course(db: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta', 'designer') LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Canvas pagination params (page default 1, per_page default 10, max 100).
fn pagination(params: &HashMap<String, String>) -> (i64, i64) {
    let page = params
        .get("page")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    let per_page = params
        .get("per_page")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(10)
        .clamp(1, 100);
    (per_page, (page - 1) * per_page)
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Canvas `content_migration_json`.
fn migration_json(row: &ContentMigrationRow, include_audit: bool) -> Value {
    let settings = parse_settings(&row.migration_settings);
    let import_immediately = settings
        .get("import_immediately")
        .map(|value| truthy(Some(value)))
        .unwrap_or(false);

    let mut json = json!({
        "id": row.id,
        "user_id": row.user_id,
        "workflow_state": api_workflow_state(&row.workflow_state, import_immediately),
        "created_at": ts(&row.created_at),
        "started_at": opt_ts(&row.started_at),
        "finished_at": opt_ts(&row.finished_at),
        "migration_type": row.migration_type,
        "migration_issues_url": row.course_id.map(|course_id| format!(
            "/api/v1/courses/{course_id}/content_migrations/{}/migration_issues",
            row.id
        )),
        "migration_issues_count": row.migration_issues_count,
    });

    if let Some(title) = migration_type_title(&row.migration_type) {
        json["migration_type_title"] = json!(title);
    }

    // Canvas exposes source course info for same-root-account course copies
    // (this workspace has a single implicit account).
    if let (Some(source_id), Some(source_name)) =
        (row.source_course_id, row.source_course_name.as_deref())
    {
        json["settings"] = json!({
            "source_course_id": source_id,
            "source_course_name": source_name,
            "source_course_html_url": format!("/courses/{source_id}"),
        });
    }

    // Canvas includes audit_info for site admins or `?include=audit_info`.
    if include_audit {
        json["audit_info"] = json!({
            "id": row.id,
            "user_id": row.user_id,
            "migration_settings": settings,
            "started_at": opt_ts(&row.started_at),
            "finished_at": opt_ts(&row.finished_at),
            "created_at": ts(&row.created_at),
            "updated_at": ts(&row.updated_at),
            "progress": Value::Null,
            "context_type": "Course",
            "source_course_id": row.source_course_id,
            "migration_type": row.migration_type,
        });
    }

    json
}

// ---------------------------------------------------------------------------
// Lookup + pre-processing timeout
// ---------------------------------------------------------------------------

async fn find_course_migration(
    db: &PgPool,
    course_id: i64,
    id: i64,
) -> Result<ContentMigrationRow, ApiError> {
    let sql = format!("{MIGRATION_SELECT} WHERE m.id = $1 AND m.course_id = $2");
    sqlx::query_as(&sql)
        .bind(id)
        .bind(course_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

async fn find_migration(db: &PgPool, id: i64) -> Result<ContentMigrationRow, ApiError> {
    let sql = format!("{MIGRATION_SELECT} WHERE m.id = $1");
    sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Canvas `check_for_pre_processing_timeout`: fail uploads stuck in
/// pre_processing for more than an hour and record a migration issue.
async fn check_pre_processing_timeout(
    db: &PgPool,
    row: &mut ContentMigrationRow,
) -> Result<(), ApiError> {
    if row.workflow_state != "pre_processing" {
        return Ok(());
    }
    if row.updated_at + Duration::hours(UPLOAD_TIMEOUT_HOURS) >= Utc::now() {
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO migration_issues \
         (content_migration_id, description, issue_type) \
         VALUES ($1, 'The file upload process timed out.', 'error')",
    )
    .bind(row.id)
    .execute(db)
    .await?;

    sqlx::query(
        "UPDATE content_migrations \
         SET workflow_state = 'failed', finished_at = now(), updated_at = now() \
         WHERE id = $1",
    )
    .bind(row.id)
    .execute(db)
    .await?;

    row.workflow_state = "failed".to_string();
    row.finished_at = Some(Utc::now());
    row.migration_issues_count += 1;
    Ok(())
}

/// Read authorization for a migration row (Canvas `manage_course_content`).
async fn authorize_migration(
    db: &PgPool,
    auth: &AuthUser,
    row: &ContentMigrationRow,
) -> Result<(), ApiError> {
    match row.course_id {
        Some(course_id) => {
            if manages_course(db, auth, course_id).await? {
                Ok(())
            } else {
                Err(ApiError::Forbidden)
            }
        }
        None => {
            if is_admin(auth) {
                Ok(())
            } else {
                Err(ApiError::Forbidden)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/content_migrations — Canvas `index`, newest first.
async fn list_migrations(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let (per_page, offset) = pagination(&params);
    let sql =
        format!("{MIGRATION_SELECT} WHERE m.course_id = $1 ORDER BY m.id DESC LIMIT $2 OFFSET $3");
    let mut rows: Vec<ContentMigrationRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    for row in rows.iter_mut() {
        check_pre_processing_timeout(&state.db, row).await?;
    }

    let include_audit = is_admin(&auth);
    Ok(Json(Value::Array(
        rows.iter()
            .map(|row| migration_json(row, include_audit))
            .collect(),
    )))
}

/// POST /courses/{courseId}/content_migrations — Canvas `create`.
async fn create_migration(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let migration_type = payload
        .get("migration_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");

    let Some(plugin) = migrator(migration_type) else {
        return Ok(bad_request("Invalid migration_type"));
    };

    let settings = payload.get("settings").and_then(Value::as_object);

    // File-upload migrators accept pre_attachment metadata, settings[file_url]
    // (remote download) or settings[content_export_id] (Canvas export import).
    let pre_attachment = payload.get("pre_attachment").and_then(Value::as_object);
    let attachment_name = pre_attachment
        .and_then(|params| params.get("name"))
        .and_then(json_string);
    let attachment_size = pre_attachment
        .and_then(|params| params.get("size"))
        .and_then(json_i64);

    if plugin.requires_file_upload
        && attachment_name.is_none()
        && !object_present(settings, "file_url")
        && !object_present(settings, "content_export_id")
    {
        return Ok(bad_request(
            "File upload, file_url, or content_export_id is required",
        ));
    }

    // Canvas::Migration::Validators::CourseCopyValidator.
    let mut source_course_id = None;
    if migration_type == "course_copy_importer" {
        let Some(source_id) = object_i64(settings, "source_course_id") else {
            return Ok(bad_request("A course copy requires a source course."));
        };
        let source: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
            .bind(source_id)
            .fetch_optional(&state.db)
            .await?;
        if source.is_none() {
            return Ok(bad_request("The source course was not found."));
        }
        if !manages_course(&state.db, &auth, source_id).await? {
            return Ok(bad_request(
                "You are not allowed to copy the source course.",
            ));
        }
        source_course_id = Some(source_id);
    }

    // Canvas::Migration::Validators::ZipImporterValidator.
    if migration_type == "zip_file_importer" {
        let Some(folder_id) = object_i64(settings, "folder_id") else {
            return Ok(bad_request("A .zip upload requires a folder to upload to."));
        };
        let folder: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM folders \
             WHERE id = $1 AND context_type = 'Course' AND context_id = $2 \
               AND workflow_state <> 'deleted'",
        )
        .bind(folder_id)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;
        if folder.is_none() {
            return Ok(bad_request(
                "The specified folder couldn't be found in this course.",
            ));
        }
    }

    // Canvas `link_content_export_attachment`: the referenced export must be
    // readable and finished before it can seed the migration.
    if object_present(settings, "content_export_id") {
        let export_id = object_i64(settings, "content_export_id");
        let export: Option<(Option<i64>, String)> = match export_id {
            Some(export_id) => {
                sqlx::query_as(
                    "SELECT course_id, workflow_state FROM content_exports WHERE id = $1",
                )
                .bind(export_id)
                .fetch_optional(&state.db)
                .await?
            }
            None => None,
        };
        let Some((export_course_id, export_state)) = export else {
            return Ok(bad_request("invalid content export"));
        };
        if export_course_id != Some(course_id) {
            return Ok(bad_request("invalid content export"));
        }
        if export_state != "exported" {
            return Ok(bad_request("content export is incomplete"));
        }
    }

    // Canvas `update_migration_settings`: settings keys merge at the top level
    // of migration_settings.
    let mut merged: Map<String, Value> = Map::new();
    if let Some(settings) = settings {
        for (key, value) in settings {
            merged.insert(key.clone(), value.clone());
        }
    }

    // Canvas `set_date_shift_options` (only persisted when enabled).
    let mut shift_dates = false;
    let mut remove_dates = false;
    if let Some(date_shift) = payload.get("date_shift_options").and_then(Value::as_object) {
        shift_dates = truthy(date_shift.get("shift_dates"));
        remove_dates = truthy(date_shift.get("remove_dates"));
        if shift_dates && remove_dates {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "date_shift_options": [
                        "cannot specify shift_dates and remove_dates simultaneously"
                    ]
                })),
            ));
        }
        if shift_dates || remove_dates {
            let mut kept = Map::new();
            for key in DATE_SHIFT_KEYS {
                if let Some(value) = date_shift.get(key) {
                    kept.insert(key.to_string(), value.clone());
                }
            }
            merged.insert("date_shift_options".to_string(), Value::Object(kept));
        }
    }

    let selective_import = truthy(payload.get("selective_import"));
    let do_not_run = truthy(payload.get("do_not_run"));

    if !selective_import {
        let selection = if migration_type == "course_copy_importer" {
            payload.get("select").filter(|value| value.is_object())
        } else {
            None
        };
        let copy = selection.or_else(|| payload.get("copy").filter(|value| value.is_object()));
        if let Some(copy_options) = copy {
            merged.insert("copy_options".to_string(), copy_options.clone());
        }
    }

    // Canvas update_migration import_immediately bookkeeping.
    if selective_import {
        merged.insert("import_immediately".to_string(), Value::Bool(false));
    } else if !object_present(Some(&merged), "copy_options") {
        merged.insert("import_immediately".to_string(), Value::Bool(true));
    }

    let now = Utc::now();
    let (workflow_state, started_at) = if pre_attachment.is_some() {
        ("pre_processing", Some(now))
    } else if selective_import && migration_type == "course_copy_importer" {
        ("exported", None)
    } else if do_not_run {
        ("created", None)
    } else {
        ("exporting", Some(now))
    };

    let migration_settings =
        serde_json::to_string(&Value::Object(merged)).unwrap_or_else(|_| "{}".to_string());

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO content_migrations \
         (course_id, user_id, migration_type, workflow_state, migration_settings, \
          source_course_id, attachment_name, attachment_size, started_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(migration_type)
    .bind(workflow_state)
    .bind(migration_settings)
    .bind(source_course_id)
    .bind(attachment_name.as_deref())
    .bind(attachment_size)
    .bind(started_at)
    .fetch_one(&state.db)
    .await?;

    let row = find_course_migration(&state.db, course_id, id).await?;
    // Canvas renders the created migration as JSON with HTTP 200.
    Ok((StatusCode::OK, Json(migration_json(&row, is_admin(&auth)))))
}

/// GET /courses/{courseId}/content_migrations/{id} — Canvas `show`.
async fn show_course_migration(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let mut row = find_course_migration(&state.db, course_id, id).await?;
    check_pre_processing_timeout(&state.db, &mut row).await?;

    let include_audit = is_admin(&auth) || includes_audit_info(&params);
    Ok(Json(migration_json(&row, include_audit)))
}

/// GET /content_migrations/{id} — flat show alias.
async fn show_migration(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let mut row = find_migration(&state.db, id).await?;
    authorize_migration(&state.db, &auth, &row).await?;
    check_pre_processing_timeout(&state.db, &mut row).await?;

    let include_audit = is_admin(&auth) || includes_audit_info(&params);
    Ok(Json(migration_json(&row, include_audit)))
}

/// GET /courses/{courseId}/content_migrations/migrators — Canvas
/// `available_migrators` filtered to Course-valid migrators.
async fn list_course_migrators(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(migrators_json()))
}

/// GET /content_migrations/migrators — flat migrators alias (same Course list).
async fn list_migrators(_auth: AuthUser) -> Json<Value> {
    Json(migrators_json())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the content_migrations router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/content_migrations",
            get(list_migrations).post(create_migration),
        )
        .route(
            "/courses/{courseId}/content_migrations/migrators",
            get(list_course_migrators),
        )
        .route(
            "/courses/{courseId}/content_migrations/{id}",
            get(show_course_migration),
        )
        .route("/content_migrations/migrators", get(list_migrators))
        .route("/content_migrations/{id}", get(show_migration))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_state_maps_to_canvas_api_states() {
        assert_eq!(api_workflow_state("created", true), "pre_processing");
        assert_eq!(api_workflow_state("pre_process_error", true), "failed");
        assert_eq!(api_workflow_state("exported", false), "waiting_for_select");
        assert_eq!(api_workflow_state("exported", true), "running");
        assert_eq!(api_workflow_state("exporting", false), "running");
        assert_eq!(api_workflow_state("importing", false), "running");
        assert_eq!(api_workflow_state("imported", false), "completed");
        assert_eq!(api_workflow_state("pre_processing", true), "pre_processing");
        assert_eq!(api_workflow_state("failed", true), "failed");
    }

    #[test]
    fn course_migrators_match_canvas_order_and_flags() {
        let types: Vec<&str> = COURSE_MIGRATORS
            .iter()
            .map(|migrator| migrator.migration_type)
            .collect();
        assert_eq!(
            types,
            vec![
                "canvas_cartridge_importer",
                "common_cartridge_importer",
                "course_copy_importer",
                "moodle_converter",
                "qti_converter",
                "zip_file_importer",
            ]
        );

        let copy = migrator("course_copy_importer").expect("course copy migrator");
        assert!(!copy.requires_file_upload);
        assert_eq!(copy.required_settings, &["source_course_id"][..]);

        let zip = migrator("zip_file_importer").expect("zip migrator");
        assert!(zip.requires_file_upload);
        assert_eq!(zip.required_settings, &["source_folder_id"][..]);

        assert!(migrator("jerk").is_none());

        let json = migrators_json();
        let first = &json.as_array().expect("migrators array")[0];
        assert_eq!(first["type"], "canvas_cartridge_importer");
        assert_eq!(first["name"], "Canvas Course Export Package");
        assert_eq!(first["requires_file_upload"], true);
        assert_eq!(first["required_settings"].as_array().map(Vec::len), Some(0));
    }
}
