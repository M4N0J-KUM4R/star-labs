//! Outcome proficiency scales — Canvas `OutcomeProficiencyApiController`
//! (parity gap).
//!
//! Routes:
//!   - GET  /accounts/{accountId}/outcome_proficiency  (show)
//!   - POST /accounts/{accountId}/outcome_proficiency  (create/upsert)
//!   - GET  /courses/{courseId}/outcome_proficiency    (show)
//!   - POST /courses/{courseId}/outcome_proficiency    (create/upsert)
//!
//! Canvas semantics:
//!   - `show` renders the RESOLVED proficiency: the context's own active
//!     row, else the account chain's (Account#resolved_outcome_proficiency
//!     walks parent accounts; Course falls back to its account). Missing →
//!     404 `{message}`.
//!   - `create` upserts the context's own row (presence || new), replaces
//!     all ratings atomically, `workflow_state = 'active'`.
//!   - Authz: show needs `read` on the context; create needs
//!     `manage_proficiency_scales` (account admin / course teacher).
//!
//! Tables: `outcome_proficiencies` (polymorphic context, 0225) +
//! `outcome_proficiency_ratings`. Serializer (`outcome_proficiency_json`):
//! id, ratings [{description, points, mastery, color}], context_id,
//! context_type, root_account_id.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{
    can_access_account, check_enrolled_or_admin, is_account_admin, is_admin, MAX_ACCOUNT_TREE_DEPTH,
};

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct ProficiencyRow {
    id: i64,
    context_id: i64,
    context_type: String,
    root_account_id: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RatingRow {
    description: String,
    points: f64,
    mastery: bool,
    color: String,
}

fn proficiency_json(row: &ProficiencyRow, ratings: Vec<RatingRow>) -> Value {
    json!({
        "id": row.id,
        "ratings": ratings
            .into_iter()
            .map(|r| json!({
                "description": r.description,
                "points": r.points,
                "mastery": r.mastery,
                "color": r.color,
            }))
            .collect::<Vec<_>>(),
        "context_id": row.context_id,
        "context_type": row.context_type,
        "root_account_id": row.root_account_id,
    })
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ProficiencyPayload {
    /// Canvas posts the ratings flat (`params.permit(ratings: [...])`).
    pub ratings: Option<Vec<RatingParams>>,
    /// Also accept the wrapped form for client convenience.
    pub outcome_proficiency: Option<Wrapped>,
}

#[derive(Debug, Deserialize)]
pub struct Wrapped {
    pub ratings: Option<Vec<RatingParams>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RatingParams {
    pub description: Option<String>,
    pub points: f64,
    pub mastery: Option<bool>,
    pub color: Option<String>,
}

fn payload_ratings(payload: &ProficiencyPayload) -> Result<&[RatingParams], ApiError> {
    let ratings = payload
        .ratings
        .as_ref()
        .or(payload
            .outcome_proficiency
            .as_ref()
            .and_then(|w| w.ratings.as_ref()))
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    if ratings.is_empty() {
        return Err(ApiError::Validation("ratings cannot be empty".into()));
    }
    for r in ratings {
        if r.description
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            return Err(ApiError::Validation("description can't be blank".into()));
        }
        if r.points < 0.0 {
            return Err(ApiError::Validation("points must be >= 0".into()));
        }
    }
    Ok(ratings)
}

// ---------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------///

/// Active own proficiency of a context (no inheritance).
async fn own_proficiency(
    pool: &PgPool,
    context_id: i64,
    context_type: &str,
) -> Result<Option<ProficiencyRow>, ApiError> {
    sqlx::query_as(
        "SELECT id, context_id, context_type, root_account_id \
         FROM outcome_proficiencies \
         WHERE context_id = $1 AND context_type = $2 AND workflow_state = 'active' \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(context_id)
    .bind(context_type)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

async fn ratings_of(pool: &PgPool, proficiency_id: i64) -> Result<Vec<RatingRow>, ApiError> {
    sqlx::query_as(
        "SELECT description, points, mastery, color FROM outcome_proficiency_ratings \
         WHERE outcome_proficiency_id = $1 AND workflow_state = 'active' \
         ORDER BY points DESC, id",
    )
    .bind(proficiency_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// The context's root account id (accounts: their own root chain head;
/// courses: the course's account chain head).
async fn root_account_of(pool: &PgPool, account_id: i64) -> Result<Option<i64>, ApiError> {
    let mut current = Some(account_id);
    let mut depth = 0;
    while let Some(id) = current {
        if depth >= MAX_ACCOUNT_TREE_DEPTH {
            return Ok(None);
        }
        depth += 1;
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        match parent {
            None => return Ok(Some(id)),
            Some((p,)) => current = p,
        }
    }
    Ok(None)
}

/// `Account#resolved_outcome_proficiency`: own active row, else recurse to
/// the parent account.
async fn resolved_account_proficiency(
    pool: &PgPool,
    account_id: i64,
    _depth: i64,
) -> Result<Option<ProficiencyRow>, ApiError> {
    let mut current = Some(account_id);
    let mut depth = 0;
    while let Some(id) = current {
        if depth >= MAX_ACCOUNT_TREE_DEPTH {
            return Ok(None);
        }
        depth += 1;
        if let Some(row) = own_proficiency(pool, id, "Account").await? {
            return Ok(Some(row));
        }
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        current = parent.and_then(|(p,)| p);
    }
    Ok(None)
}

/// Replace all ratings of a proficiency in one transaction.
async fn replace_ratings(
    pool: &PgPool,
    proficiency_id: i64,
    root_account_id: Option<i64>,
    ratings: &[RatingParams],
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM outcome_proficiency_ratings WHERE outcome_proficiency_id = $1")
        .bind(proficiency_id)
        .execute(&mut *tx)
        .await?;
    for r in ratings {
        sqlx::query(
            "INSERT INTO outcome_proficiency_ratings \
                (outcome_proficiency_id, description, points, mastery, color, root_account_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(proficiency_id)
        .bind(r.description.as_deref().unwrap_or_default().trim())
        .bind(r.points)
        .bind(r.mastery.unwrap_or(false))
        .bind(r.color.as_deref().unwrap_or(""))
        .bind(root_account_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET+POST /accounts/{accountId}/outcome_proficiency
async fn account_outcome_proficiency(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    method: axum::http::Method,
    auth: AuthUser,
    body: Option<Json<ProficiencyPayload>>,
) -> Result<Json<Value>, ApiError> {
    // get_context: unknown account → 404.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }

    if method == axum::http::Method::POST {
        // create: manage_proficiency_scales → account admin.
        if !is_admin(&auth) && !is_account_admin(&state.db, auth.id, account_id).await? {
            return Err(ApiError::Forbidden);
        }
        let Json(payload) = body.ok_or_else(|| ApiError::Validation("ratings required".into()))?;
        let ratings = payload_ratings(&payload)?;
        let root = root_account_of(&state.db, account_id).await?;

        // Upsert: reuse the context's own row if present (any state), else
        // insert a fresh one (Canvas `presence || OutcomeProficiency.new`).
        let row: ProficiencyRow = match own_proficiency_any_state(&state.db, account_id, "Account")
            .await?
        {
            Some(id) => {
                sqlx::query(
                    "UPDATE outcome_proficiencies SET workflow_state = 'active', updated_at = now() \
                     WHERE id = $1",
                )
                .bind(id)
                .execute(&state.db)
                .await?;
                ProficiencyRow {
                    id,
                    context_id: account_id,
                    context_type: "Account".into(),
                    root_account_id: root,
                }
            }
            None => {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO outcome_proficiencies (context_id, context_type, root_account_id, created_at, updated_at) \
                     VALUES ($1, 'Account', $2, now(), now()) RETURNING id",
                )
                .bind(account_id)
                .bind(root)
                .fetch_one(&state.db)
                .await?;
                ProficiencyRow {
                    id,
                    context_id: account_id,
                    context_type: "Account".into(),
                    root_account_id: root,
                }
            }
        };
        replace_ratings(&state.db, row.id, root, ratings).await?;
        let ratings = ratings_of(&state.db, row.id).await?;
        return Ok(Json(proficiency_json(&row, ratings)));
    }

    // show: read on the account (admin chain), 404 `{message}` when none
    // resolves.
    if !can_access_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    match resolved_account_proficiency(&state.db, account_id, 0).await? {
        Some(row) => {
            let ratings = ratings_of(&state.db, row.id).await?;
            Ok(Json(proficiency_json(&row, ratings)))
        }
        None => Err(ApiError::NotFound),
    }
}

/// GET+POST /courses/{courseId}/outcome_proficiency
async fn course_outcome_proficiency(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    method: axum::http::Method,
    auth: AuthUser,
    body: Option<Json<ProficiencyPayload>>,
) -> Result<Json<Value>, ApiError> {
    let account_id: Option<i64> = sqlx::query_scalar(
        "SELECT account_id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let account_id = account_id.ok_or(ApiError::NotFound)?;

    if method == axum::http::Method::POST {
        // create: manage_proficiency_scales on the course → teacher-or-better
        // via the course manage check used across this crate.
        let can_manage = check_course_manage(&state.db, auth.id, course_id, &auth).await?;
        if !can_manage {
            return Err(ApiError::Forbidden);
        }
        let Json(payload) = body.ok_or_else(|| ApiError::Validation("ratings required".into()))?;
        let ratings = payload_ratings(&payload)?;
        let root = root_account_of(&state.db, account_id).await?;

        let row: ProficiencyRow = match own_proficiency_any_state(&state.db, course_id, "Course")
            .await?
        {
            Some(id) => {
                sqlx::query(
                    "UPDATE outcome_proficiencies SET workflow_state = 'active', updated_at = now() \
                     WHERE id = $1",
                )
                .bind(id)
                .execute(&state.db)
                .await?;
                ProficiencyRow {
                    id,
                    context_id: course_id,
                    context_type: "Course".into(),
                    root_account_id: root,
                }
            }
            None => {
                let id: i64 = sqlx::query_scalar(
                    "INSERT INTO outcome_proficiencies (context_id, context_type, root_account_id, created_at, updated_at) \
                     VALUES ($1, 'Course', $2, now(), now()) RETURNING id",
                )
                .bind(course_id)
                .bind(root)
                .fetch_one(&state.db)
                .await?;
                ProficiencyRow {
                    id,
                    context_id: course_id,
                    context_type: "Course".into(),
                    root_account_id: root,
                }
            }
        };
        replace_ratings(&state.db, row.id, root, ratings).await?;
        let ratings = ratings_of(&state.db, row.id).await?;
        return Ok(Json(proficiency_json(&row, ratings)));
    }

    // show: enrolled or admin (course read).
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    // Course#resolved_outcome_proficiency: own active row, else account chain.
    let resolved = match own_proficiency(&state.db, course_id, "Course").await? {
        Some(row) => Some(row),
        None => resolved_account_proficiency(&state.db, account_id, 0).await?,
    };
    match resolved {
        Some(row) => {
            let ratings = ratings_of(&state.db, row.id).await?;
            Ok(Json(proficiency_json(&row, ratings)))
        }
        None => Err(ApiError::NotFound),
    }
}

/// The context's own proficiency row regardless of workflow_state (create
/// reactivates a soft-deleted row).
async fn own_proficiency_any_state(
    pool: &PgPool,
    context_id: i64,
    context_type: &str,
) -> Result<Option<i64>, ApiError> {
    sqlx::query_scalar(
        "SELECT id FROM outcome_proficiencies \
         WHERE context_id = $1 AND context_type = $2 ORDER BY id DESC LIMIT 1",
    )
    .bind(context_id)
    .bind(context_type)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Teacher-or-better on the course (manage_proficiency_scales proxy).
async fn check_course_manage(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let teacher: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
           AND workflow_state <> 'deleted')",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(pool)
    .await?;
    if teacher {
        return Ok(true);
    }
    // Account admins of the course's account chain can also manage.
    let account_id: Option<i64> =
        sqlx::query_scalar("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    if let Some(account_id) = account_id {
        return is_account_admin(pool, user_id, account_id).await;
    }
    Ok(false)
}

/// Mount the proficiency routes.
pub fn outcome_proficiency_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{accountId}/outcome_proficiency",
            get(account_outcome_proficiency).post(account_outcome_proficiency),
        )
        .route(
            "/courses/{courseId}/outcome_proficiency",
            get(course_outcome_proficiency).post(course_outcome_proficiency),
        )
}
