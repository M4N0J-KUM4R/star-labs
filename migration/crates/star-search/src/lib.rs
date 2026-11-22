//! star-search — search recipients/all_courses/rubrics and course smartsearch
//! (Canvas SearchController, SmartSearchController) — issue #91, spec
//! `specs/p5-rest-search-analytics.md`.
//!
//! Endpoints (spec §3/§4, 7 routes):
//! - GET /search/recipients                     — recipient picker (users + contexts)
//! - GET /conversations/find_recipients         — deprecated alias, same action
//! - GET /search/all_courses                    — PUBLIC course catalog
//! - GET /search/rubrics                        — rubric matcher (bare array)
//! - GET /courses/{course_id}/smartsearch        — smart search (keyword first cut)
//! - GET /courses/{course_id}/smartsearch/index_status — index readiness poll
//! - GET /courses/{course_id}/smartsearch/log     — telemetry no-op (204)
//!
//! Behavior contract (spec §3.1/§3.2/§4 + Canvas reading):
//! - recipients: AND-term name matching (whitespace-separated terms, all
//!   must match — Canvas `search.strip.split(/\s+/)` + `wildcard`), user
//!   visibility computed from shared enrollments/group_memberships
//!   (AddressBook semantics), `context` scoping to course_N/group_N (an
//!   invalid context is nulled, never a 404 — Canvas AddressBook.
//!   load_context), `exclude[]` in both forms (plain user ids and
//!   course_/group_ asset strings), `user_id` exact lookup returning at
//!   most one result (with from_conversation_id widening "known" to
//!   co-participants), `permissions[]` reported per context (only
//!   requested keys; empty object when none requested), `messageable_only`
//!   filters contexts on send_messages. User entries follow Canvas'
//!   conversation_user_json (id/name/full_name/avatar_url/common_courses/
//!   common_groups); context entries follow matching_contexts (id asset
//!   string/name/type/user_count/permissions).
//! - all_courses: PUBLIC (Canvas `skip_before_action :require_user`); the
//!   root account's `enable_course_catalog` setting gates it — off → 401
//!   `{"status":"unauthenticated","errors":[{"message":"user authorization
//!   required"}]}` (Canvas render_json_unauthorized for an anonymous
//!   caller). Catalog on → available courses of the root account ordered by
//!   created_at with `search` (name/course_code/id LIKE — Canvas name_like),
//!   `public_only` and `open_enrollment_only` filters. DEVIATION: the
//!   star-lms courses table has no indexed/is_public/open_enrollment
//!   columns, so the flags read from the root account's settings JSONB
//!   `course_catalog` map ({course_id: {indexed, is_public,
//!   open_enrollment}}; indexed defaults true — Canvas courses are indexed
//!   unless explicitly unindexed). JSON path returns the bare course array
//!   (Canvas `render json: @courses.as_json`, CourseJson BASE attributes;
//!   per_page 0/negative → unset, default 50, max 100).
//! - rubrics: the caller's managed-context rubrics (teacher enrollments;
//!   admins see every course) plus the public reusable pool (DEVIATION: no
//!   `reusable` column — the pool is account-level rubrics, course_id NULL),
//!   then the verified double title downcase-match filter, BARE ARRAY
//!   response. Course reader not required by Canvas (the action is
//!   `get_context`-exempt for :recipients but rubrics uses get_context —
//!   unknown/hidden course → 404, non-reader → 403 per spec §3.1).
//! - smartsearch: course context (404 unknown/deleted); Canvas guard order
//!   is course `:read` then SmartSearch.smart_search_available? — both
//!   render_unauthorized_action, which is 401 for anonymous callers and
//!   403 for authenticated ones (render_json_unauthorized :forbidden). Availability is the course's
//!   `smart_search` feature flag (DEVIATION: Canvas also requires the
//!   Bedrock provider, which this port does not have). `q` missing → 400
//!   `{"error": "missing 'q' param"}`. Results carry content_id/
//!   content_type/readable_type/title/body/html_url/distance/relevance over
//!   pages, assignments, announcements and discussion_topics (SQL ILIKE
//!   first cut; DEVIATION: distance approximated — 0.0 title hit, 0.5
//!   body-only — per spec §4.1/§8 Q2). When the index row is not indexed
//!   the response adds status "index_incomplete" + indexing_progress and
//!   the touch marks the course indexed/100 (spec §4.1 trivial first cut).
//!   include[]=status adds published/due_date; include[]=modules adds an
//!   empty modules array (content_tags not wired). index_status reads the
//!   smart_search_indexes row (migration 0131): no row → {"status":
//!   "indexing", "progress": 0} (Canvas check_course first touch). log
//!   accepts a/c/course_id/oid/ot/q and discards → 204 (Canvas
//!   `head :no_content`).
//! - Permission order everywhere: 401 anonymous FIRST (AuthUser extractor),
//!   then 404 unknown context (scoped lookups), then 403 failed ability,
//!   then 400 validation.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Error type (PollError style: plain-text statuses, JSON where Canvas sends
// JSON)
// ---------------------------------------------------------------------------

enum SearchError {
    NotFound,
    /// Canvas render_json_unauthorized: 401 with the status/errors envelope.
    Unauthorized,
    Forbidden,
    /// 400 with a JSON body (Canvas `render json: {...}, status:
    /// :bad_request`).
    BadRequest(Value),
    Db,
}

impl From<sqlx::Error> for SearchError {
    fn from(e: sqlx::Error) -> Self {
        eprintln!("database error: {e}");
        SearchError::Db
    }
}

impl IntoResponse for SearchError {
    fn into_response(self) -> Response {
        match self {
            SearchError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            SearchError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "status": "unauthenticated",
                    "errors": [{ "message": "user authorization required" }]
                })),
            )
                .into_response(),
            SearchError::Forbidden => (StatusCode::FORBIDDEN, "forbidden").into_response(),
            SearchError::BadRequest(body) => (StatusCode::BAD_REQUEST, Json(body)).into_response(),
            SearchError::Db => {
                (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Whitespace-separated search terms, downcased (Canvas
/// `search.strip.split(/\s+/)` + `wildcard` downcasing); AND semantics —
/// every term must match.
fn search_terms(raw: &str) -> Vec<String> {
    raw.trim()
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Canvas `wildcard` ILIKE pattern: %term% with LIKE wildcards escaped.
fn like_pattern(term: &str) -> String {
    let escaped = term
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// The site-admin role bypasses course ability checks (Canvas grants site
/// admins course `:read`).
fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Course reader: enrolled (any role) or site admin (Canvas course `:read`).
async fn course_reader(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, SearchError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let reader: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(pool)
    .await?;
    Ok(reader)
}

/// The root account (id, settings) — Canvas `@domain_root_account`.
async fn root_account(pool: &PgPool) -> Result<Option<(i64, Value)>, SearchError> {
    let row: Option<(i64, Value)> = sqlx::query_as(
        "SELECT id, settings FROM accounts \
         WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Canvas boolean query param: "true"/"1" → true, anything else blank.
fn bool_query(params: &[(String, String)], key: &str) -> bool {
    params
        .iter()
        .find(|(k, _)| k == key)
        .is_some_and(|(_, v)| v == "true" || v == "1")
}

/// One value of a query param (last wins, matching Rails).
fn one(params: &[(String, String)], key: &str) -> Option<String> {
    params
        .iter()
        .filter(|(k, _)| k == key)
        .last()
        .map(|(_, v)| v.clone())
}

/// All values of an array query param (`exclude[]`, `filter[]`, ...).
fn all(params: &[(String, String)], key: &str, array_key: &str) -> Vec<String> {
    params
        .iter()
        .filter(|(k, _)| k == key || k == array_key)
        .map(|(_, v)| v.clone())
        .collect()
}

/// Canvas `Api.paginate` per_page: 0/negative → unset (Canvas
/// `params[:per_page] = nil if params[:per_page].to_i <= 0`), default 50,
/// max 100.
fn per_page_of(params: &[(String, String)]) -> i64 {
    one(params, "per_page")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(50)
        .clamp(1, 100)
}

// ---------------------------------------------------------------------------
// GET /search/recipients + /conversations/find_recipients (search#recipients)
// ---------------------------------------------------------------------------

/// One addressable context (Canvas load_all_contexts: enrolled courses and
/// the caller's groups).
struct RecipientContext {
    /// Asset string: `course_{id}` or `group_{id}`.
    asset_string: String,
    name: String,
    /// "course" | "group".
    kind: &'static str,
    /// The course the context lives in (groups carry their course).
    course_id: i64,
}

/// Load the caller's addressable contexts: courses the caller is enrolled in
/// plus groups the caller belongs to (Canvas `load_all_contexts`, the
/// no-context branch: current courses + current groups).
async fn load_contexts(pool: &PgPool, caller: i64) -> Result<Vec<RecipientContext>, SearchError> {
    let courses: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT c.id, c.name FROM courses c \
         JOIN enrollments e ON e.course_id = c.id \
         WHERE e.user_id = $1 AND e.workflow_state <> 'deleted' \
           AND c.workflow_state <> 'deleted'",
    )
    .bind(caller)
    .fetch_all(pool)
    .await?;
    let groups: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT DISTINCT g.id, g.name, g.course_id FROM groups g \
         JOIN group_memberships m ON m.group_id = g.id \
         WHERE m.user_id = $1 AND m.workflow_state = 'accepted' \
           AND g.workflow_state <> 'deleted' AND g.course_id IS NOT NULL",
    )
    .bind(caller)
    .fetch_all(pool)
    .await?;
    let mut contexts: Vec<RecipientContext> = courses
        .into_iter()
        .map(|(id, name)| RecipientContext {
            asset_string: format!("course_{id}"),
            name,
            kind: "course",
            course_id: id,
        })
        .collect();
    contexts.extend(
        groups
            .into_iter()
            .map(|(id, name, course_id)| RecipientContext {
                asset_string: format!("group_{id}"),
                name,
                kind: "group",
                course_id,
            }),
    );
    Ok(contexts)
}

/// send_messages per context: Canvas grants course messaging through the
/// `send_messages` role override (default on); groups inherit their course.
/// The workspace has no role-override storage, so every loaded context
/// grants it (documented simplification).
fn context_permissions(_context: &RecipientContext) -> Value {
    json!({ "send_messages": true })
}

/// The `type`/`types[]` filter (Canvas search_contexts_and_users): "user" →
/// users only; "context" (or course/section/group) → contexts.
fn type_allows_contexts(params: &[(String, String)]) -> bool {
    let types = all(params, "type", "types[]");
    types.is_empty()
        || types.iter().any(|t| {
            matches!(
                t.as_str(),
                "context" | "course" | "section" | "group" | "differentiation_tag"
            )
        })
}

fn type_allows_users(params: &[(String, String)]) -> bool {
    let types = all(params, "type", "types[]");
    types.is_empty() || types.iter().any(|t| t == "user")
}

/// Partition `exclude[]` into user ids and context asset strings (Canvas
/// `AddressBook.partition_recipients`): plain integers exclude users,
/// `course_N`/`group_N`/`section_N` values exclude contexts.
fn partition_excludes(params: &[(String, String)]) -> (Vec<i64>, Vec<String>) {
    let mut user_ids = Vec::new();
    let mut contexts = Vec::new();
    for value in all(params, "exclude", "exclude[]") {
        let v = value.trim();
        if v.starts_with("course_") || v.starts_with("group_") || v.starts_with("section_") {
            contexts.push(v.to_string());
        } else if let Ok(id) = v.parse::<i64>() {
            user_ids.push(id);
        }
    }
    (user_ids, contexts)
}

/// Context entries (Canvas matching_contexts): AND-term name match, exclude
/// filter, messageable_only filter, messageable user counts, reported
/// permissions.
async fn context_recipients(
    pool: &PgPool,
    caller: i64,
    params: &[(String, String)],
    search: &str,
    scoped_course: Option<i64>,
    permissions_requested: &[String],
    messageable_only: bool,
) -> Result<Vec<Value>, SearchError> {
    let terms = search_terms(search);
    let (_, exclude_contexts) = partition_excludes(params);
    let mut out = Vec::new();
    for context in load_contexts(pool, caller).await? {
        // `context` scoping: only the named course (or the groups of that
        // course, Canvas `matching_contexts` when context_name is present).
        if let Some(course_id) = scoped_course {
            if context.course_id != course_id {
                continue;
            }
        }
        if exclude_contexts.iter().any(|x| *x == context.asset_string) {
            continue;
        }
        let lower = context.name.to_lowercase();
        if !terms.iter().all(|t| lower.contains(t.as_str())) {
            continue;
        }
        let permissions = context_permissions(&context);
        if messageable_only
            && !permissions
                .get("send_messages")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            continue;
        }
        // Messageable user count (Canvas count_in_contexts).
        let (count,): (i64,) = if context.kind == "course" {
            sqlx::query_as(
                "SELECT COUNT(DISTINCT e.user_id) FROM enrollments e \
                 WHERE e.course_id = $1 AND e.workflow_state <> 'deleted'",
            )
            .bind(context.course_id)
            .fetch_one(pool)
            .await?
        } else {
            let group_id: i64 = context.asset_string["group_".len()..]
                .parse()
                .unwrap_or_default();
            sqlx::query_as(
                "SELECT COUNT(DISTINCT m.user_id) FROM group_memberships m \
                 WHERE m.group_id = $1 AND m.workflow_state = 'accepted'",
            )
            .bind(group_id)
            .fetch_one(pool)
            .await?
        };
        // permissions[]: only requested keys are reported (Canvas
        // rights_status(*permissions)); {} when none requested.
        let reported = if permissions_requested.is_empty() {
            json!({})
        } else {
            let mut map = Map::new();
            for p in permissions_requested {
                if let Some(v) = permissions.get(p) {
                    map.insert(p.clone(), v.clone());
                }
            }
            Value::Object(map)
        };
        out.push(json!({
            "id": context.asset_string,
            "name": context.name,
            "type": "context",
            "user_count": count,
            "permissions": reported,
        }));
    }
    Ok(out)
}

/// Users known to the caller through a shared enrollment or group (Canvas
/// MessageableUser search: AND terms on name, exclude ids, context
/// scoping), serialized with common_courses/common_groups (AddressBook
/// common_*: {context_id: [role]}).
async fn user_recipients(
    pool: &PgPool,
    caller: i64,
    params: &[(String, String)],
    search: &str,
    scoped_course: Option<i64>,
    per_page: i64,
) -> Result<Vec<Value>, SearchError> {
    let terms = search_terms(search);
    let (exclude_users, _) = partition_excludes(params);
    // Shared-visibility core (AddressBook known-users): a shared enrollment
    // or a shared group membership.
    // Canvas search_messageable_users merges a "self" collection with the
    // shared-context collections, so the caller matches their own name too.
    let mut conditions = String::from(
        "(u.id = $1 \
          OR u.id IN (SELECT e2.user_id FROM enrollments e2 \
           JOIN enrollments e1 ON e1.course_id = e2.course_id \
           WHERE e1.user_id = $1 AND e2.user_id <> $1 \
             AND e1.workflow_state <> 'deleted' AND e2.workflow_state <> 'deleted') \
          OR u.id IN (SELECT m2.user_id FROM group_memberships m2 \
           JOIN group_memberships m1 ON m1.group_id = m2.group_id \
           WHERE m1.user_id = $1 AND m2.user_id <> $1 \
             AND m1.workflow_state = 'accepted' AND m2.workflow_state = 'accepted'))",
    );
    if let Some(course_id) = scoped_course {
        // Context scoping: users known through that course, including group
        // members of the course's groups (Canvas
        // messageable_users_in_context for a Course context).
        conditions.push_str(&format!(
            " AND (u.id IN (SELECT e.user_id FROM enrollments e \
               WHERE e.course_id = {course_id} AND e.workflow_state <> 'deleted') \
            OR u.id IN (SELECT m.user_id FROM group_memberships m \
               JOIN groups g ON g.id = m.group_id \
               WHERE g.course_id = {course_id} AND m.workflow_state = 'accepted'))"
        ));
    }
    for (i, _term) in terms.iter().enumerate() {
        let param = format!("${}", i + 2);
        conditions.push_str(&format!(
            " AND (LOWER(u.name) LIKE {param} \
               OR LOWER(COALESCE(u.sortable_name, u.name)) LIKE {param})"
        ));
    }
    let mut query = format!("SELECT DISTINCT u.id, u.name FROM users u WHERE {conditions}");
    if !exclude_users.is_empty() {
        query.push_str(&format!(" AND u.id <> ALL(${})", terms.len() + 2));
    }
    query.push_str(" ORDER BY LOWER(COALESCE(u.sortable_name, u.name)) ASC, u.id ASC");
    query.push_str(&format!(" LIMIT {per_page}"));
    let mut q = sqlx::query_as::<_, (i64, String)>(&query).bind(caller);
    for term in &terms {
        q = q.bind(like_pattern(term));
    }
    if !exclude_users.is_empty() {
        q = q.bind(exclude_users);
    }
    let rows = q.fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, name) in rows {
        // common_courses: shared courses with the caller's roles for that
        // user (Canvas common_courses → {course_id: [role]}).
        let courses: Vec<(i64, String)> = sqlx::query_as(
            "SELECT DISTINCT c.id, e2.role FROM courses c \
             JOIN enrollments e2 ON e2.course_id = c.id \
             JOIN enrollments e1 ON e1.course_id = c.id \
             WHERE e1.user_id = $1 AND e2.user_id = $2 \
               AND e1.workflow_state <> 'deleted' AND e2.workflow_state <> 'deleted' \
               AND c.workflow_state <> 'deleted'",
        )
        .bind(caller)
        .bind(id)
        .fetch_all(pool)
        .await?;
        let mut common_courses = Map::new();
        for (course_id, role) in courses {
            let key = course_id.to_string();
            match common_courses.get_mut(&key) {
                Some(Value::Array(arr)) => {
                    if !arr.iter().any(|r| r.as_str() == Some(role.as_str())) {
                        arr.push(json!(role));
                    }
                }
                _ => {
                    common_courses.insert(key, json!([role]));
                }
            }
        }
        // common_groups: shared groups, membership role "Member" (Canvas
        // GroupMembership role for students).
        let groups: Vec<(i64,)> = sqlx::query_as(
            "SELECT DISTINCT g.id FROM groups g \
             JOIN group_memberships m2 ON m2.group_id = g.id \
             JOIN group_memberships m1 ON m1.group_id = g.id \
             WHERE m1.user_id = $1 AND m2.user_id = $2 \
               AND m1.workflow_state = 'accepted' AND m2.workflow_state = 'accepted'",
        )
        .bind(caller)
        .bind(id)
        .fetch_all(pool)
        .await?;
        let mut common_groups = Map::new();
        for (group_id,) in groups {
            common_groups.insert(group_id.to_string(), json!(["Member"]));
        }
        out.push(json!({
            "id": id,
            "name": name,
            "full_name": name,
            "avatar_url": Value::Null,
            "common_courses": Value::Object(common_courses),
            "common_groups": Value::Object(common_groups),
        }));
    }
    Ok(out)
}

/// The `recipients` action (spec §3 rows 1–2; both paths share it). Guard
/// order: 401 anonymous (AuthUser extractor) — then parameter handling; an
/// unknown `context` is nulled (Canvas AddressBook.load_context), never a
/// 404.
async fn recipients(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, SearchError> {
    let search = one(&params, "search").unwrap_or_default();
    let messageable_only = bool_query(&params, "messageable_only");
    let permissions_requested = all(&params, "permissions", "permissions[]");
    let per_page = per_page_of(&params);

    // user_id: exact lookup, at most one result, ignoring search/context
    // (Canvas known_user). from_conversation_id widens "known" to
    // co-participants of that conversation.
    if let Some(user_id_raw) = one(&params, "user_id") {
        let user_id = user_id_raw
            .trim()
            .parse::<i64>()
            .map_err(|_| SearchError::BadRequest(json!({"error": "invalid user_id parameter"})))?;
        let from_conversation =
            one(&params, "from_conversation_id").and_then(|v| v.trim().parse::<i64>().ok());
        let known: Option<(i64, String)> = sqlx::query_as(
            "SELECT u.id, u.name FROM users u WHERE u.id = $1 AND ( \
               u.id IN (SELECT e2.user_id FROM enrollments e2 \
                 JOIN enrollments e1 ON e1.course_id = e2.course_id \
                 WHERE e1.user_id = $2 AND e2.user_id = $1 \
                   AND e1.workflow_state <> 'deleted' AND e2.workflow_state <> 'deleted') \
            OR u.id IN (SELECT m2.user_id FROM group_memberships m2 \
                 JOIN group_memberships m1 ON m1.group_id = m2.group_id \
                 WHERE m1.user_id = $2 AND m2.user_id = $1 \
                   AND m1.workflow_state = 'accepted' AND m2.workflow_state = 'accepted') \
            OR ($3::BIGINT IS NOT NULL AND u.id IN (SELECT p2.user_id \
                 FROM conversation_participants p2 \
                 JOIN conversation_participants p1 ON p1.conversation_id = p2.conversation_id \
                 WHERE p1.user_id = $2 AND p2.user_id = $1 \
                   AND p1.workflow_state <> 'deleted' AND p2.workflow_state <> 'deleted')))",
        )
        .bind(user_id)
        .bind(auth.id)
        .bind(from_conversation)
        .fetch_optional(&state.db)
        .await?;
        let items = known
            .map(|(id, name)| {
                json!({
                    "id": id,
                    "name": name,
                    "full_name": name,
                    "avatar_url": Value::Null,
                    "common_courses": {},
                    "common_groups": {},
                })
            })
            .into_iter()
            .collect();
        return Ok(Json(items));
    }

    // context scoping: course_N / group_N → the course id to scope by (an
    // invalid or unknown context is dropped, Canvas nulls the param).
    let mut effective_course: Option<i64> = None;
    if let Some(context) = one(&params, "context") {
        if let Some(rest) = context.strip_prefix("course_") {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(course_id) = digits.parse::<i64>() {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM courses \
                     WHERE id = $1 AND workflow_state <> 'deleted')",
                )
                .bind(course_id)
                .fetch_one(&state.db)
                .await?;
                if exists {
                    effective_course = Some(course_id);
                }
            }
        } else if let Some(rest) = context.strip_prefix("group_") {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(group_id) = digits.parse::<i64>() {
                let course: Option<(Option<i64>,)> = sqlx::query_as(
                    "SELECT course_id FROM groups \
                     WHERE id = $1 AND workflow_state <> 'deleted'",
                )
                .bind(group_id)
                .fetch_optional(&state.db)
                .await?;
                effective_course = course.and_then(|(course_id,)| course_id);
            }
        }
    }

    // Canvas builds the collections only when a context or search term is
    // given; contexts are always enumerable, users only with context/search.
    let mut items: Vec<Value> = Vec::new();
    if type_allows_contexts(&params) {
        items.extend(
            context_recipients(
                &state.db,
                auth.id,
                &params,
                &search,
                effective_course,
                &permissions_requested,
                messageable_only,
            )
            .await?,
        );
    }
    if type_allows_users(&params) && (!search.trim().is_empty() || effective_course.is_some()) {
        items.extend(
            user_recipients(
                &state.db,
                auth.id,
                &params,
                &search,
                effective_course,
                per_page,
            )
            .await?,
        );
    }
    Ok(Json(items))
}

// ---------------------------------------------------------------------------
// GET /search/all_courses (search#all_courses)
// ---------------------------------------------------------------------------

/// Canvas all_courses: `Course.where(root_account_id:).where(indexed: true,
/// workflow_state: 'available').order(:created_at)` with name_like /
/// is_public / open_enrollment filters, rendered as the bare course array.
/// The catalog flags live in the root account's settings JSONB (see the
/// crate doc — the courses table has no such columns).
async fn all_courses(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Result<Json<Vec<Value>>, SearchError> {
    // PUBLIC route (no auth extractor): the catalog gate comes first —
    // catalog off → the 401 JSON envelope for every caller.
    let Some((root_id, settings)) = root_account(&state.db).await? else {
        return Err(SearchError::Unauthorized);
    };
    if !settings
        .get("enable_course_catalog")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(SearchError::Unauthorized);
    }

    let search = one(&params, "search").unwrap_or_default();
    let public_only = bool_query(&params, "public_only");
    let open_enrollment_only = bool_query(&params, "open_enrollment_only");
    let per_page = per_page_of(&params);
    let page = one(&params, "page")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    let offset = page.saturating_sub(1).saturating_mul(per_page);

    let rows: Vec<(i64, String, String, Option<i64>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, name, course_code, account_id, created_at FROM courses \
         WHERE workflow_state = 'available' AND account_id = $1 \
         ORDER BY created_at ASC, id ASC",
    )
    .bind(root_id)
    .fetch_all(&state.db)
    .await?;
    let catalog_cfg = settings.get("course_catalog").cloned().unwrap_or(json!({}));
    let courses: Vec<(i64, String, String, Option<i64>, DateTime<Utc>, bool)> = rows
        .into_iter()
        .filter_map(|(id, name, course_code, account_id, created_at)| {
            let cfg = catalog_cfg.get(id.to_string());
            let flag = |key: &str, default: bool| {
                cfg.and_then(|c| c.get(key))
                    .and_then(Value::as_bool)
                    .unwrap_or(default)
            };
            // Canvas: indexed default true (courses appear unless unindexed).
            (flag("indexed", true)).then_some((
                id,
                name,
                course_code,
                account_id,
                created_at,
                flag("is_public", false),
            ))
        })
        .filter(|c| !public_only || c.5)
        .collect();
    // search: Canvas name_like — name/course_code substring or exact id.
    let filtered: Vec<&(i64, String, String, Option<i64>, DateTime<Utc>, bool)> =
        if !search.trim().is_empty() {
            let q = search.trim().to_lowercase();
            let by_id = search.trim().parse::<i64>().ok();
            courses
                .iter()
                .filter(|(id, name, code, _, _, _)| {
                    Some(*id) == by_id
                        || name.to_lowercase().contains(&q)
                        || code.to_lowercase().contains(&q)
                })
                .collect()
        } else {
            courses.iter().collect()
        };
    let open_filtered: Vec<&&(i64, String, String, Option<i64>, DateTime<Utc>, bool)> =
        if open_enrollment_only {
            // open_enrollment flag (settings map, same stand-in as is_public).
            filtered
                .iter()
                .filter(|c| {
                    catalog_cfg
                        .get(c.0.to_string())
                        .and_then(|cfg| cfg.get("open_enrollment"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .collect()
        } else {
            filtered.iter().collect()
        };
    let items: Vec<Value> = open_filtered
        .iter()
        .skip(offset as usize)
        .take(per_page as usize)
        .map(
            |(id, name, course_code, account_id, created_at, is_public)| {
                // Canvas `@courses.as_json` — the CourseJson BASE attribute set;
                // columns this workspace lacks serialize as null.
                json!({
                    "id": id,
                    "name": name,
                    "course_code": course_code,
                    "account_id": account_id,
                    "created_at": created_at,
                    "start_at": Value::Null,
                    "default_view": Value::Null,
                    "enrollment_term_id": Value::Null,
                    "is_public": is_public,
                    "grading_standard_id": Value::Null,
                    "root_account_id": account_id,
                    "uuid": Value::Null,
                    "license": Value::Null,
                    "grade_passback_setting": Value::Null,
                })
            },
        )
        .collect();
    Ok(Json(items))
}

// ---------------------------------------------------------------------------
// GET /search/rubrics (search#rubrics)
// ---------------------------------------------------------------------------

/// Canvas SearchController#rubrics: `@current_user.management_contexts`
/// (courses the caller manages) contribute their rubrics, plus
/// `Rubric.publicly_reusable.matching(q)`; the union then goes through the
/// verified second title downcase-match filter; BARE ARRAY response.
/// Guard order per spec §3.1: the action runs under `get_context` — but
/// takes no course_id param, so an unauthenticated caller is 401 and the
/// matcher itself is the body. star-lms pins: teacher-of-nothing sees only
/// the public pool (Canvas management_contexts rescue → []).
async fn rubrics(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, SearchError> {
    let q = one(&params, "q").unwrap_or_default();
    let q_lower = q.trim().to_lowercase();

    // Managed courses: teacher enrollments; site admins manage everything
    // (Canvas management_contexts covers account administration).
    let course_ids: Vec<i64> = if is_admin(&auth) {
        sqlx::query_scalar("SELECT id FROM courses WHERE workflow_state <> 'deleted' ORDER BY id")
            .fetch_all(&state.db)
            .await?
    } else {
        sqlx::query_scalar(
            "SELECT DISTINCT course_id FROM enrollments \
             WHERE user_id = $1 AND role = 'teacher' AND workflow_state <> 'deleted'",
        )
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?
    };

    let rows: Vec<(i64, i64, String, f64, String, DateTime<Utc>)> = if course_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT id, course_id, title, points_possible, workflow_state, created_at \
             FROM rubrics WHERE course_id = ANY($1) \
               AND workflow_state NOT IN ('deleted','archived') \
             ORDER BY title ASC, id ASC",
        )
        .bind(&course_ids)
        .fetch_all(&state.db)
        .await?
    };
    // The publicly-reusable pool (Canvas Rubric.publicly_reusable) has no
    // star-lms representation: rubrics.course_id is NOT NULL (no
    // account-level rows can exist) and there is no `reusable` column —
    // the pool is structurally empty (documented deviation).

    // The verified double filter — the second pass is a title downcase
    // containment match.
    let items: Vec<Value> = rows
        .iter()
        .filter(|(_, _, title, _, _, _)| {
            q_lower.is_empty() || title.to_lowercase().contains(&q_lower)
        })
        .map(
            |(id, course_id, title, points, workflow_state, created_at)| {
                json!({
                    "id": id,
                    "course_id": course_id,
                    "title": title,
                    "points_possible": points,
                    "workflow_state": workflow_state,
                    "created_at": created_at,
                })
            },
        )
        .collect();
    Ok(Json(items))
}

// ---------------------------------------------------------------------------
// Smart search
// ---------------------------------------------------------------------------

/// Resolve the course context (Canvas get_context on Course.active): 404 for
/// an unknown or deleted course.
async fn load_course(pool: &PgPool, course_id: i64) -> Result<(i64, String), SearchError> {
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(SearchError::NotFound)
}

/// Canvas `SmartSearch.smart_search_available?(context)`: the course's
/// `smart_search` feature flag. Canvas additionally requires the Bedrock
/// provider — this port has none, so the flag alone gates (documented
/// deviation). Feature-flag states: the star-features crate resolves
/// "on"/"allowed_on" as enabled; some tests seed "enabled" directly, which
/// also counts.
async fn smart_search_available(pool: &PgPool, course_id: i64) -> Result<bool, SearchError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT state FROM feature_flags \
         WHERE context_type = 'Course' AND context_id = $1 AND feature = 'smart_search' \
         LIMIT 1",
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(matches!(
        row,
        Some((state,)) if state == "on" || state == "allowed_on" || state == "enabled"
    ))
}

/// The course's index row: none → ("indexing", 0.0) — Canvas check_course on
/// first touch queues the job and reports the live progress (0 before
/// anything is embedded).
async fn index_status_row(pool: &PgPool, course_id: i64) -> Result<(String, f64), SearchError> {
    let row: Option<(String, f64)> =
        sqlx::query_as("SELECT status, progress FROM smart_search_indexes WHERE course_id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.unwrap_or(("indexing".to_string(), 0.0)))
}

/// GET /courses/{course_id}/smartsearch — Canvas
/// SmartSearchController#search. Guard order: course context (404), course
/// `:read` (401), smart_search availability (401), then the `q` requirement
/// (400 `{"error": "missing 'q' param"}`).
async fn smart_search(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<Value>, SearchError> {
    let (course_id, _course_name) = load_course(&state.db, course_id).await?;
    // Canvas render_unauthorized_action: anonymous → 401 (the extractor),
    // logged-in failure → 403 (render_json_unauthorized :forbidden).
    if !course_reader(&state.db, &auth, course_id).await? {
        return Err(SearchError::Forbidden);
    }
    if !smart_search_available(&state.db, course_id).await? {
        return Err(SearchError::Forbidden);
    }
    let Some(q) = one(&params, "q") else {
        return Err(SearchError::BadRequest(
            json!({"error": "missing 'q' param"}),
        ));
    };
    let filters = all(&params, "filter", "filter[]");
    let includes = all(&params, "include", "include[]");

    let mut response = Map::new();
    let (status, progress) = index_status_row(&state.db, course_id).await?;
    if status != "indexed" {
        response.insert("status".to_string(), json!("index_incomplete"));
        response.insert("indexing_progress".to_string(), json!(progress as i64));
        // Spec §4.1 trivial first cut: the first touch marks the course
        // indexed (no worker runs in this workspace).
        sqlx::query(
            "INSERT INTO smart_search_indexes (course_id, status, progress, updated_at) \
             VALUES ($1, 'indexed', 100, now()) \
             ON CONFLICT (course_id) DO UPDATE \
             SET status = 'indexed', progress = 100, updated_at = now()",
        )
        .bind(course_id)
        .execute(&state.db)
        .await?;
    }

    let mut results: Vec<Value> = Vec::new();
    if !q.trim().is_empty() {
        let pattern = like_pattern(&q.trim().to_lowercase());
        let allowed =
            |kind: &str| filters.is_empty() || filters.iter().any(|f| f == kind || f == "context");
        // Pages (Canvas WikiPage, filter "pages"); ScopedToUser visibility
        // approximated as non-deleted (readers see published + unpublished
        // pages of a published course in this workspace).
        if allowed("pages") {
            let rows: Vec<(i64, String, Option<String>, String)> = sqlx::query_as(
                "SELECT id, title, body, workflow_state FROM pages \
                 WHERE course_id = $1 AND workflow_state <> 'deleted' \
                   AND (LOWER(title) LIKE $2 OR LOWER(COALESCE(body, '')) LIKE $2)",
            )
            .bind(course_id)
            .bind(&pattern)
            .fetch_all(&state.db)
            .await?;
            for (id, title, body, wf) in rows {
                let title_hit = title.to_lowercase().contains(&q.trim().to_lowercase());
                results.push(search_result_json(
                    id,
                    "WikiPage",
                    "Page",
                    &title,
                    body.as_deref().unwrap_or(""),
                    &format!("/courses/{course_id}/pages/{}", slug(&title)),
                    title_hit,
                    wf == "published",
                    None,
                    &includes,
                ));
            }
        }
        if allowed("assignments") {
            let rows: Vec<(i64, String, Option<String>, String, Option<DateTime<Utc>>)> =
                sqlx::query_as(
                    "SELECT id, title, description, workflow_state, due_at FROM assignments \
                     WHERE course_id = $1 AND workflow_state <> 'deleted' \
                       AND (LOWER(title) LIKE $2 OR LOWER(COALESCE(description, '')) LIKE $2)",
                )
                .bind(course_id)
                .bind(&pattern)
                .fetch_all(&state.db)
                .await?;
            for (id, title, body, wf, due_at) in rows {
                let title_hit = title.to_lowercase().contains(&q.trim().to_lowercase());
                results.push(search_result_json(
                    id,
                    "Assignment",
                    "Assignment",
                    &title,
                    body.as_deref().unwrap_or(""),
                    &format!("/courses/{course_id}/assignments/{id}"),
                    title_hit,
                    wf == "published",
                    due_at,
                    &includes,
                ));
            }
        }
        // Announcements (DiscussionTopic subclass; filter "announcements").
        if allowed("announcements") {
            let rows: Vec<(i64, String, Option<String>, String)> = sqlx::query_as(
                "SELECT id, title, message, workflow_state FROM announcements \
                 WHERE course_id = $1 AND workflow_state <> 'deleted' \
                   AND (LOWER(title) LIKE $2 OR LOWER(COALESCE(message, '')) LIKE $2)",
            )
            .bind(course_id)
            .bind(&pattern)
            .fetch_all(&state.db)
            .await?;
            for (id, title, body, wf) in rows {
                let title_hit = title.to_lowercase().contains(&q.trim().to_lowercase());
                results.push(search_result_json(
                    id,
                    "Announcement",
                    "Announcement",
                    &title,
                    body.as_deref().unwrap_or(""),
                    &format!("/courses/{course_id}/discussion_topics/{id}"),
                    title_hit,
                    wf == "active",
                    None,
                    &includes,
                ));
            }
        }
        // Plain discussion topics (filter "discussion_topics"; the
        // discussion_topics table also holds announcements upstream, but in
        // this workspace announcements live in their own table).
        if allowed("discussion_topics") {
            let rows: Vec<(i64, String, Option<String>, String)> = sqlx::query_as(
                "SELECT id, title, message, workflow_state FROM discussion_topics \
                 WHERE course_id = $1 AND workflow_state = 'active' \
                   AND (LOWER(title) LIKE $2 OR LOWER(COALESCE(message, '')) LIKE $2)",
            )
            .bind(course_id)
            .bind(&pattern)
            .fetch_all(&state.db)
            .await?;
            for (id, title, body, wf) in rows {
                let title_hit = title.to_lowercase().contains(&q.trim().to_lowercase());
                results.push(search_result_json(
                    id,
                    "DiscussionTopic",
                    "Discussion Topic",
                    &title,
                    body.as_deref().unwrap_or(""),
                    &format!("/courses/{course_id}/discussion_topics/{id}"),
                    title_hit,
                    wf == "active",
                    None,
                    &includes,
                ));
            }
        }
        // Order by the approximated distance ascending (Canvas: distance
        // ASC — closer matches first).
        results.sort_by(|a, b| {
            a["distance"]
                .as_f64()
                .unwrap_or(f64::MAX)
                .partial_cmp(&b["distance"].as_f64().unwrap_or(f64::MAX))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a["content_id"].as_i64().cmp(&b["content_id"].as_i64()))
        });
    }
    response.insert("results".to_string(), Value::Array(results));
    Ok(Json(Value::Object(response)))
}

/// One SearchResult hash (Canvas lib/api/v1/search_result.rb):
/// content_id/content_type/readable_type/title/body/html_url/distance/
/// relevance, plus include[]=status (published/due_date) and
/// include[]=modules (empty array — content_tags not wired; documented
/// stand-in). `title_hit` drives the approximated distance: 0.0 for title
/// matches (closer), 0.5 for body-only matches; relevance mirrors Canvas v1
/// `100*(1-distance)` (documented deviation, spec §8 Q2).
#[allow(clippy::too_many_arguments)]
fn search_result_json(
    content_id: i64,
    content_type: &str,
    readable_type: &str,
    title: &str,
    body: &str,
    html_url: &str,
    title_hit: bool,
    published: bool,
    due_at: Option<DateTime<Utc>>,
    includes: &[String],
) -> Value {
    let distance = if title_hit { 0.0 } else { 0.5 };
    let mut hash = Map::new();
    hash.insert("content_id".to_string(), json!(content_id));
    hash.insert("content_type".to_string(), json!(content_type));
    hash.insert("readable_type".to_string(), json!(readable_type));
    hash.insert("title".to_string(), json!(title));
    hash.insert("body".to_string(), json!(html_to_text(body)));
    hash.insert("html_url".to_string(), json!(html_url));
    hash.insert("distance".to_string(), json!(distance));
    hash.insert(
        "relevance".to_string(),
        json!((100.0f64 * (1.0 - distance)).round() as i64),
    );
    if includes.iter().any(|i| i == "modules") {
        hash.insert("modules".to_string(), json!([]));
    }
    if includes.iter().any(|i| i == "status") {
        hash.insert("published".to_string(), json!(published));
        hash.insert("due_date".to_string(), json!(due_at));
    }
    Value::Object(hash)
}

/// Minimal HTML-to-text (Canvas HtmlTextHelper html_to_text): strip tags,
/// collapse whitespace.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Canvas WikiPage url slug (title downcased, non-alphanumerics to '-').
fn slug(title: &str) -> String {
    let mut slug = String::new();
    for ch in title.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_string()
}

/// GET /courses/{course_id}/smartsearch/index_status — Canvas
/// SmartSearchController#index_status: checks availability (401 when off)
/// but not the search tab nor course `:read`.
async fn smart_search_index_status(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    _auth: AuthUser,
) -> Result<Json<Value>, SearchError> {
    let (course_id, _) = load_course(&state.db, course_id).await?;
    // Same 401-anonymous / 403-authenticated split as #search.
    if !smart_search_available(&state.db, course_id).await? {
        return Err(SearchError::Forbidden);
    }
    let (status, progress) = index_status_row(&state.db, course_id).await?;
    Ok(Json(json!({
        "status": if status == "indexed" { "indexed" } else { "indexing" },
        "progress": progress,
    })))
}

/// GET /courses/{course_id}/smartsearch/log — Canvas
/// SmartSearchController#log: a telemetry no-op ("TODO: do something more")
/// accepting a/c/course_id/oid/ot/q; `head :no_content` → 204.
async fn smart_search_log(
    Path(_course_id): Path<i64>,
    Query(_params): Query<Vec<(String, String)>>,
    _auth: AuthUser,
) -> StatusCode {
    StatusCode::NO_CONTENT
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/search/recipients", get(recipients))
        .route("/conversations/find_recipients", get(recipients))
        .route("/search/all_courses", get(all_courses))
        .route("/search/rubrics", get(rubrics))
        .route("/courses/{course_id}/smartsearch", get(smart_search))
        .route(
            "/courses/{course_id}/smartsearch/index_status",
            get(smart_search_index_status),
        )
        .route(
            "/courses/{course_id}/smartsearch/log",
            get(smart_search_log),
        )
}
