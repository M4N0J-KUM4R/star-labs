//! Canvas `GradingStandardsApiController` — a faithful port of
//! `canvas-src/app/controllers/grading_standards_api_controller.rb`.
//!
//! # Routes
//!
//! | Method | Path                                           | Canvas action   |
//! |--------|------------------------------------------------|-----------------|
//! | GET    | `/accounts/{accountId}/grading_standards`      | `context_index` |
//! | GET    | `/accounts/{accountId}/grading_standards/{id}` | `context_show`  |
//! | POST   | `/accounts/{accountId}/grading_standards`      | `create`        |
//! | PUT    | `/courses/{courseId}/grading_standards/{id}`   | `update`        |
//! | PUT    | `/accounts/{accountId}/grading_standards/{id}` | `update`        |
//! | DELETE | `/accounts/{accountId}/grading_standards/{id}` | `destroy`       |
//!
//! The path strings are byte-identical to the ones `lib.rs` already registers
//! for the course scope, so axum merges the two method routers (GET/DELETE in
//! `lib.rs`, PUT here) into the GET/PUT/DELETE trio Canvas serves on that
//! path. A *different* spelling of the same path shape would be a hard error
//! (`Insertion failed due to conflict`) when the two routers are merged,
//! because matchit normalises parameter names.
//!
//! # DDL
//!
//! None required. The routes use `grading_standards` and
//! `grading_scheme_entries` (migration `0027_grading_standards.sql`) including
//! the `account_id`, `points_based` and `scaling_factor` columns added by
//! `0072_grading_schemes.sql`.
//!
//! # Faithfulness notes
//!
//! * The response body is `Api::V1::GradingStandard#grading_standard_json`:
//!   `id`, `title`, `context_type`, `context_id`, `scaling_factor`,
//!   `points_based` and `grading_scheme` (an array of
//!   `{name, value, calculated_value}`). `context_type` is `"Account"` or
//!   `"Course"` and `context_id` the id of that context.
//! * `create` and `update` answer **200** on success (Canvas renders the
//!   standard without a `:created` status) and **400** with Rails' error hash
//!   (`{"data": ["..."]}`) when the model is invalid.
//! * `destroy` answers **200** with the standard it just deleted; the row is
//!   soft-deleted (`workflow_state = 'deleted'`).
//! * Permission order follows the Ruby action bodies exactly:
//!   `context_index`/`context_show` check the context right **before** the
//!   standard lookup (403 before 404), while `update`/`destroy` look the
//!   standard up inside the context **before** the permission check (404
//!   before 403).
//! * The context itself is resolved first: a missing account/course is a 404,
//!   an anonymous caller is a 401 (the `AuthUser` extractor).
//! * `GradingStandard#data_with_calculated_value` renders the scheme
//!   highest-bound first; `calculated_value` is
//!   `(value * scaling_factor).round(2)` where the scaling factor is 100 for
//!   percentage schemes and the scheme's own `scaling_factor` for points-based
//!   schemes.
//! * `GradingStandard#standard_data=` divides the submitted values by the
//!   scheme's scaling factor (100 for percentage schemes) and stores them as
//!   lower bounds; the request values are therefore 0–100 integers. A row
//!   without a name or without a value is skipped, and a repeated name keeps
//!   the last value.
//! * `GradingStandard#assessed_assignment?` cannot be answered by this schema
//!   (`assignments` has no `grading_standard_id` column and `accounts` has no
//!   `grading_standard_id` column), so the standard is never "used for
//!   grading" here: the `update` route never answers the "Cannot modify
//!   grading scheme data" 400 and `destroy` never fails with "You cannot
//!   delete a used scheme".
//! * `GradingStandard#user` is not stored: `grading_standards` has no
//!   `user_id` column.
//! * The account rights are the crate's usual approximation of the Canvas
//!   policies (`can_read_account` / `can_manage_account`): the workspace has no
//!   role-override tables, so an account admin, a site admin or somebody
//!   enrolled in a course of the account may read, and only an account admin
//!   or a site admin may manage.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{
    can_manage_account, can_read_account, check_enrolled_or_admin, check_teacher_or_admin,
    course_exists,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Canvas `GradingStandard.maximum_string_length`.
const MAX_NAME_LENGTH: usize = 255;

/// The number of decimals `GradingStandard#data=` keeps.
const DATA_DECIMALS: i32 = 4;

/// The number of decimals `data_with_calculated_value` rounds to.
const CALCULATED_DECIMALS: i32 = 2;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// The context a grading standard belongs to (`GradingStandard#context`).
#[derive(Debug, Clone, Copy)]
enum Context {
    Account(i64),
    Course(i64),
}

impl Context {
    /// Canvas renders the polymorphic context type.
    fn context_type(&self) -> &'static str {
        match self {
            Context::Account(_) => "Account",
            Context::Course(_) => "Course",
        }
    }

    /// Canvas renders the polymorphic context id.
    fn id(&self) -> i64 {
        match self {
            Context::Account(id) | Context::Course(id) => *id,
        }
    }
}

/// A `grading_standards` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct StandardRow {
    id: i64,
    title: String,
    points_based: bool,
    scaling_factor: f64,
}

/// One entry of the scheme data (`GradingStandard#data`).
#[derive(Debug, Clone)]
struct SchemeDatum {
    /// The name as submitted (`trim_whitespace` runs on save).
    name: String,
    /// The lower bound of the grade range.
    value: f64,
}

/// One entry of `GradingStandard#data_with_calculated_value`.
#[derive(Debug, Clone, Serialize)]
struct SchemeEntryJson {
    name: String,
    value: f64,
    calculated_value: f64,
}

/// `Api::V1::GradingStandard#grading_standard_json`.
#[derive(Debug, Clone, Serialize)]
struct GradingStandardJson {
    id: i64,
    title: String,
    context_type: &'static str,
    context_id: i64,
    scaling_factor: f64,
    points_based: bool,
    grading_scheme: Vec<SchemeEntryJson>,
}

/// The body of the `create` and `update` routes.
///
/// Every field is optional: Canvas decides what to do with the request from
/// the *presence* of the keys (`params.key?(:grading_scheme_entry)`), not from
/// their value.
#[derive(Debug, Default, Deserialize)]
pub struct GradingStandardParams {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub points_based: Option<bool>,
    #[serde(default)]
    pub scaling_factor: Option<f64>,
    #[serde(default)]
    pub grading_scheme_entry: Option<Vec<SchemeEntryParam>>,
}

/// One `grading_scheme_entry[][name]` / `[][value]` pair.
///
/// Both keys are optional: `GradingStandard#standard_data=` skips a row
/// without a name or without a value.
#[derive(Debug, Deserialize)]
pub struct SchemeEntryParam {
    pub name: Option<String>,
    pub value: Option<f64>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// Canvas `require_context`: the context of the request must exist.
///
/// An account is looked up in `Account.active` (`workflow_state <> 'deleted'`),
/// a course in `Course` (an API request carries a token, so a course of any
/// state is found).
async fn require_context(pool: &PgPool, ctx: Context) -> Result<(), ApiError> {
    match ctx {
        Context::Account(account_id) => {
            let row: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted' LIMIT 1",
            )
            .bind(account_id)
            .fetch_optional(pool)
            .await?;
            match row {
                Some(_) => Ok(()),
                None => Err(ApiError::NotFound),
            }
        }
        Context::Course(course_id) => course_exists(pool, course_id).await,
    }
}

/// Canvas `authorized_action(@context, @current_user, :read)`.
async fn can_read_context(pool: &PgPool, ctx: Context, auth: &AuthUser) -> Result<bool, ApiError> {
    match ctx {
        Context::Account(account_id) => can_read_account(pool, auth.id, account_id, auth).await,
        Context::Course(course_id) => check_enrolled_or_admin(pool, auth.id, course_id, auth).await,
    }
}

/// Canvas `authorized_action(@context, @current_user, :manage_grading_schemes)`
/// and `authorized_action(grading_standard, @current_user, :manage)` — the
/// standard inherits the right of its context.
async fn can_manage_context(
    pool: &PgPool,
    ctx: Context,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    match ctx {
        Context::Account(account_id) => can_manage_account(pool, auth.id, account_id, auth).await,
        Context::Course(course_id) => check_teacher_or_admin(pool, auth.id, course_id, auth).await,
    }
}

// ---------------------------------------------------------------------------
// Data access
// ---------------------------------------------------------------------------

/// `@context.grading_standards` — the standards of the context, deleted ones
/// excluded (`has_many :grading_standards, -> { where("workflow_state<>'deleted'") }`).
async fn list_for_context(pool: &PgPool, ctx: Context) -> Result<Vec<StandardRow>, ApiError> {
    let rows = match ctx {
        Context::Account(account_id) => {
            sqlx::query_as::<_, StandardRow>(
                r#"SELECT id, title, points_based, scaling_factor
                   FROM grading_standards
                   WHERE account_id = $1 AND workflow_state <> 'deleted'
                   ORDER BY id ASC"#,
            )
            .bind(account_id)
            .fetch_all(pool)
            .await?
        }
        Context::Course(course_id) => {
            sqlx::query_as::<_, StandardRow>(
                r#"SELECT id, title, points_based, scaling_factor
                   FROM grading_standards
                   WHERE course_id = $1 AND workflow_state <> 'deleted'
                   ORDER BY id ASC"#,
            )
            .bind(course_id)
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows)
}

/// `@context.grading_standards.find(params[:grading_standard_id])` — a standard
/// of another context (or a deleted one) is not found.
async fn find_for_context(
    pool: &PgPool,
    ctx: Context,
    standard_id: i64,
) -> Result<Option<StandardRow>, ApiError> {
    let row = match ctx {
        Context::Account(account_id) => {
            sqlx::query_as::<_, StandardRow>(
                r#"SELECT id, title, points_based, scaling_factor
                   FROM grading_standards
                   WHERE id = $1 AND account_id = $2 AND workflow_state <> 'deleted'"#,
            )
            .bind(standard_id)
            .bind(account_id)
            .fetch_optional(pool)
            .await?
        }
        Context::Course(course_id) => {
            sqlx::query_as::<_, StandardRow>(
                r#"SELECT id, title, points_based, scaling_factor
                   FROM grading_standards
                   WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'"#,
            )
            .bind(standard_id)
            .bind(course_id)
            .fetch_optional(pool)
            .await?
        }
    };
    Ok(row)
}

/// The stored scheme data of a standard, highest bound first.
async fn load_entries(pool: &PgPool, standard_id: i64) -> Result<Vec<SchemeDatum>, ApiError> {
    let rows: Vec<(String, f64)> = sqlx::query_as(
        r#"SELECT name, min_score::float8
           FROM grading_scheme_entries
           WHERE grading_standard_id = $1
           ORDER BY position ASC, id ASC"#,
    )
    .bind(standard_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(name, value)| SchemeDatum { name, value })
        .collect())
}

/// Insert the scheme data of a standard.
async fn insert_entries(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    standard_id: i64,
    data: &[SchemeDatum],
) -> Result<(), ApiError> {
    for (position, datum) in data.iter().enumerate() {
        sqlx::query(
            r#"INSERT INTO grading_scheme_entries (grading_standard_id, name, min_score, position)
               VALUES ($1, $2, $3, $4)"#,
        )
        .bind(standard_id)
        // `GradingStandard#trim_whitespace` strips the scheme names on save.
        .bind(datum.name.trim())
        .bind(datum.value)
        .bind(position as i32)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Model behaviour
// ---------------------------------------------------------------------------

/// Round like Ruby: `Float#round(n)`.
fn round_to(value: f64, decimals: i32) -> f64 {
    let factor = 10f64.powi(decimals);
    (value * factor).round() / factor
}

/// `GradingStandard#standard_data=`: the submitted values are divided by the
/// scaling factor of the scheme (100 for percentage schemes) and stored as
/// lower bounds. A repeated name keeps the last value, and the data is
/// sorted from the highest bound to the lowest one.
fn build_scheme_data(
    entries: Option<&[SchemeEntryParam]>,
    points_based: bool,
    scaling_factor: f64,
) -> Vec<SchemeDatum> {
    let divisor = if points_based { scaling_factor } else { 100.0 };
    let mut data: Vec<SchemeDatum> = Vec::new();

    for entry in entries.unwrap_or(&[]) {
        // `standard_data=` skips a row without a name or without a value.
        let (name, raw_value) = match (entry.name.as_ref(), entry.value) {
            (Some(name), Some(raw_value)) => (name, raw_value),
            _ => continue,
        };
        let value = round_to(raw_value / divisor, DATA_DECIMALS);
        match data.iter_mut().find(|datum| datum.name == name.as_str()) {
            Some(existing) => existing.value = value,
            None => data.push(SchemeDatum {
                name: name.clone(),
                value,
            }),
        }
    }

    data.sort_by(|a, b| {
        b.value
            .partial_cmp(&a.value)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    data
}

/// The validations of `GradingStandard`, in the order Rails runs them.
///
/// Returns `None` when the record is valid, otherwise the error hash Rails
/// renders (`render json: @standard.errors`).
fn validation_errors(
    data: &[SchemeDatum],
    points_based: bool,
    scaling_factor: f64,
) -> Option<Value> {
    // `validates :data, presence: true`
    if data.is_empty() {
        return Some(json!({ "data": ["can't be blank"] }));
    }

    let mut data_errors: Vec<&str> = Vec::new();

    // `valid_grading_scheme_data`
    if data.iter().any(|datum| datum.value < 0.0) {
        data_errors.push("grading scheme values cannot be negative");
    }
    let mut values: Vec<f64> = Vec::with_capacity(data.len());
    let mut duplicate = false;
    for datum in data {
        if values.contains(&datum.value) {
            duplicate = true;
        }
        values.push(datum.value);
    }
    if duplicate {
        data_errors.push("grading scheme cannot contain duplicate values");
    }
    if data
        .iter()
        .any(|datum| datum.name.chars().count() > MAX_NAME_LENGTH)
    {
        data_errors.push("a grading scheme name is too long");
    }

    // `full_range_scheme`
    if !data.iter().any(|datum| datum.value.abs() < f64::EPSILON) {
        data_errors.push("grading schemes must have 0% for the lowest grade");
    }

    let mut errors = serde_json::Map::new();
    if !data_errors.is_empty() {
        errors.insert("data".to_string(), json!(data_errors));
    }

    // `scaling_factor_points_based`
    if !points_based && scaling_factor != 1.0 {
        errors.insert(
            "scaling_factor".to_string(),
            json!(["must be 1 if scheme points_based is false"]),
        );
    }

    if errors.is_empty() {
        None
    } else {
        Some(Value::Object(errors))
    }
}

/// `Api::V1::GradingStandard#grading_standard_json`.
async fn standard_json(
    pool: &PgPool,
    row: StandardRow,
    ctx: Context,
) -> Result<GradingStandardJson, ApiError> {
    let entries = load_entries(pool, row.id).await?;
    let scaling = if row.points_based {
        row.scaling_factor
    } else {
        100.0
    };

    Ok(GradingStandardJson {
        id: row.id,
        title: row.title,
        context_type: ctx.context_type(),
        context_id: ctx.id(),
        scaling_factor: row.scaling_factor,
        points_based: row.points_based,
        grading_scheme: entries
            .into_iter()
            .map(|datum| SchemeEntryJson {
                name: datum.name,
                value: datum.value,
                calculated_value: round_to(datum.value * scaling, CALCULATED_DECIMALS),
            })
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /accounts/{accountId}/grading_standards — Canvas `context_index`.
async fn context_index_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = Context::Account(account_id);
    require_context(&state.db, ctx).await?;

    if !can_read_context(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows = list_for_context(&state.db, ctx).await?;
    let mut standards = Vec::with_capacity(rows.len());
    for row in rows {
        standards.push(standard_json(&state.db, row, ctx).await?);
    }

    Ok((StatusCode::OK, Json(standards)).into_response())
}

/// GET /accounts/{accountId}/grading_standards/{id} — Canvas `context_show`.
async fn context_show_account(
    State(state): State<AppState>,
    Path((account_id, standard_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = Context::Account(account_id);
    require_context(&state.db, ctx).await?;

    // Canvas authorizes the context before it looks the standard up.
    if !can_read_context(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row = find_for_context(&state.db, ctx, standard_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok((
        StatusCode::OK,
        Json(standard_json(&state.db, row, ctx).await?),
    )
        .into_response())
}

/// POST /accounts/{accountId}/grading_standards — Canvas `create`.
async fn create_account_grading_standard(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<GradingStandardParams>,
) -> Result<Response, ApiError> {
    let ctx = Context::Account(account_id);
    require_context(&state.db, ctx).await?;

    if !can_manage_context(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let points_based = payload.points_based.unwrap_or(false);
    let scaling_factor = payload.scaling_factor.unwrap_or(1.0);
    let title = payload.title.unwrap_or_default();
    let data = build_scheme_data(
        payload.grading_scheme_entry.as_deref(),
        points_based,
        scaling_factor,
    );

    if let Some(errors) = validation_errors(&data, points_based, scaling_factor) {
        return Ok((StatusCode::BAD_REQUEST, Json(errors)).into_response());
    }

    let mut tx = state.db.begin().await?;
    let (id,): (i64,) = sqlx::query_as(
        r#"INSERT INTO grading_standards (account_id, title, points_based, scaling_factor)
           VALUES ($1, $2, $3, $4)
           RETURNING id"#,
    )
    .bind(account_id)
    .bind(&title)
    .bind(points_based)
    .bind(scaling_factor)
    .fetch_one(&mut *tx)
    .await?;
    insert_entries(&mut tx, id, &data).await?;
    tx.commit().await?;

    let row = StandardRow {
        id,
        title,
        points_based,
        scaling_factor,
    };
    Ok((
        StatusCode::OK,
        Json(standard_json(&state.db, row, ctx).await?),
    )
        .into_response())
}

/// PUT /courses/{courseId}/grading_standards/{id} — Canvas `update`.
async fn update_course_grading_standard(
    State(state): State<AppState>,
    Path((course_id, standard_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<GradingStandardParams>,
) -> Result<Response, ApiError> {
    update_grading_standard(
        &state,
        Context::Course(course_id),
        standard_id,
        &auth,
        payload,
    )
    .await
}

/// PUT /accounts/{accountId}/grading_standards/{id} — Canvas `update`.
async fn update_account_grading_standard(
    State(state): State<AppState>,
    Path((account_id, standard_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<GradingStandardParams>,
) -> Result<Response, ApiError> {
    update_grading_standard(
        &state,
        Context::Account(account_id),
        standard_id,
        &auth,
        payload,
    )
    .await
}

/// Canvas `update`.
///
/// The standard is looked up inside the context before the permission is
/// checked, so a standard of another context is a 404 and not a 403.
async fn update_grading_standard(
    state: &AppState,
    ctx: Context,
    standard_id: i64,
    auth: &AuthUser,
    payload: GradingStandardParams,
) -> Result<Response, ApiError> {
    require_context(&state.db, ctx).await?;

    let row = find_for_context(&state.db, ctx, standard_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if !can_manage_context(&state.db, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }

    // `params.key?(:grading_scheme_entry) || params.key?(:points_based) ||
    //  params.key?(:scaling_factor)` decides whether the scheme is rebuilt.
    let rebuilds_scheme = payload.grading_scheme_entry.is_some()
        || payload.points_based.is_some()
        || payload.scaling_factor.is_some();

    let points_based = payload.points_based.unwrap_or(row.points_based);
    let scaling_factor = payload.scaling_factor.unwrap_or(row.scaling_factor);
    let title = payload.title.unwrap_or_else(|| row.title.clone());

    let new_data = if rebuilds_scheme {
        Some(build_scheme_data(
            payload.grading_scheme_entry.as_deref(),
            points_based,
            scaling_factor,
        ))
    } else {
        None
    };

    // A title-only update still validates the stored scheme data.
    let data = match &new_data {
        Some(data) => data.clone(),
        None => load_entries(&state.db, row.id).await?,
    };

    if let Some(errors) = validation_errors(&data, points_based, scaling_factor) {
        return Ok((StatusCode::BAD_REQUEST, Json(errors)).into_response());
    }

    let mut tx = state.db.begin().await?;
    sqlx::query(
        r#"UPDATE grading_standards
           SET title = $2, points_based = $3, scaling_factor = $4, updated_at = now()
           WHERE id = $1"#,
    )
    .bind(row.id)
    .bind(&title)
    .bind(points_based)
    .bind(scaling_factor)
    .execute(&mut *tx)
    .await?;

    if let Some(data) = &new_data {
        sqlx::query("DELETE FROM grading_scheme_entries WHERE grading_standard_id = $1")
            .bind(row.id)
            .execute(&mut *tx)
            .await?;
        insert_entries(&mut tx, row.id, data).await?;
    }
    tx.commit().await?;

    let updated = StandardRow {
        id: row.id,
        title,
        points_based,
        scaling_factor,
    };
    Ok((
        StatusCode::OK,
        Json(standard_json(&state.db, updated, ctx).await?),
    )
        .into_response())
}

/// DELETE /accounts/{accountId}/grading_standards/{id} — Canvas `destroy`.
async fn destroy_account_grading_standard(
    State(state): State<AppState>,
    Path((account_id, standard_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = Context::Account(account_id);
    require_context(&state.db, ctx).await?;

    // Canvas looks the standard up inside the context before it checks the
    // permission.
    let row = find_for_context(&state.db, ctx, standard_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if !can_manage_context(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    sqlx::query(
        "UPDATE grading_standards SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(row.id)
    .execute(&state.db)
    .await?;

    // Canvas renders the standard it just deleted.
    Ok((
        StatusCode::OK,
        Json(standard_json(&state.db, row, ctx).await?),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The Canvas `GradingStandardsApiController` routes.
pub fn router() -> Router<AppState> {
    Router::new()
        // Canvas `GradingStandardsApiController#context_index` / `#create`
        // (config/routes.rb:2698 and :2701).
        .route(
            "/accounts/{accountId}/grading_standards",
            get(context_index_account).post(create_account_grading_standard),
        )
        // Canvas `GradingStandardsApiController#context_show` / `#update` /
        // `#destroy` (config/routes.rb:2700, :2704 and :2706).
        .route(
            "/accounts/{accountId}/grading_standards/{id}",
            get(context_show_account)
                .put(update_account_grading_standard)
                .delete(destroy_account_grading_standard),
        )
        // Canvas `GradingStandardsApiController#update` for the course scope
        // (config/routes.rb:2703). The path string is byte-identical to the one
        // `lib.rs` registers for the same path, so axum merges the two method
        // routers (GET/DELETE there, PUT here) into the GET/PUT/DELETE trio
        // Canvas serves on this path.
        .route(
            "/courses/{courseId}/grading_standards/{id}",
            put(update_course_grading_standard),
        )
}
