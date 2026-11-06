//! star-grading-standards — grading schemes and letter-grade computation.
//!
//! Implements Canvas API v1:
//!   POST /courses/{courseId}/grading_standards  — create grading standard
//!   GET  /courses/{courseId}/grading_standards — list standards
//!   GET  /grading_standards/{id}               — get standard + nested entries
//!   DELETE /grading_standards/{id}              — soft-delete
//!   GET  /courses/{courseId}/final_grade?user_id=N — compute letter grade

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
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

/// Grading standard row as stored in the DB.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GradingStandard {
    pub id: i64,
    pub course_id: Option<i64>,
    pub title: String,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A single entry within a grading scheme.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GradingSchemeEntry {
    pub id: i64,
    pub name: String,
    pub min_score: f64,
    pub position: i32,
}

/// Full grading standard response with nested entries (Canvas API v1 shape).
#[derive(Debug, Clone, Serialize)]
pub struct GradingStandardResponse {
    pub id: i64,
    pub title: String,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grading_scheme: Option<Vec<GradingSchemeEntry>>,
}

impl GradingStandardResponse {
    /// Fetch a standard from DB and expand its entries, or None if not found.
    pub async fn from_id(pool: &PgPool, id: i64) -> Result<Option<Self>, sqlx::Error> {
        let std: Option<GradingStandard> = sqlx::query_as(
            r#"SELECT id, course_id, title, workflow_state, created_at, updated_at
               FROM grading_standards WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;

        match std {
            Some(s) if s.workflow_state == "deleted" => Ok(None),
            Some(s) => {
                let entries: Vec<GradingSchemeEntry> = sqlx::query_as(
                    r#"SELECT id, name, min_score::float8, position
                       FROM grading_scheme_entries
                       WHERE grading_standard_id = $1
                       ORDER BY position ASC"#,
                )
                .bind(id)
                .fetch_all(pool)
                .await?;

                Ok(Some(GradingStandardResponse {
                    id: s.id,
                    title: s.title,
                    workflow_state: s.workflow_state,
                    created_at: s.created_at,
                    updated_at: s.updated_at,
                    grading_scheme: Some(entries),
                }))
            }
            None => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/grading_standards
#[derive(Debug, Deserialize)]
pub struct CreateGradingStandardRequest {
    pub title: String,
    pub entries: Vec<CreateSchemeEntry>,
}

#[derive(Debug, Deserialize)]
pub struct CreateSchemeEntry {
    pub name: String,
    pub min_score: f64,
}

/// GET /courses/{courseId}/final_grade?user_id=N
#[derive(Debug, Deserialize)]
pub struct FinalGradeQuery {
    pub user_id: i64,
}

/// Response for final grade computation.
#[derive(Debug, Serialize)]
pub struct FinalGradeResponse {
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub letter: Option<String>,
}

// ---------------------------------------------------------------------------
// Letter-grade computation (pure function)
// ---------------------------------------------------------------------------

/// Compute the letter grade for a score using the ordered scheme entries.
///
/// Finds the entry with the highest `min_score` such that `min_score <= score`
/// (each entry's min_score is its lower-bound percentage, e.g. 0.90 = 90%).
/// Entries must be sorted by position ASC (lowest position = lowest grade).
/// Returns `None` if the score is below the lowest entry's min_score
/// (should not happen with a properly-formed scheme that has a 0.0 floor).
///
/// # Examples
/// ```ignore
/// let entries = vec![
///     GradingSchemeEntry { id: 1, name: "A".into(),  min_score: 0.94, position: 0 },
///     GradingSchemeEntry { id: 2, name: "A-".into(), min_score: 0.90, position: 1 },
///     GradingSchemeEntry { id: 3, name: "B".into(),  min_score: 0.80, position: 2 },
/// ];
/// assert_eq!(compute_letter_grade(92.0, &entries), Some("A-".to_string()));
/// assert_eq!(compute_letter_grade(91.0, &entries), Some("A-".to_string()));
/// assert_eq!(compute_letter_grade(85.0, &entries), Some("B".to_string()));
/// ```
pub fn compute_letter_grade(score: f64, entries: &[GradingSchemeEntry]) -> Option<String> {
    let score_pct = score / 100.0;

    let mut best: Option<&GradingSchemeEntry> = None;
    for entry in entries {
        if entry.min_score <= score_pct {
            match &best {
                None => best = Some(entry),
                Some(cur) if entry.min_score > cur.min_score => best = Some(entry),
                _ => {}
            }
        }
    }

    best.map(|e| e.name.clone())
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
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
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
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

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/grading_standards — create a grading standard
async fn create_grading_standard(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateGradingStandardRequest>,
) -> Result<(StatusCode, Json<GradingStandardResponse>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }
    if payload.entries.is_empty() {
        return Err(ApiError::Validation("entries cannot be empty".into()));
    }

    // Validate entries
    for entry in &payload.entries {
        if entry.name.trim().is_empty() {
            return Err(ApiError::Validation("entry name cannot be empty".into()));
        }
        if !(0.0..=1.0).contains(&entry.min_score) {
            return Err(ApiError::Validation(
                "entry min_score must be between 0.0 and 1.0".into(),
            ));
        }
    }

    let std: GradingStandard = sqlx::query_as(
        r#"INSERT INTO grading_standards (course_id, title)
           VALUES ($1, $2)
           RETURNING id, course_id, title, workflow_state, created_at, updated_at"#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .fetch_one(&state.db)
    .await?;

    let std_id = std.id;
    let now = std.created_at;

    // Insert entries
    for (pos, entry) in payload.entries.iter().enumerate() {
        sqlx::query(
            r#"INSERT INTO grading_scheme_entries (grading_standard_id, name, min_score, position)
               VALUES ($1, $2, $3, $4)"#,
        )
        .bind(std_id)
        .bind(&entry.name)
        .bind(entry.min_score)
        .bind(pos as i32)
        .execute(&state.db)
        .await?;
    }

    // Fetch back entries
    let entries: Vec<GradingSchemeEntry> = sqlx::query_as(
        r#"SELECT id, name, min_score::float8, position
           FROM grading_scheme_entries
           WHERE grading_standard_id = $1
           ORDER BY position ASC"#,
    )
    .bind(std_id)
    .fetch_all(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(GradingStandardResponse {
            id: std.id,
            title: std.title,
            workflow_state: std.workflow_state,
            created_at: now,
            updated_at: std.updated_at,
            grading_scheme: Some(entries),
        }),
    ))
}

/// GET /courses/{courseId}/grading_standards — list active standards for a course
async fn list_grading_standards(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<GradingStandardResponse>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let standards: Vec<GradingStandard> = sqlx::query_as(
        r#"SELECT id, course_id, title, workflow_state, created_at, updated_at
           FROM grading_standards
           WHERE course_id = $1 AND workflow_state = 'active'
           ORDER BY created_at ASC"#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    let mut result = Vec::with_capacity(standards.len());
    for std in standards {
        let entries: Vec<GradingSchemeEntry> = sqlx::query_as(
            r#"SELECT id, name, min_score::float8, position
               FROM grading_scheme_entries
               WHERE grading_standard_id = $1
               ORDER BY position ASC"#,
        )
        .bind(std.id)
        .fetch_all(&state.db)
        .await?;

        result.push(GradingStandardResponse {
            id: std.id,
            title: std.title,
            workflow_state: std.workflow_state,
            created_at: std.created_at,
            updated_at: std.updated_at,
            grading_scheme: Some(entries),
        });
    }

    Ok(Json(result))
}

/// GET /grading_standards/{id} — get a single standard with entries
async fn get_grading_standard(
    State(state): State<AppState>,
    Path(standard_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<GradingStandardResponse>, ApiError> {
    let std = GradingStandardResponse::from_id(&state.db, standard_id)
        .await
        .map_err(|e| ApiError::Db(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    // Course-level check: verify user has access to the associated course
    let course_id = std.id;
    if course_id > 0 {
        // We need to check the course_id from the standard...
        // Actually re-fetch to get course_id
        let raw: Option<GradingStandard> = sqlx::query_as(
            r#"SELECT id, course_id, title, workflow_state, created_at, updated_at
               FROM grading_standards WHERE id = $1"#,
        )
        .bind(standard_id)
        .fetch_optional(&state.db)
        .await?;

        if let Some(s) = raw {
            if let Some(cid) = s.course_id {
                if !check_enrolled_or_admin(&state.db, auth.id, cid, &auth).await? {
                    return Err(ApiError::Forbidden);
                }
            }
        }
    }

    Ok(Json(std))
}

/// DELETE /grading_standards/{id} — soft-delete a grading standard
async fn delete_grading_standard(
    State(state): State<AppState>,
    Path(standard_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Fetch to get course_id for permission check
    let raw: Option<GradingStandard> = sqlx::query_as(
        r#"SELECT id, course_id, title, workflow_state, created_at, updated_at
           FROM grading_standards WHERE id = $1"#,
    )
    .bind(standard_id)
    .fetch_optional(&state.db)
    .await?;

    let std = raw.ok_or(ApiError::NotFound)?;

    if std.workflow_state == "deleted" {
        return Err(ApiError::NotFound);
    }

    if let Some(course_id) = std.course_id {
        if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
            return Err(ApiError::Forbidden);
        }
    } else if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    sqlx::query(
        r#"UPDATE grading_standards SET workflow_state = 'deleted', updated_at = now()
           WHERE id = $1"#,
    )
    .bind(standard_id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /courses/{courseId}/final_grade?user_id=N — compute letter grade for user
///
/// Uses the course's active grading standard and computes the weighted grade
/// from graded submissions (via star-assignments grade computation).
async fn get_final_grade(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<FinalGradeQuery>,
    auth: AuthUser,
) -> Result<Json<FinalGradeResponse>, ApiError> {
    // Must be teacher or admin to view another student's grade
    // (or you can view your own grade)
    let is_viewing_self = auth.id == query.user_id;
    if !is_viewing_self && !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Check: user must be enrolled in the course (or admin)
    if !check_enrolled_or_admin(&state.db, query.user_id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // 1. Fetch the active grading standard for the course
    let std_row: Option<GradingStandard> = sqlx::query_as(
        r#"SELECT id, course_id, title, workflow_state, created_at, updated_at
           FROM grading_standards
           WHERE course_id = $1 AND workflow_state = 'active'
           LIMIT 1"#,
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    let entries: Option<Vec<GradingSchemeEntry>> = if let Some(std) = std_row {
        let e: Vec<GradingSchemeEntry> = sqlx::query_as(
            r#"SELECT id, name, min_score::float8, position
               FROM grading_scheme_entries
               WHERE grading_standard_id = $1
               ORDER BY position ASC"#,
        )
        .bind(std.id)
        .fetch_all(&state.db)
        .await?;
        Some(e)
    } else {
        None
    };

    // 2. Compute the grade percentage from graded submissions
    //    (reuse the grade_arrays_from_submissions + compute_grade_percent from star-assignments)
    let assignments: Vec<star_assignments::Assignment> = sqlx::query_as(
        r#"SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
           FROM assignments WHERE course_id = $1"#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    let submissions: Vec<star_assignments::Submission> = sqlx::query_as(
        r#"SELECT s.id, s.assignment_id, s.user_id, s.body, s.score::float8, s.submitted_at, s.created_at, s.updated_at
           FROM submissions s
           JOIN assignments a ON a.id = s.assignment_id
           WHERE a.course_id = $1 AND s.user_id = $2"#,
    )
    .bind(course_id)
    .bind(query.user_id)
    .fetch_all(&state.db)
    .await?;

    let (scores, points_possible) =
        star_assignments::grade_arrays_from_submissions(&assignments, &submissions);
    let score = star_assignments::compute_grade_percent(&scores, &points_possible);

    let score = match score {
        Some(s) => s,
        None => {
            return Ok(Json(FinalGradeResponse {
                score: 0.0,
                letter: None,
            }));
        }
    };

    let letter = entries
        .as_ref()
        .and_then(|e| compute_letter_grade(score, e));

    Ok(Json(FinalGradeResponse { score, letter }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/grading_standards", post(create_grading_standard))
        .route("/courses/{courseId}/grading_standards", get(list_grading_standards))
        .route("/courses/{courseId}/final_grade", get(get_final_grade))
        .route("/grading_standards/{id}", get(get_grading_standard))
        .route("/grading_standards/{id}", delete(delete_grading_standard))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: i64, name: &str, min_score: f64, position: i32) -> GradingSchemeEntry {
        GradingSchemeEntry {
            id,
            name: name.to_string(),
            min_score,
            position,
        }
    }

    #[test]
    fn test_letter_grade_exact_boundary() {
        let entries = vec![
            make_entry(1, "A", 0.94, 0),
            make_entry(2, "A-", 0.90, 1),
            make_entry(3, "B+", 0.87, 2),
            make_entry(4, "B", 0.84, 3),
            make_entry(5, "B-", 0.80, 4),
            make_entry(6, "C+", 0.77, 5),
            make_entry(7, "C", 0.74, 6),
            make_entry(8, "C-", 0.70, 7),
            make_entry(9, "D+", 0.67, 8),
            make_entry(10, "D", 0.64, 9),
            make_entry(11, "D-", 0.61, 10),
            make_entry(12, "F", 0.0, 11),
        ];

        // Exact boundaries
        assert_eq!(compute_letter_grade(94.0, &entries), Some("A".to_string()));
        assert_eq!(compute_letter_grade(90.0, &entries), Some("A-".to_string()));
        assert_eq!(compute_letter_grade(87.0, &entries), Some("B+".to_string()));
        assert_eq!(compute_letter_grade(84.0, &entries), Some("B".to_string()));
        assert_eq!(compute_letter_grade(80.0, &entries), Some("B-".to_string()));
        assert_eq!(compute_letter_grade(77.0, &entries), Some("C+".to_string()));
        assert_eq!(compute_letter_grade(74.0, &entries), Some("C".to_string()));
        assert_eq!(compute_letter_grade(70.0, &entries), Some("C-".to_string()));
        assert_eq!(compute_letter_grade(67.0, &entries), Some("D+".to_string()));
        assert_eq!(compute_letter_grade(64.0, &entries), Some("D".to_string()));
        assert_eq!(compute_letter_grade(61.0, &entries), Some("D-".to_string()));
        assert_eq!(compute_letter_grade(0.0, &entries), Some("F".to_string()));
    }

    #[test]
    fn test_letter_grade_in_between() {
        let entries = vec![
            make_entry(1, "A", 0.94, 0),
            make_entry(2, "A-", 0.90, 1),
            make_entry(3, "F", 0.0, 2),
        ];

        // Scores between a boundary and the next higher threshold use the lower bracket.
        assert_eq!(compute_letter_grade(92.0, &entries), Some("A-".to_string()));
        assert_eq!(compute_letter_grade(55.0, &entries), Some("F".to_string()));
        assert_eq!(compute_letter_grade(100.0, &entries), Some("A".to_string()));
        assert_eq!(compute_letter_grade(89.9999, &entries), Some("F".to_string())); // 89.99 < 90
    }

    #[test]
    fn test_letter_grade_empty_entries() {
        assert_eq!(compute_letter_grade(92.0, &[]), None);
    }
}
