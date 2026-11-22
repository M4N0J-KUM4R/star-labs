//! sis_import_errors — Canvas `SisImportsErrorsApiController` parity.
//!
//! Routes (the crate router mounts this module under the `/api/v1` nest):
//! - `GET /accounts/{account_id}/sis_import_errors` — the account's errors
//! - `GET /accounts/{account_id}/sis_imports/{id}/errors` — the errors of
//!   one import
//!
//! Canvas controller
//! (`app/controllers/sis_import_errors_api_controller.rb`): `index`
//! guards `%i[import_sis manage_sis]` on the account, scopes the
//! account's `sis_batch_errors` newest first, narrows to one batch when
//! the import id is in the path, keeps only the failed rows for a truthy
//! `failure` parameter, paginates and renders
//! `{"sis_import_errors": [...]}`.
//!
//! `Api::V1::SisImportError#sis_import_error_json`: `sis_import_id` (the
//! batch id), `file`, `message`, `row_info`, `row`.

use axum::{
    extract::{Path, RawQuery, State},
    response::Response,
    routing::get,
    Router,
};
use serde_json::{json, Value};
use star_auth::AuthUser;
use star_common::AppState;

use crate::sis_imports_api::{
    authorize_sis, check_account, json_response, load_account, pagination_link, parse_id,
    ruby_to_i, value_to_boolean, Params, SisError, MAX_PER_PAGE, PER_PAGE,
};

/// A `sis_import_errors` row.
#[derive(Debug, sqlx::FromRow)]
struct SisImportErrorRow {
    sis_batch_id: i64,
    file: Option<String>,
    message: Option<String>,
    row_info: Option<String>,
    row: Option<i32>,
}

/// Domain router for the Canvas sis-import-error routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounts/{account_id}/sis_import_errors", get(index))
        .route(
            "/accounts/{account_id}/sis_imports/{id}/errors",
            get(index_for_batch),
        )
}

/// `GET /accounts/{account_id}/sis_import_errors` — every error of the
/// account, newest first.
async fn index(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Response, SisError> {
    errors_response(&state, &account_id, None, raw_query.as_deref(), &auth).await
}

/// `GET /accounts/{account_id}/sis_imports/{id}/errors` — the errors of
/// one import (Canvas scopes by the batch of `params[:id]`).
async fn index_for_batch(
    State(state): State<AppState>,
    Path((account_id, import_id)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Response, SisError> {
    errors_response(
        &state,
        &account_id,
        Some(&import_id),
        raw_query.as_deref(),
        &auth,
    )
    .await
}

/// The shared implementation of both index routes.
async fn errors_response(
    state: &AppState,
    raw_account_id: &str,
    raw_import_id: Option<&str>,
    raw_query: Option<&str>,
    auth: &AuthUser,
) -> Result<Response, SisError> {
    let account_id = parse_id(raw_account_id)?;
    let account = load_account(&state.db, account_id).await?;
    check_account(&account)?;
    authorize_sis(&state.db, auth, account_id).await?;

    // The per-batch variant narrows to the batch — an unknown import is a
    // 404 (Canvas `@account.sis_batches.find`).
    let batch_id = match raw_import_id {
        Some(raw) => {
            let id = parse_id(raw)?;
            let found: Option<(i64,)> =
                sqlx::query_as("SELECT id FROM sis_batches WHERE id = $1 AND account_id = $2")
                    .bind(id)
                    .bind(account_id)
                    .fetch_optional(&state.db)
                    .await?;
            Some(found.ok_or_else(SisError::not_found)?.0)
        }
        None => None,
    };

    let params = Params::parse(raw_query);
    let failure_only = value_to_boolean(params.get("failure"));
    let per_page = match params.get("per_page") {
        Some(raw) => ruby_to_i(raw).clamp(1, MAX_PER_PAGE),
        None => PER_PAGE,
    };
    let page = params.get("page").map(ruby_to_i).unwrap_or(0).max(1);
    let offset = (page - 1) * per_page;

    let rows: Vec<SisImportErrorRow> = sqlx::query_as(
        "SELECT sis_batch_id, file, message, row_info, row \
         FROM sis_import_errors \
         WHERE account_id = $1 AND ($2::bigint IS NULL OR sis_batch_id = $2) \
           AND (NOT $3 OR failure) \
         ORDER BY created_at DESC, id DESC LIMIT $4 OFFSET $5",
    )
    .bind(account_id)
    .bind(batch_id)
    .bind(failure_only)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM sis_import_errors \
         WHERE account_id = $1 AND ($2::bigint IS NULL OR sis_batch_id = $2) \
           AND (NOT $3 OR failure)",
    )
    .bind(account_id)
    .bind(batch_id)
    .bind(failure_only)
    .fetch_one(&state.db)
    .await?;

    let path = match raw_import_id {
        Some(id) => format!("/api/v1/accounts/{account_id}/sis_imports/{id}/errors"),
        None => format!("/api/v1/accounts/{account_id}/sis_import_errors"),
    };
    let link = pagination_link(&path, &params, page, per_page, total.0);
    Ok(json_response(
        json!({ "sis_import_errors": rows.iter().map(sis_import_error_json).collect::<Vec<_>>() }),
        link,
    ))
}

/// `Api::V1::SisImportError#sis_import_error_json`.
fn sis_import_error_json(row: &SisImportErrorRow) -> Value {
    json!({
        "sis_import_id": row.sis_batch_id,
        "file": row.file,
        "message": row.message,
        "row_info": row.row_info,
        "row": row.row,
    })
}
