//! Authentication audit — Canvas `AuthenticationAuditApiController`
//! (parity gap).
//!
//! Routes:
//!   - GET /audit/authentication/logins/{loginId}   (for_login)
//!   - GET /audit/authentication/accounts/{accountId} (for_account)
//!   - GET /audit/authentication/users/{userId}      (for_user)
//!
//! Canvas semantics (AuditorApiController + AuthenticationAuditApi):
//!   - for_login: the pseudonym (login) must be active (404 otherwise); the
//!     caller must administer the pseudonym's account or be a site admin —
//!     401 otherwise (render_unauthorized_action).
//!   - for_account: root account by id (404 unknown); same visibility.
//!   - for_user: self, or site admin, or an admin of one of the user's
//!     pseudonym accounts.
//!   - query options: start_time/end_time window (oldest/newest).
//!   - Response: the auditors "events" array with per-event created_at and
//!     pseudonym_id; `links` carries the queried URL.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::is_admin;

// ---------------------------------------------------------------------------
// Params / rows
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct AuthEventRow {
    id: i64,
    pseudonym_id: i64,
    user_id: i64,
    account_id: Option<i64>,
    event_type: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn event_json(e: &AuthEventRow) -> Value {
    json!({
        "id": e.id,
        "pseudonym_id": e.pseudonym_id,
        "user_id": e.user_id,
        "account_id": e.account_id,
        "event_type": e.event_type,
        "created_at": e.created_at.to_rfc3339(),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_time(
    value: Option<&str>,
    field: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiError> {
    match value {
        None => Ok(None),
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&chrono::Utc))
            .map(Some)
            .map_err(|_| ApiError::BadRequest(format!("invalid {field}"))),
    }
}

/// Site-admin visibility proxy: the workspace has no separate site-admin
/// account, so the global admin role stands in (Canvas Account.site_admin).
async fn is_account_admin(pool: &PgPool, account_id: i64, user_id: i64) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM account_users \
         WHERE account_id = $1 AND user_id = $2 AND role = 'admin')",
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

async fn fetch_events(
    pool: &PgPool,
    query: &AuditQuery,
    where_sql: &str,
    bind_id: i64,
) -> Result<Vec<AuthEventRow>, ApiError> {
    let oldest = parse_time(query.start_time.as_deref(), "start_time")?;
    let newest = parse_time(query.end_time.as_deref(), "end_time")?;
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);

    let sql = format!(
        "SELECT id, pseudonym_id, user_id, account_id, event_type, created_at \
         FROM authentication_events {where_sql} \
           AND ($2::timestamptz IS NULL OR created_at >= $2) \
           AND ($3::timestamptz IS NULL OR created_at <= $3) \
         ORDER BY created_at DESC LIMIT $4"
    );
    sqlx::query_as(&sql)
        .bind(bind_id)
        .bind(oldest)
        .bind(newest)
        .bind(per_page)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

fn events_response(rows: &[AuthEventRow], links_url: &str) -> Value {
    json!({
        "events": rows.iter().map(event_json).collect::<Vec<_>>(),
        "links": { "current": links_url },
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /audit/authentication/logins/{loginId}
async fn for_login(
    State(state): State<AppState>,
    Path(login_id): Path<i64>,
    Query(query): Query<AuditQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    // Pseudonym.active.find → 404 for unknown/deleted logins.
    let account: Option<i64> = sqlx::query_scalar(
        "SELECT account_id FROM pseudonyms WHERE id = $1 AND workflow_state = 'active'",
    )
    .bind(login_id)
    .fetch_optional(&state.db)
    .await?;
    let account_id = account.ok_or(ApiError::NotFound)?;

    if !is_admin(&auth) && !is_account_admin(&state.db, account_id, auth.id).await? {
        return Err(ApiError::Unauthorized);
    }
    let rows = fetch_events(&state.db, &query, "WHERE pseudonym_id = $1", login_id).await?;
    Ok(Json(events_response(
        &rows,
        &format!("/api/v1/audit/authentication/logins/{login_id}"),
    )))
}

/// GET /audit/authentication/accounts/{accountId}
async fn for_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    Query(query): Query<AuditQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    // api_find on root_accounts: the account must exist (404 otherwise).
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(account_id)
    .fetch_one(&state.db)
    .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    if !is_admin(&auth) && !is_account_admin(&state.db, account_id, auth.id).await? {
        return Err(ApiError::Unauthorized);
    }
    let rows = fetch_events(&state.db, &query, "WHERE account_id = $1", account_id).await?;
    Ok(Json(events_response(
        &rows,
        &format!("/api/v1/audit/authentication/accounts/{account_id}"),
    )))
}

/// GET /audit/authentication/users/{userId}
async fn for_user(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(query): Query<AuditQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    // api_find(User.active) → 404 unknown.
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }

    let allowed = user_id == auth.id || is_admin(&auth) || {
        // Admin of any of the user's active pseudonym accounts.
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM account_users au \
                 JOIN pseudonyms p ON p.account_id = au.account_id \
                 WHERE p.user_id = $1 AND p.workflow_state <> 'deleted' \
                   AND au.user_id = $2 AND au.role = 'admin')",
        )
        .bind(user_id)
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?
    };
    if !allowed {
        return Err(ApiError::Unauthorized);
    }

    let rows = fetch_events(&state.db, &query, "WHERE user_id = $1", user_id).await?;
    Ok(Json(events_response(
        &rows,
        &format!("/api/v1/audit/authentication/users/{user_id}"),
    )))
}

/// Mount the authentication-audit routes.
pub fn authentication_audit_routes() -> Router<AppState> {
    Router::new()
        .route("/audit/authentication/logins/{loginId}", get(for_login))
        .route(
            "/audit/authentication/accounts/{accountId}",
            get(for_account),
        )
        .route("/audit/authentication/users/{userId}", get(for_user))
}
