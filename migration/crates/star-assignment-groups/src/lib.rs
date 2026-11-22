//! star-assignment-groups: assignment group management for star-lms.
//!
//! Implements Canvas API v1 Assignment Groups endpoint. Grading rules are not a
//! sub-resource: Canvas permits `rules` on the create/update payload
//! (`API_ALLOWED_ASSIGNMENT_GROUP_INPUT_FIELDS`) and serialises the parsed
//! `assignment_groups.rules` string on every group JSON
//! (`Api::V1::AssignmentGroup::assignment_group_json`).

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
#[derive(Debug, Clone, Serialize)]
pub struct AssignmentGroup {
    pub id: i64,
    pub course_id: i64,
    pub name: String,
    pub position: i32,
    pub group_weight: f64,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub rules: RulesHash,
}

/// Grading rules as Canvas' `AssignmentGroup#rules_hash` serialises them: keys
/// only appear when the stored rule string carries them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RulesHash {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_lowest: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_highest: Option<i32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub never_drop: Vec<i64>,
}

impl RulesHash {
    fn is_empty(&self) -> bool {
        self.drop_lowest.is_none() && self.drop_highest.is_none() && self.never_drop.is_empty()
    }
}

impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for AssignmentGroup {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            id: row.try_get("id")?,
            course_id: row.try_get("course_id")?,
            name: row.try_get("name")?,
            position: row.try_get("position")?,
            group_weight: row.try_get("group_weight")?,
            workflow_state: row.try_get("workflow_state")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            rules: parse_rules(row.try_get("rules")?),
        })
    }
}

/// Request to create an assignment group.
#[derive(Debug, Deserialize)]
pub struct CreateAssignmentGroup {
    pub name: String,
    #[serde(default)]
    pub group_weight: Option<f64>,
    #[serde(default = "default_position")]
    pub position: Option<i32>,
    #[serde(default)]
    pub rules: Option<GradingRulesPayload>,
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
    pub rules: Option<GradingRulesPayload>,
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

/// Canvas `AssignmentGroup#rules_hash=` — the incoming rules hash replaces the
/// whole string, and keys the payload omits are not written at all.
fn format_rules(rules: &GradingRulesPayload) -> String {
    let mut out = String::new();
    if let Some(drop_lowest) = rules.drop_lowest {
        out.push_str(&format!("drop_lowest:{drop_lowest}\n"));
    }
    if let Some(drop_highest) = rules.drop_highest {
        out.push_str(&format!("drop_highest:{drop_highest}\n"));
    }
    if let Some(never_drop) = &rules.never_drop {
        for id in never_drop {
            out.push_str(&format!("never_drop:{id}\n"));
        }
    }
    out
}

/// Canvas `AssignmentGroup#rules_hash` — parses `key:value` lines; repeated
/// `never_drop` lines accumulate into the id list.
fn parse_rules(raw: Option<&str>) -> RulesHash {
    let mut rules = RulesHash::default();
    for line in raw.unwrap_or_default().lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().parse::<i64>().unwrap_or_default();
        match key.trim() {
            "drop_lowest" => rules.drop_lowest = Some(value as i32),
            "drop_highest" => rules.drop_highest = Some(value as i32),
            "never_drop" => rules.never_drop.push(value),
            _ => {}
        }
    }
    rules
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

/// Mirrors the parsed rules into `assignment_group_rules`, the projection
/// `star-graphql`'s `assignmentGroupsConnection` reads. An empty hash removes
/// the row so GraphQL resolves `rules` to null, like a nil Canvas `rules`.
async fn project_rules(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    group_id: i64,
    rules: &RulesHash,
) -> Result<(), ApiError> {
    if rules.is_empty() {
        sqlx::query("DELETE FROM assignment_group_rules WHERE assignment_group_id = $1")
            .bind(group_id)
            .execute(&mut *tx)
            .await?;
        return Ok(());
    }

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
    .bind(rules.drop_lowest.unwrap_or_default())
    .bind(rules.drop_highest.unwrap_or_default())
    .bind(&rules.never_drop)
    .execute(&mut *tx)
    .await?;

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
        INSERT INTO assignment_groups (course_id, name, group_weight, position, rules)
        VALUES ($1, $2, COALESCE($3, 0), COALESCE($4, 1), $5)
        RETURNING id, course_id, name, position, group_weight, workflow_state, created_at, updated_at, rules
        "#,
    )
    .bind(course_id)
    .bind(&payload.name)
    .bind(payload.group_weight)
    .bind(payload.position)
    .bind(payload.rules.as_ref().map(format_rules).unwrap_or_default())
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
        SELECT id, course_id, name, position, group_weight, workflow_state, created_at, updated_at, rules
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
        SELECT id, course_id, name, position, group_weight, workflow_state, created_at, updated_at, rules
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

/// PUT/PATCH /courses/{courseId}/assignment_groups/{id} — Canvas
/// assignment_groups_controller#update —
/// PUT|PATCH /api/v1/courses/:course_id/assignment_groups/:id (both verbs
/// route to `update` in Canvas' routes.rb).
async fn update_assignment_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateAssignmentGroup>,
) -> Result<Json<AssignmentGroup>, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas replaces the whole rules string when the payload carries `rules`
    // and leaves it untouched otherwise (`Api::V1::AssignmentGroup#update_assignment_group`).
    let mut tx = state.db.begin().await?;

    let existing: Option<(String, i32, f64, Option<String>)> = sqlx::query_as(
        r#"
        SELECT name, position, group_weight, rules
        FROM assignment_groups
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (current_name, current_position, current_weight, current_rules) =
        existing.ok_or(ApiError::NotFound)?;

    let name = payload.name.unwrap_or(current_name);
    let position = payload.position.unwrap_or(current_position);
    let group_weight = payload.group_weight.unwrap_or(current_weight);

    let rules = match &payload.rules {
        Some(incoming) => {
            let rendered = format_rules(incoming);
            let parsed = parse_rules(Some(rendered.as_str()));

            let (assignment_count,): (i64,) = sqlx::query_as(
                r#"
                SELECT COUNT(*) FROM assignments
                WHERE assignment_group_id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
                "#,
            )
            .bind(group_id)
            .bind(course_id)
            .fetch_one(&mut *tx)
            .await?;

            validate_drop_rules(
                parsed.drop_lowest.unwrap_or_default(),
                parsed.drop_highest.unwrap_or_default(),
                assignment_count,
            )?;

            project_rules(&mut tx, group_id, &parsed).await?;

            rendered
        }
        None => current_rules.unwrap_or_default(),
    };

    let updated: AssignmentGroup = sqlx::query_as(
        r#"
        UPDATE assignment_groups
        SET name = $3, position = $4, group_weight = $5, rules = $6, updated_at = now()
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        RETURNING id, course_id, name, position, group_weight, workflow_state, created_at, updated_at, rules
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .bind(&name)
    .bind(position)
    .bind(group_weight)
    .bind(&rules)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

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
            put(update_assignment_group).patch(update_assignment_group),
        )
        .route(
            "/courses/{courseId}/assignment_groups/{id}",
            delete(delete_assignment_group),
        )
        .route(
            "/courses/{courseId}/assignment_groups/{groupId}/assignments",
            get(list_group_assignments),
        )
}

#[cfg(test)]
mod tests {
    use super::{format_rules, parse_rules, validate_drop_rules, GradingRulesPayload};

    fn payload(
        drop_lowest: Option<i32>,
        drop_highest: Option<i32>,
        never_drop: Option<Vec<i64>>,
    ) -> GradingRulesPayload {
        GradingRulesPayload {
            drop_lowest,
            drop_highest,
            never_drop,
        }
    }

    #[test]
    fn format_writes_the_canvas_rule_string() {
        assert_eq!(
            format_rules(&payload(Some(1), None, Some(vec![33, 17]))),
            "drop_lowest:1\nnever_drop:33\nnever_drop:17\n"
        );
    }

    #[test]
    fn format_writes_zero_and_omits_absent_keys() {
        assert_eq!(
            format_rules(&payload(Some(0), None, None)),
            "drop_lowest:0\n"
        );
        assert_eq!(format_rules(&payload(None, None, None)), "");
    }

    #[test]
    fn parse_round_trips_the_canvas_rule_string() {
        let rules = parse_rules(Some(
            "drop_lowest:2\ndrop_highest:1\nnever_drop:12\nnever_drop:14\n",
        ));
        assert_eq!(rules.drop_lowest, Some(2));
        assert_eq!(rules.drop_highest, Some(1));
        assert_eq!(rules.never_drop, vec![12, 14]);

        assert!(parse_rules(None).is_empty());
        assert!(parse_rules(Some("")).is_empty());
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
