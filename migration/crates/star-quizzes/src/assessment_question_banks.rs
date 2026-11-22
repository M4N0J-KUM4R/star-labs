//! Assessment question banks — Canvas `AssessmentQuestionBanksController`
//! (parity gap).
//!
//! Routes:
//!   - GET /question_banks             (index — context_type + context_id)
//!   - GET /question_banks/{id}        (show)
//!   - GET /question_banks/{id}/questions (questions)
//!
//! Canvas semantics:
//!   - index: `context_type` + `context_id` query params REQUIRED (400
//!     `{errors: "..."}` otherwise); Course/Account contexts only; the
//!     caller needs `read_question_banks` (enrolled/admin on the course,
//!     account admin).
//!   - require_bank: an unknown bank is an UNAUTHORIZED ACTION (401), not
//!     404 — Canvas quirk kept for parity.
//!   - show/questions: `read` on the bank (same role check routed through
//!     the bank's context); questions are the bank's active
//!     assessment_questions ordered by position, paginated.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct QuestionBankRow {
    id: i64,
    context_id: Option<i64>,
    context_type: Option<String>,
    title: Option<String>,
    workflow_state: Option<String>,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn bank_json(b: &QuestionBankRow, question_count: Option<i64>) -> Value {
    let mut v = json!({
        "id": b.id,
        "context_id": b.context_id,
        "context_type": b.context_type,
        "title": b.title,
        "workflow_state": b.workflow_state,
        "created_at": b.created_at.map(|d| d.to_rfc3339()),
        "updated_at": b.updated_at.map(|d| d.to_rfc3339()),
        "context_code": match (b.context_type.as_deref(), b.context_id) {
            (Some(t), Some(id)) => format!("{t}_{id}"),
            _ => Value::Null.to_string(),
        },
    });
    if let Some(count) = question_count {
        v["assessment_question_count"] = json!(count);
    }
    v
}

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct BankIndexQuery {
    pub context_type: Option<String>,
    pub context_id: Option<i64>,
    pub include_question_count: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct QuestionsQuery {
    pub per_page: Option<i64>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// read_question_banks on the bank's context: account admin/global for
/// Account contexts; enrolled-or-admin for Course contexts.
async fn can_read_context(
    pool: &PgPool,
    auth: &AuthUser,
    context_type: &str,
    context_id: i64,
) -> Result<bool, ApiError> {
    match context_type {
        "Account" => {
            if auth.role == "admin" {
                return Ok(true);
            }
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM account_users \
                 WHERE account_id = $1 AND user_id = $2 AND role = 'admin')",
            )
            .bind(context_id)
            .bind(auth.id)
            .fetch_one(pool)
            .await
            .map_err(Into::into)
        }
        "Course" => {
            if auth.role == "admin" {
                return Ok(true);
            }
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM enrollments \
                 WHERE user_id = $1 AND course_id = $2 \
                   AND workflow_state <> 'deleted')",
            )
            .bind(auth.id)
            .bind(context_id)
            .fetch_one(pool)
            .await
            .map_err(Into::into)
        }
        _ => Ok(false),
    }
}

async fn load_bank(pool: &PgPool, bank_id: i64) -> Result<QuestionBankRow, ApiError> {
    sqlx::query_as(
        "SELECT id, context_id, context_type, title, workflow_state, created_at, updated_at \
         FROM assessment_question_banks WHERE id = $1",
    )
    .bind(bank_id)
    .fetch_optional(pool)
    .await?
    // Canvas require_bank renders unauthorized for a missing bank.
    .ok_or(ApiError::Unauthorized)
}

async fn question_count(pool: &PgPool, bank_id: i64) -> Result<i64, ApiError> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM assessment_questions \
         WHERE assessment_question_bank_id = $1 AND workflow_state = 'active'",
    )
    .bind(bank_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /question_banks
async fn index_question_banks(
    State(state): State<AppState>,
    Query(query): Query<BankIndexQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    // require_context: both params required, Course/Account only.
    let (context_type, context_id) = match (query.context_type.as_deref(), query.context_id) {
        (Some("Course") | Some("Account"), Some(id)) => (query.context_type.clone().unwrap(), id),
        _ => {
            return Err(ApiError::BadRequest(
                "context_type and context_id are required".into(),
            ));
        }
    };

    // The context must exist (Canvas Course.find/Account.find → 404).
    let exists: bool = match context_type.as_str() {
        "Course" => sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
        )
        .bind(context_id)
        .fetch_one(&state.db)
        .await?,
        "Account" => sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted')",
        )
        .bind(context_id)
        .fetch_one(&state.db)
        .await?,
        _ => false,
    };
    if !exists {
        return Err(ApiError::NotFound);
    }

    if !can_read_context(&state.db, &auth, &context_type, context_id).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<QuestionBankRow> = sqlx::query_as(
        "SELECT id, context_id, context_type, title, workflow_state, created_at, updated_at \
         FROM assessment_question_banks \
         WHERE context_type = $1 AND context_id = $2 AND workflow_state = 'active' \
         ORDER BY id",
    )
    .bind(&context_type)
    .bind(context_id)
    .fetch_all(&state.db)
    .await?;

    let include_count = query.include_question_count.unwrap_or(false);
    let mut out = Vec::with_capacity(rows.len());
    for bank in &rows {
        let count = if include_count {
            Some(question_count(&state.db, bank.id).await?)
        } else {
            None
        };
        out.push(bank_json(bank, count));
    }
    Ok(Json(out))
}

/// GET /question_banks/{id}
async fn show_question_bank(
    State(state): State<AppState>,
    Path(bank_id): Path<i64>,
    Query(query): Query<BankIndexQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let bank = load_bank(&state.db, bank_id).await?;
    let (context_type, context_id) = (
        bank.context_type.clone().unwrap_or_default(),
        bank.context_id.unwrap_or_default(),
    );
    if !can_read_context(&state.db, &auth, &context_type, context_id).await? {
        return Err(ApiError::Forbidden);
    }
    let count = if query.include_question_count.unwrap_or(false) {
        Some(question_count(&state.db, bank.id).await?)
    } else {
        None
    };
    Ok(Json(bank_json(&bank, count)))
}

/// GET /question_banks/{id}/questions
async fn bank_questions(
    State(state): State<AppState>,
    Path(bank_id): Path<i64>,
    Query(query): Query<QuestionsQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    let bank = load_bank(&state.db, bank_id).await?;
    let (context_type, context_id) = (
        bank.context_type.clone().unwrap_or_default(),
        bank.context_id.unwrap_or_default(),
    );
    if !can_read_context(&state.db, &auth, &context_type, context_id).await? {
        return Err(ApiError::Forbidden);
    }

    type QuestionRow = (i64, Option<String>, Option<String>, Option<i32>);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let rows: Vec<QuestionRow> = sqlx::query_as(
        "SELECT id, name, question_data, position FROM assessment_questions \
         WHERE assessment_question_bank_id = $1 AND workflow_state = 'active' \
         ORDER BY position NULLS LAST, id LIMIT $2",
    )
    .bind(bank_id)
    .bind(per_page)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, name, question_data, position)| {
                json!({
                    "id": id,
                    "name": name,
                    "question_data": question_data
                        .as_deref()
                        .and_then(|q| serde_json::from_str::<Value>(q).ok())
                        .unwrap_or(Value::Null),
                    "position": position,
                })
            })
            .collect(),
    ))
}

/// Mount the question-bank routes.
pub fn question_banks_routes() -> Router<AppState> {
    Router::new()
        .route("/question_banks", get(index_question_banks))
        .route("/question_banks/{id}", get(show_question_bank))
        .route("/question_banks/{id}/questions", get(bank_questions))
}
