//! Grading period sets — Canvas `GradingPeriodSetsController` (parity gap).
//!
//! Routes (JSON-API style envelope, Canvas `serialize_json_api`):
//!   GET    /accounts/{accountId}/grading_period_sets       — index
//!   POST   /accounts/{accountId}/grading_period_sets       — create (201)
//!   PATCH  /accounts/{accountId}/grading_period_sets/{id}  — update (204)
//!   DELETE /accounts/{accountId}/grading_period_sets/{id}  — destroy (204)
//!
//! Canvas authorization:
//!   - index: `read` on the account
//!   - create/update/destroy: the context must be a ROOT account and the
//!     caller must have `manage` (check_manage_rights)
//! - Unknown account → 404 (get_context).
//! - Update of a set not belonging to the account → 404
//!   (GradingPeriodGroup.for(@context).find).
//! - Save failures → 422 with the Rails error shape.
//!
//! Serializer (GradingPeriodSetSerializer): id, title, weighted,
//! display_totals_for_all_grading_periods, account_id, course_id,
//! grading_periods (active, serialized flat), permissions, created_at.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Rows and serializers
// ---------------------------------------------------------------------------

/// `grading_period_groups` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct GradingPeriodSetRow {
    id: i64,
    account_id: Option<i64>,
    course_id: Option<i64>,
    title: Option<String>,
    weighted: bool,
    display_totals_for_all_grading_periods: bool,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Active grading period rows for one group (serialized flat, Canvas nests
/// GradingPeriodSerializer output under `grading_periods`).
#[derive(Debug, Clone, sqlx::FromRow)]
struct SetPeriodRow {
    id: i64,
    title: String,
    start_date: chrono::DateTime<chrono::Utc>,
    end_date: chrono::DateTime<chrono::Utc>,
    close_date: Option<chrono::DateTime<chrono::Utc>>,
    weight: Option<f64>,
}

impl From<SetPeriodRow> for Value {
    fn from(p: SetPeriodRow) -> Value {
        json!({
            "id": p.id,
            "title": p.title,
            "start_date": p.start_date.to_rfc3339(),
            "end_date": p.end_date.to_rfc3339(),
            "close_date": p.close_date.map(|d| d.to_rfc3339()),
            "weight": p.weight,
        })
    }
}

/// The `grading_period_set` JSON-API resource object.
fn serialize_set(row: &GradingPeriodSetRow, periods: Vec<Value>, can_manage: bool) -> Value {
    json!({
        "id": row.id.to_string(),
        "title": row.title,
        "weighted": row.weighted,
        "display_totals_for_all_grading_periods": row.display_totals_for_all_grading_periods,
        "account_id": row.account_id,
        "course_id": row.course_id,
        "grading_periods": periods,
        "permissions": {
            "read": true,
            "manage": can_manage,
        },
        "created_at": row.created_at.to_rfc3339(),
    })
}

// ---------------------------------------------------------------------------
// Request payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GradingPeriodSetPayload {
    pub grading_period_set: Option<SetParams>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SetParams {
    pub title: Option<String>,
    pub weighted: Option<bool>,
    pub display_totals_for_all_grading_periods: Option<bool>,
    /// Accepted for Canvas compatibility; the workspace has no
    /// enrollment_terms table yet (see migration 0230).
    #[allow(dead_code)]
    pub enrollment_term_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize)]
pub struct IndexQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `get_context`: the account must exist (404 otherwise).
async fn load_account(pool: &PgPool, account_id: i64) -> Result<(i64, bool), ApiError> {
    let row: Option<(i64, bool)> = sqlx::query_as(
        "SELECT id, (parent_account_id IS NULL) AS is_root \
         FROM accounts WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

/// `check_manage_rights`: root account + manage right. Root check is
/// `account_users` admin membership in this workspace.
async fn can_manage_account(
    pool: &PgPool,
    account_id: i64,
    user: &AuthUser,
) -> Result<bool, ApiError> {
    let is_admin: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM account_users \
         WHERE account_id = $1 AND user_id = $2 AND role = 'admin')",
    )
    .bind(account_id)
    .bind(user.id)
    .fetch_one(pool)
    .await?;
    Ok(is_admin)
}

/// `check_read_rights`: read on the account (admin) or any enrollment in a
/// course of the account.
async fn can_read_account(
    pool: &PgPool,
    account_id: i64,
    user: &AuthUser,
) -> Result<bool, ApiError> {
    let allowed: bool = sqlx::query_scalar(
        "SELECT (SELECT EXISTS(SELECT 1 FROM account_users \
                    WHERE account_id = $1 AND user_id = $2 AND role = 'admin') \
              OR SELECT EXISTS(SELECT 1 FROM enrollments e \
                    JOIN courses c ON c.id = e.course_id \
                    WHERE c.account_id = $1 AND e.user_id = $2 \
                      AND e.workflow_state <> 'deleted'))",
    )
    .bind(account_id)
    .bind(user.id)
    .fetch_one(pool)
    .await?;
    Ok(allowed)
}

async fn load_set(
    pool: &PgPool,
    account_id: i64,
    set_id: i64,
) -> Result<GradingPeriodSetRow, ApiError> {
    sqlx::query_as(
        "SELECT id, account_id, course_id, title, weighted, \
                display_totals_for_all_grading_periods, created_at \
         FROM grading_period_groups \
         WHERE id = $1 AND account_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(set_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

async fn load_periods(pool: &PgPool, set_id: i64) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<SetPeriodRow> = sqlx::query_as(
        "SELECT id, title, start_date, end_date, close_date, weight \
         FROM grading_periods \
         WHERE grading_period_group_id = $1 AND workflow_state = 'active' \
         ORDER BY start_date",
    )
    .bind(set_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Rails-style error payload for save failures (`render json: errors`).
fn save_error(message: &str) -> ApiError {
    ApiError::Validation(message.to_string())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /accounts/{accountId}/grading_period_sets
async fn list_grading_period_sets(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    Query(query): Query<IndexQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let (account_id, _is_root) = load_account(&state.db, account_id).await?;
    if !can_read_account(&state.db, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let rows: Vec<GradingPeriodSetRow> = sqlx::query_as(
        "SELECT id, account_id, course_id, title, weighted, \
                display_totals_for_all_grading_periods, created_at \
         FROM grading_period_groups \
         WHERE account_id = $1 AND workflow_state <> 'deleted' \
         ORDER BY id LIMIT $2 OFFSET $3",
    )
    .bind(account_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let can_manage = can_manage_account(&state.db, account_id, &auth).await?;
    let mut data = Vec::with_capacity(rows.len());
    for r in &rows {
        let periods = load_periods(&state.db, r.id).await.unwrap_or_default();
        data.push(serialize_set(r, periods, can_manage));
    }

    Ok(Json(json!({ "grading_period_sets": data })))
}

/// POST /accounts/{accountId}/grading_period_sets
async fn create_grading_period_set(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<GradingPeriodSetPayload>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (account_id, is_root) = load_account(&state.db, account_id).await?;
    if !is_root || !can_manage_account(&state.db, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let params = payload.grading_period_set.unwrap_or_default();
    let title = params.title.as_deref().map(str::trim).unwrap_or("");
    if title.is_empty() {
        return Err(save_error("title can't be blank"));
    }

    let row: GradingPeriodSetRow = sqlx::query_as(
        "INSERT INTO grading_period_groups \
            (account_id, title, weighted, display_totals_for_all_grading_periods) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, account_id, course_id, title, weighted, \
                   display_totals_for_all_grading_periods, created_at",
    )
    .bind(account_id)
    .bind(title)
    .bind(params.weighted.unwrap_or(false))
    .bind(
        params
            .display_totals_for_all_grading_periods
            .unwrap_or(false),
    )
    .fetch_one(&state.db)
    .await?;

    let periods = load_periods(&state.db, row.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "grading_period_set": serialize_set(&row, periods, true)
        })),
    ))
}

/// PATCH /accounts/{accountId}/grading_period_sets/{id}
async fn update_grading_period_set(
    State(state): State<AppState>,
    Path((account_id, set_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<GradingPeriodSetPayload>,
) -> Result<StatusCode, ApiError> {
    let (account_id, is_root) = load_account(&state.db, account_id).await?;
    if !is_root || !can_manage_account(&state.db, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let existing = load_set(&state.db, account_id, set_id).await?;
    let params = payload.grading_period_set.unwrap_or_default();

    let title = match params.title.as_deref() {
        Some(t) if t.trim().is_empty() => {
            return Err(save_error("title can't be blank"));
        }
        Some(t) => Some(t.to_string()),
        None => None,
    };

    sqlx::query(
        "UPDATE grading_period_groups SET \
            title = COALESCE($2, title), \
            weighted = COALESCE($3, weighted), \
            display_totals_for_all_grading_periods = \
                COALESCE($4, display_totals_for_all_grading_periods), \
            updated_at = now() \
         WHERE id = $1",
    )
    .bind(existing.id)
    .bind(title)
    .bind(params.weighted)
    .bind(params.display_totals_for_all_grading_periods)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /accounts/{accountId}/grading_period_sets/{id}
async fn delete_grading_period_set(
    State(state): State<AppState>,
    Path((account_id, set_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let (account_id, is_root) = load_account(&state.db, account_id).await?;
    if !is_root || !can_manage_account(&state.db, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let existing = load_set(&state.db, account_id, set_id).await?;
    // Canvas `destroy` hard-deletes; the group cascade removes its periods.
    sqlx::query("DELETE FROM grading_period_groups WHERE id = $1")
        .bind(existing.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Mount into the crate router (merged by `router()` below).
pub fn grading_period_sets_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{accountId}/grading_period_sets",
            get(list_grading_period_sets).post(create_grading_period_set),
        )
        .route(
            "/accounts/{accountId}/grading_period_sets/{id}",
            axum::routing::patch(update_grading_period_set).delete(delete_grading_period_set),
        )
}
