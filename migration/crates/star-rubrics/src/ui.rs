//! Pub data loaders + the shared assessment write for the server-rendered
//! SpeedGrader rubric panel (star-ui, issue #108).
//!
//! Same extraction pattern as `star_conversations::ui`: the grading page
//! needs the rubric associated with the assignment (via the Canvas
//! `rubric_associations` join the REST API reaches only through separate
//! endpoints), plus the existing grading assessment for the selected
//! submission. The write path mirrors the exact INSERT shape of the REST
//! `insert_assessment` helper (assessment_type "grading", course/user
//! resolved through the submission's assignment, artifact backfilled), so
//! the shell cannot fork rubric validation.
//!
//! Authorization stays with the caller: the shell gates the whole grading
//! page through `star_courses::check_teacher_or_admin` before any of these
//! run (the REST handlers enforce the same gate per route).

use sqlx::PgPool;
use star_common::ApiError;

use crate::RubricAssessment;

/// The rubric panel of one assignment: the rubric associated with it
/// (Canvas `association_type = 'Assignment'`, newest active association
/// wins) with its criteria and ratings in position order.
#[derive(Debug, Clone)]
pub struct AssignmentRubricPanel {
    pub rubric_id: i64,
    pub title: String,
    pub points_possible: f64,
    /// The association id (stored on the persisted assessment).
    pub association_id: i64,
    pub criteria: Vec<PanelCriterion>,
}

/// One criterion row of the rubric panel, carrying its rating options.
///
/// `ratings` is filled by [`load_assignment_rubric_panel`] (grouped in
/// Rust, three queries total — association, criteria, ratings — never
/// N+1), so it is skipped by the row decoder.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PanelCriterion {
    pub id: i64,
    pub description: String,
    pub points: f64,
    pub position: i32,
    #[sqlx(skip)]
    pub ratings: Vec<PanelRating>,
}

/// One rating option of a criterion, for the no-JS `<select>`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PanelRating {
    pub criterion_id: i64,
    pub id: i64,
    pub description: String,
    pub points: f64,
    pub position: i32,
}

/// Load the rubric panel for an assignment, or `None` when the assignment
/// has no active rubric association (the grading page then renders no
/// rubric section at all, Canvas' behavior).
///
/// Ordering: criteria by `position`, ratings by `position` — the same
/// order `fetch_rubric_with_nested` serves the REST API.
pub async fn load_assignment_rubric_panel(
    db: &PgPool,
    assignment_id: i64,
) -> Result<Option<AssignmentRubricPanel>, ApiError> {
    let association: Option<(i64, i64, String, f64)> = sqlx::query_as(
        r#"
        SELECT r.id, ra.id, r.title, r.points_possible::float8
        FROM rubric_associations ra
        JOIN rubrics r ON r.id = ra.rubric_id
        WHERE ra.association_id = $1
          AND ra.association_type = 'Assignment'
          AND ra.workflow_state <> 'deleted'
          AND r.workflow_state <> 'deleted'
        ORDER BY ra.id DESC
        LIMIT 1
        "#,
    )
    .bind(assignment_id)
    .fetch_optional(db)
    .await?;

    let Some((rubric_id, association_id, title, points_possible)) = association else {
        return Ok(None);
    };

    let mut criteria: Vec<PanelCriterion> = sqlx::query_as(
        r#"
        SELECT id, description, points::float8, position
        FROM rubric_criteria WHERE rubric_id = $1 ORDER BY position ASC
        "#,
    )
    .bind(rubric_id)
    .fetch_all(db)
    .await?;

    if !criteria.is_empty() {
        let ratings: Vec<PanelRating> = sqlx::query_as(
            r#"
            SELECT criterion_id, id, description, points::float8, position
            FROM rubric_ratings
            WHERE criterion_id = ANY($1)
            ORDER BY position ASC
            "#,
        )
        .bind(&criteria.iter().map(|c| c.id).collect::<Vec<_>>())
        .fetch_all(db)
        .await?;
        for criterion in criteria.iter_mut() {
            criterion.ratings = ratings
                .iter()
                .filter(|r| r.criterion_id == criterion.id)
                .cloned()
                .collect();
        }
    }

    Ok(Some(AssignmentRubricPanel {
        rubric_id,
        title,
        points_possible,
        association_id,
        criteria,
    }))
}

/// Load the existing GRADING assessment of one submission against a rubric —
/// the scores the panel pre-fills. Oldest wins when several exist (Canvas
/// shows the current grading assessment; re-submitting replaces it via
/// [`upsert_grading_assessment`]).
pub async fn load_grading_assessment(
    db: &PgPool,
    rubric_id: i64,
    submission_id: i64,
) -> Result<Option<RubricAssessment>, ApiError> {
    let sql = format!(
        "SELECT {ASSESSMENT_COLUMNS} FROM rubric_assessments \
         WHERE rubric_id = $1 AND submission_id = $2 \
           AND assessment_type = 'grading' AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1"
    );
    Ok(sqlx::query_as::<_, RubricAssessment>(&sql)
        .bind(rubric_id)
        .bind(submission_id)
        .fetch_optional(db)
        .await?)
}

/// Record (or replace) the grading assessment of a submission against a
/// rubric — the shell's write path for the rubric panel form.
///
/// Same column set and resolution rules as the REST `insert_assessment`
/// helper: `assessment_type` fixed to "grading", `course_id`/`user_id`
/// resolved through the submission's assignment (404 when the submission is
/// gone), artifact backfilled as (`submission_id`, 'Submission'), `data`
/// stored as the Canvas `{criterion_id, rating_id, points, comments}`
/// array. There is no unique constraint on (rubric, submission, type), so
/// the upsert is select-then-update/insert; a repeated submit replaces the
/// prior grading assessment — one active grading assessment per
/// submission+rubric, matching Canvas' `RubricAssociation#update` semantics.
pub async fn upsert_grading_assessment(
    db: &PgPool,
    rubric_id: i64,
    association_id: i64,
    submission_id: i64,
    assessor_id: i64,
    score: f64,
    data: serde_json::Value,
) -> Result<i64, ApiError> {
    // Resolve (course_id, user_id) through the submission's assignment —
    // the same backfill `insert_assessment` performs.
    let context: Option<(i64, i64)> = sqlx::query_as(
        r#"
        SELECT a.course_id, s.user_id
        FROM submissions s
        JOIN assignments a ON a.id = s.assignment_id
        WHERE s.id = $1
        "#,
    )
    .bind(submission_id)
    .fetch_optional(db)
    .await?;
    let Some((course_id, user_id)) = context else {
        return Err(ApiError::NotFound);
    };

    // Replace the existing grading assessment when one exists.
    let updated: Option<(i64,)> = sqlx::query_as(
        r#"
        UPDATE rubric_assessments
        SET rubric_association_id = $2, score = $3, data = $4, updated_at = now()
        WHERE id = (
            SELECT id FROM rubric_assessments
            WHERE rubric_id = $1 AND submission_id = $5
              AND assessment_type = 'grading' AND workflow_state <> 'deleted'
            ORDER BY id ASC LIMIT 1
        )
        RETURNING id
        "#,
    )
    .bind(rubric_id)
    .bind(association_id)
    .bind(score)
    .bind(&data)
    .bind(submission_id)
    .fetch_optional(db)
    .await?;
    if let Some((id,)) = updated {
        return Ok(id);
    }

    let (id,): (i64,) = sqlx::query_as(
        r#"
        INSERT INTO rubric_assessments
            (rubric_id, course_id, rubric_association_id, submission_id,
             artifact_id, artifact_type, assessment_type, user_id, assessor_id,
             score, data)
        VALUES ($1, $2, $3, $4, $4, 'Submission', 'grading', $5, $6, $7, $8)
        RETURNING id
        "#,
    )
    .bind(rubric_id)
    .bind(course_id)
    .bind(association_id)
    .bind(submission_id)
    .bind(user_id)
    .bind(assessor_id)
    .bind(score)
    .bind(&data)
    .fetch_one(db)
    .await?;
    Ok(id)
}
