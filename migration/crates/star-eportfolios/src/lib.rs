//! star-eportfolios — user ePortfolios and their pages (Canvas
//! EportfoliosApiController) with spam moderation (issue #91, spec
//! `specs/p5-rest-eportfolios.md`).
//!
//! Endpoints (spec §2, 7 routes):
//! - GET    /users/{user_id}/eportfolios                  — index (own, or a
//!   moderator's view of the target user; `include[]=deleted`)
//! - GET    /eportfolios/{id}                            — show
//! - GET    /eportfolios/{eportfolio_id}/pages           — pages (flat,
//!   position-ordered entry list)
//! - DELETE /eportfolios/{id}                            — delete (soft)
//! - PUT    /eportfolios/{eportfolio_id}/restore         — restore (un-delete)
//! - PUT    /eportfolios/{eportfolio_id}/moderate        — moderate (spam_status)
//! - PUT    /users/{user_id}/eportfolios                 — moderate_all
//!
//! Behavior contract (verified against canvas-lms master,
//! EportfoliosApiController + Api::V1::Eportfolio + Eportfolio model):
//! - index: `user == current_user || :moderate_user_content`; the scope is
//!   `user.eportfolios.active` UNLESS the caller is a moderator viewing
//!   ANOTHER user AND `include[]` contains "deleted" — the owner's own
//!   listing stays active-only even with include[]=deleted (upstream line:
//!   `if user == @current_user || params[:include].exclude?("deleted")`).
//!   Ordered by `updated_at`, rendered as a BARE JSON array (upstream
//!   `render json: portfolios.map { ... }`, no envelope root) with
//!   `meta.pagination` (repo convention for paginated bare arrays).
//! - show: global `Eportfolio.find` then `:read`. `:read` is granted to the
//!   author (active, eportfolios enabled — spam included), to anyone when the
//!   portfolio is public and not spam, and to moderators. Deleted portfolios
//!   are unreadable by the author (no grant); moderators can still read them.
//! - pages: `:read` on the portfolio, then ALL entries ordered by `position`
//!   — a FLAT list, not category-grouped (upstream
//!   `portfolio.eportfolio_entries.order(:position)`), rendered as a bare
//!   array. Entry JSON: id, eportfolio_id, position, name, content,
//!   created_at, updated_at.
//! - delete: `:delete` (author of an active portfolio; moderators on anyone's)
//!   → soft delete: workflow_state='deleted', deleted_at=now (upstream
//!   `Eportfolio#destroy` overrides destroy to a state change). Renders the
//!   portfolio JSON. Deleting an already-deleted portfolio 404s for the
//!   author (no :delete grant on a deleted portfolio) and succeeds for a
//!   moderator (idempotent state rewrite).
//! - restore: `:restore` — moderators only (the author never holds :restore
//!   in the upstream policy); sets workflow_state='active', deleted_at=null,
//!   renders the portfolio JSON. Restoring a non-deleted portfolio is an
//!   idempotent success.
//! - moderate: `:moderate` (moderators only); `spam_status` must be one of
//!   `marked_as_safe` / `marked_as_spam` (Eportfolio::SPAM_MODERATIONS) —
//!   anything else is 400 with `{"error": "spam_status must be one of
//!   [\"marked_as_safe\", \"marked_as_spam\"]"}`. Renders the portfolio JSON.
//! - moderate_all: `:moderate_user_content` on the target user; validates
//!   spam_status the same way, then updates ALL ACTIVE portfolios of the user
//!   (deleted ones are excluded — upstream `user.eportfolios.active`),
//!   rendering the JSON literal `success`.
//! - Guards (upstream before_action chain, both on every action):
//!   `reject_student_view_student` — no student-view concept in star-lms, a
//!   no-op here; `verified_user_check` — the acting user needs a confirmed
//!   (workflow_state='active') communication channel; without one Canvas
//!   answers 401 with `{"status": "unverified", "errors": [{...}]}` on every
//!   action. star-lms pins the same 401 + body (the extractor's auth check
//!   runs first, so anonymous callers get the plain 401).
//! - Permission order: 401 anon/unverified → 404 unknown id → 403 failed
//!   ability → 400 validation (spam_status check comes AFTER authorization
//!   upstream, so an unauthorized caller with a bad spam_status gets 403).
//!
//! Schema: the canvas-schema snapshot migration 0224 already creates
//! `eportfolios`, `eportfolio_categories` and `eportfolio_entries` with
//! Canvas' real column layout (no new migration here; reserved number 0130
//! was released unused). Categories are not serialized by any API route;
//! `eportfolio_entries.eportfolio_category_id` is NOT NULL in the snapshot,
//! so seeded tests attach entries to a category row.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::NaiveDateTime;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// ePortfolio-domain errors. `Unverified` is the `verified_user_check`
/// failure (Canvas 401 + `{"status": "unverified", ...}`); `Invalid` carries
/// the controller's `{"error": ...}` 400 body; the plain variants map to the
/// same status codes as `star_common::ApiError`.
enum EportfolioError {
    NotFound,
    Forbidden,
    Unverified,
    Invalid(Value),
    Db,
}

impl From<sqlx::Error> for EportfolioError {
    fn from(e: sqlx::Error) -> Self {
        eprintln!("database error: {e}");
        EportfolioError::Db
    }
}

impl IntoResponse for EportfolioError {
    fn into_response(self) -> Response {
        match self {
            EportfolioError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            EportfolioError::Forbidden => (StatusCode::FORBIDDEN, "forbidden").into_response(),
            // verified_user_check (ApplicationController#render_unverified_error):
            // 401 with the unverified JSON body.
            EportfolioError::Unverified => (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "status": "unverified",
                    "errors": [{ "message": "user not authorized to perform that action until verifying email" }]
                })),
            )
                .into_response(),
            EportfolioError::Invalid(body) => {
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            EportfolioError::Db => {
                (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
            }
        }
    }
}

/// The upstream invalid-spam_status 400 body:
/// `{"error": "spam_status must be one of [\"marked_as_safe\", \"marked_as_spam\"]"}`.
fn spam_status_error() -> EportfolioError {
    EportfolioError::Invalid(json!({
        "error": "spam_status must be one of [\"marked_as_safe\", \"marked_as_spam\"]"
    }))
}

// ---------------------------------------------------------------------------
// Row shapes (only the columns the API serializes)
// ---------------------------------------------------------------------------

/// A row of `eportfolios` with the columns `eportfolio_json` serializes
/// (EPORTFOLIO_ATTRIBUTES). `public` is NOT NULL-normalized on the way out
/// (`!!hash["public"]`) and `deleted_at`/`spam_status` are nullable.
#[derive(Debug, Clone, sqlx::FromRow)]
struct EportfolioRow {
    id: i64,
    user_id: i64,
    name: Option<String>,
    public: Option<bool>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
    workflow_state: String,
    deleted_at: Option<NaiveDateTime>,
    spam_status: Option<String>,
}

/// A row of `eportfolio_entries` with the columns `eportfolio_entry_json`
/// serializes (ENTRY_ATTRIBUTES).
#[derive(Debug, Clone, sqlx::FromRow)]
struct EntryRow {
    id: i64,
    eportfolio_id: i64,
    position: Option<i32>,
    name: Option<String>,
    content: Option<String>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}

const PORTFOLIO_COLUMNS: &str = "id, user_id, name, public, created_at, updated_at, \
     workflow_state, deleted_at, spam_status";
const ENTRY_COLUMNS: &str = "id, eportfolio_id, position, name, content, created_at, updated_at";

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// Pagination query params (Canvas `Api.paginate`): 1-based page, per_page
/// defaulting to 50 and clamped to [1, 100].
#[derive(Debug, Default, Deserialize)]
struct PaginationParams {
    page: Option<i64>,
    per_page: Option<i64>,
}

fn pagination_values(params: &PaginationParams) -> (i64, i64, i64) {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).clamp(1, 100);
    // Saturating arithmetic: an extreme `page` must not overflow the offset.
    let offset = page.saturating_sub(1).saturating_mul(per_page);
    (page, per_page, offset)
}

/// Query params for GET /users/{user_id}/eportfolios, parsed from raw
/// key/value pairs (`Query<Vec<(String, String)>>`) so repeated `include[]`
/// params survive, exactly as Rails parses them (mirrors star-accounts).
#[derive(Debug, Default)]
struct IndexQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    /// Normalized `include[]` names; only "deleted" is meaningful here.
    includes: Vec<String>,
}

impl IndexQuery {
    fn from_pairs(pairs: &[(String, String)]) -> Self {
        let mut query = IndexQuery::default();
        for (key, value) in pairs {
            match key.as_str() {
                "page" => query.page = value.trim().parse().ok(),
                "per_page" => query.per_page = value.trim().parse().ok(),
                // Canvas sends `include[]=deleted`; `include=deleted` (and a
                // comma list) are also accepted.
                "include[]" | "include" => {
                    for part in value.split(',') {
                        let name = part.trim().to_ascii_lowercase();
                        if !name.is_empty() && !query.includes.contains(&name) {
                            query.includes.push(name);
                        }
                    }
                }
                _ => {}
            }
        }
        query
    }
}

/// The moderation actions' accepted `spam_status` values
/// (Eportfolio::SPAM_MODERATIONS).
const SPAM_MODERATIONS: [&str; 2] = ["marked_as_safe", "marked_as_spam"];

/// Moderation params: `spam_status` as a flat param (`params[:spam_status]`);
/// an `eportfolios`-wrapped object is tolerated.
struct ModerateParams;

impl ModerateParams {
    /// Extract the `spam_status` param from an optional JSON body (an absent
    /// body is blank → None, which fails the validation with the 400 error
    /// body).
    fn spam_status(body: &Option<Json<Value>>) -> Option<String> {
        let Json(value) = body.as_ref()?;
        let flat = value.get("spam_status").and_then(Value::as_str);
        flat.or_else(|| {
            value
                .get("eportfolios")
                .and_then(|w| w.get("spam_status"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// `eportfolio_json` — api_json with only EPORTFOLIO_ATTRIBUTES, `public`
/// coerced to a strict boolean (`!!hash["public"]`, so a NULL column
/// serializes as false). Timestamps render ISO8601 with a Z suffix (the
/// snapshot columns are naive; Canvas writes UTC).
fn eportfolio_json(row: &EportfolioRow) -> Value {
    json!({
        "id": row.id,
        "user_id": row.user_id,
        "name": row.name,
        "public": row.public.unwrap_or(false),
        "created_at": row.created_at.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "updated_at": row.updated_at.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "workflow_state": row.workflow_state,
        "deleted_at": row.deleted_at
            .map(|t| t.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        "spam_status": row.spam_status,
    })
}

/// `eportfolio_entry_json` — api_json with only ENTRY_ATTRIBUTES.
fn eportfolio_entry_json(row: &EntryRow) -> Value {
    json!({
        "id": row.id,
        "eportfolio_id": row.eportfolio_id,
        "position": row.position,
        "name": row.name,
        "content": row.content,
        "created_at": row.created_at.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "updated_at": row.updated_at.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })
}

/// Paginated bare-array response (upstream `render json: records.map {...}`
/// plus Api.paginate): a top-level JSON array with the repo's
/// `meta.pagination` envelope alongside it.
fn paginated_array(items: Vec<Value>, page: i64, per_page: i64, total: i64) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "items": items,
            "meta": {"pagination": {"page": page, "per_page": per_page, "total": total}},
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Guards and lookups
// ---------------------------------------------------------------------------

/// `verified_user_check` (upstream before_action on every action): the
/// acting user must hold a confirmed communication channel
/// (workflow_state='active'). Users without one get the 401 unverified body.
async fn require_verified_user(pool: &PgPool, auth: &AuthUser) -> Result<(), EportfolioError> {
    let verified: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM communication_channels \
         WHERE user_id = $1 AND workflow_state = 'active')",
    )
    .bind(auth.id)
    .fetch_one(pool)
    .await?;
    if verified {
        Ok(())
    } else {
        Err(EportfolioError::Unverified)
    }
}

/// `:moderate_user_content` on a user (upstream: the caller is an account
/// admin of one of the target's accounts with the moderation right). In
/// star-lms only the global admin role grants it.
fn is_moderator(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// `Eportfolio.find` — a global, unscoped lookup: portfolio ids are globally
/// addressable, so unknown ids are 404s for everyone.
async fn find_portfolio(pool: &PgPool, id: i64) -> Result<EportfolioRow, EportfolioError> {
    sqlx::query_as::<_, EportfolioRow>(&format!(
        "SELECT {PORTFOLIO_COLUMNS} FROM eportfolios WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(EportfolioError::NotFound)
}

/// The `:read` ability on a portfolio (Eportfolio policy):
/// - the author of an ACTIVE portfolio (spam included — "whether this
///   eportfolio is spam or not, the author can see it and delete it");
/// - anyone, when the portfolio is active, public and not spam;
/// - moderators (`self.user != user && grants_right?(:moderate_user_content)`).
///
/// Deleted portfolios are unreadable by their author.
fn can_read(row: &EportfolioRow, auth: &AuthUser) -> bool {
    let active = row.workflow_state == "active";
    let spam = matches!(
        row.spam_status.as_deref(),
        Some("marked_as_spam") | Some("flagged_as_possible_spam")
    );
    if is_moderator(auth) {
        return true;
    }
    if active && row.user_id == auth.id {
        return true;
    }
    active && row.public.unwrap_or(false) && !spam
}

/// The `:delete` ability (the controller's delete action): the author of an
/// ACTIVE portfolio, or a moderator. A deleted portfolio no longer grants
/// `:delete` to its author.
fn can_delete(row: &EportfolioRow, auth: &AuthUser) -> bool {
    if is_moderator(auth) {
        return true;
    }
    row.workflow_state == "active" && row.user_id == auth.id
}

/// The `:restore` and `:moderate` abilities: moderators only (the author is
/// never granted either right in the upstream policy; restore additionally
/// requires the portfolio to be deleted for the admin grant, but restoring a
/// non-deleted portfolio is an idempotent success, so the check is not
/// state-dependent here).
fn can_moderate(auth: &AuthUser) -> bool {
    is_moderator(auth)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /users/{user_id}/eportfolios — index.
///
/// The target user is looked up first (unknown user → 404), then the ability:
/// the owner, or `:moderate_user_content` (admin) on the target. The scope is
/// the target's active portfolios, EXCEPT for a moderator viewing ANOTHER
/// user with `include[]=deleted`, which lifts the active filter. Ordered by
/// `updated_at`, bare-array + meta.pagination.
async fn index(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(pairs): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Response, EportfolioError> {
    require_verified_user(&state.db, &auth).await?;
    let user_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
    if !user_exists {
        return Err(EportfolioError::NotFound);
    }
    let own = user_id == auth.id;
    if !own && !is_moderator(&auth) {
        return Err(EportfolioError::Forbidden);
    }
    let query = IndexQuery::from_pairs(&pairs);
    // Upstream: `scope = scope.active if user == @current_user ||
    // params[:include].exclude?("deleted")` — the owner NEVER sees deleted
    // rows, a moderator only with include[]=deleted.
    let include_deleted = !own && query.includes.iter().any(|inc| inc == "deleted");
    let (page, per_page, offset) = pagination_values(&PaginationParams {
        page: query.page,
        per_page: query.per_page,
    });
    let active_filter = if include_deleted {
        ""
    } else {
        "AND workflow_state <> 'deleted'"
    };
    let (total,): (i64,) = sqlx::query_as(&format!(
        "SELECT COUNT(*) FROM eportfolios WHERE user_id = $1 {active_filter}"
    ))
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;
    let rows: Vec<EportfolioRow> = sqlx::query_as(&format!(
        "SELECT {PORTFOLIO_COLUMNS} FROM eportfolios WHERE user_id = $1 {active_filter} \
         ORDER BY updated_at ASC, id ASC LIMIT $2 OFFSET $3"
    ))
    .bind(user_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    let items = rows.iter().map(eportfolio_json).collect();
    Ok(paginated_array(items, page, per_page, total))
}

/// GET /eportfolios/{id} — show. Global find, then `:read` (owner of an
/// active portfolio / public-and-not-spam / moderator).
async fn show(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, EportfolioError> {
    require_verified_user(&state.db, &auth).await?;
    let row = find_portfolio(&state.db, id).await?;
    if !can_read(&row, &auth) {
        return Err(EportfolioError::Forbidden);
    }
    Ok((StatusCode::OK, Json(eportfolio_json(&row))).into_response())
}

/// GET /eportfolios/{eportfolio_id}/pages — the portfolio's entries, ALL of
/// them in `position` order (a FLAT list — upstream
/// `portfolio.eportfolio_entries.order(:position)`; categories are not part
/// of this response). Same `:read` ability as show; bare array +
/// meta.pagination.
async fn pages(
    State(state): State<AppState>,
    Path(eportfolio_id): Path<i64>,
    Query(params): Query<PaginationParams>,
    auth: AuthUser,
) -> Result<Response, EportfolioError> {
    require_verified_user(&state.db, &auth).await?;
    let row = find_portfolio(&state.db, eportfolio_id).await?;
    if !can_read(&row, &auth) {
        return Err(EportfolioError::Forbidden);
    }
    let (page, per_page, offset) = pagination_values(&params);
    let (total,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM eportfolio_entries WHERE eportfolio_id = $1")
            .bind(row.id)
            .fetch_one(&state.db)
            .await?;
    let rows: Vec<EntryRow> = sqlx::query_as(&format!(
        "SELECT {ENTRY_COLUMNS} FROM eportfolio_entries WHERE eportfolio_id = $1 \
         ORDER BY position ASC, id ASC LIMIT $2 OFFSET $3"
    ))
    .bind(row.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    let items = rows.iter().map(eportfolio_entry_json).collect();
    Ok(paginated_array(items, page, per_page, total))
}

/// DELETE /eportfolios/{id} — soft delete (`Eportfolio#destroy` overrides
/// destroy to workflow_state='deleted' + deleted_at=now). Ability `:delete`:
/// the author of an active portfolio, or a moderator. Renders the portfolio
/// JSON (with its new deleted state).
async fn delete_portfolio(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, EportfolioError> {
    require_verified_user(&state.db, &auth).await?;
    let row = find_portfolio(&state.db, id).await?;
    if !can_delete(&row, &auth) {
        return Err(EportfolioError::Forbidden);
    }
    let updated: EportfolioRow = sqlx::query_as(&format!(
        "UPDATE eportfolios SET workflow_state = 'deleted', deleted_at = now(), \
         updated_at = now() WHERE id = $1 RETURNING {PORTFOLIO_COLUMNS}"
    ))
    .bind(row.id)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::OK, Json(eportfolio_json(&updated))).into_response())
}

/// PUT /eportfolios/{eportfolio_id}/restore — un-delete
/// (`Eportfolio#restore`: workflow_state='active', deleted_at=nil). Ability
/// `:restore`: moderators only. Renders the portfolio JSON; restoring a
/// non-deleted portfolio is an idempotent success.
async fn restore(
    State(state): State<AppState>,
    Path(eportfolio_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, EportfolioError> {
    require_verified_user(&state.db, &auth).await?;
    let row = find_portfolio(&state.db, eportfolio_id).await?;
    if !can_moderate(&auth) {
        return Err(EportfolioError::Forbidden);
    }
    let updated: EportfolioRow = sqlx::query_as(&format!(
        "UPDATE eportfolios SET workflow_state = 'active', deleted_at = NULL, \
         updated_at = now() WHERE id = $1 RETURNING {PORTFOLIO_COLUMNS}"
    ))
    .bind(row.id)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::OK, Json(eportfolio_json(&updated))).into_response())
}

/// PUT /eportfolios/{eportfolio_id}/moderate — set `spam_status`. Ability
/// `:moderate`: moderators only. The spam_status validation runs AFTER the
/// ability check (a non-moderator with an invalid value gets 403); invalid
/// values get the upstream 400 error body. Renders the portfolio JSON.
async fn moderate(
    State(state): State<AppState>,
    Path(eportfolio_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<Response, EportfolioError> {
    require_verified_user(&state.db, &auth).await?;
    let row = find_portfolio(&state.db, eportfolio_id).await?;
    if !can_moderate(&auth) {
        return Err(EportfolioError::Forbidden);
    }
    let status = ModerateParams::spam_status(&body).ok_or_else(spam_status_error)?;
    if !SPAM_MODERATIONS.contains(&status.as_str()) {
        return Err(spam_status_error());
    }
    let updated: EportfolioRow = sqlx::query_as(&format!(
        "UPDATE eportfolios SET spam_status = $1, updated_at = now() \
         WHERE id = $2 RETURNING {PORTFOLIO_COLUMNS}"
    ))
    .bind(&status)
    .bind(row.id)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::OK, Json(eportfolio_json(&updated))).into_response())
}

/// PUT /users/{user_id}/eportfolios — moderate_all. Ability
/// `:moderate_user_content` on the target user (admin). Validates spam_status
/// after authorization exactly like moderate, then updates every ACTIVE
/// portfolio of the user (deleted ones excluded — upstream
/// `user.eportfolios.active`), also refreshing their updated_at. Renders the
/// JSON literal `success` (upstream `render json: :success`).
async fn moderate_all(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<Response, EportfolioError> {
    require_verified_user(&state.db, &auth).await?;
    let user_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
    if !user_exists {
        return Err(EportfolioError::NotFound);
    }
    if !is_moderator(&auth) {
        return Err(EportfolioError::Forbidden);
    }
    let status = ModerateParams::spam_status(&body).ok_or_else(spam_status_error)?;
    if !SPAM_MODERATIONS.contains(&status.as_str()) {
        return Err(spam_status_error());
    }
    sqlx::query(
        "UPDATE eportfolios SET spam_status = $1, updated_at = now() \
         WHERE user_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(&status)
    .bind(user_id)
    .execute(&state.db)
    .await?;
    // Upstream `render json: :success` — the to_json of the Symbol :success
    // is the JSON string "success".
    Ok((StatusCode::OK, Json(json!("success"))).into_response())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Domain router for star-eportfolios. All portfolio-id path params share the
/// `{eportfolio_id}` name except DELETE /eportfolios/{id} (Canvas' own route
/// shape); they never collide because the methods differ.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users/{user_id}/eportfolios", get(index).put(moderate_all))
        .route(
            "/eportfolios/{eportfolio_id}",
            get(show).delete(delete_portfolio),
        )
        .route("/eportfolios/{eportfolio_id}/pages", get(pages))
        .route("/eportfolios/{eportfolio_id}/restore", put(restore))
        .route("/eportfolios/{eportfolio_id}/moderate", put(moderate))
}
