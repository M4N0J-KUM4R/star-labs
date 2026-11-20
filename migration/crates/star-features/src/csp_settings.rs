//! Canvas Content Security Policy settings (`CspSettingsController`).
//!
//! Canvas routes (config/routes.rb, `scope(controller: :csp_settings)`):
//!
//! - GET    /accounts/{accountId}/csp_settings — `get_csp_settings`
//! - GET    /courses/{courseId}/csp_settings — `get_csp_settings`
//! - PUT    /accounts/{accountId}/csp_settings — `set_csp_setting`
//! - PUT    /courses/{courseId}/csp_settings — `set_csp_setting`
//! - PUT    /accounts/{accountId}/csp_settings/lock — `set_csp_lock`
//! - POST   /accounts/{accountId}/csp_settings/domains — `add_domain`
//! - POST   /accounts/{accountId}/csp_settings/domains/batch_create — `add_multiple_domains`
//! - DELETE /accounts/{accountId}/csp_settings/domains — `remove_domain`
//!
//! State model (Canvas `app/models/csp/account_helper.rb` and
//! `app/models/csp/course_helper.rb`):
//!
//! - An account keeps its CSP state in `accounts.settings` under
//!   `csp_inherited_data`: `{"value": [enabled, account_id], "locked": bool}`.
//!   The setting is inheritable, so an account without its own entry
//!   resolves to the nearest ancestor that has one. `csp_inherited?` is true
//!   when the account named in `value[1]` is not the account itself.
//! - A course keeps only `courses.csp_disabled`. The course is CSP-enabled
//!   when its account is enabled and the course has not disabled CSP; the
//!   course inherits the account's lock.
//! - Allowed domains live in `csp_domains` (soft deletable). Re-adding a
//!   removed domain revives the row, as Canvas' `add_domain!` does.
//!
//! Permissions, in the order Canvas checks them:
//!
//! - reads need `read_as_admin` on the context: an account admin for an
//!   account, an account admin or a course teacher for a course.
//! - writes need `manage_account_settings` on the account — for a course
//!   that is the course's own account.
//!
//! Response shape (`csp_settings_json`):
//!
//! ```json
//! {
//!   "enabled": false,
//!   "inherited": false,
//!   "settings_locked": false,
//!   "effective_whitelist": ["example.com"],
//!   "tools_whitelist": {"example.com": [{"id": 1, "name": "Tool", "account_id": 1}]},
//!   "current_account_whitelist": ["example.com"]
//! }
//! ```
//!
//! `effective_whitelist` is present only when CSP is enabled. `tools_whitelist`
//! and `current_account_whitelist` are account-only; the account whitelist
//! lists the domains this account allows explicitly, and the effective
//! whitelist lists the domains of the account that turned CSP on.
//!
//! Known simplifications of this port:
//!
//! - `ApiError` renders plain text, so error bodies carry the Canvas status
//!   codes but not its `{"message": ...}` JSON envelopes.
//! - Canvas adds the request host, the global whitelist and the file host to
//!   the effective whitelist; those depend on a running request and on
//!   instance configuration, so the effective whitelist holds the explicitly
//!   allowed domains of the enabling account plus the external tool domains.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Contexts
// ---------------------------------------------------------------------------

/// The CSP state of one account.
#[derive(Debug, Clone)]
struct AccountCsp {
    /// CSP is switched on for this account (or inherited from an ancestor).
    enabled: bool,
    /// The state comes from an ancestor account, not from this one.
    inherited: bool,
    /// Sub-accounts and courses may not override the inherited state.
    locked: bool,
    /// The account that set the state (`csp_account_id` in Canvas).
    source_account_id: i64,
}

/// The CSP state of one course, derived from its account.
#[derive(Debug, Clone)]
struct CourseCsp {
    enabled: bool,
    inherited: bool,
    locked: bool,
    account_id: Option<i64>,
}

/// Canvas `Csp::AccountHelper#csp_enabled?` / `#csp_inherited?` /
/// `#csp_locked?`, resolved against the account chain.
async fn account_csp(pool: &PgPool, account_id: i64) -> Result<AccountCsp, ApiError> {
    let chain = account_ancestry(pool, account_id).await?;
    for id in &chain {
        let stored: Option<String> =
            sqlx::query_scalar("SELECT settings::text FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let Some(stored) = stored else { continue };
        let Ok(Value::Object(settings)) = serde_json::from_str::<Value>(&stored) else {
            continue;
        };
        let Some(data) = settings
            .get("csp_inherited_data")
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        let value = data.get("value").and_then(|v| v.as_array());
        let enabled = value
            .and_then(|array| array.first())
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let source = value
            .and_then(|array| array.get(1))
            .and_then(|v| v.as_i64())
            .unwrap_or(*id);
        let locked = data
            .get("locked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        return Ok(AccountCsp {
            enabled,
            inherited: source != account_id,
            locked,
            source_account_id: source,
        });
    }
    // No ancestor sets CSP: off, not inherited, not locked.
    Ok(AccountCsp {
        enabled: false,
        inherited: false,
        locked: false,
        source_account_id: account_id,
    })
}

/// The account chain, starting at `account_id` and walking up to the root.
/// Cycle-safe: a parent cycle stops the walk.
async fn account_ancestry(pool: &PgPool, account_id: i64) -> Result<Vec<i64>, ApiError> {
    let mut chain: Vec<i64> = Vec::new();
    let mut current = Some(account_id);
    while let Some(id) = current {
        if chain.contains(&id) {
            break;
        }
        chain.push(id);
        let row: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        current = match row {
            Some((parent,)) => parent,
            None => None,
        };
    }
    Ok(chain)
}

/// Canvas `Csp::CourseHelper`: the course follows its account, unless the
/// course disabled CSP itself.
async fn course_csp(pool: &PgPool, course_id: i64) -> Result<CourseCsp, ApiError> {
    let row: Option<(Option<i64>, Option<bool>,)> = sqlx::query_as(
        "SELECT account_id, csp_disabled FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    let Some((account_id, disabled)) = row else {
        return Err(ApiError::NotFound);
    };
    let account = match account_id {
        Some(account_id) => account_csp(pool, account_id).await?,
        None => AccountCsp {
            enabled: false,
            inherited: false,
            locked: false,
            source_account_id: course_id,
        },
    };
    let disabled = disabled.unwrap_or(false);
    let inherited = account.locked || !disabled;
    Ok(CourseCsp {
        enabled: account.enabled && inherited,
        inherited,
        locked: account.locked,
        account_id,
    })
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas account rights flow through `account_users` grants on the account
/// or any of its ancestors.
async fn account_member(
    pool: &PgPool,
    user_id: i64,
    account_ids: &[i64],
) -> Result<bool, ApiError> {
    if account_ids.is_empty() {
        return Ok(false);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE user_id = $1 AND account_id = ANY($2) LIMIT 1",
    )
    .bind(user_id)
    .bind(account_ids)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

async fn course_teacher(pool: &PgPool, user_id: i64, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `require_read_permissions`: `authorized_action(@context, @current_user,
/// :read_as_admin)`.
async fn can_read_settings(
    pool: &PgPool,
    auth: &AuthUser,
    account_ids: &[i64],
    course_id: Option<i64>,
) -> Result<bool, ApiError> {
    if is_admin(auth) || account_member(pool, auth.id, account_ids).await? {
        return Ok(true);
    }
    match course_id {
        Some(course_id) => course_teacher(pool, auth.id, course_id).await,
        None => Ok(false),
    }
}

/// Canvas `require_permissions`: `authorized_action(account, @current_user,
/// :manage_account_settings)`.
async fn can_manage_settings(
    pool: &PgPool,
    auth: &AuthUser,
    account_ids: &[i64],
) -> Result<bool, ApiError> {
    Ok(is_admin(auth) || account_member(pool, auth.id, account_ids).await?)
}

// ---------------------------------------------------------------------------
// Domains
// ---------------------------------------------------------------------------

/// The active domains of an account, sorted, as `csp_domains.active.pluck`
/// returns them.
async fn account_domains(pool: &PgPool, account_id: i64) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT domain FROM csp_domains WHERE account_id = $1 AND workflow_state = 'active' ORDER BY domain",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(domain,)| domain).collect())
}

/// The external tool domains of an account, grouped by domain, as Canvas'
/// `csp_tools_grouped_by_domain` reports them.
async fn account_tool_domains(
    pool: &PgPool,
    account_id: i64,
) -> Result<Map<String, Value>, ApiError> {
    let rows: Vec<(i64, String, String, Option<i64>)> = sqlx::query_as(
        r#"
        SELECT id, name, COALESCE(domain, ''), account_id
        FROM external_tools
        WHERE account_id = $1 AND workflow_state = 'active' AND domain IS NOT NULL AND domain <> ''
        ORDER BY id
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    let mut grouped: Map<String, Value> = Map::new();
    for (id, name, domain, tool_account_id) in rows {
        let entry = grouped
            .entry(domain)
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(list) = entry {
            list.push(json!({
                "id": id,
                "name": name,
                "account_id": tool_account_id,
            }));
        }
    }
    Ok(grouped)
}

/// Canvas `Csp::Domain#validate_domain`: the domain must parse as a URI.
///
/// `URI.parse` accepts a relative reference such as `example.com` and
/// rejects a string with spaces or control characters, so that is the rule
/// the port applies.
fn domain_is_valid(domain: &str) -> bool {
    if domain.trim().is_empty() {
        return false;
    }
    !domain.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Canvas `Csp::AccountHelper#add_domain!`: lowercase the domain and revive
/// a soft-deleted row instead of inserting a second one.
async fn add_domain(pool: &PgPool, account_id: i64, domain: &str) -> Result<bool, ApiError> {
    if !domain_is_valid(domain) {
        return Ok(false);
    }
    let domain = domain.to_lowercase();
    let res = sqlx::query(
        r#"
        INSERT INTO csp_domains (account_id, domain, workflow_state)
        VALUES ($1, $2, 'active')
        ON CONFLICT (account_id, domain)
        DO UPDATE SET workflow_state = 'active', updated_at = now()
        "#,
    )
    .bind(account_id)
    .bind(domain.as_str())
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Canvas `Csp::AccountHelper#remove_domain!`: soft delete the domain.
async fn remove_domain(pool: &PgPool, account_id: i64, domain: &str) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE csp_domains SET workflow_state = 'deleted', updated_at = now() WHERE account_id = $1 AND domain = $2",
    )
    .bind(account_id)
    .bind(domain.to_lowercase())
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

/// Canvas `params.require(:status)` for `set_csp_setting`.
#[derive(Debug, Deserialize)]
struct CspSettingBody {
    status: Option<String>,
}

/// Canvas `params.require(:settings_locked)` for `set_csp_lock`.
#[derive(Debug, Deserialize)]
struct CspLockBody {
    settings_locked: Option<String>,
}

/// Canvas `params.require(:domain)` for `add_domain` and `remove_domain`.
#[derive(Debug, Deserialize)]
struct DomainBody {
    domain: Option<String>,
}

/// Canvas `params.require(:domains)` for `add_multiple_domains`.
#[derive(Debug, Deserialize)]
struct DomainBatchBody {
    domains: Option<Vec<String>>,
}

/// Canvas `value_to_boolean` (`Canvas::Plugin.value_to_boolean`).
fn value_to_boolean(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    match value.to_ascii_lowercase().as_str() {
        "yes" | "y" | "true" | "t" | "on" | "1" => true,
        "no" | "n" | "false" | "f" | "off" | "0" => false,
        other => other
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '+')
            .collect::<String>()
            .parse::<i64>()
            .is_ok_and(|n| n != 0),
    }
}

// ---------------------------------------------------------------------------
// Response bodies
// ---------------------------------------------------------------------------

/// Canvas `csp_settings_json` for an account.
async fn account_settings_json(
    pool: &PgPool,
    account_id: i64,
    state: &AccountCsp,
) -> Result<Value, ApiError> {
    let mut json_map = Map::new();
    json_map.insert("enabled".to_string(), json!(state.enabled));
    json_map.insert("inherited".to_string(), json!(state.inherited));
    json_map.insert("settings_locked".to_string(), json!(state.locked));

    if state.enabled {
        let mut whitelist = account_domains(pool, state.source_account_id).await?;
        let tools = account_tool_domains(pool, state.source_account_id).await?;
        whitelist.extend(tools.keys().cloned());
        whitelist.sort();
        whitelist.dedup();
        json_map.insert("effective_whitelist".to_string(), json!(whitelist));
    }

    json_map.insert(
        "tools_whitelist".to_string(),
        Value::Object(account_tool_domains(pool, account_id).await?),
    );
    json_map.insert(
        "current_account_whitelist".to_string(),
        json!(account_domains(pool, account_id).await?),
    );
    Ok(Value::Object(json_map))
}

/// Canvas `csp_settings_json` for a course.
async fn course_settings_json(pool: &PgPool, state: &CourseCsp) -> Result<Value, ApiError> {
    let mut json_map = Map::new();
    json_map.insert("enabled".to_string(), json!(state.enabled));
    json_map.insert("inherited".to_string(), json!(state.inherited));
    json_map.insert("settings_locked".to_string(), json!(state.locked));

    if state.enabled {
        if let Some(account_id) = state.account_id {
            let account = account_csp(pool, account_id).await?;
            let mut whitelist = account_domains(pool, account.source_account_id).await?;
            let tools = account_tool_domains(pool, account.source_account_id).await?;
            whitelist.extend(tools.keys().cloned());
            whitelist.sort();
            whitelist.dedup();
            json_map.insert("effective_whitelist".to_string(), json!(whitelist));
        }
    }
    Ok(Value::Object(json_map))
}

/// Canvas `{"current_account_whitelist": [...]}`.
async fn whitelist_json(pool: &PgPool, account_id: i64) -> Result<Value, ApiError> {
    Ok(json!({
        "current_account_whitelist": account_domains(pool, account_id).await?,
    }))
}

/// A Canvas error envelope with a message, rendered as JSON.
fn message_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "message": message }))).into_response()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /accounts/{accountId}/csp_settings — Canvas `get_csp_settings`.
async fn get_account_settings(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let chain = account_ancestry(&state.db, account_id).await?;
    if chain.is_empty() {
        return Err(ApiError::NotFound);
    }
    if !can_read_settings(&state.db, &auth, &chain, None).await? {
        return Err(ApiError::Forbidden);
    }
    let csp = account_csp(&state.db, account_id).await?;
    Ok(Json(
        account_settings_json(&state.db, account_id, &csp).await?,
    ))
}

/// GET /courses/{courseId}/csp_settings — Canvas `get_csp_settings`.
async fn get_course_settings(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let csp = course_csp(&state.db, course_id).await?;
    let chain = match csp.account_id {
        Some(account_id) => account_ancestry(&state.db, account_id).await?,
        None => Vec::new(),
    };
    if !can_read_settings(&state.db, &auth, &chain, Some(course_id)).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(course_settings_json(&state.db, &csp).await?))
}

/// PUT /accounts/{accountId}/csp_settings — Canvas `set_csp_setting`.
async fn set_account_setting(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    body: Option<Json<CspSettingBody>>,
) -> Result<Response, ApiError> {
    let chain = account_ancestry(&state.db, account_id).await?;
    if chain.is_empty() {
        return Err(ApiError::NotFound);
    }
    if !can_manage_settings(&state.db, &auth, &chain).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas `params.require(:status)` answers 400 when the parameter is
    // missing or invalid.
    let Some(Json(body)) = body else {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "param is missing or invalid",
        ));
    };
    let status = body.status.unwrap_or_default();
    let csp = account_csp(&state.db, account_id).await?;

    if matches!(status.as_str(), "enabled" | "disabled") && csp.inherited && csp.locked {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "cannot set when locked by parent account",
        ));
    }

    match status.as_str() {
        "enabled" | "disabled" => {
            let enabled = status == "enabled";
            write_account_csp_setting(&state.db, account_id, enabled, csp.locked).await?;
        }
        "inherited" => clear_account_csp_setting(&state.db, account_id).await?,
        _ => {
            return Ok(message_error(StatusCode::BAD_REQUEST, "invalid setting"));
        }
    }

    let csp = account_csp(&state.db, account_id).await?;
    Ok(Json(account_settings_json(&state.db, account_id, &csp).await?).into_response())
}

/// Canvas `Csp::AccountHelper#set_csp_setting!`: write the account's own
/// `csp_inherited_data` value, keeping the lock flag.
async fn write_account_csp_setting(
    pool: &PgPool,
    account_id: i64,
    enabled: bool,
    locked: bool,
) -> Result<(), ApiError> {
    let mut settings = account_settings_object(pool, account_id).await?;
    let mut data = settings
        .get("csp_inherited_data")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    data.insert("value".to_string(), json!([enabled, account_id]));
    data.insert("locked".to_string(), json!(locked));
    settings.insert("csp_inherited_data".to_string(), Value::Object(data));
    save_account_settings(pool, account_id, settings).await
}

/// Canvas `Csp::AccountHelper#set_csp_locked!`: flip the lock flag of the
/// account's own setting.
async fn write_account_csp_lock(
    pool: &PgPool,
    account_id: i64,
    locked: bool,
) -> Result<(), ApiError> {
    let mut settings = account_settings_object(pool, account_id).await?;
    let mut data = settings
        .get("csp_inherited_data")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    data.insert("locked".to_string(), json!(locked));
    settings.insert("csp_inherited_data".to_string(), Value::Object(data));
    save_account_settings(pool, account_id, settings).await
}

/// The account's `settings` hash as an object; `{}` when it is unset.
async fn account_settings_object(
    pool: &PgPool,
    account_id: i64,
) -> Result<Map<String, Value>, ApiError> {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT settings::text FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(pool)
            .await?;
    Ok(stored
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default())
}

async fn save_account_settings(
    pool: &PgPool,
    account_id: i64,
    settings: Map<String, Value>,
) -> Result<(), ApiError> {
    let text = serde_json::to_string(&Value::Object(settings))
        .map_err(|err| ApiError::Db(format!("encode account settings: {err}")))?;
    sqlx::query("UPDATE accounts SET settings = $2::jsonb, updated_at = now() WHERE id = $1")
        .bind(account_id)
        .bind(text)
        .execute(pool)
        .await?;
    Ok(())
}

/// Canvas `Csp::AccountHelper#inherit_csp!`: drop the account's own setting
/// so it falls back to the parent account.
async fn clear_account_csp_setting(pool: &PgPool, account_id: i64) -> Result<(), ApiError> {
    let mut settings = account_settings_object(pool, account_id).await?;
    settings.remove("csp_inherited_data");
    save_account_settings(pool, account_id, settings).await
}

/// PUT /courses/{courseId}/csp_settings — Canvas `set_csp_setting`.
async fn set_course_setting(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
    body: Option<Json<CspSettingBody>>,
) -> Result<Response, ApiError> {
    let csp = course_csp(&state.db, course_id).await?;
    let chain = match csp.account_id {
        Some(account_id) => account_ancestry(&state.db, account_id).await?,
        None => Vec::new(),
    };
    if !can_manage_settings(&state.db, &auth, &chain).await? {
        return Err(ApiError::Forbidden);
    }
    let Some(Json(body)) = body else {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "param is missing or invalid",
        ));
    };
    let status = body.status.unwrap_or_default();

    if matches!(status.as_str(), "enabled" | "disabled") && csp.inherited && csp.locked {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "cannot set when locked by parent account",
        ));
    }

    match status.as_str() {
        "enabled" => {
            // Canvas: a course may only enable CSP when the account has it on.
            let account_enabled = match csp.account_id {
                Some(account_id) => account_csp(&state.db, account_id).await?.enabled,
                None => false,
            };
            if !account_enabled {
                return Ok(message_error(
                    StatusCode::BAD_REQUEST,
                    "must be enabled on account-level first",
                ));
            }
            set_course_disabled(&state.db, course_id, false).await?;
        }
        "disabled" => set_course_disabled(&state.db, course_id, true).await?,
        "inherited" => set_course_disabled(&state.db, course_id, false).await?,
        _ => {
            return Ok(message_error(StatusCode::BAD_REQUEST, "invalid setting"));
        }
    }

    let csp = course_csp(&state.db, course_id).await?;
    Ok(Json(course_settings_json(&state.db, &csp).await?).into_response())
}

/// Canvas `Csp::CourseHelper#enable_csp!` / `#disable_csp!` / `#inherit_csp!`.
async fn set_course_disabled(
    pool: &PgPool,
    course_id: i64,
    disabled: bool,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE courses SET csp_disabled = $2, updated_at = now() WHERE id = $1")
        .bind(course_id)
        .bind(disabled)
        .execute(pool)
        .await?;
    Ok(())
}

/// PUT /accounts/{accountId}/csp_settings/lock — Canvas `set_csp_lock`.
async fn set_csp_lock(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    body: Option<Json<CspLockBody>>,
) -> Result<Response, ApiError> {
    let chain = account_ancestry(&state.db, account_id).await?;
    if chain.is_empty() {
        return Err(ApiError::NotFound);
    }
    if !can_manage_settings(&state.db, &auth, &chain).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas `params.require(:settings_locked)`.
    let Some(Json(body)) = body else {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "param is missing or invalid",
        ));
    };
    let csp = account_csp(&state.db, account_id).await?;
    if csp.inherited {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "CSP must be explicitly set on this account",
        ));
    }
    // Canvas raises when the account has no own setting to lock; the port
    // answers the same 400 it uses for an inherited setting.
    let stored: Option<String> =
        sqlx::query_scalar("SELECT settings::text FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await?;
    let has_own = stored
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.get("csp_inherited_data").cloned())
        .is_some();
    if !has_own {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "CSP must be explicitly set on this account",
        ));
    }
    let locked = value_to_boolean(body.settings_locked.as_deref());
    write_account_csp_lock(&state.db, account_id, locked).await?;

    let csp = account_csp(&state.db, account_id).await?;
    Ok(Json(account_settings_json(&state.db, account_id, &csp).await?).into_response())
}

/// POST /accounts/{accountId}/csp_settings/domains — Canvas `add_domain`.
async fn add_domain_action(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    body: Option<Json<DomainBody>>,
) -> Result<Response, ApiError> {
    let chain = account_ancestry(&state.db, account_id).await?;
    if chain.is_empty() {
        return Err(ApiError::NotFound);
    }
    if !can_manage_settings(&state.db, &auth, &chain).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas `params.require(:domain)`.
    let Some(Json(body)) = body else {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "param is missing or invalid",
        ));
    };
    let Some(domain) = body.domain else {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "param is missing or invalid",
        ));
    };
    if !add_domain(&state.db, account_id, &domain).await? {
        return Ok(message_error(StatusCode::BAD_REQUEST, "invalid domain"));
    }
    Ok(Json(whitelist_json(&state.db, account_id).await?).into_response())
}

/// POST /accounts/{accountId}/csp_settings/domains/batch_create — Canvas
/// `add_multiple_domains`.
async fn add_multiple_domains(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    body: Option<Json<DomainBatchBody>>,
) -> Result<Response, ApiError> {
    let chain = account_ancestry(&state.db, account_id).await?;
    if chain.is_empty() {
        return Err(ApiError::NotFound);
    }
    if !can_manage_settings(&state.db, &auth, &chain).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas `params.require(:domains)`.
    let Some(Json(body)) = body else {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "param is missing or invalid",
        ));
    };
    let Some(domains) = body.domains else {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            "param is missing or invalid",
        ));
    };
    let invalid: Vec<&str> = domains
        .iter()
        .filter(|domain| !domain_is_valid(domain))
        .map(|domain| domain.as_str())
        .collect();
    if !invalid.is_empty() {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            &format!("invalid domains: {}", invalid.join(", ")),
        ));
    }
    let mut failed: Vec<String> = Vec::new();
    for domain in &domains {
        if !add_domain(&state.db, account_id, domain).await? {
            failed.push(domain.clone());
        }
    }
    if !failed.is_empty() {
        return Ok(message_error(
            StatusCode::BAD_REQUEST,
            &format!("failed adding some domains: {}", failed.join(", ")),
        ));
    }
    Ok(Json(whitelist_json(&state.db, account_id).await?).into_response())
}

/// DELETE /accounts/{accountId}/csp_settings/domains — Canvas `remove_domain`.
async fn remove_domain_action(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    Query(params): Query<DomainBody>,
    body: Option<Json<DomainBody>>,
) -> Result<Response, ApiError> {
    let chain = account_ancestry(&state.db, account_id).await?;
    if chain.is_empty() {
        return Err(ApiError::NotFound);
    }
    if !can_manage_settings(&state.db, &auth, &chain).await? {
        return Err(ApiError::Forbidden);
    }
    let domain = body
        .and_then(|Json(body)| body.domain)
        .or(params.domain)
        .unwrap_or_default();
    if domain.trim().is_empty() {
        return Ok(message_error(StatusCode::BAD_REQUEST, "invalid domain"));
    }
    remove_domain(&state.db, account_id, &domain).await?;
    Ok(Json(whitelist_json(&state.db, account_id).await?).into_response())
}

/// The CSP router. Merged into the crate router by `lib.rs`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{accountId}/csp_settings",
            get(get_account_settings).put(set_account_setting),
        )
        .route(
            "/courses/{courseId}/csp_settings",
            get(get_course_settings).put(set_course_setting),
        )
        .route("/accounts/{accountId}/csp_settings/lock", put(set_csp_lock))
        .route(
            "/accounts/{accountId}/csp_settings/domains",
            post(add_domain_action).delete(remove_domain_action),
        )
        .route(
            "/accounts/{accountId}/csp_settings/domains/batch_create",
            post(add_multiple_domains),
        )
}
