//! GraphQL mutations — Batch A: assignment create/update/delete (issue #29).
//!
//! Mirrors `app/graphql/mutations/create_assignment.rb`,
//! `update_assignment.rb` and `assignment_base.rb` in canvas-lms, per
//! `specs/p5-gql-mutations.md` §2.1. Canvas has **no DeleteAssignment
//! mutation** — deletion is `UpdateAssignment` with `state: deleted` (soft
//! delete via the `assignments.workflow_state` column, migration 0110);
//! this slice mirrors that exactly and does not invent a `deleteAssignment`.
//!
//! Error channels (spec §3):
//! - unauthenticated callers never reach this code (HTTP 401 from the shared
//!   `AuthUser` extractor in the router);
//! - authorization failures / missing records are top-level execution errors
//!   with Canvas's exact strings (`"invalid course: {id}"`,
//!   `"assignment not found: {id}"`, `"insufficient permission"`), which
//!   null out `data.createAssignment`/`data.updateAssignment`;
//! - validation failures are payload `errors: [{ attribute, message }]`
//!   inside HTTP 200. Divergence documented in spec §7 Q7: Canvas defaults an
//!   omitted attribute to the literal `"message"`; star-lms emits `null`.
//!
//! Authorization parity: Canvas derives the abilities from course rights
//! (`manage_assignments_add/edit/delete`, abstract_assignment.rb `set_policy`).
//! The star-lms role model collapses all three onto the REST gate
//! `check_teacher_or_admin` (course teacher enrollment or global admin), the
//! same gate the REST create/update handlers use. Create conflates "missing
//! course" and "unauthorized caller" into one message — no existence leak.

use async_graphql::{Context, InputObject, Object, SimpleObject};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::assignment::Assignment;
use crate::db_err;

// ---------------------------------------------------------------------------
// Mutation root object (merged into lib::Mutation)
// ---------------------------------------------------------------------------

/// Batch-A mutation container. Each mutation takes exactly one `input`
/// argument (Canvas `HasSingleInputArgument`) and delegates to the free
/// functions below, which receive the pool + caller straight from the
/// request's `GraphQLContext`.
#[derive(Default)]
pub struct AssignmentMutations;

#[Object]
impl AssignmentMutations {
    /// Canvas `createAssignment` (`app/graphql/mutations/create_assignment.rb`).
    async fn create_assignment(
        &self,
        ctx: &Context<'_>,
        input: CreateAssignmentInput,
    ) -> Result<CreateAssignmentPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        create_assignment(&gctx.pool, gctx.auth_user.id, gctx.is_admin(), input).await
    }

    /// Canvas `updateAssignment` (`app/graphql/mutations/update_assignment.rb`).
    /// Also the delete path: `state: deleted` soft-deletes (Canvas has no
    /// `deleteAssignment` mutation).
    async fn update_assignment(
        &self,
        ctx: &Context<'_>,
        input: UpdateAssignmentInput,
    ) -> Result<UpdateAssignmentPayload, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        update_assignment(&gctx.pool, gctx.auth_user.id, gctx.is_admin(), input).await
    }
}

// ---------------------------------------------------------------------------
// Input types
// ---------------------------------------------------------------------------

/// Canvas `AssignmentState` enum (`app/graphql/types/assignment_state_type.rb`).
/// The full Canvas enum also carries `duplicating`/`importing_verification`
/// etc.; those states never appear in mutation inputs, so this slice accepts
/// only the three the mutations can actually write. Values outside the enum
/// fail GraphQL input coercion before any handler runs (Canvas behaves the
/// same way for unknown enum values).
#[derive(Debug, Clone, Copy, PartialEq, Eq, async_graphql::Enum)]
pub enum AssignmentState {
    Published,
    Unpublished,
    Deleted,
}

impl AssignmentState {
    /// The `assignments.workflow_state` value this state maps onto.
    fn workflow_state(self) -> &'static str {
        match self {
            AssignmentState::Published => "published",
            AssignmentState::Unpublished => "unpublished",
            AssignmentState::Deleted => "deleted",
        }
    }
}

/// Canvas `CreateAssignmentInput` (`create_assignment.rb`).
///
/// Field deviations (documented in the PR and below): `unlockAt`, `lockAt`,
/// `allowedExtensions` and `submissionTypes` are part of the Canvas input
/// shape but have **no backing columns** in star-db, so they are accepted
/// for client compatibility and ignored on write — identical to the REST
/// `POST /courses/:id/assignments` payload, which ignores them too. Spec §4
/// lists no migration for them in Batch A beyond `workflow_state`; they land
/// with their own column migrations in a later batch.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "CreateAssignmentInput")]
pub struct CreateAssignmentInput {
    pub course_id: async_graphql::ID,
    pub name: String,
    pub description: Option<String>,
    pub points_possible: Option<f64>,
    pub due_at: Option<DateTime<Utc>>,
    /// Accepted-for-shape only: no `availability`/`unlock_at` column exists
    /// in star-db yet (see module note above). Ignored on write.
    #[allow(dead_code)]
    pub unlock_at: Option<DateTime<Utc>>,
    /// Accepted-for-shape only: no `lock_at` column exists in star-db yet.
    /// Ignored on write.
    #[allow(dead_code)]
    pub lock_at: Option<DateTime<Utc>>,
    pub state: Option<AssignmentState>,
    /// Accepted-for-shape only: no `allowed_extensions` column exists in
    /// star-db yet. Ignored on write.
    #[allow(dead_code)]
    pub allowed_extensions: Option<Vec<String>>,
    /// Accepted-for-shape only: no `submission_types` column exists in
    /// star-db yet. Ignored on write.
    #[allow(dead_code)]
    pub submission_types: Option<Vec<String>>,
}

/// Canvas `UpdateAssignmentInput` (`update_assignment.rb`). Partial update:
/// omitted fields keep their current values (Canvas assigns every provided
/// key; nil keys are skipped by `active_record_update!`). The same four
/// columns without star-db backing (`unlockAt`, `lockAt`,
/// `allowedExtensions`, `submissionTypes`) are accepted and ignored.
#[derive(Debug, Clone, InputObject)]
#[graphql(input_name = "UpdateAssignmentInput")]
pub struct UpdateAssignmentInput {
    pub id: async_graphql::ID,
    pub name: Option<String>,
    pub description: Option<String>,
    pub points_possible: Option<f64>,
    pub due_at: Option<DateTime<Utc>>,
    /// Accepted-for-shape only; no backing column (see create input).
    #[allow(dead_code)]
    pub unlock_at: Option<DateTime<Utc>>,
    /// Accepted-for-shape only; no backing column (see create input).
    #[allow(dead_code)]
    pub lock_at: Option<DateTime<Utc>>,
    pub state: Option<AssignmentState>,
    /// Accepted-for-shape only; no backing column (see create input).
    #[allow(dead_code)]
    pub allowed_extensions: Option<Vec<String>>,
    /// Accepted-for-shape only; no backing column (see create input).
    #[allow(dead_code)]
    pub submission_types: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Payload types
// ---------------------------------------------------------------------------

/// Canvas `ValidationErrorType` (`app/graphql/types/validation_error_type.rb`):
/// `{ attribute: String (nullable), message: String! }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct ValidationError {
    pub attribute: Option<String>,
    pub message: String,
}

/// Canvas `CreateAssignmentPayload` — `{ assignment, errors }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct CreateAssignmentPayload {
    pub assignment: Option<Assignment>,
    pub errors: Option<Vec<ValidationError>>,
}

/// Canvas `UpdateAssignmentPayload` — `{ assignment, errors }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct UpdateAssignmentPayload {
    pub assignment: Option<Assignment>,
    pub errors: Option<Vec<ValidationError>>,
}

// ---------------------------------------------------------------------------
// Row fetch + mapping
// ---------------------------------------------------------------------------

/// One assignment row, including the Batch-A `workflow_state` column.
#[derive(Debug, Clone, sqlx::FromRow)]
struct AssignRow {
    id: i64,
    course_id: i64,
    name: String,
    description: Option<String>,
    points_possible: f64,
    due_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    workflow_state: String,
}

impl AssignRow {
    /// Map onto the shared GraphQL `Assignment` object. Canvas exposes
    /// `state` on the type; the merged #26/#28 schema does not carry it yet,
    /// so the field set stays byte-identical to `assignmentsConnection`
    /// nodes (adding fields here would fork the type — deferred to the
    /// Assignment-type parity issue).
    fn into_assignment(self) -> Assignment {
        Assignment {
            id: async_graphql::ID(self.id.to_string()),
            course_id: async_graphql::ID(self.course_id.to_string()),
            name: self.name,
            description: self.description,
            points_possible: self.points_possible,
            due_at: self.due_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

const ASSIGN_COLUMNS: &str = r#"
        SELECT id, course_id, title AS name, description,
               points_possible::float8 AS points_possible,
               due_at, created_at, updated_at, workflow_state
        FROM assignments WHERE id = $1"#;

async fn fetch_assignment(pool: &PgPool, id: i64) -> Result<Option<AssignRow>, async_graphql::Error> {
    let row: Option<AssignRow> = sqlx::query_as(ASSIGN_COLUMNS)
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(db_err)?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Validation (Canvas model validations surfaced as payload errors)
// ---------------------------------------------------------------------------

/// Canvas `Assignment` model validations relevant to these inputs:
/// `validates :name, presence: true` (attribute `name`) and
/// `validates_numericality_of :points_possible, greater_than_or_equal_to: 0`
/// (attribute `points_possible`). Messages follow the star-lms REST wording
/// already used by `create_assignment` in star-assignments.
fn validate_assignment_input(
    name: Option<&str>,
    points_possible: Option<f64>,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    if let Some(name) = name {
        if name.trim().is_empty() {
            errors.push(ValidationError {
                attribute: Some("name".to_string()),
                message: "can't be blank".to_string(),
            });
        }
    }
    if let Some(pp) = points_possible {
        if pp < 0.0 || !pp.is_finite() {
            errors.push(ValidationError {
                attribute: Some("points_possible".to_string()),
                message: "must be greater than or equal to 0".to_string(),
            });
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// `createAssignment(input: CreateAssignmentInput!)` — Canvas
/// `CreateAssignment` (`create_assignment.rb`).
///
/// Canvas flow: load course → build assignment → `raise ExecutionError,
/// "invalid course: {id}"` unless `grants_right?(:create)`. Missing course
/// and unauthorized caller produce the *same* message deliberately (no
/// existence leak). On success inserts with `workflow_state` mapped from
/// `state` (default `unpublished`, Canvas's model default).
pub async fn create_assignment(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    input: CreateAssignmentInput,
) -> Result<CreateAssignmentPayload, async_graphql::Error> {
    // Canvas conflates invalid id, missing course, and lack of :create into
    // one message; parse failures take the same path.
    let course_id: i64 = match input.course_id.0.parse() {
        Ok(id) => id,
        Err(_) => {
            return Err(async_graphql::Error::new(format!(
                "invalid course: {}",
                input.course_id.0
            )))
        }
    };

    let manages = is_admin || crate::user_teaches_course(pool, user_id, course_id).await?;
    if !manages {
        return Err(async_graphql::Error::new(format!(
            "invalid course: {course_id}"
        )));
    }

    if let Some(errors) = nonempty(validate_assignment_input(
        Some(input.name.as_str()),
        input.points_possible,
    )) {
        return Ok(CreateAssignmentPayload {
            assignment: None,
            errors: Some(errors),
        });
    }

    // Canvas: workflow_state starts "unpublished"; published/unpublished
    // come from the `state` input. star-lms column default is 'published'
    // (REST parity backfill), so the mutation always writes explicitly.
    let workflow_state = input
        .state
        .map(AssignmentState::workflow_state)
        .unwrap_or("unpublished");

    let row: AssignRow = sqlx::query_as(
        r#"
        INSERT INTO assignments (course_id, title, description, points_possible, due_at, workflow_state)
        VALUES ($1, $2, $3, COALESCE($4, 0), $5, $6)
        RETURNING id, course_id, title AS name, description,
                  points_possible::float8 AS points_possible,
                  due_at, created_at, updated_at, workflow_state
        "#,
    )
    .bind(course_id)
    .bind(&input.name)
    .bind(&input.description)
    .bind(input.points_possible)
    .bind(input.due_at)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(CreateAssignmentPayload {
        assignment: Some(row.into_assignment()),
        errors: None,
    })
}

/// `updateAssignment(input: UpdateAssignmentInput!)` — Canvas
/// `UpdateAssignment` (`update_assignment.rb` + `assignment_base.rb`).
///
/// Canvas flow: `Assignment.find` miss → ExecutionError
/// `"assignment not found: {id}"`; without `:update` → ExecutionError
/// `"insufficient permission"`. `state: deleted` routes through
/// `ensure_destroyed`, which **re-checks `:delete`** and soft-deletes
/// idempotently; `published`/`unpublished` write the state and restore a
/// deleted row when needed (restore also requires the delete right — the
/// star-lms role model gives teachers/admins both, so one gate covers it).
/// There is no separate `deleteAssignment` mutation, mirroring Canvas.
pub async fn update_assignment(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    input: UpdateAssignmentInput,
) -> Result<UpdateAssignmentPayload, async_graphql::Error> {
    let assignment_id: i64 = match input.id.0.parse() {
        Ok(id) => id,
        Err(_) => {
            return Err(async_graphql::Error::new(format!(
                "assignment not found: {}",
                input.id.0
            )))
        }
    };

    let Some(existing) = fetch_assignment(pool, assignment_id).await? else {
        return Err(async_graphql::Error::new(format!(
            "assignment not found: {assignment_id}"
        )));
    };

    // :update derives from manage_assignments_edit; :delete (destroy/
    // restore paths) from manage_assignments_delete. Both collapse onto
    // check_teacher_or_admin in the star-lms role model.
    let manages =
        is_admin || crate::user_teaches_course(pool, user_id, existing.course_id).await?;
    if !manages {
        return Err(async_graphql::Error::new("insufficient permission"));
    }

    if let Some(errors) = nonempty(validate_assignment_input(
        input.name.as_deref(),
        input.points_possible,
    )) {
        return Ok(UpdateAssignmentPayload {
            assignment: None,
            errors: Some(errors),
        });
    }

    // Destroy path (assignment_base.rb ensure_destroyed): idempotent —
    // already-deleted rows return early without touching updated_at.
    if input.state == Some(AssignmentState::Deleted) {
        if existing.workflow_state != "deleted" {
            let result = sqlx::query(
                "UPDATE assignments SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
            )
            .bind(assignment_id)
            .execute(pool)
            .await
            .map_err(db_err)?;
            if result.rows_affected() == 0 {
                return Err(async_graphql::Error::new(format!(
                    "assignment not found: {assignment_id}"
                )));
            }
        }
        let row = fetch_assignment(pool, assignment_id)
            .await?
            .ok_or_else(|| async_graphql::Error::new(format!("assignment not found: {assignment_id}")))?;
        return Ok(UpdateAssignmentPayload {
            assignment: Some(row.into_assignment()),
            errors: None,
        });
    }

    // Field updates + published/unpublished (with implicit restore of a
    // deleted row — Canvas publishes through the destroyed->restored path).
    let name = input.name.unwrap_or(existing.name.clone());
    let description = input.description.or(existing.description.clone());
    let points_possible = input.points_possible.unwrap_or(existing.points_possible);
    let due_at = input.due_at.or(existing.due_at);
    let workflow_state = input
        .state
        .map(AssignmentState::workflow_state)
        // Canvas leaves the state untouched when `state` is omitted.
        .unwrap_or(existing.workflow_state.as_str());

    let row: AssignRow = sqlx::query_as(
        r#"
        UPDATE assignments
        SET title = $2, description = $3, points_possible = $4, due_at = $5,
            workflow_state = $6, updated_at = now()
        WHERE id = $1
        RETURNING id, course_id, title AS name, description,
                  points_possible::float8 AS points_possible,
                  due_at, created_at, updated_at, workflow_state
        "#,
    )
    .bind(assignment_id)
    .bind(&name)
    .bind(&description)
    .bind(points_possible)
    .bind(due_at)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;

    Ok(UpdateAssignmentPayload {
        assignment: Some(row.into_assignment()),
        errors: None,
    })
}

fn nonempty(errors: Vec<ValidationError>) -> Option<Vec<ValidationError>> {
    if errors.is_empty() {
        None
    } else {
        Some(errors)
    }
}
