//! ai_experiences — Canvas `AiExperiencesController` and
//! `AiConversationsController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /courses/{course_id}/ai_experiences`
//! - `POST   /courses/{course_id}/ai_experiences`
//! - `GET    /courses/{course_id}/ai_experiences/new`
//! - `GET    /courses/{course_id}/ai_experiences/{id}`
//! - `GET    /courses/{course_id}/ai_experiences/{id}/edit`
//! - `PUT    /courses/{course_id}/ai_experiences/{id}`
//! - `DELETE /courses/{course_id}/ai_experiences/{id}`
//! - `GET    /courses/{course_id}/ai_experiences/{id}/ai_conversations`
//! - `GET    /courses/{course_id}/ai_experiences/{id}/ai_conversations/{conversation_id}`
//! - `GET    /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations`
//! - `GET    /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}`
//! - `POST   /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations`
//! - `POST   /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}/messages`
//! - `POST   /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}/messages/{message_id}/feedback`
//! - `DELETE /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}/messages/{message_id}/feedback/{feedback_id}`
//! - `DELETE /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}`
//! - `GET    /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}/evaluation`
//!
//! Canvas behavior mirrored here
//! (`app/controllers/ai_experiences_controller.rb`,
//! `app/controllers/ai_conversations_controller.rb`,
//! `app/models/ai_experience.rb`, `app/models/ai_conversation.rb`,
//! `lib/api/v1/ai_experience.rb`):
//!
//! - `check_ai_experiences_feature_flag` answers `404`
//!   `{"error": "Resource Not Found"}` when the `ai_experiences` feature is
//!   off for the course (or its account).
//! - `require_access_right` (index, show and every AI Conversations route)
//!   allows a course manager or any enrolled member; anyone else gets the
//!   Canvas `render_unauthorized_action` body — `403`
//!   `{"status":"unauthorized","errors":[{"message":"user not authorized to
//!   perform that action"}]}`.
//! - `require_manage_rights` (create, new, edit, update, destroy and the
//!   teacher-only conversation routes) uses the same 403 body.
//! - `load_experience` answers `404 {"error": "Resource Not Found"}` when the
//!   experience does not exist, belongs to another course or is deleted.
//! - the `AiExperience` policy lets a manager read any experience and a
//!   student read only a *published* one; `authorized_action` answers 403 for
//!   the student/unpublished combination.
//! - validation failures answer `400` with the Rails error hash
//!   (`{"title":["can't be blank"], ...}`).
//!
//! The AI features delegate the conversation itself to the external
//! `llm-conversation` service. This port keeps the persisted rows, the
//! authorization and the Ruby error handling; every request that needs the
//! service answers with the Ruby rescue path — `503 {"error": ...}`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Feature flag that gates the whole controller pair.
const FEATURE_AI_EXPERIENCES: &str = "ai_experiences";

/// Feature flag that gates the context-file upload sub-feature.
const FEATURE_CONTEXT_FILE_UPLOAD: &str = "ai_experiences_context_file_upload";

/// Canvas `AiExperience::WORKFLOW_STATES` — the model allows `deleted` too.
const EXPERIENCE_WORKFLOW_STATES: &[&str] = &["unpublished", "published", "deleted"];

/// Maximum length of `AiExperience#title`.
const TITLE_MAX_LENGTH: usize = 255;

/// Guard against a circular account hierarchy.
const MAX_ACCOUNT_DEPTH: usize = 10;

/// The message the external `llm-conversation` service failures produce.
/// Canvas renders the exception message of the service client; this port has
/// no client for the service yet and answers the Ruby rescue path instead.
const LLM_SERVICE_UNAVAILABLE: &str = "llm-conversation service is unavailable";

/// Columns of the `ai_experiences` row, in the order the port reads them.
const EXPERIENCE_COLUMNS: &str = "id, course_id, account_id, root_account_id, title, description, \
     facts, learning_objective, pedagogical_guidance, workflow_state, \
     llm_conversation_context_id, context_index_status, created_at, updated_at";

/// Columns of the `ai_conversations` row, in the order the port reads them.
const CONVERSATION_COLUMNS: &str = "id, ai_experience_id, user_id, course_id, account_id, \
     root_account_id, llm_conversation_id, workflow_state, created_at, updated_at";

// ---------------------------------------------------------------------------
// Canvas-shaped error bodies
// ---------------------------------------------------------------------------

/// Canvas `render_404` (JSON branch) — `{"error": "Resource Not Found"}`.
fn resource_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "Resource Not Found" })),
    )
        .into_response()
}

/// Canvas `render_unauthorized_action` (JSON branch). A logged-in user gets
/// `403` and the `unauthorized` status string.
fn unauthorized() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "status": "unauthorized",
            "errors": [{ "message": "user not authorized to perform that action" }],
        })),
    )
        .into_response()
}

/// Canvas `render json: { error: "Conversation not found" }`.
fn conversation_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "Conversation not found" })),
    )
        .into_response()
}

/// Canvas `LlmConversation::Errors::ConversationError` rescue — 503.
fn llm_service_unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": LLM_SERVICE_UNAVAILABLE })),
    )
        .into_response()
}

/// Canvas `render json: { error: "message is required" }` — 400.
fn message_required() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "message is required" })),
    )
        .into_response()
}

/// Canvas `render json: @experience.errors` — 400 with the Rails error hash.
fn validation_errors(errors: Map<String, Value>) -> Response {
    (StatusCode::BAD_REQUEST, Json(Value::Object(errors))).into_response()
}

// ---------------------------------------------------------------------------
// Domain rows
// ---------------------------------------------------------------------------

/// A row of the `ai_experiences` table.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
struct AiExperienceRow {
    id: i64,
    course_id: i64,
    account_id: i64,
    root_account_id: i64,
    title: String,
    description: Option<String>,
    facts: Option<String>,
    learning_objective: String,
    pedagogical_guidance: String,
    workflow_state: String,
    llm_conversation_context_id: Option<String>,
    context_index_status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// A row of the `ai_conversations` table.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
struct AiConversationRow {
    id: i64,
    ai_experience_id: i64,
    user_id: i64,
    course_id: i64,
    account_id: i64,
    root_account_id: i64,
    llm_conversation_id: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// A student of the course, with the Canvas `user_json` subset the
/// conversation list renders.
#[derive(Debug, Clone, sqlx::FromRow)]
struct StudentRow {
    id: i64,
    name: String,
    sortable_name: Option<String>,
    email: String,
    role: String,
    avatar_url: Option<String>,
    created_at: DateTime<Utc>,
}

/// Query parameters of `GET /courses/{course_id}/ai_experiences`.
#[derive(Debug, Default, Deserialize)]
struct ExperienceListQuery {
    workflow_state: Option<String>,
}

/// Path parameters of the AI Experiences controller routes.
#[derive(Debug, Deserialize)]
struct ExperiencePath {
    course_id: i64,
    id: i64,
}

/// Path parameters of the AI Conversations controller routes.
#[derive(Debug, Deserialize)]
struct ConversationPath {
    course_id: i64,
    ai_experience_id: i64,
    id: i64,
}

/// Path parameters of the teacher conversation routes of the AI Experiences
/// controller.
#[derive(Debug, Deserialize)]
struct ExperienceConversationPath {
    course_id: i64,
    id: i64,
    conversation_id: i64,
}

/// Path parameters of the message feedback routes.
#[derive(Debug, Deserialize)]
struct FeedbackPath {
    course_id: i64,
    ai_experience_id: i64,
    id: i64,
    message_id: String,
    feedback_id: String,
}

// ---------------------------------------------------------------------------
// Permissions, features and course scope
// ---------------------------------------------------------------------------

/// The account chain of a course — the course's own account first, then its
/// ancestors, root account last.
async fn course_account_chain(pool: &PgPool, course_id: i64) -> Result<Vec<i64>, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    let Some((account_id,)) = row else {
        return Ok(Vec::new());
    };

    let mut chain = Vec::new();
    let mut current = account_id;
    for _ in 0..MAX_ACCOUNT_DEPTH {
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(current)
                .fetch_optional(pool)
                .await?;
        chain.push(current);
        match parent {
            Some((Some(parent_id))) => current = parent_id,
            _ => break,
        }
    }
    Ok(chain)
}

/// Canvas `@context.feature_enabled?(:feature)`.
///
/// A course level override wins over the account overrides; the account
/// chain is walked from the course's account up to the root account and the
/// first override found wins.
async fn feature_enabled(pool: &PgPool, course_id: i64, feature: &str) -> Result<bool, ApiError> {
    let course_state: Option<(String,)> = sqlx::query_as(
        "SELECT state FROM feature_flags \
         WHERE context_type = 'Course' AND context_id = $1 AND feature = $2 LIMIT 1",
    )
    .bind(course_id)
    .bind(feature)
    .fetch_optional(pool)
    .await?;
    if let Some((state,)) = course_state {
        return Ok(state == "enabled");
    }

    for account_id in course_account_chain(pool, course_id).await? {
        let state: Option<(String,)> = sqlx::query_as(
            "SELECT state FROM feature_flags \
             WHERE context_type = 'Account' AND context_id = $1 AND feature = $2 LIMIT 1",
        )
        .bind(account_id)
        .bind(feature)
        .fetch_optional(pool)
        .await?;
        if let Some((state,)) = state {
            return Ok(state == "enabled");
        }
    }
    Ok(false)
}

/// Canvas `grants_any_right?(user, :manage_assignments_add,
/// :manage_assignments_edit, :manage_assignments_delete)` — a course teacher
/// (or a site administrator).
async fn can_manage(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    if crate::is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' \
           AND workflow_state <> 'deleted' LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `grants_right?(user, :read_as_member)` — any live enrollment.
async fn is_course_member(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if crate::is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 \
           AND workflow_state NOT IN ('deleted', 'rejected') LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `require_access_right` — a manager or an enrolled member.
async fn require_access_right(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if can_manage(pool, auth, course_id).await? {
        return Ok(true);
    }
    is_course_member(pool, auth, course_id).await
}

/// The `account_id` and `root_account_id` of a course — the two account
/// associations Canvas sets on a new experience.
async fn course_account_ids(pool: &PgPool, course_id: i64) -> Result<(i64, i64), ApiError> {
    let chain = course_account_chain(pool, course_id).await?;
    let account_id = chain.first().copied().unwrap_or(1);
    let root_account_id = chain.last().copied().unwrap_or(account_id);
    Ok((account_id, root_account_id))
}

/// Canvas `load_experience` — `AiExperience.find_by(id: params[:id])` and
/// then the course and deleted checks.
async fn load_experience(
    pool: &PgPool,
    course_id: i64,
    id: i64,
) -> Result<Option<AiExperienceRow>, ApiError> {
    let row: Option<AiExperienceRow> = sqlx::query_as(&format!(
        "SELECT {EXPERIENCE_COLUMNS} FROM ai_experiences WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.filter(|r| r.course_id == course_id && r.workflow_state != "deleted"))
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Options of the Canvas `ai_experience_json` helper.
struct ExperienceJsonOptions {
    /// `Some(true)`/`Some(false)` renders the `can_manage` key, `None`
    /// omits it — Canvas only renders the key when the caller passed it.
    can_manage: Option<bool>,
    /// The `submission_status` key of the student index.
    submission_status: Option<String>,
    /// `failed_context_file_names` — only rendered when it is not blank.
    failed_context_file_names: Option<Vec<String>>,
}

/// Canvas `ai_experience_json` — the `Api::V1::AiExperience::API_JSON_OPTS`
/// field set plus the optional keys.
async fn experience_json(
    pool: &PgPool,
    row: &AiExperienceRow,
    opts: ExperienceJsonOptions,
) -> Result<Value, ApiError> {
    let mut json = json!({
        "id": row.id,
        "title": row.title,
        "description": row.description,
        "facts": row.facts,
        "learning_objective": row.learning_objective,
        "pedagogical_guidance": row.pedagogical_guidance,
        "workflow_state": row.workflow_state,
        "course_id": row.course_id,
        "context_index_status": row.context_index_status,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
    });

    if let Some(can_manage) = opts.can_manage {
        json["can_manage"] = json!(can_manage);
    }
    if let Some(submission_status) = opts.submission_status {
        json["submission_status"] = json!(submission_status);
    }

    if opts.can_manage == Some(true) {
        json["can_unpublish"] = json!(can_unpublish(pool, row).await?);
        json["context_ready"] = json!(can_publish(pool, row).await?);
        if let Some(names) = opts.failed_context_file_names {
            if !names.is_empty() {
                json["failed_context_file_names"] = json!(names);
            }
        }
    } else {
        // Teacher facing fields are hidden from non managers.
        if let Some(obj) = json.as_object_mut() {
            obj.remove("facts");
            obj.remove("pedagogical_guidance");
        }
    }

    if feature_enabled(pool, row.course_id, FEATURE_CONTEXT_FILE_UPLOAD).await? {
        json["context_files"] = json!(context_files(pool, row.id).await?);
    }

    Ok(json)
}

/// The JSON of the unsaved experience the Canvas `new` form renders.
fn new_experience_json(course_id: i64) -> Value {
    json!({
        "id": Value::Null,
        "title": Value::Null,
        "description": Value::Null,
        "facts": Value::Null,
        "learning_objective": Value::Null,
        "pedagogical_guidance": Value::Null,
        "workflow_state": "unpublished",
        "course_id": course_id,
        "context_index_status": "not_started",
        "created_at": Value::Null,
        "updated_at": Value::Null,
    })
}

/// Canvas `ai_conversation_json` — the `CONVERSATION_JSON_OPTS` field set.
fn conversation_json(row: &AiConversationRow) -> Value {
    json!({
        "id": row.id,
        "llm_conversation_id": row.llm_conversation_id,
        "workflow_state": row.workflow_state,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "user_id": row.user_id,
    })
}

/// The `user_json` subset the teacher conversation list renders.
fn student_json(student: &StudentRow) -> Value {
    json!({
        "id": student.id,
        "name": student.name,
        "sortable_name": student.sortable_name.clone().unwrap_or_else(|| student.name.clone()),
        "short_name": student.name,
        "email": student.email,
        "role": student.role,
        "avatar_url": student.avatar_url,
        "created_at": student.created_at,
    })
}

/// The short student object Canvas renders for a student without a
/// conversation — the id is a string here.
fn student_brief_json(student: &StudentRow) -> Value {
    json!({
        "id": student.id.to_string(),
        "name": student.name,
        "avatar_url": student.avatar_url,
    })
}

// ---------------------------------------------------------------------------
// Publish state helpers (Canvas `AiExperience`)
// ---------------------------------------------------------------------------

/// Canvas `indexing_allows_publish_changes?` — only the context file upload
/// feature restricts the publish state.
async fn indexing_allows_publish_changes(
    pool: &PgPool,
    row: &AiExperienceRow,
) -> Result<bool, ApiError> {
    if !feature_enabled(pool, row.course_id, FEATURE_CONTEXT_FILE_UPLOAD).await? {
        return Ok(true);
    }
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM ai_experience_context_files WHERE ai_experience_id = $1",
    )
    .bind(row.id)
    .fetch_one(pool)
    .await?;
    if count == 0 {
        return Ok(true);
    }
    Ok(row.context_index_status == "completed")
}

/// Canvas `student_conversations?` — a student already started a
/// conversation on this experience.
async fn student_conversations_exist(
    pool: &PgPool,
    row: &AiExperienceRow,
) -> Result<bool, ApiError> {
    let found: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM ai_conversations c \
         JOIN enrollments e ON e.user_id = c.user_id AND e.course_id = $2 \
         WHERE c.ai_experience_id = $1 \
           AND c.workflow_state <> 'deleted' \
           AND e.role = 'student' \
           AND e.workflow_state NOT IN ('deleted', 'rejected') LIMIT 1",
    )
    .bind(row.id)
    .bind(row.course_id)
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

/// Canvas `can_unpublish?`.
async fn can_unpublish(pool: &PgPool, row: &AiExperienceRow) -> Result<bool, ApiError> {
    if student_conversations_exist(pool, row).await? {
        return Ok(false);
    }
    indexing_allows_publish_changes(pool, row).await
}

/// Canvas `can_publish?`.
async fn can_publish(pool: &PgPool, row: &AiExperienceRow) -> Result<bool, ApiError> {
    indexing_allows_publish_changes(pool, row).await
}

/// The context files of an experience — Canvas renders them only when the
/// context file upload feature is enabled.
async fn context_files(pool: &PgPool, experience_id: i64) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<(i64, String, i64, String)> = sqlx::query_as(
        "SELECT f.id, f.filename, f.size_bytes, f.content_type \
         FROM ai_experience_context_files cef \
         JOIN files f ON f.id = cef.attachment_id \
         WHERE cef.ai_experience_id = $1 ORDER BY cef.position",
    )
    .bind(experience_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, filename, size, content_type)| {
            json!({
                "id": id.to_string(),
                "display_name": filename,
                "size": size,
                "content_type": content_type,
                "url": Value::Null,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Parameters and validation
// ---------------------------------------------------------------------------

/// The permitted experience fields. `None` means "the parameter was not
/// sent"; `Some(None)` means "the parameter was sent as null".
#[derive(Debug, Default, Clone)]
struct ExperienceFields {
    title: Option<Option<String>>,
    description: Option<Option<String>>,
    facts: Option<Option<String>>,
    learning_objective: Option<Option<String>>,
    pedagogical_guidance: Option<Option<String>>,
    workflow_state: Option<Option<String>>,
    context_file_ids: Option<Vec<String>>,
}

/// Read one permitted field out of the request body.
///
/// Canvas wraps the fields in `ai_experience`; this port also accepts the
/// flat body, like the other Canvas wrappers of this crate.
fn read_experience_fields(body: &Value) -> ExperienceFields {
    let source = match body.get("ai_experience") {
        Some(inner) if inner.is_object() => inner.clone(),
        _ => body.clone(),
    };
    let map = match source.as_object() {
        Some(map) => map.clone(),
        None => Map::new(),
    };

    ExperienceFields {
        title: read_field(&map, "title"),
        description: read_field(&map, "description"),
        facts: read_field(&map, "facts"),
        learning_objective: read_field(&map, "learning_objective"),
        pedagogical_guidance: read_field(&map, "pedagogical_guidance"),
        workflow_state: read_field(&map, "workflow_state"),
        context_file_ids: map
            .get("context_file_ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .map(|id| match id {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect()
            }),
    }
}

/// One field of the request body — `None` when absent, `Some(None)` when the
/// client sent an explicit null.
fn read_field(map: &Map<String, Value>, key: &str) -> Option<Option<String>> {
    let value = map.get(key)?;
    Some(match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    })
}

/// The effective values of an experience.
///
/// Canvas validates the merged record, so an update that does not send a
/// required field keeps the stored value.
#[derive(Debug, Clone)]
struct ExperienceValues {
    title: Option<String>,
    learning_objective: Option<String>,
    pedagogical_guidance: Option<String>,
    workflow_state: String,
}

impl ExperienceValues {
    /// The values of a create — the request only.
    fn for_create(fields: &ExperienceFields) -> Self {
        Self {
            title: fields.title.clone().flatten(),
            learning_objective: fields.learning_objective.clone().flatten(),
            pedagogical_guidance: fields.pedagogical_guidance.clone().flatten(),
            workflow_state: fields
                .workflow_state
                .clone()
                .flatten()
                .unwrap_or_else(|| "unpublished".to_string()),
        }
    }

    /// The values of an update — the request merged over the stored row.
    ///
    /// A field that was sent as an explicit null clears the stored value, a
    /// field that was not sent keeps it.
    fn for_update(fields: &ExperienceFields, row: &AiExperienceRow) -> Self {
        Self {
            title: match fields.title.clone() {
                Some(value) => value,
                None => Some(row.title.clone()),
            },
            learning_objective: match fields.learning_objective.clone() {
                Some(value) => value,
                None => Some(row.learning_objective.clone()),
            },
            pedagogical_guidance: match fields.pedagogical_guidance.clone() {
                Some(value) => value,
                None => Some(row.pedagogical_guidance.clone()),
            },
            workflow_state: match fields.workflow_state.clone() {
                Some(Some(state)) => state,
                // An explicit null clears the value and fails the validation.
                Some(None) => String::new(),
                None => row.workflow_state.clone(),
            },
        }
    }
}

/// The Rails error hash of the `AiExperience` validations.
///
/// `title` is required and at most 255 characters, `learning_objective` and
/// `pedagogical_guidance` are required and `workflow_state` has to be one of
/// the three model states.
fn validate_experience_values(values: &ExperienceValues) -> Map<String, Value> {
    let mut errors = Map::new();

    match values.title.as_deref() {
        None => {
            errors.insert("title".into(), json!(["can't be blank"]));
        }
        Some(title) if title.trim().is_empty() => {
            errors.insert("title".into(), json!(["can't be blank"]));
        }
        Some(title) if title.chars().count() > TITLE_MAX_LENGTH => {
            errors.insert(
                "title".into(),
                json!([format!(
                    "is too long (maximum is {TITLE_MAX_LENGTH} characters)"
                )]),
            );
        }
        Some(_) => {}
    }

    if values
        .learning_objective
        .as_deref()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        errors.insert("learning_objective".into(), json!(["can't be blank"]));
    }

    if values
        .pedagogical_guidance
        .as_deref()
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
    {
        errors.insert("pedagogical_guidance".into(), json!(["can't be blank"]));
    }

    let workflow_state = values.workflow_state.as_str();
    if workflow_state.trim().is_empty() {
        errors.insert("workflow_state".into(), json!(["can't be blank"]));
    } else if !EXPERIENCE_WORKFLOW_STATES.contains(&workflow_state) {
        errors.insert(
            "workflow_state".into(),
            json!(["is not included in the list"]),
        );
    }

    errors
}

/// The publish state validations of the model — `publish_ok?` and
/// `unpublish_ok?` only run when the workflow state really changes.
async fn validate_workflow_state_change(
    pool: &PgPool,
    row: &AiExperienceRow,
    new_workflow_state: &str,
) -> Map<String, Value> {
    let mut errors = Map::new();
    if new_workflow_state == row.workflow_state {
        return errors;
    }

    if new_workflow_state == "published" {
        if !can_publish(pool, row).await.unwrap_or(false) {
            errors.insert(
                "workflow_state".into(),
                json!(["Cannot publish while source files are still processing"]),
            );
        }
    } else if new_workflow_state == "unpublished" {
        if !can_unpublish(pool, row).await.unwrap_or(false) {
            if !indexing_allows_publish_changes(pool, row)
                .await
                .unwrap_or(true)
            {
                errors.insert(
                    "workflow_state".into(),
                    json!(["Cannot unpublish while source files are still processing"]),
                );
            } else {
                errors.insert(
                    "workflow_state".into(),
                    json!(["Cannot unpublish if students have started conversations"]),
                );
            }
        }
    }
    errors
}

/// Replace the context files of an experience — the local part of the Canvas
/// `sync_context_files` callback. The indexing call to the external service
/// is not ported (see the module docs).
async fn replace_context_files(
    pool: &PgPool,
    experience_id: i64,
    root_account_id: i64,
    attachment_ids: &[String],
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM ai_experience_context_files WHERE ai_experience_id = $1")
        .bind(experience_id)
        .execute(pool)
        .await?;

    for (index, attachment_id) in attachment_ids.iter().enumerate() {
        let Ok(attachment_id) = attachment_id.parse::<i64>() else {
            continue;
        };
        sqlx::query(
            "INSERT INTO ai_experience_context_files \
             (ai_experience_id, attachment_id, root_account_id, position) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (ai_experience_id, attachment_id) DO NOTHING",
        )
        .bind(experience_id)
        .bind(attachment_id)
        .bind(root_account_id)
        .bind(index as i32 + 1)
        .execute(pool)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers — AI Experiences controller
// ---------------------------------------------------------------------------

/// `GET /courses/{course_id}/ai_experiences` — Canvas `index`.
async fn index_experiences(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<ExperienceListQuery>,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }

    let manager = can_manage(&state.db, &auth, course_id).await?;
    if !manager && !require_access_right(&state.db, &auth, course_id).await? {
        return Ok(unauthorized());
    }

    let workflow_state = query
        .workflow_state
        .as_ref()
        .map(|state| state.trim().to_string())
        .filter(|state| !state.is_empty());

    let rows: Vec<AiExperienceRow> = sqlx::query_as(&format!(
        "SELECT {EXPERIENCE_COLUMNS} FROM ai_experiences \
         WHERE course_id = $1 \
           AND workflow_state <> 'deleted' \
           AND ($2::bool OR workflow_state = 'published') \
           AND ($3::text IS NULL OR workflow_state = $3) \
         ORDER BY id"
    ))
    .bind(course_id)
    .bind(manager)
    .bind(workflow_state)
    .fetch_all(&state.db)
    .await?;

    let mut experiences = Vec::with_capacity(rows.len());
    for row in &rows {
        let opts = if manager {
            ExperienceJsonOptions {
                can_manage: Some(true),
                submission_status: None,
                failed_context_file_names: None,
            }
        } else {
            ExperienceJsonOptions {
                can_manage: Some(false),
                submission_status: Some(submission_status(&state.db, row, auth.id).await?),
                failed_context_file_names: None,
            }
        };
        experiences.push(experience_json(&state.db, row, opts).await?);
    }

    Ok(Json(json!({
        "experiences": experiences,
        "can_manage": manager,
    }))
    .into_response())
}

/// The `submission_status` of the student index — `not_started`,
/// `in_progress` or `submitted`.
async fn submission_status(
    pool: &PgPool,
    row: &AiExperienceRow,
    user_id: i64,
) -> Result<String, ApiError> {
    let latest: Option<(String,)> = sqlx::query_as(
        "SELECT workflow_state FROM ai_conversations \
         WHERE ai_experience_id = $1 AND user_id = $2 AND workflow_state <> 'deleted' \
         ORDER BY updated_at DESC, id DESC LIMIT 1",
    )
    .bind(row.id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(match latest {
        None => "not_started".to_string(),
        Some((state,)) if state == "completed" => "submitted".to_string(),
        Some(_) => "in_progress".to_string(),
    })
}

/// `POST /courses/{course_id}/ai_experiences` — Canvas `create`.
async fn create_experience(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !can_manage(&state.db, &auth, course_id).await? {
        return Ok(unauthorized());
    }

    let fields = read_experience_fields(&body);
    let values = ExperienceValues::for_create(&fields);
    let errors = validate_experience_values(&values);
    if !errors.is_empty() {
        return Ok(validation_errors(errors));
    }

    let (account_id, root_account_id) = course_account_ids(&state.db, course_id).await?;
    let workflow_state = values.workflow_state.clone();

    let row: AiExperienceRow = sqlx::query_as(&format!(
        "INSERT INTO ai_experiences \
         (course_id, account_id, root_account_id, title, description, facts, \
          learning_objective, pedagogical_guidance, workflow_state, context_index_status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'not_started') \
         RETURNING {EXPERIENCE_COLUMNS}"
    ))
    .bind(course_id)
    .bind(account_id)
    .bind(root_account_id)
    .bind(fields.title.clone().flatten().unwrap_or_default())
    .bind(fields.description.clone().flatten())
    .bind(fields.facts.clone().flatten())
    .bind(
        fields
            .learning_objective
            .clone()
            .flatten()
            .unwrap_or_default(),
    )
    .bind(
        fields
            .pedagogical_guidance
            .clone()
            .flatten()
            .unwrap_or_default(),
    )
    .bind(workflow_state)
    .fetch_one(&state.db)
    .await?;

    // Canvas only permits `context_file_ids` when the upload feature is on.
    if feature_enabled(&state.db, course_id, FEATURE_CONTEXT_FILE_UPLOAD).await? {
        if let Some(ids) = fields.context_file_ids.as_ref() {
            replace_context_files(&state.db, row.id, root_account_id, ids).await?;
        }
    }

    let json = experience_json(
        &state.db,
        &row,
        ExperienceJsonOptions {
            can_manage: None,
            submission_status: None,
            failed_context_file_names: None,
        },
    )
    .await?;

    Ok((StatusCode::CREATED, Json(json)).into_response())
}

/// `GET /courses/{course_id}/ai_experiences/new` — Canvas `new`.
///
/// Canvas renders the HTML form; this JSON port answers the form defaults.
async fn new_experience(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !can_manage(&state.db, &auth, course_id).await? {
        return Ok(unauthorized());
    }
    Ok(Json(new_experience_json(course_id)).into_response())
}

/// `GET /courses/{course_id}/ai_experiences/{id}` — Canvas `show`.
async fn show_experience(
    State(state): State<AppState>,
    Path(path): Path<ExperiencePath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !require_access_right(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }

    let Some(row) = load_experience(&state.db, path.course_id, path.id).await? else {
        return Ok(resource_not_found());
    };

    // Canvas `authorized_action(@ai_experience, @current_user, :read)`: a
    // manager reads any experience, a student only a published one.
    let can_manage = can_manage(&state.db, &auth, path.course_id).await?;
    if !can_manage && row.workflow_state != "published" {
        return Ok(unauthorized());
    }

    let json = experience_json(
        &state.db,
        &row,
        ExperienceJsonOptions {
            can_manage: Some(can_manage),
            submission_status: None,
            failed_context_file_names: None,
        },
    )
    .await?;

    Ok(Json(json).into_response())
}

/// `GET /courses/{course_id}/ai_experiences/{id}/edit` — Canvas `edit`.
///
/// Canvas renders the HTML form; this JSON port answers the experience the
/// form is bound to.
async fn edit_experience(
    State(state): State<AppState>,
    Path(path): Path<ExperiencePath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !can_manage(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(row) = load_experience(&state.db, path.course_id, path.id).await? else {
        return Ok(resource_not_found());
    };

    let json = experience_json(
        &state.db,
        &row,
        ExperienceJsonOptions {
            can_manage: Some(true),
            submission_status: None,
            failed_context_file_names: None,
        },
    )
    .await?;

    Ok(Json(json).into_response())
}

/// `PUT /courses/{course_id}/ai_experiences/{id}` — Canvas `update`.
async fn update_experience(
    State(state): State<AppState>,
    Path(path): Path<ExperiencePath>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !can_manage(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(row) = load_experience(&state.db, path.course_id, path.id).await? else {
        return Ok(resource_not_found());
    };

    let fields = read_experience_fields(&body);
    let values = ExperienceValues::for_update(&fields, &row);
    let errors = validate_experience_values(&values);
    if !errors.is_empty() {
        return Ok(validation_errors(errors));
    }

    if let Some(new_state) = fields.workflow_state.clone().flatten() {
        let errors = validate_workflow_state_change(&state.db, &row, &new_state).await?;
        if !errors.is_empty() {
            return Ok(validation_errors(errors));
        }
    }

    let updated: AiExperienceRow = sqlx::query_as(&format!(
        "UPDATE ai_experiences SET \
           title = CASE WHEN $2 THEN $3 ELSE title END, \
           description = CASE WHEN $4 THEN $5 ELSE description END, \
           facts = CASE WHEN $6 THEN $7 ELSE facts END, \
           learning_objective = CASE WHEN $8 THEN $9 ELSE learning_objective END, \
           pedagogical_guidance = CASE WHEN $10 THEN $11 ELSE pedagogical_guidance END, \
           workflow_state = CASE WHEN $12 THEN $13 ELSE workflow_state END, \
           updated_at = now() \
         WHERE id = $1 RETURNING {EXPERIENCE_COLUMNS}"
    ))
    .bind(path.id)
    .bind(fields.title.is_some())
    .bind(fields.title.clone().flatten())
    .bind(fields.description.is_some())
    .bind(fields.description.clone().flatten())
    .bind(fields.facts.is_some())
    .bind(fields.facts.clone().flatten())
    .bind(fields.learning_objective.is_some())
    .bind(fields.learning_objective.clone().flatten())
    .bind(fields.pedagogical_guidance.is_some())
    .bind(fields.pedagogical_guidance.clone().flatten())
    .bind(fields.workflow_state.is_some())
    .bind(fields.workflow_state.clone().flatten())
    .fetch_one(&state.db)
    .await?;

    if feature_enabled(&state.db, path.course_id, FEATURE_CONTEXT_FILE_UPLOAD).await? {
        if let Some(ids) = fields.context_file_ids.as_ref() {
            replace_context_files(&state.db, updated.id, updated.root_account_id, ids).await?;
        }
    }

    let json = experience_json(
        &state.db,
        &updated,
        ExperienceJsonOptions {
            can_manage: None,
            submission_status: None,
            failed_context_file_names: None,
        },
    )
    .await?;

    Ok(Json(json).into_response())
}

/// `DELETE /courses/{course_id}/ai_experiences/{id}` — Canvas `destroy`.
///
/// The delete is a soft delete — the row keeps existing with the
/// `deleted` workflow state.
async fn destroy_experience(
    State(state): State<AppState>,
    Path(path): Path<ExperiencePath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !can_manage(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(row) = load_experience(&state.db, path.course_id, path.id).await? else {
        return Ok(resource_not_found());
    };

    // Canvas `delete` uses `update_column` — the callbacks do not run and
    // `updated_at` is not touched.
    let deleted: AiExperienceRow = sqlx::query_as(&format!(
        "UPDATE ai_experiences SET workflow_state = 'deleted' \
         WHERE id = $1 RETURNING {EXPERIENCE_COLUMNS}"
    ))
    .bind(row.id)
    .fetch_one(&state.db)
    .await?;

    let json = experience_json(
        &state.db,
        &deleted,
        ExperienceJsonOptions {
            can_manage: None,
            submission_status: None,
            failed_context_file_names: None,
        },
    )
    .await?;

    Ok(Json(json).into_response())
}

/// `GET /courses/{course_id}/ai_experiences/{id}/ai_conversations` —
/// Canvas `ai_conversations_index`.
///
/// The latest conversation of every student of the course. The students
/// without a conversation are listed with `has_conversation: false`.
async fn ai_conversations_index(
    State(state): State<AppState>,
    Path(path): Path<ExperienceConversationPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !can_manage(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) = load_experience(&state.db, path.course_id, path.id).await? else {
        return Ok(resource_not_found());
    };

    let students: Vec<StudentRow> = sqlx::query_as(
        "SELECT DISTINCT ON (u.id) \
           u.id, u.name, u.sortable_name, u.email, e.role, p.avatar_url, u.created_at \
         FROM enrollments e \
         JOIN users u ON u.id = e.user_id \
         LEFT JOIN user_profiles p ON p.user_id = u.id \
         WHERE e.course_id = $1 AND e.role = 'student' \
           AND e.workflow_state NOT IN ('deleted', 'rejected') \
         ORDER BY u.id, e.id",
    )
    .bind(path.course_id)
    .fetch_all(&state.db)
    .await?;

    let student_ids: Vec<i64> = students.iter().map(|student| student.id).collect();
    let latest: Vec<AiConversationRow> = if student_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(&format!(
            "SELECT DISTINCT ON (user_id) {CONVERSATION_COLUMNS} FROM ai_conversations \
             WHERE ai_experience_id = $1 AND user_id = ANY($2) AND workflow_state <> 'deleted' \
             ORDER BY user_id, updated_at DESC, id DESC"
        ))
        .bind(experience.id)
        .bind(&student_ids)
        .fetch_all(&state.db)
        .await?
    };

    let mut conversations = Vec::with_capacity(students.len());
    for student in &students {
        match latest.iter().find(|row| row.user_id == student.id) {
            Some(conversation) => {
                let mut json = conversation_json(conversation);
                json["student"] = student_json(student);
                conversations.push(json);
            }
            None => {
                conversations.push(json!({
                    "id": Value::Null,
                    "user_id": student.id.to_string(),
                    "student": student_brief_json(student),
                    "has_conversation": false,
                }));
            }
        }
    }

    Ok(Json(json!({ "conversations": conversations })).into_response())
}

/// `GET /courses/{course_id}/ai_experiences/{id}/ai_conversations/{conversation_id}`
/// — Canvas `ai_conversation_show`.
///
/// The message history lives in the external `llm-conversation` service, so
/// this port answers the Canvas rescue path.
async fn ai_conversation_show(
    State(state): State<AppState>,
    Path(path): Path<ExperienceConversationPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !can_manage(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) = load_experience(&state.db, path.course_id, path.id).await? else {
        return Ok(resource_not_found());
    };

    let conversation: Option<AiConversationRow> = sqlx::query_as(&format!(
        "SELECT {CONVERSATION_COLUMNS} FROM ai_conversations WHERE id = $1"
    ))
    .bind(path.conversation_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(conversation) = conversation else {
        return Ok(conversation_not_found());
    };
    if conversation.ai_experience_id != experience.id || conversation.course_id != path.course_id {
        return Ok(conversation_not_found());
    }

    // The messages come from the external service.
    Ok(llm_service_unavailable())
}

// ---------------------------------------------------------------------------
// Handlers — AI Conversations controller
// ---------------------------------------------------------------------------

/// Canvas `load_conversation` — a teacher loads any conversation of the
/// experience, a student only an own active conversation.
async fn load_conversation(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    experience_id: i64,
    conversation_id: i64,
) -> Result<Option<AiConversationRow>, ApiError> {
    let row: Option<AiConversationRow> = sqlx::query_as(&format!(
        "SELECT {CONVERSATION_COLUMNS} FROM ai_conversations WHERE id = $1"
    ))
    .bind(conversation_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.ai_experience_id != experience_id || row.course_id != course_id {
        return Ok(None);
    }

    if can_manage(pool, auth, course_id).await? {
        return Ok(Some(row));
    }

    if row.user_id == auth.id && row.workflow_state == "active" {
        return Ok(Some(row));
    }
    Ok(None)
}

/// `GET /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations`
/// — Canvas `active_conversation`.
///
/// Answers an empty object when the user has no active conversation.
async fn active_conversation(
    State(state): State<AppState>,
    Path(path): Path<ConversationPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !require_access_right(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) =
        load_experience(&state.db, path.course_id, path.ai_experience_id).await?
    else {
        return Ok(resource_not_found());
    };

    let conversation: Option<AiConversationRow> = sqlx::query_as(&format!(
        "SELECT {CONVERSATION_COLUMNS} FROM ai_conversations \
         WHERE ai_experience_id = $1 AND user_id = $2 AND workflow_state = 'active' \
         ORDER BY id LIMIT 1"
    ))
    .bind(experience.id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;

    if conversation.is_none() {
        // Canvas renders an empty object when there is no active conversation.
        return Ok(Json(json!({})).into_response());
    }

    // The messages come from the external service.
    Ok(llm_service_unavailable())
}

/// `GET /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}`
/// — Canvas `show`.
async fn show_conversation(
    State(state): State<AppState>,
    Path(path): Path<ConversationPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !require_access_right(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) =
        load_experience(&state.db, path.course_id, path.ai_experience_id).await?
    else {
        return Ok(resource_not_found());
    };
    if load_conversation(&state.db, &auth, path.course_id, experience.id, path.id)
        .await?
        .is_none()
    {
        return Ok(resource_not_found());
    }

    // Only teachers may read a conversation.
    if !can_manage(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }

    // The messages come from the external service.
    Ok(llm_service_unavailable())
}

/// `POST /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations`
/// — Canvas `create`.
async fn create_conversation(
    State(state): State<AppState>,
    Path(path): Path<ConversationPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !require_access_right(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) =
        load_experience(&state.db, path.course_id, path.ai_experience_id).await?
    else {
        return Ok(resource_not_found());
    };

    // Canvas completes the running conversation before it starts a new one.
    sqlx::query(
        "UPDATE ai_conversations SET workflow_state = 'completed' \
         WHERE ai_experience_id = $1 AND user_id = $2 AND workflow_state = 'active'",
    )
    .bind(experience.id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    // The conversation itself is created by the external service.
    Ok(llm_service_unavailable())
}

/// `POST /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}/messages`
/// — Canvas `post_message`.
async fn post_message(
    State(state): State<AppState>,
    Path(path): Path<ConversationPath>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !require_access_right(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) =
        load_experience(&state.db, path.course_id, path.ai_experience_id).await?
    else {
        return Ok(resource_not_found());
    };
    if load_conversation(&state.db, &auth, path.course_id, experience.id, path.id)
        .await?
        .is_none()
    {
        return Ok(resource_not_found());
    }

    let message = body.get("message").and_then(Value::as_str).unwrap_or("");
    if message.trim().is_empty() {
        return Ok(message_required());
    }

    // The answer of the AI comes from the external service.
    Ok(llm_service_unavailable())
}

/// `POST .../conversations/{id}/messages/{message_id}/feedback` — Canvas
/// `create_feedback`.
async fn create_feedback(
    State(state): State<AppState>,
    Path(path): Path<FeedbackPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !require_access_right(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) =
        load_experience(&state.db, path.course_id, path.ai_experience_id).await?
    else {
        return Ok(resource_not_found());
    };
    if load_conversation(&state.db, &auth, path.course_id, experience.id, path.id)
        .await?
        .is_none()
    {
        return Ok(resource_not_found());
    }

    // The feedback is stored by the external service.
    Ok(llm_service_unavailable())
}

/// `DELETE .../conversations/{id}/messages/{message_id}/feedback/{feedback_id}`
/// — Canvas `delete_feedback`.
async fn delete_feedback(
    State(state): State<AppState>,
    Path(path): Path<FeedbackPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !require_access_right(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) =
        load_experience(&state.db, path.course_id, path.ai_experience_id).await?
    else {
        return Ok(resource_not_found());
    };
    if load_conversation(&state.db, &auth, path.course_id, experience.id, path.id)
        .await?
        .is_none()
    {
        return Ok(resource_not_found());
    }

    // The feedback is removed by the external service.
    Ok(llm_service_unavailable())
}

/// `DELETE /courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}`
/// — Canvas `destroy`.
///
/// The conversation is completed, not removed.
async fn destroy_conversation(
    State(state): State<AppState>,
    Path(path): Path<ConversationPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !require_access_right(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) =
        load_experience(&state.db, path.course_id, path.ai_experience_id).await?
    else {
        return Ok(resource_not_found());
    };
    let Some(conversation) =
        load_conversation(&state.db, &auth, path.course_id, experience.id, path.id).await?
    else {
        return Ok(resource_not_found());
    };

    // Canvas `complete!` uses `update_column` — `updated_at` is not touched.
    sqlx::query(
        "UPDATE ai_conversations SET workflow_state = 'completed' \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(conversation.id)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({ "message": "Conversation completed successfully" })).into_response())
}

/// `GET .../conversations/{id}/evaluation` — Canvas `evaluation`.
async fn evaluation(
    State(state): State<AppState>,
    Path(path): Path<ConversationPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !feature_enabled(&state.db, path.course_id, FEATURE_AI_EXPERIENCES).await? {
        return Ok(resource_not_found());
    }
    if !require_access_right(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }
    let Some(experience) =
        load_experience(&state.db, path.course_id, path.ai_experience_id).await?
    else {
        return Ok(resource_not_found());
    };
    if load_conversation(&state.db, &auth, path.course_id, experience.id, path.id)
        .await?
        .is_none()
    {
        return Ok(resource_not_found());
    }

    // Only teachers may read an evaluation.
    if !can_manage(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }

    // The evaluation is computed by the external service.
    Ok(llm_service_unavailable())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The router of the AI Experiences and AI Conversations routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{course_id}/ai_experiences",
            get(index_experiences).post(create_experience),
        )
        .route("/courses/{course_id}/ai_experiences/new", get(new_experience))
        .route(
            "/courses/{course_id}/ai_experiences/{id}",
            get(show_experience)
                .put(update_experience)
                .delete(destroy_experience),
        )
        .route(
            "/courses/{course_id}/ai_experiences/{id}/edit",
            get(edit_experience),
        )
        .route(
            "/courses/{course_id}/ai_experiences/{id}/ai_conversations",
            get(ai_conversations_index),
        )
        .route(
            "/courses/{course_id}/ai_experiences/{id}/ai_conversations/{conversation_id}",
            get(ai_conversation_show),
        )
        .route(
            "/courses/{course_id}/ai_experiences/{ai_experience_id}/conversations",
            get(active_conversation).post(create_conversation),
        )
        .route(
            "/courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}",
            get(show_conversation).delete(destroy_conversation),
        )
        .route(
            "/courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}/messages",
            post(post_message),
        )
        .route(
            "/courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}/messages/{message_id}/feedback",
            post(create_feedback),
        )
        .route(
            "/courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}/messages/{message_id}/feedback/{feedback_id}",
            delete(delete_feedback),
        )
        .route(
            "/courses/{course_id}/ai_experiences/{ai_experience_id}/conversations/{id}/evaluation",
            get(evaluation),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn routes_build() {
        let _ = router();
    }

    #[test]
    fn read_field_distinguishes_absent_from_null() {
        let body = json!({ "title": null, "facts": "f" });
        let map = body.as_object().unwrap();
        assert_eq!(read_field(map, "title"), Some(None));
        assert_eq!(read_field(map, "facts"), Some(Some("f".to_string())));
        assert_eq!(read_field(map, "missing"), None);
    }

    #[test]
    fn read_experience_fields_reads_the_wrapped_body() {
        let body = json!({ "ai_experience": { "title": "T", "learning_objective": "L" } });
        let fields = read_experience_fields(&body);
        assert_eq!(fields.title, Some(Some("T".to_string())));
        assert_eq!(fields.learning_objective, Some(Some("L".to_string())));
        assert_eq!(fields.workflow_state, None);
    }

    #[test]
    fn read_experience_fields_reads_the_flat_body() {
        let body = json!({ "title": "T", "context_file_ids": [1, "2"] });
        let fields = read_experience_fields(&body);
        assert_eq!(fields.title, Some(Some("T".to_string())));
        assert_eq!(
            fields.context_file_ids,
            Some(vec!["1".to_string(), "2".to_string()])
        );
    }

    #[test]
    fn validate_experience_values_reports_the_rails_errors() {
        let values = ExperienceValues::for_create(&ExperienceFields::default());
        let errors = validate_experience_values(&values);
        assert_eq!(errors.get("title").unwrap()[0], "can't be blank");
        assert_eq!(
            errors.get("learning_objective").unwrap()[0],
            "can't be blank"
        );
        assert_eq!(
            errors.get("pedagogical_guidance").unwrap()[0],
            "can't be blank"
        );
    }

    #[test]
    fn validate_experience_values_rejects_a_long_title() {
        let fields = ExperienceFields {
            title: Some(Some("x".repeat(256))),
            learning_objective: Some(Some("L".into())),
            pedagogical_guidance: Some(Some("G".into())),
            ..Default::default()
        };
        let errors = validate_experience_values(&ExperienceValues::for_create(&fields));
        assert_eq!(
            errors.get("title").unwrap()[0],
            "is too long (maximum is 255 characters)"
        );
    }

    #[test]
    fn validate_experience_values_checks_the_workflow_state() {
        let fields = ExperienceFields {
            title: Some(Some("T".into())),
            learning_objective: Some(Some("L".into())),
            pedagogical_guidance: Some(Some("G".into())),
            workflow_state: Some(Some("archived".into())),
            ..Default::default()
        };
        let errors = validate_experience_values(&ExperienceValues::for_create(&fields));
        assert_eq!(
            errors.get("workflow_state").unwrap()[0],
            "is not included in the list"
        );
    }

    #[test]
    fn validate_experience_values_accepts_a_complete_body() {
        let fields = ExperienceFields {
            title: Some(Some("T".into())),
            learning_objective: Some(Some("L".into())),
            pedagogical_guidance: Some(Some("G".into())),
            workflow_state: Some(Some("published".into())),
            ..Default::default()
        };
        let errors = validate_experience_values(&ExperienceValues::for_create(&fields));
        assert!(errors.is_empty());
    }

    #[test]
    fn an_update_keeps_the_stored_values_of_the_untouched_fields() {
        let row = AiExperienceRow {
            id: 1,
            course_id: 2,
            account_id: 1,
            root_account_id: 1,
            title: "Stored".into(),
            description: None,
            facts: None,
            learning_objective: "Stored objective".into(),
            pedagogical_guidance: "Stored guidance".into(),
            workflow_state: "published".into(),
            llm_conversation_context_id: None,
            context_index_status: "not_started".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let fields = ExperienceFields {
            title: Some(Some("New".into())),
            ..Default::default()
        };
        let values = ExperienceValues::for_update(&fields, &row);
        let errors = validate_experience_values(&values);
        assert!(errors.is_empty(), "errors: {errors:?}");
        assert_eq!(values.title.as_deref(), Some("New"));
        assert_eq!(
            values.learning_objective.as_deref(),
            Some("Stored objective")
        );
        assert_eq!(values.workflow_state, "published");
    }
}
