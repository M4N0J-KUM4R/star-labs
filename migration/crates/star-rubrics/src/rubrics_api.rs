//! star-rubrics — Canvas `RubricsApiController`, account scope.
//!
//! Ground truth: `canvas-src/app/controllers/rubrics_api_controller.rb`
//! (plus `lib/api/v1/rubric.rb`, `app/models/rubric.rb`,
//! `app/models/rubric_import.rb`, `lib/rubric_context.rb`).
//!
//! Routes served here (Canvas `config/routes.rb` → `ApiRouteSet.draw`,
//! `scope :rubrics_api` / `rubrics` resource blocks):
//!
//! | Verb | Canvas path                                                    |
//! |------|----------------------------------------------------------------|
//! | GET  | `/api/v1/accounts/:account_id/rubrics`                         |
//! | GET  | `/api/v1/accounts/:account_id/rubrics/:id`                     |
//! | GET  | `/api/v1/accounts/:account_id/rubrics/:id/used_locations`      |
//! | POST | `/api/v1/accounts/:account_id/rubrics/upload`                   |
//! | GET  | `/api/v1/accounts/:account_id/rubrics/upload/:id`              |
//! | GET  | `/api/v1/courses/:course_id/rubrics/upload/:id`                |
//! | GET  | `/api/v1/accounts/:account_id/rubrics/upload/:id/rubrics`      |
//! | GET  | `/api/v1/courses/:course_id/rubrics/upload/:id/rubrics`        |
//!
//! Two structural differences between Canvas and star-lms are handled here
//! (both are documented in the crate README/ledger by the coordinator):
//!
//! 1. **Account scope.** Canvas stores rubrics in a polymorphic `context`
//!    (`Course` *or* `Account`). star-lms rubrics are course scoped
//!    (`rubrics.course_id NOT NULL REFERENCES courses(id)`), and
//!    `rubric_associations.context_id` also references `courses(id)`. An
//!    "account rubric" is therefore the rubric of one of the account's
//!    courses — the same mapping the crate already uses for
//!    `POST /accounts/{accountId}/rubrics/download_rubrics`.
//! 2. **Upload payload.** Canvas uploads a multipart CSV file
//!    (`params[:attachment]`); star-lms has no attachments table, so the
//!    port accepts a JSON body with `attachment_id`, exactly like the
//!    crate's existing `POST /courses/{courseId}/rubrics/upload`.

use axum::{
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, ApiJson, AppState};

use crate::{
    check_teacher_or_admin, create_rubric_import, fetch_rubric_with_nested, is_admin, Rubric,
    RubricAssessment, RubricAssociation, RubricImportResponse, RubricImportRow,
    UploadRubricRequest, UsedLocationAssignment, UsedLocationCourse, ASSESSMENT_COLUMNS,
    ASSOCIATION_COLUMNS,
};

// ---------------------------------------------------------------------------
// Canvas constants
// ---------------------------------------------------------------------------

/// Canvas `VALID_ASSESSMENT_SCOPES`.
const VALID_ASSESSMENT_SCOPES: [&str; 3] =
    ["assessments", "graded_assessments", "peer_assessments"];

/// Canvas `VALID_ASSOCIATION_SCOPES`.
const VALID_ASSOCIATION_SCOPES: [&str; 4] = [
    "associations",
    "assignment_associations",
    "course_associations",
    "account_associations",
];

/// Canvas `validate_args`: the accepted `style` values.
const VALID_STYLE_ARGS: [&str; 2] = ["full", "comments_only"];

// ---------------------------------------------------------------------------
// Canvas response bodies
// ---------------------------------------------------------------------------

/// Render a JSON body with an explicit status code.
fn json_response(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

/// Canvas `ApplicationController#api_error_json` renders a missing context
/// (raised `ActiveRecord::RecordNotFound`) as `errors: [{ message: ... }]`.
fn context_not_found() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        json!({"errors": [{"message": "The specified resource does not exist."}]}),
    )
}

/// Canvas `RubricsApiController#show` / `#used_locations` render
/// `{"message": "Rubric not found"}` when the rubric is unknown.
fn rubric_not_found() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        json!({"message": "Rubric not found"}),
    )
}

/// Canvas `render_json_unauthorized` (authenticated but not authorized).
///
/// The body is rendered by the shared renderer in `star-common`, so the key
/// order (`status` before `errors`) is Canvas' and not `serde_json`'s
/// alphabetical one.
fn forbidden() -> Response {
    ApiError::Forbidden.into_response()
}

/// Canvas `RubricsApiController#upload_status` renders the ActiveRecord
/// message of the raised `ActiveRecord::RecordNotFound`.
fn import_not_found(id: &str) -> Response {
    let message = if id == "latest" {
        // `raise ActiveRecord::RecordNotFound` without a message.
        "ActiveRecord::RecordNotFound".to_string()
    } else {
        // Rails casts the id to an integer before looking the record up.
        format!(
            "Couldn't find RubricImport with 'id'={}",
            id.parse::<i64>().unwrap_or(0)
        )
    };
    json_response(StatusCode::NOT_FOUND, json!({ "message": message }))
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Canvas `before_action :require_context` + `authorized_action(@context,
/// @current_user, :manage_rubrics)` for an **account** context.
///
/// `manage_rubrics` is an account permission whose `true_for` roles are
/// `AccountAdmin` (`config/initializers/permissions_registry.rb`), so only
/// account administrators may use the account scoped rubric routes. A
/// system admin is accepted as well, like everywhere else in the crate.
async fn account_gate(
    state: &AppState,
    account_id: i64,
    auth: &AuthUser,
) -> Result<Option<Response>, ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE id = $1)")
        .bind(account_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Ok(Some(context_not_found()));
    }

    if !is_admin(auth) && !is_account_admin(&state.db, auth.id, account_id).await? {
        return Ok(Some(forbidden()));
    }

    Ok(None)
}

/// Canvas `before_action :require_context` + `authorized_action(@context,
/// @current_user, :manage_rubrics)` for a **course** context: the course has
/// to exist and the user has to manage its rubrics (teacher or admin).
async fn course_gate(
    state: &AppState,
    course_id: i64,
    auth: &AuthUser,
) -> Result<Option<Response>, ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM courses WHERE id = $1)")
        .bind(course_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Ok(Some(context_not_found()));
    }

    if !check_teacher_or_admin(&state.db, auth.id, course_id, auth).await? {
        return Ok(Some(forbidden()));
    }

    Ok(None)
}

/// Is the user an administrator of the account?
async fn is_account_admin(pool: &PgPool, user_id: i64, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM account_users
        WHERE account_id = $1 AND user_id = $2
          AND role = 'admin' AND workflow_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Rubric lookups
// ---------------------------------------------------------------------------

/// Fetch a rubric that belongs to the account.
///
/// Canvas resolves the rubric through the context's bookmarked rubric
/// associations (`@context.rubric_associations.bookmarked.find_by(...)`).
/// star-lms has no account owned rubrics, so the account's rubrics are the
/// ones owned by the account's courses. Deleted rubrics are excluded, like
/// Canvas' `rubric.deleted?` guard.
async fn fetch_account_rubric(
    pool: &PgPool,
    account_id: i64,
    rubric_id: i64,
) -> Result<Option<Rubric>, ApiError> {
    let rubric: Option<Rubric> = sqlx::query_as(
        r#"
        SELECT r.id, r.course_id, r.title, r.points_possible, r.workflow_state,
               r.created_at, r.updated_at
        FROM rubrics r
        JOIN courses c ON c.id = r.course_id
        WHERE r.id = $1 AND c.account_id = $2 AND r.workflow_state <> 'deleted'
        "#,
    )
    .bind(rubric_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    Ok(rubric)
}

// ---------------------------------------------------------------------------
// `include` / `style` arguments
// ---------------------------------------------------------------------------

/// Canvas `params[:include]` and `params[:style]`.
#[derive(Debug, Default)]
struct ShowArgs {
    includes: Vec<String>,
    style: Option<String>,
}

impl ShowArgs {
    /// Canvas `api_includes` → `Array(params[:include])`.
    ///
    /// Canvas accepts `include[]=assessments` (repeated) as well as
    /// `include=assessments`.
    fn parse(raw: Option<&str>) -> Self {
        let mut includes = Vec::new();
        let mut style = None;

        for pair in raw.unwrap_or_default().split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (key, value),
                None => (pair, ""),
            };
            match percent_decode(key).as_str() {
                "include[]" | "include" => {
                    for part in percent_decode(value).split(',') {
                        let part = part.trim();
                        if !part.is_empty() {
                            includes.push(part.to_string());
                        }
                    }
                }
                "style" => style = Some(percent_decode(value)),
                _ => {}
            }
        }

        Self { includes, style }
    }

    /// Canvas `(api_includes & VALID_PARAMS)[0]`: the first requested include
    /// that belongs to the given category.
    fn first_include(&self, valid: &[&str]) -> Option<&str> {
        self.includes
            .iter()
            .map(String::as_str)
            .find(|include| valid.contains(include))
    }
}

/// Minimal percent decoder for query values (`%XX` and `+`).
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 3 <= bytes.len() => {
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

    String::from_utf8_lossy(&out).into_owned()
}

/// Canvas `VALID_INCLUDE_PARAMS`.
fn valid_include_params() -> Vec<&'static str> {
    VALID_ASSESSMENT_SCOPES
        .iter()
        .chain(VALID_ASSOCIATION_SCOPES.iter())
        .copied()
        .collect()
}

/// Canvas `RubricsApiController#validate_args`.
///
/// Returns the rendered error body (`ActiveModel::Errors` serialized as
/// `{ "style": ["..."] }`) or `None` when the arguments are valid.
fn validate_args(args: &ShowArgs) -> Option<Value> {
    let mut errs: Map<String, Value> = Map::new();

    let invalid_style = args
        .style
        .as_deref()
        .is_some_and(|style| !VALID_STYLE_ARGS.contains(&style));
    if invalid_style {
        errs.insert(
            "style".to_string(),
            json!(format!(
                "invalid style requested. Must be one of the following: {}",
                VALID_STYLE_ARGS.join(", ")
            )),
        );
    }

    let invalid: Vec<&str> = args
        .includes
        .iter()
        .map(String::as_str)
        .filter(|include| {
            !VALID_ASSESSMENT_SCOPES.contains(include)
                && !VALID_ASSOCIATION_SCOPES.contains(include)
        })
        .collect();

    if !invalid.is_empty() {
        errs.insert(
            "include".to_string(),
            json!(format!(
                "invalid include value requested. Must be one of the following: {}",
                valid_include_params().join(", ")
            )),
        );
    } else {
        let associations: Vec<&str> = args
            .includes
            .iter()
            .map(String::as_str)
            .filter(|include| VALID_ASSOCIATION_SCOPES.contains(include))
            .collect();
        if associations.len() > 1 {
            errs.insert(
                "include".to_string(),
                json!(format!(
                    "cannot list multiple association includes. Multiple given: {}",
                    associations.join(", ")
                )),
            );
        }

        let assessments: Vec<&str> = args
            .includes
            .iter()
            .map(String::as_str)
            .filter(|include| VALID_ASSESSMENT_SCOPES.contains(include))
            .collect();
        if assessments.len() > 1 {
            errs.insert(
                "include".to_string(),
                json!(format!(
                    "cannot list multiple assessment includes. Multiple given: {}",
                    assessments.join(", ")
                )),
            );
        }

        // Canvas only counts the assessments as requested when exactly one
        // assessment include was given.
        let include_assessments = assessments.len() == 1;
        if args.style.is_some() && !include_assessments {
            errs.insert(
                "style".to_string(),
                json!("invalid parameters. Style parameter passed without requesting assessments"),
            );
        }
    }

    if errs.is_empty() {
        return None;
    }

    // Canvas renders the errors map as-is — every value a plain string
    // (rubrics_api_controller.rb:524-540).
    Some(Value::Object(errs.into_iter().collect()))
}

// ---------------------------------------------------------------------------
// Rubric assessments / associations
// ---------------------------------------------------------------------------

/// Canvas `RubricsApiController#rubric_assessments`.
///
/// Returns `None` when no assessment include was requested — the
/// `assessments` key is then absent from the rubric JSON.
async fn rubric_assessments(
    pool: &PgPool,
    rubric_id: i64,
    args: &ShowArgs,
) -> Result<Option<Vec<RubricAssessment>>, ApiError> {
    let Some(scope) = args.first_include(&VALID_ASSESSMENT_SCOPES) else {
        return Ok(None);
    };

    let rows: Vec<RubricAssessment> = match scope {
        "graded_assessments" => {
            let sql = format!(
                "SELECT {} FROM rubric_assessments \
                 WHERE rubric_id = $1 AND assessment_type = $2 ORDER BY id ASC",
                ASSESSMENT_COLUMNS
            );
            sqlx::query_as(&sql)
                .bind(rubric_id)
                .bind("grading")
                .fetch_all(pool)
                .await?
        }
        "peer_assessments" => {
            let sql = format!(
                "SELECT {} FROM rubric_assessments \
                 WHERE rubric_id = $1 AND assessment_type = $2 ORDER BY id ASC",
                ASSESSMENT_COLUMNS
            );
            sqlx::query_as(&sql)
                .bind(rubric_id)
                .bind("peer_review")
                .fetch_all(pool)
                .await?
        }
        _ => {
            let sql = format!(
                "SELECT {} FROM rubric_assessments WHERE rubric_id = $1 ORDER BY id ASC",
                ASSESSMENT_COLUMNS
            );
            sqlx::query_as(&sql).bind(rubric_id).fetch_all(pool).await?
        }
    };

    Ok(Some(rows))
}

/// Canvas `RubricsApiController#rubric_associations`.
///
/// `rubric.rubric_associations` is scoped to `workflow_state: "active"`.
async fn rubric_associations(
    pool: &PgPool,
    rubric_id: i64,
    args: &ShowArgs,
) -> Result<Option<Vec<RubricAssociation>>, ApiError> {
    let Some(scope) = args.first_include(&VALID_ASSOCIATION_SCOPES) else {
        return Ok(None);
    };

    let rows: Vec<RubricAssociation> = match scope {
        "assignment_associations" => {
            let sql = format!(
                "SELECT {} FROM rubric_associations \
                 WHERE rubric_id = $1 AND workflow_state = 'active' \
                   AND association_type = $2 ORDER BY id ASC",
                ASSOCIATION_COLUMNS
            );
            sqlx::query_as(&sql)
                .bind(rubric_id)
                .bind("Assignment")
                .fetch_all(pool)
                .await?
        }
        "course_associations" => {
            let sql = format!(
                "SELECT {} FROM rubric_associations \
                 WHERE rubric_id = $1 AND workflow_state = 'active' \
                   AND association_type = $2 ORDER BY id ASC",
                ASSOCIATION_COLUMNS
            );
            sqlx::query_as(&sql)
                .bind(rubric_id)
                .bind("Course")
                .fetch_all(pool)
                .await?
        }
        "account_associations" => {
            let sql = format!(
                "SELECT {} FROM rubric_associations \
                 WHERE rubric_id = $1 AND workflow_state = 'active' \
                   AND association_type = $2 ORDER BY id ASC",
                ASSOCIATION_COLUMNS
            );
            sqlx::query_as(&sql)
                .bind(rubric_id)
                .bind("Account")
                .fetch_all(pool)
                .await?
        }
        _ => {
            let sql = format!(
                "SELECT {} FROM rubric_associations \
                 WHERE rubric_id = $1 AND workflow_state = 'active' ORDER BY id ASC",
                ASSOCIATION_COLUMNS
            );
            sqlx::query_as(&sql).bind(rubric_id).fetch_all(pool).await?
        }
    };

    Ok(Some(rows))
}

// ---------------------------------------------------------------------------
// Rubric imports
// ---------------------------------------------------------------------------

/// Canvas `RubricImport.find_latest_rubric_import` /
/// `RubricImport.find_specific_rubric_import`.
///
/// `latest` is Canvas' `.last` (highest id) of the context's imports.
async fn find_import(
    pool: &PgPool,
    context_id: i64,
    context_type: &str,
    id: &str,
) -> Result<Option<RubricImportRow>, ApiError> {
    if id == "latest" {
        let row: Option<RubricImportRow> = sqlx::query_as(
            r#"
            SELECT id, user_id, attachment_id, workflow_state,
                   progress, error_count, error_data, created_at, updated_at
            FROM rubric_imports
            WHERE context_id = $1 AND context_type = $2
            ORDER BY id DESC
            LIMIT 1
            "#,
        )
        .bind(context_id)
        .bind(context_type)
        .fetch_optional(pool)
        .await?;
        return Ok(row);
    }

    // Rails casts the id to an integer; a non numeric id never matches.
    let Ok(import_id) = id.parse::<i64>() else {
        return Ok(None);
    };

    let row: Option<RubricImportRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, attachment_id, workflow_state,
               progress, error_count, error_data, created_at, updated_at
        FROM rubric_imports
        WHERE id = $1 AND context_id = $2 AND context_type = $3
        "#,
    )
    .bind(import_id)
    .bind(context_id)
    .bind(context_type)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Canvas `api_json(import, ...)` plus the `user` and `attachment` slices.
async fn import_response(
    pool: &PgPool,
    mut import: RubricImportResponse,
) -> Result<RubricImportResponse, ApiError> {
    if let Some(user_id) = import.user_id {
        let user_row: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
        if let Some((name,)) = user_row {
            import.user = Some(json!({
                "id": user_id,
                "display_name": name,
            }));
        }
    }

    if let Some(attachment_id) = import.attachment_id {
        // star-lms has no attachments table, so only the id is known.
        import.attachment = Some(json!({ "id": attachment_id }));
    }

    Ok(import)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /accounts/{accountId}/rubrics  → 200
///
/// Canvas `RubricsApiController#index`: the active rubrics of the context.
pub async fn list_account_rubrics(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if let Some(reject) = account_gate(&state, account_id, &auth).await? {
        return Ok(reject);
    }

    // Canvas `Rubric.active` excludes archived, deleted and draft rubrics.
    let rows: Vec<Rubric> = sqlx::query_as(
        r#"
        SELECT r.id, r.course_id, r.title, r.points_possible, r.workflow_state,
               r.created_at, r.updated_at
        FROM rubrics r
        JOIN courses c ON c.id = r.course_id
        WHERE c.account_id = $1
          AND r.workflow_state NOT IN ('archived', 'deleted', 'draft')
        ORDER BY r.created_at ASC, r.id ASC
        "#,
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await?;

    Ok((StatusCode::OK, Json(rows)).into_response())
}

/// GET /accounts/{accountId}/rubrics/{id}  → 200
///
/// Canvas `RubricsApiController#show`.
pub async fn show_account_rubric(
    State(state): State<AppState>,
    Path((account_id, rubric_id)): Path<(i64, i64)>,
    RawQuery(raw): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if let Some(reject) = account_gate(&state, account_id, &auth).await? {
        return Ok(reject);
    }

    // Canvas resolves the rubric first and only then reports the argument
    // errors, so an unknown rubric wins over an invalid `style`.
    let Some(rubric) = fetch_account_rubric(&state.db, account_id, rubric_id).await? else {
        return Ok(rubric_not_found());
    };

    let args = ShowArgs::parse(raw.as_deref());
    if let Some(errors) = validate_args(&args) {
        return Ok(json_response(StatusCode::BAD_REQUEST, errors));
    }

    let nested = fetch_rubric_with_nested(&state.db, rubric.id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let mut body = json!({
        "id": nested.id,
        "course_id": nested.course_id,
        "title": nested.title,
        "points_possible": nested.points_possible,
        "workflow_state": nested.workflow_state,
        "created_at": nested.created_at,
        "updated_at": nested.updated_at,
        "criteria": nested.criteria,
    });

    if let Some(assessments) = rubric_assessments(&state.db, rubric.id, &args).await? {
        body["assessments"] = json!(assessments);
    }
    if let Some(associations) = rubric_associations(&state.db, rubric.id, &args).await? {
        body["associations"] = json!(associations);
    }

    Ok(json_response(StatusCode::OK, body))
}

/// GET /accounts/{accountId}/rubrics/{id}/used_locations  → 200
///
/// Canvas `RubricsApiController#used_locations`.
pub async fn account_rubric_used_locations(
    State(state): State<AppState>,
    Path((account_id, rubric_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if let Some(reject) = account_gate(&state, account_id, &auth).await? {
        return Ok(reject);
    }

    let Some(rubric) = fetch_account_rubric(&state.db, account_id, rubric_id).await? else {
        return Ok(rubric_not_found());
    };

    let locations = used_locations_for(&state.db, rubric.id).await?;
    Ok((StatusCode::OK, Json(locations)).into_response())
}

/// Canvas `RubricsApiController#used_locations_for`.
///
/// Canvas lists every course/assignment the rubric is used in, not only the
/// courses of the context.
async fn used_locations_for(
    pool: &PgPool,
    rubric_id: i64,
) -> Result<Vec<UsedLocationCourse>, ApiError> {
    let rows: Vec<UsedLocationRow> = sqlx::query_as(
        r#"
        SELECT c.id AS course_id,
               c.name AS course_name,
               (c.workflow_state = 'completed'
                OR (c.end_at IS NOT NULL AND c.end_at < now())) AS concluded,
               a.id AS assignment_id,
               a.title AS assignment_title
        FROM assignments a
        JOIN rubric_associations ra
          ON ra.association_id = a.id
         AND ra.association_type = 'Assignment'
         AND ra.workflow_state <> 'deleted'
        JOIN courses c ON c.id = a.course_id
        WHERE ra.rubric_id = $1
          AND c.workflow_state <> 'deleted'
        ORDER BY c.name ASC, c.id ASC, a.title ASC
        "#,
    )
    .bind(rubric_id)
    .fetch_all(pool)
    .await?;

    let mut locations: Vec<UsedLocationCourse> = Vec::new();
    for row in rows {
        let assignment = UsedLocationAssignment {
            id: row.assignment_id,
            title: row.assignment_title,
        };
        match locations.last_mut() {
            Some(course) if course.id == row.course_id => course.assignments.push(assignment),
            _ => locations.push(UsedLocationCourse {
                id: row.course_id,
                name: row.course_name,
                concluded: row.concluded,
                assignments: vec![assignment],
            }),
        }
    }

    Ok(locations)
}

/// Raw row for the `used_locations` join (grouped in Rust by course).
#[derive(Debug, sqlx::FromRow)]
struct UsedLocationRow {
    course_id: i64,
    course_name: String,
    concluded: bool,
    assignment_id: i64,
    assignment_title: String,
}

/// POST /accounts/{accountId}/rubrics/upload  → 200
///
/// Canvas `RubricsApiController#upload`: creates the rubric import and hands
/// the CSV parsing to the asynchronous processor.
pub async fn upload_account_rubric(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<UploadRubricRequest>,
) -> Result<Response, ApiError> {
    if let Some(reject) = account_gate(&state, account_id, &auth).await? {
        return Ok(reject);
    }

    let Some(attachment_id) = payload.attachment_id else {
        // Canvas renders `{"message": "No file attached"}` with a 400.
        return Ok(json_response(
            StatusCode::BAD_REQUEST,
            json!({"message": "No file attached"}),
        ));
    };

    let import = create_rubric_import(
        &state.db,
        account_id,
        "Account",
        Some(auth.id),
        attachment_id,
    )
    .await?;

    let import = import_response(&state.db, import).await?;
    // Canvas renders the import JSON with the default status — 200.
    Ok((StatusCode::OK, Json(import)).into_response())
}

/// GET /accounts/{accountId}/rubrics/upload/{id}  → 200
///
/// Canvas `RubricsApiController#upload_status` for an account context.
pub async fn account_upload_status(
    State(state): State<AppState>,
    Path((account_id, import_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if let Some(reject) = account_gate(&state, account_id, &auth).await? {
        return Ok(reject);
    }

    let Some(row) = find_import(&state.db, account_id, "Account", &import_id).await? else {
        return Ok(import_not_found(&import_id));
    };

    let import = import_response(&state.db, RubricImportResponse::from_row(row)).await?;
    Ok((StatusCode::OK, Json(import)).into_response())
}

/// GET /courses/{courseId}/rubrics/upload/{id}  → 200
///
/// Canvas `RubricsApiController#upload_status` for a course context.
pub async fn course_upload_status(
    State(state): State<AppState>,
    Path((course_id, import_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if let Some(reject) = course_gate(&state, course_id, &auth).await? {
        return Ok(reject);
    }

    let Some(row) = find_import(&state.db, course_id, "Course", &import_id).await? else {
        return Ok(import_not_found(&import_id));
    };

    let import = import_response(&state.db, RubricImportResponse::from_row(row)).await?;
    Ok((StatusCode::OK, Json(import)).into_response())
}

/// GET /accounts/{accountId}/rubrics/upload/{id}/rubrics  → 200
///
/// Canvas `RubricsApiController#rubrics_by_import_id` for an account context.
pub async fn account_rubrics_by_import_id(
    State(state): State<AppState>,
    Path((account_id, import_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if let Some(reject) = account_gate(&state, account_id, &auth).await? {
        return Ok(reject);
    }

    // Rails casts the id to an integer; a non numeric id matches no import.
    let Ok(import_id) = import_id.parse::<i64>() else {
        return Ok((StatusCode::OK, Json(Vec::<Rubric>::new())).into_response());
    };

    let rows: Vec<Rubric> = sqlx::query_as(
        r#"
        SELECT r.id, r.course_id, r.title, r.points_possible, r.workflow_state,
               r.created_at, r.updated_at
        FROM rubrics r
        JOIN courses c ON c.id = r.course_id
        WHERE c.account_id = $1
          AND r.rubric_imports_id = $2
          AND r.workflow_state <> 'deleted'
        ORDER BY r.created_at ASC, r.id ASC
        "#,
    )
    .bind(account_id)
    .bind(import_id)
    .fetch_all(&state.db)
    .await?;

    Ok((StatusCode::OK, Json(rows)).into_response())
}

/// GET /courses/{courseId}/rubrics/upload/{id}/rubrics  → 200
///
/// Canvas `RubricsApiController#rubrics_by_import_id` for a course context.
pub async fn course_rubrics_by_import_id(
    State(state): State<AppState>,
    Path((course_id, import_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if let Some(reject) = course_gate(&state, course_id, &auth).await? {
        return Ok(reject);
    }

    let Ok(import_id) = import_id.parse::<i64>() else {
        return Ok((StatusCode::OK, Json(Vec::<Rubric>::new())).into_response());
    };

    let rows: Vec<Rubric> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, points_possible, workflow_state, created_at, updated_at
        FROM rubrics
        WHERE course_id = $1
          AND rubric_imports_id = $2
          AND workflow_state <> 'deleted'
        ORDER BY created_at ASC, id ASC
        "#,
    )
    .bind(course_id)
    .bind(import_id)
    .fetch_all(&state.db)
    .await?;

    Ok((StatusCode::OK, Json(rows)).into_response())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The Canvas `RubricsApiController` routes of this slice.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounts/{accountId}/rubrics", get(list_account_rubrics))
        .route(
            "/accounts/{accountId}/rubrics/{id}",
            get(show_account_rubric),
        )
        .route(
            "/accounts/{accountId}/rubrics/{id}/used_locations",
            get(account_rubric_used_locations),
        )
        .route(
            "/accounts/{accountId}/rubrics/upload",
            post(upload_account_rubric),
        )
        .route(
            "/accounts/{accountId}/rubrics/upload/{id}",
            get(account_upload_status),
        )
        .route(
            "/courses/{courseId}/rubrics/upload/{id}",
            get(course_upload_status),
        )
        .route(
            "/accounts/{accountId}/rubrics/upload/{id}/rubrics",
            get(account_rubrics_by_import_id),
        )
        .route(
            "/courses/{courseId}/rubrics/upload/{id}/rubrics",
            get(course_rubrics_by_import_id),
        )
}
