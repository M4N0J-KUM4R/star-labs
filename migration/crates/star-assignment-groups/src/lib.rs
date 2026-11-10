//! star-assignment-groups: assignment group management for star-lms.
//!
//! Implements Canvas API v1 Assignment Groups endpoint.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// AssignmentGroup record matching Canvas API v1 shape.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AssignmentGroup {
    pub id: i64,
    pub course_id: i64,
    pub name: String,
    pub position: i32,
    pub group_weight: f64,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request to create an assignment group.
#[derive(Debug, Deserialize)]
pub struct CreateAssignmentGroup {
    pub name: String,
    #[serde(default)]
    pub group_weight: Option<f64>,
    #[serde(default = "default_position")]
    pub position: Option<i32>,
}

fn default_position() -> Option<i32> {
    Some(1)
}

/// Request to update an assignment group.
#[derive(Debug, Deserialize)]
pub struct UpdateAssignmentGroup {
    pub name: Option<String>,
    pub position: Option<i32>,
    pub group_weight: Option<f64>,
}

/// Assignment record returned when listing assignments in a group.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GroupAssignment {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub points_possible: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Grading rules for an assignment group, matching Canvas `GradingRules`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AssignmentGroupRules {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    pub assignment_group_id: i64,
    pub group_weight: f64,
    pub drop_lowest: i32,
    pub drop_highest: i32,
    pub never_drop: Vec<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Nested Canvas-style `rules` object
/// (`{"rules": {"drop_lowest": 1, "never_drop": [33]}}`).
#[derive(Debug, Deserialize)]
pub struct GradingRulesPayload {
    #[serde(default)]
    pub drop_lowest: Option<i32>,
    #[serde(default)]
    pub drop_highest: Option<i32>,
    #[serde(default)]
    pub never_drop: Option<Vec<i64>>,
}

/// Request to update the rules of an assignment group.
///
/// Accepts flat fields as well as the nested `rules` object used by Canvas.
#[derive(Debug, Deserialize)]
pub struct UpdateAssignmentGroupRules {
    #[serde(default)]
    pub group_weight: Option<f64>,
    #[serde(default)]
    pub drop_lowest: Option<i32>,
    #[serde(default)]
    pub drop_highest: Option<i32>,
    #[serde(default)]
    pub never_drop: Option<Vec<i64>>,
    #[serde(default)]
    pub rules: Option<GradingRulesPayload>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

async fn check_enrolled_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

async fn check_teacher_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Rule helpers
// ---------------------------------------------------------------------------

/// Selects the rules row joined with the group so `group_weight` always comes
/// from `assignment_groups` (Canvas keeps it on the group). A missing rules
/// row yields Canvas defaults.
const SELECT_RULES_SQL: &str = r#"
    SELECT r.id,
           g.id AS assignment_group_id,
           g.group_weight,
           COALESCE(r.drop_lowest, 0) AS drop_lowest,
           COALESCE(r.drop_highest, 0) AS drop_highest,
           COALESCE(r.never_drop, ARRAY[]::BIGINT[]) AS never_drop,
           COALESCE(r.created_at, g.created_at) AS created_at,
           COALESCE(r.updated_at, g.updated_at) AS updated_at
    FROM assignment_groups g
    LEFT JOIN assignment_group_rules r ON r.assignment_group_id = g.id
    WHERE g.id = $1 AND g.course_id = $2 AND g.workflow_state <> 'deleted'
"#;

/// Merge a rules update into the existing values. Flat fields win over the
/// nested `rules` object; omitted fields keep the current value.
fn merge_rule_values(
    existing: (i32, i32, Vec<i64>),
    payload: &UpdateAssignmentGroupRules,
) -> (i32, i32, Vec<i64>) {
    let nested = payload.rules.as_ref();
    let drop_lowest = payload
        .drop_lowest
        .or_else(|| nested.and_then(|r| r.drop_lowest))
        .unwrap_or(existing.0);
    let drop_highest = payload
        .drop_highest
        .or_else(|| nested.and_then(|r| r.drop_highest))
        .unwrap_or(existing.1);
    let never_drop = payload
        .never_drop
        .clone()
        .or_else(|| nested.and_then(|r| r.never_drop.clone()))
        .unwrap_or(existing.2);
    (drop_lowest, drop_highest, never_drop)
}

/// Canvas `AssignmentGroup#validate_assignment_group_rules`: drop rules must
/// be non-negative and cannot exceed the number of assignments in the group.
fn validate_drop_rules(
    drop_lowest: i32,
    drop_highest: i32,
    assignment_count: i64,
) -> Result<(), ApiError> {
    if drop_lowest < 0 || drop_highest < 0 {
        return Err(ApiError::Validation(
            "Drop rules must be a positive number".into(),
        ));
    }

    if i64::from(drop_lowest) > assignment_count || i64::from(drop_highest) > assignment_count {
        return Err(ApiError::Validation(
            "Drop rules cannot be higher than the number of assignments".into(),
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/assignment_groups — create (teacher)
async fn create_assignment_group(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateAssignmentGroup>,
) -> Result<(StatusCode, Json<AssignmentGroup>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.name.trim().is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }

    let row: AssignmentGroup = sqlx::query_as(
        r#"
        INSERT INTO assignment_groups (course_id, name, group_weight, position)
        VALUES ($1, $2, COALESCE($3, 0), COALESCE($4, 1))
        RETURNING id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.name)
    .bind(payload.group_weight)
    .bind(payload.position)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/assignment_groups — list (enrolled)
async fn list_assignment_groups(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<AssignmentGroup>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<AssignmentGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        FROM assignment_groups
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        ORDER BY position ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /courses/{courseId}/assignment_groups/{id} — get one (enrolled)
async fn get_assignment_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<AssignmentGroup>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<AssignmentGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        FROM assignment_groups
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    row.map(Json).ok_or(ApiError::NotFound)
}

/// PUT /courses/{courseId}/assignment_groups/{id} — update (teacher)
async fn update_assignment_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateAssignmentGroup>,
) -> Result<Json<AssignmentGroup>, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Fetch existing to merge fields
    let existing: Option<AssignmentGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        FROM assignment_groups
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    let existing = existing.ok_or(ApiError::NotFound)?;

    let name = payload.name.unwrap_or(existing.name);
    let position = payload.position.unwrap_or(existing.position);
    let group_weight = payload.group_weight.unwrap_or(existing.group_weight);

    let updated: AssignmentGroup = sqlx::query_as(
        r#"
        UPDATE assignment_groups
        SET name = $3, position = $4, group_weight = $5, updated_at = now()
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        RETURNING id, course_id, name, position, group_weight, workflow_state, created_at, updated_at
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .bind(&name)
    .bind(position)
    .bind(group_weight)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /courses/{courseId}/assignment_groups/{id} — soft-delete (teacher)
async fn delete_assignment_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        r#"
        UPDATE assignment_groups
        SET workflow_state = 'deleted', updated_at = now()
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// GET /courses/{courseId}/assignment_groups/{groupId}/assignments — assignments in group (enrolled)
async fn list_group_assignments(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<GroupAssignment>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify group exists and is not deleted
    let group: Option<(i64,)> = sqlx::query_as(
        r#"SELECT id FROM assignment_groups WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'"#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    if group.is_none() {
        return Err(ApiError::NotFound);
    }

    let rows: Vec<GroupAssignment> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        FROM assignments
        WHERE assignment_group_id = $1 AND course_id = $2
        ORDER BY created_at ASC
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /courses/{courseId}/assignment_groups/{id}/rules — grading rules (enrolled)
async fn get_assignment_group_rules(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<AssignmentGroupRules>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<AssignmentGroupRules> = sqlx::query_as(SELECT_RULES_SQL)
        .bind(group_id)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;

    row.map(Json).ok_or(ApiError::NotFound)
}

/// PUT /courses/{courseId}/assignment_groups/{id}/rules — update rules (teacher)
async fn update_assignment_group_rules(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateAssignmentGroupRules>,
) -> Result<Json<AssignmentGroupRules>, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let mut tx = state.db.begin().await?;

    let group_weight: Option<(f64,)> = sqlx::query_as(
        r#"
        SELECT group_weight
        FROM assignment_groups
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&mut *tx)
    .await?;

    let group_weight = group_weight.ok_or(ApiError::NotFound)?.0;

    let (assignment_count,): (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM assignments
        WHERE assignment_group_id = $1 AND course_id = $2
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_one(&mut *tx)
    .await?;

    let existing: Option<(i32, i32, Vec<i64>)> = sqlx::query_as(
        r#"
        SELECT drop_lowest, drop_highest, never_drop
        FROM assignment_group_rules
        WHERE assignment_group_id = $1
        "#,
    )
    .bind(group_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (drop_lowest, drop_highest, never_drop) =
        merge_rule_values(existing.unwrap_or((0, 0, Vec::new())), &payload);

    validate_drop_rules(drop_lowest, drop_highest, assignment_count)?;

    let group_weight = payload.group_weight.unwrap_or(group_weight);

    sqlx::query(
        r#"
        UPDATE assignment_groups
        SET group_weight = $3, updated_at = now()
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .bind(group_weight)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO assignment_group_rules
            (assignment_group_id, drop_lowest, drop_highest, never_drop)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (assignment_group_id) DO UPDATE
        SET drop_lowest = EXCLUDED.drop_lowest,
            drop_highest = EXCLUDED.drop_highest,
            never_drop = EXCLUDED.never_drop,
            updated_at = now()
        "#,
    )
    .bind(group_id)
    .bind(drop_lowest)
    .bind(drop_highest)
    .bind(&never_drop)
    .execute(&mut *tx)
    .await?;

    let row: AssignmentGroupRules = sqlx::query_as(SELECT_RULES_SQL)
        .bind(group_id)
        .bind(course_id)
        .fetch_one(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(Json(row))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/assignment_groups",
            post(create_assignment_group),
        )
        .route(
            "/courses/{courseId}/assignment_groups",
            get(list_assignment_groups),
        )
        .route(
            "/courses/{courseId}/assignment_groups/{id}",
            get(get_assignment_group),
        )
        .route(
            "/courses/{courseId}/assignment_groups/{id}",
            put(update_assignment_group),
        )
        .route(
            "/courses/{courseId}/assignment_groups/{id}",
            delete(delete_assignment_group),
        )
        .route(
            "/courses/{courseId}/assignment_groups/{groupId}/assignments",
            get(list_group_assignments),
        )
        .route(
            "/courses/{courseId}/assignment_groups/{id}/rules",
            get(get_assignment_group_rules).put(update_assignment_group_rules),
        )
}

#[cfg(test)]
mod tests {
    use super::{
        merge_rule_values, validate_drop_rules, GradingRulesPayload, UpdateAssignmentGroupRules,
    };

    fn empty_payload() -> UpdateAssignmentGroupRules {
        UpdateAssignmentGroupRules {
            group_weight: None,
            drop_lowest: None,
            drop_highest: None,
            never_drop: None,
            rules: None,
        }
    }

    #[test]
    fn merge_keeps_existing_values_when_fields_are_omitted() {
        let merged = merge_rule_values((2, 1, vec![33]), &empty_payload());
        assert_eq!(merged, (2, 1, vec![33]));
    }

    #[test]
    fn merge_reads_nested_canvas_rules_payload() {
        let payload = UpdateAssignmentGroupRules {
            group_weight: Some(40.0),
            drop_lowest: None,
            drop_highest: Some(0),
            never_drop: None,
            rules: Some(GradingRulesPayload {
                drop_lowest: Some(1),
                drop_highest: None,
                never_drop: Some(vec![17, 24]),
            }),
        };

        assert_eq!(
            merge_rule_values((0, 0, Vec::new()), &payload),
            (1, 0, vec![17, 24])
        );
    }

    #[test]
    fn negative_drop_rules_are_rejected() {
        assert!(validate_drop_rules(-1, 0, 5).is_err());
        assert!(validate_drop_rules(0, -1, 5).is_err());
    }

    #[test]
    fn drop_rules_cannot_exceed_assignment_count() {
        assert!(validate_drop_rules(2, 0, 2).is_ok());
        assert!(validate_drop_rules(3, 0, 2).is_err());
    }
}
