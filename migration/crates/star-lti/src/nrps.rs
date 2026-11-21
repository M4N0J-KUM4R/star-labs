//! LTI NRPS (Names and Roles Provisioning Service) v2 —
//! `GET /api/lti/courses/{course_id}/names_and_roles` and
//! `GET /api/lti/groups/{group_id}/names_and_roles` (issue #94).
//!
//! Canvas source: `app/controllers/lti/ims/names_and_roles_controller.rb`,
//! `app/serializers/lti/ims/names_and_roles_serializer.rb`,
//! `app/controllers/lti/ims/providers/{course,group}_memberships_provider.rb`,
//! `lib/lti/substitutions_helper.rb` (role map), route
//! `config/routes.rb:3282-3285`.
//!
//! Auth chain (AdvantageServices, no GradebookServices): token → developer
//! key → `contextmembership.readonly` scope (401s) → context exists (404)
//! → tool bound to the key (401).
//!
//! Serializer (names_and_roles_serializer.rb:23-125):
//! `{ id: <page url>, context: { id, label, title }, members: [...] }` where
//! each member is `{ status: "Active", name, given_name, family_name,
//! email, lis_person_sourcedid, user_id, lti11_legacy_user_id, roles: [...] }`
//! with nulls compacted. `user_id` is Canvas's `lti_id` — star-lms has no
//! `users.lti_id` column, so the user id string is used (documented
//! deviation, consistent with `ags_scores.rs`); `lti11_legacy_user_id` is
//! the same opaque handle (no shard-prefixed legacy ids here).
//!
//! Roles map (`LIS_V2_LTI_ADVANTAGE_ROLE_MAP`, substitutions_helper.rb:69-95):
//! - student  → `http://purl.imsglobal.org/vocab/lis/v2/membership#Learner`
//! - teacher  → `http://purl.imsglobal.org/vocab/lis/v2/membership#Instructor`
//! - ta       → `membership/Instructor#TeachingAssistant` + `membership#Instructor`
//! - observer → `http://purl.imsglobal.org/vocab/lis/v2/membership#Mentor`
//! - designer → `http://purl.imsglobal.org/vocab/lis/v2/membership#ContentDeveloper`
//!
//! Filtering (`course_memberships_provider.rb`): `role` keeps only members
//! whose enrollment type maps to that URN (`queryable_roles`); `limit` caps
//! the page (Canvas default 50 via Api.paginate). Group memberships return
//! the `#Member` role (`:group_member` in the map).

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use star_common::AppState;

use crate::ags_auth::{
    advantage_error, verify_advantage_access, verify_advantage_course, verify_advantage_tool,
    SCOPE_NRPS_MEMBERSHIP_READ_ONLY,
};

/// Canvas MIME type (names_and_roles_controller.rb:265).
const NRPS_MIME: &str = "application/vnd.ims.lti-nrps.v2.membershipcontainer+json";

/// Canvas NRPS default page size (Api.paginate default; documented default
/// of 50 in the API docs).
const DEFAULT_PAGE_SIZE: i64 = 50;

#[derive(Debug, Deserialize)]
pub struct MembershipsQuery {
    /// Fully-qualified LTI/LIS role URN — keep only matching members.
    role: Option<String>,
    /// Page size cap.
    limit: Option<i64>,
}

/// star-lms enrollment role → LIS membership URNs
/// (`LIS_V2_LTI_ADVANTAGE_ROLE_MAP` enrollment entries).
fn enrollment_role_urns(role: &str) -> Vec<&'static str> {
    match role {
        "student" => vec!["http://purl.imsglobal.org/vocab/lis/v2/membership#Learner"],
        "teacher" => vec!["http://purl.imsglobal.org/vocab/lis/v2/membership#Instructor"],
        "ta" => vec![
            "http://purl.imsglobal.org/vocab/lis/v2/membership/Instructor#TeachingAssistant",
            "http://purl.imsglobal.org/vocab/lis/v2/membership#Instructor",
        ],
        "observer" => vec!["http://purl.imsglobal.org/vocab/lis/v2/membership#Mentor"],
        "designer" => vec!["http://purl.imsglobal.org/vocab/lis/v2/membership#ContentDeveloper"],
        _ => vec![],
    }
}

/// Group membership role URN (`:group_member`).
const GROUP_MEMBER_URN: &str = "http://purl.imsglobal.org/vocab/lis/v2/membership#Member";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/courses/{course_id}/names_and_roles",
            get(course_index),
        )
        .route(
            "/api/lti/groups/{group_id}/names_and_roles",
            get(group_index),
        )
}

/// The AdvantageServices chain (token → key → scope → context → tool) for a
/// course context. NRPS has no line-item gate.
#[allow(clippy::result_large_err)]
async fn verify_nrps_course(
    state: &AppState,
    course_id: i64,
    authorization: Option<&str>,
) -> Result<(), Response> {
    let auth =
        verify_advantage_access(state, authorization, SCOPE_NRPS_MEMBERSHIP_READ_ONLY).await?;
    verify_advantage_course(&state.db, course_id).await?;
    verify_advantage_tool(&state.db, auth.developer_key_id).await?;
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct CourseMemberRow {
    user_id: i64,
    name: String,
    email: String,
    role: String,
}

fn member_json(user_id: i64, name: &str, email: &str, roles: Vec<&str>) -> Value {
    // `.compact` — null members (never here) drop out; every field below is
    // present in star-lms, matching Canvas with name/email always included.
    let mut map = Map::new();
    map.insert("status".into(), json!("Active"));
    map.insert("name".into(), json!(name));
    map.insert("email".into(), json!(email));
    map.insert("user_id".into(), json!(user_id.to_string()));
    map.insert("lti11_legacy_user_id".into(), json!(user_id.to_string()));
    map.insert("roles".into(), json!(roles));
    Value::Object(map)
}

fn membership_json(
    page_id: &str,
    context_id: i64,
    context_label: Option<&str>,
    context_title: Option<&str>,
    members: Vec<Value>,
) -> Value {
    let mut ctx = Map::new();
    ctx.insert("id".into(), json!(context_id.to_string()));
    if let Some(label) = context_label {
        ctx.insert("label".into(), json!(label));
    }
    if let Some(title) = context_title {
        ctx.insert("title".into(), json!(title));
    }
    json!({
        "id": page_id,
        "context": Value::Object(ctx),
        "members": members,
    })
}

fn nrps_response(payload: Value) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, NRPS_MIME)],
        Json(payload),
    )
        .into_response()
}

async fn course_index(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<MembershipsQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if let Err(resp) = verify_nrps_course(&state, course_id, authorization).await {
        return resp;
    }

    let limit = params.limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, 100);

    // Course context label/title for the context claim.
    let course: Option<(String, String)> =
        sqlx::query_as("SELECT name, COALESCE(course_code, name) FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let Some((course_name, course_label)) = course else {
        return advantage_error(StatusCode::NOT_FOUND, "Context is deleted or not found");
    };

    // Active enrollments; the role filter maps the URN back to enrollment
    // roles (`queryable_roles` via the inverted map).
    let rows: Vec<CourseMemberRow> = match sqlx::query_as(
        "SELECT DISTINCT e.user_id, u.name, u.email, e.role \
         FROM enrollments e JOIN users u ON u.id = e.user_id \
         WHERE e.course_id = $1 AND e.workflow_state = 'active' \
         ORDER BY e.user_id \
         LIMIT $2",
    )
    .bind(course_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    };

    let members: Vec<Value> = rows
        .iter()
        .filter(|row| {
            // role? → keep only members whose URNs include it; a nonsense
            // URN filters out everybody (course_memberships_provider.rb:78).
            params
                .role
                .as_deref()
                .map(|wanted| {
                    let urns = enrollment_role_urns(&row.role);
                    !urns.is_empty() && urns.contains(&wanted)
                })
                .unwrap_or(true)
        })
        .map(|row| {
            member_json(
                row.user_id,
                &row.name,
                &row.email,
                enrollment_role_urns(&row.role),
            )
        })
        .collect();

    let payload = membership_json(
        &format!("/api/lti/courses/{course_id}/names_and_roles"),
        course_id,
        Some(&course_label),
        Some(&course_name),
        members,
    );
    nrps_response(payload)
}

#[derive(Debug, sqlx::FromRow)]
struct GroupMemberRow {
    user_id: i64,
    name: String,
    email: String,
}

async fn group_index(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(params): Query<MembershipsQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    // The AdvantageServices chain against the group's context: token → key
    // → scope, then the group must exist (404), then the tool gate.
    let auth = match verify_advantage_access(&state, authorization, SCOPE_NRPS_MEMBERSHIP_READ_ONLY)
        .await
    {
        Ok(a) => a,
        Err(resp) => return resp,
    };

    // Groups context (group_memberships_provider): 404 when deleted/missing.
    let group: Option<(String,)> =
        sqlx::query_as("SELECT name FROM groups WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(group_id)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
    let Some((group_name,)) = group else {
        return advantage_error(StatusCode::NOT_FOUND, "Context is deleted or not found");
    };

    if let Err(resp) = verify_advantage_tool(&state.db, auth.developer_key_id).await {
        return resp;
    }

    let limit = params.limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, 100);

    let rows: Vec<GroupMemberRow> = match sqlx::query_as(
        "SELECT gm.user_id, u.name, u.email \
         FROM group_memberships gm JOIN users u ON u.id = gm.user_id \
         WHERE gm.group_id = $1 AND gm.workflow_state <> 'deleted' \
         ORDER BY gm.user_id LIMIT $2",
    )
    .bind(group_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(_) => return advantage_error(StatusCode::INTERNAL_SERVER_ERROR, "database error"),
    };

    // Group roles: only #Member (and #Manager for leaders, which star-lms
    // does not model) are supported per the API docs.
    let members: Vec<Value> = rows
        .iter()
        .filter(|_| {
            params
                .role
                .as_deref()
                .map(|wanted| wanted == GROUP_MEMBER_URN)
                .unwrap_or(true)
        })
        .map(|row| member_json(row.user_id, &row.name, &row.email, vec![GROUP_MEMBER_URN]))
        .collect();

    let payload = membership_json(
        &format!("/api/lti/groups/{group_id}/names_and_roles"),
        group_id,
        None,
        Some(&group_name),
        members,
    );
    nrps_response(payload)
}
