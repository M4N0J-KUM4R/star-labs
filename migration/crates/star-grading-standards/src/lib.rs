//! star-grading-standards — grading schemes and letter-grade computation.
//!
//! Implements Canvas API v1:
//!   POST /courses/{courseId}/grading_standards  — create grading standard
//!   GET  /courses/{courseId}/grading_standards — list standards
//!   GET  /grading_standards/{id}               — get standard + nested entries
//!   DELETE /grading_standards/{id}              — soft-delete
//!   GET  /courses/{courseId}/grading_standards/{id}    — course-scoped alias (get)
//!   DELETE /courses/{courseId}/grading_standards/{id}  — course-scoped alias (delete)
//!   GET  /courses/{courseId}/final_grade?user_id=N — compute letter grade
//!   GET  /accounts/{accountId}/grading_periods  — list account grading periods
//!   POST /accounts/{accountId}/grading_periods  — create grading period(s)
//!   GET  /grading_periods/{id}                  — get grading period
//!   PUT  /grading_periods/{id}                  — update grading period
//!   DELETE /grading_periods/{id}                — soft-delete grading period

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
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

/// 404 unless the grading standard exists and belongs to `course_id`.
///
/// Used by the Canvas v1 nested aliases so a standard can only be reached
/// through its owning course. Global (course_id IS NULL) standards and
/// cross-course ids are treated as not found.
async fn require_standard_in_course(
    pool: &PgPool,
    course_id: i64,
    standard_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT course_id FROM grading_standards WHERE id = $1")
            .bind(standard_id)
            .fetch_optional(pool)
            .await?;

    match row {
        Some((Some(owner),)) if owner == course_id => Ok(()),
        _ => Err(ApiError::NotFound),
    }
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

/// GET /courses/{courseId}/grading_standards/{id} — Canvas v1 nested alias.
///
/// Verifies the standard belongs to the course in the path (mismatch or
/// missing -> 404), then delegates to the flat handler (including its authz).
async fn get_course_grading_standard(
    State(state): State<AppState>,
    Path((course_id, standard_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<GradingStandardResponse>, ApiError> {
    require_standard_in_course(&state.db, course_id, standard_id).await?;
    get_grading_standard(State(state), Path(standard_id), auth).await
}

/// DELETE /courses/{courseId}/grading_standards/{id} — Canvas v1 nested alias.
///
/// Same course-scope check as the GET alias, then delegates to the flat
/// delete handler (including its authz).
async fn delete_course_grading_standard(
    State(state): State<AppState>,
    Path((course_id, standard_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_standard_in_course(&state.db, course_id, standard_id).await?;
    delete_grading_standard(State(state), Path(standard_id), auth).await
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
// Grading periods (Canvas GradingPeriod / GradingPeriodGroup parity)
// ---------------------------------------------------------------------------

/// Upper bound on parent-account traversal (guards against cycles).
const MAX_ACCOUNT_TREE_DEPTH: i64 = 10;

const OVERLAP_MESSAGE: &str =
    "Grading period cannot overlap with existing grading periods in group";

/// Grading period row from `grading_periods`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GradingPeriod {
    pub id: i64,
    pub grading_period_group_id: i64,
    pub title: String,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub close_date: DateTime<Utc>,
    pub weight: Option<f64>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Canvas `PermissionsSerializer` block for a grading period.
#[derive(Debug, Clone, Serialize)]
pub struct GradingPeriodPermissions {
    pub read: bool,
    pub update: bool,
    pub delete: bool,
}

/// Canvas API v1 `GradingPeriod` shape.
#[derive(Debug, Clone, Serialize)]
pub struct GradingPeriodResponse {
    pub id: i64,
    pub grading_period_group_id: i64,
    pub title: String,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub close_date: DateTime<Utc>,
    pub weight: Option<f64>,
    pub is_closed: bool,
    pub permissions: GradingPeriodPermissions,
}

impl GradingPeriodResponse {
    fn from_row(row: GradingPeriod, can_manage: bool, now: DateTime<Utc>) -> Self {
        Self {
            id: row.id,
            grading_period_group_id: row.grading_period_group_id,
            title: row.title,
            start_date: row.start_date,
            end_date: row.end_date,
            close_date: row.close_date,
            weight: row.weight,
            is_closed: now > row.close_date,
            permissions: GradingPeriodPermissions {
                read: true,
                update: can_manage,
                delete: can_manage,
            },
        }
    }
}

/// Canvas wraps grading period payloads under a `grading_periods` root.
#[derive(Debug, Serialize)]
pub struct GradingPeriodsResponse {
    pub grading_periods: Vec<GradingPeriodResponse>,
}

/// Canvas account-level index response (list plus create/read-only flags).
#[derive(Debug, Serialize)]
pub struct GradingPeriodsIndexResponse {
    pub grading_periods: Vec<GradingPeriodResponse>,
    pub can_create_grading_periods: bool,
    pub grading_periods_read_only: bool,
}

/// Request fields for one grading period (Canvas `grading_periods[]` member).
#[derive(Debug, Deserialize)]
pub struct GradingPeriodParams {
    pub title: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub close_date: Option<String>,
    pub weight: Option<f64>,
}

/// Accepts Canvas' `{"grading_periods": [...]}` envelope or a flat object.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum GradingPeriodPayload {
    Wrapped {
        grading_periods: Vec<GradingPeriodParams>,
    },
    Single(GradingPeriodParams),
}

impl GradingPeriodPayload {
    fn into_periods(self) -> Vec<GradingPeriodParams> {
        match self {
            GradingPeriodPayload::Wrapped { grading_periods } => grading_periods,
            GradingPeriodPayload::Single(params) => vec![params],
        }
    }

    /// Canvas' update actions read `params[:grading_periods].first`.
    fn into_single(self) -> Result<GradingPeriodParams, ApiError> {
        self.into_periods()
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::Validation("grading_periods cannot be empty".into()))
    }
}

/// Validated date range for a grading period.
#[derive(Debug, Clone)]
struct GradingPeriodWindow {
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
    close_date: DateTime<Utc>,
}

/// Parse Canvas date params: RFC3339 timestamps or plain `YYYY-MM-DD` dates.
fn parse_grading_period_date(value: &str) -> Result<DateTime<Utc>, ApiError> {
    let value = value.trim();

    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let naive = date.and_time(NaiveTime::MIN);
        return Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
    }

    Err(ApiError::Validation(format!("invalid date: {value}")))
}

fn required_date(value: &Option<String>, field: &str) -> Result<DateTime<Utc>, ApiError> {
    match value {
        Some(v) => parse_grading_period_date(v),
        None => Err(ApiError::Validation(format!("{field} is required"))),
    }
}

/// Canvas validations: end_date >= start_date and close_date >= end_date.
fn validate_period_window(window: &GradingPeriodWindow) -> Result<(), ApiError> {
    if window.end_date < window.start_date {
        return Err(ApiError::Validation(
            "end_date must be after start date".into(),
        ));
    }
    if window.close_date < window.end_date {
        return Err(ApiError::Validation(
            "close_date must be on or after end date".into(),
        ));
    }
    Ok(())
}

/// Canvas ranges overlap when each starts before the other ends; shared
/// endpoints are allowed.
fn windows_overlap(a: &GradingPeriodWindow, b: &GradingPeriodWindow) -> bool {
    a.start_date < b.end_date && b.start_date < a.end_date
}

// ---------------------------------------------------------------------------
// Grading period permission helpers
// ---------------------------------------------------------------------------

/// True when the user is an account admin for exactly this account.
async fn is_account_admin(pool: &PgPool, user_id: i64, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

/// True when the user is an account admin for the account or any ancestor.
async fn is_account_admin_in_chain(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
) -> Result<bool, ApiError> {
    let mut current = Some(account_id);
    let mut depth = 0;

    while let Some(id) = current {
        if depth >= MAX_ACCOUNT_TREE_DEPTH {
            return Ok(false);
        }
        depth += 1;

        if is_account_admin(pool, user_id, id).await? {
            return Ok(true);
        }

        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;

        current = parent.and_then(|(parent_id,)| parent_id);
    }

    Ok(false)
}

async fn account_exists(pool: &PgPool, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.is_some())
}

/// Manage rights: global admin or account admin (own account or an ancestor).
async fn can_manage_account(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    is_account_admin_in_chain(pool, user_id, account_id).await
}

/// Read rights: manage rights, or an enrollment in a course owned by the
/// account (or one of its sub-accounts).
async fn can_read_account(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if can_manage_account(pool, user_id, account_id, auth).await? {
        return Ok(true);
    }

    let row: Option<(i32,)> = sqlx::query_as(
        r#"WITH RECURSIVE account_tree AS (
               SELECT id FROM accounts WHERE id = $1
               UNION
               SELECT a.id FROM accounts a JOIN account_tree t ON a.parent_account_id = t.id
           )
           SELECT 1
           FROM enrollments e
           JOIN courses c ON c.id = e.course_id
           WHERE e.user_id = $2 AND c.account_id IN (SELECT id FROM account_tree)
           LIMIT 1"#,
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

/// Return the account's active grading period set, creating one on demand.
async fn ensure_account_group(pool: &PgPool, account_id: i64) -> Result<i64, ApiError> {
    let existing: Option<(i64,)> = sqlx::query_as(
        r#"SELECT id FROM grading_period_groups
           WHERE account_id = $1 AND workflow_state = 'active'
           ORDER BY id ASC
           LIMIT 1"#,
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;

    if let Some((id,)) = existing {
        return Ok(id);
    }

    let (id,): (i64,) =
        sqlx::query_as("INSERT INTO grading_period_groups (account_id) VALUES ($1) RETURNING id")
            .bind(account_id)
            .fetch_one(pool)
            .await?;

    Ok(id)
}

/// Account owning a grading period group (course-level groups have none).
async fn account_id_for_group(pool: &PgPool, group_id: i64) -> Result<Option<i64>, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM grading_period_groups WHERE id = $1")
            .bind(group_id)
            .fetch_optional(pool)
            .await?;

    Ok(row.and_then(|(account_id,)| account_id))
}

async fn fetch_grading_period(pool: &PgPool, id: i64) -> Result<Option<GradingPeriod>, ApiError> {
    let row: Option<GradingPeriod> = sqlx::query_as(
        r#"SELECT id, grading_period_group_id, title, start_date, end_date, close_date,
                  weight, workflow_state, created_at, updated_at
           FROM grading_periods
           WHERE id = $1 AND workflow_state = 'active'"#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row)
}

/// True when another active period for the account overlaps `window`.
async fn has_overlapping_period(
    pool: &PgPool,
    account_id: i64,
    exclude_id: Option<i64>,
    window: &GradingPeriodWindow,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1
           FROM grading_periods gp
           JOIN grading_period_groups g ON g.id = gp.grading_period_group_id
           WHERE g.account_id = $1
             AND g.workflow_state = 'active'
             AND gp.workflow_state = 'active'
             AND gp.id <> $2
             AND gp.end_date > $3
             AND gp.start_date < $4
           LIMIT 1"#,
    )
    .bind(account_id)
    .bind(exclude_id.unwrap_or(0))
    .bind(window.start_date)
    .bind(window.end_date)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Grading period handlers
// ---------------------------------------------------------------------------

/// GET /accounts/{accountId}/grading_periods — list grading periods.
async fn list_account_grading_periods(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<GradingPeriodsIndexResponse>, ApiError> {
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !can_read_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let can_manage = can_manage_account(&state.db, auth.id, account_id, &auth).await?;

    let rows: Vec<GradingPeriod> = sqlx::query_as(
        r#"SELECT gp.id, gp.grading_period_group_id, gp.title, gp.start_date, gp.end_date,
                  gp.close_date, gp.weight, gp.workflow_state, gp.created_at, gp.updated_at
           FROM grading_periods gp
           JOIN grading_period_groups g ON g.id = gp.grading_period_group_id
           WHERE g.account_id = $1
             AND g.workflow_state = 'active'
             AND gp.workflow_state = 'active'
           ORDER BY gp.start_date ASC, gp.id ASC"#,
    )
    .bind(account_id)
    .fetch_all(&state.db)
    .await?;

    let now = Utc::now();
    let grading_periods = rows
        .into_iter()
        .map(|row| GradingPeriodResponse::from_row(row, can_manage, now))
        .collect();

    Ok(Json(GradingPeriodsIndexResponse {
        grading_periods,
        can_create_grading_periods: can_manage,
        grading_periods_read_only: false,
    }))
}

/// POST /accounts/{accountId}/grading_periods — create grading period(s).
async fn create_account_grading_period(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<GradingPeriodPayload>,
) -> Result<(StatusCode, Json<GradingPeriodsResponse>), ApiError> {
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !can_manage_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let params = payload.into_periods();
    if params.is_empty() {
        return Err(ApiError::Validation(
            "grading_periods cannot be empty".into(),
        ));
    }

    // Validate every period before writing anything.
    let mut titles = Vec::with_capacity(params.len());
    let mut windows = Vec::with_capacity(params.len());
    for param in &params {
        let title = param.title.as_deref().map(str::trim).unwrap_or_default();
        if title.is_empty() {
            return Err(ApiError::Validation("title is required".into()));
        }

        let start_date = required_date(&param.start_date, "start_date")?;
        let end_date = required_date(&param.end_date, "end_date")?;
        let close_date = match &param.close_date {
            Some(value) => parse_grading_period_date(value)?,
            None => end_date,
        };
        let window = GradingPeriodWindow {
            start_date,
            end_date,
            close_date,
        };
        validate_period_window(&window)?;

        titles.push(title.to_string());
        windows.push(window);
    }

    // Canvas rejects overlapping periods within the batch and against the
    // account's existing active periods.
    for (index, window) in windows.iter().enumerate() {
        for other in windows.iter().skip(index + 1) {
            if windows_overlap(window, other) {
                return Err(ApiError::Validation(OVERLAP_MESSAGE.into()));
            }
        }
        if has_overlapping_period(&state.db, account_id, None, window).await? {
            return Err(ApiError::Validation(OVERLAP_MESSAGE.into()));
        }
    }

    let group_id = ensure_account_group(&state.db, account_id).await?;

    let mut tx = state.db.begin().await?;
    let mut created = Vec::with_capacity(params.len());
    for ((title, window), param) in titles.iter().zip(windows.iter()).zip(params.iter()) {
        let row: GradingPeriod = sqlx::query_as(
            r#"INSERT INTO grading_periods
                   (grading_period_group_id, title, start_date, end_date, close_date, weight)
               VALUES ($1, $2, $3, $4, $5, $6)
               RETURNING id, grading_period_group_id, title, start_date, end_date, close_date,
                         weight, workflow_state, created_at, updated_at"#,
        )
        .bind(group_id)
        .bind(title)
        .bind(window.start_date)
        .bind(window.end_date)
        .bind(window.close_date)
        .bind(param.weight)
        .fetch_one(&mut *tx)
        .await?;

        created.push(row);
    }
    tx.commit().await?;

    let now = Utc::now();
    let grading_periods = created
        .into_iter()
        .map(|row| GradingPeriodResponse::from_row(row, true, now))
        .collect();

    Ok((
        StatusCode::CREATED,
        Json(GradingPeriodsResponse { grading_periods }),
    ))
}

/// GET /grading_periods/{id} — get a single grading period.
async fn get_grading_period(
    State(state): State<AppState>,
    Path(period_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<GradingPeriodsResponse>, ApiError> {
    let period = fetch_grading_period(&state.db, period_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let account_id = account_id_for_group(&state.db, period.grading_period_group_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if !can_read_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let can_manage = can_manage_account(&state.db, auth.id, account_id, &auth).await?;

    Ok(Json(GradingPeriodsResponse {
        grading_periods: vec![GradingPeriodResponse::from_row(
            period,
            can_manage,
            Utc::now(),
        )],
    }))
}

/// PUT /grading_periods/{id} — update a grading period.
async fn update_grading_period(
    State(state): State<AppState>,
    Path(period_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<GradingPeriodPayload>,
) -> Result<Json<GradingPeriodsResponse>, ApiError> {
    let params = payload.into_single()?;

    let period = fetch_grading_period(&state.db, period_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let account_id = account_id_for_group(&state.db, period.grading_period_group_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if !can_manage_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let title = match params.title {
        Some(title) => {
            let title = title.trim().to_string();
            if title.is_empty() {
                return Err(ApiError::Validation("title cannot be empty".into()));
            }
            title
        }
        None => period.title.clone(),
    };

    let start_date = match &params.start_date {
        Some(value) => parse_grading_period_date(value)?,
        None => period.start_date,
    };
    let end_date = match &params.end_date {
        Some(value) => parse_grading_period_date(value)?,
        None => period.end_date,
    };
    let close_date = match &params.close_date {
        Some(value) => parse_grading_period_date(value)?,
        None => period.close_date,
    };
    let weight = params.weight.or(period.weight);

    let window = GradingPeriodWindow {
        start_date,
        end_date,
        close_date,
    };
    validate_period_window(&window)?;
    if has_overlapping_period(&state.db, account_id, Some(period_id), &window).await? {
        return Err(ApiError::Validation(OVERLAP_MESSAGE.into()));
    }

    let updated: GradingPeriod = sqlx::query_as(
        r#"UPDATE grading_periods
           SET title = $2, start_date = $3, end_date = $4, close_date = $5,
               weight = $6, updated_at = now()
           WHERE id = $1
           RETURNING id, grading_period_group_id, title, start_date, end_date, close_date,
                     weight, workflow_state, created_at, updated_at"#,
    )
    .bind(period_id)
    .bind(&title)
    .bind(start_date)
    .bind(end_date)
    .bind(close_date)
    .bind(weight)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(GradingPeriodsResponse {
        grading_periods: vec![GradingPeriodResponse::from_row(updated, true, Utc::now())],
    }))
}

/// DELETE /grading_periods/{id} — soft-delete a grading period.
async fn delete_grading_period(
    State(state): State<AppState>,
    Path(period_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let period = fetch_grading_period(&state.db, period_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let account_id = account_id_for_group(&state.db, period.grading_period_group_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if !can_manage_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    sqlx::query(
        r#"UPDATE grading_periods SET workflow_state = 'deleted', updated_at = now()
           WHERE id = $1"#,
    )
    .bind(period_id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/grading_standards",
            post(create_grading_standard),
        )
        .route(
            "/courses/{courseId}/grading_standards",
            get(list_grading_standards),
        )
        .route("/courses/{courseId}/final_grade", get(get_final_grade))
        .route("/grading_standards/{id}", get(get_grading_standard))
        .route("/grading_standards/{id}", delete(delete_grading_standard))
        .route(
            "/courses/{courseId}/grading_standards/{id}",
            get(get_course_grading_standard),
        )
        .route(
            "/courses/{courseId}/grading_standards/{id}",
            delete(delete_course_grading_standard),
        )
        .route(
            "/accounts/{accountId}/grading_periods",
            get(list_account_grading_periods),
        )
        .route(
            "/accounts/{accountId}/grading_periods",
            post(create_account_grading_period),
        )
        .route("/grading_periods/{id}", get(get_grading_period))
        .route("/grading_periods/{id}", put(update_grading_period))
        .route("/grading_periods/{id}", delete(delete_grading_period))
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
        assert_eq!(
            compute_letter_grade(89.9999, &entries),
            Some("F".to_string())
        ); // 89.99 < 90
    }

    #[test]
    fn test_letter_grade_empty_entries() {
        assert_eq!(compute_letter_grade(92.0, &[]), None);
    }

    #[test]
    fn test_parse_grading_period_date_formats() {
        let timestamp = parse_grading_period_date("2014-01-07T15:04:00Z").unwrap();
        assert_eq!(timestamp.to_rfc3339(), "2014-01-07T15:04:00+00:00");

        let date_only = parse_grading_period_date("2014-01-07").unwrap();
        assert_eq!(date_only.to_rfc3339(), "2014-01-07T00:00:00+00:00");

        let without_offset = parse_grading_period_date(" 2014-01-07T15:04:00 ").unwrap();
        assert_eq!(without_offset, timestamp);

        assert!(parse_grading_period_date("not-a-date").is_err());
    }

    #[test]
    fn test_validate_period_window_ordering() {
        let window = |start: &str, end: &str, close: &str| GradingPeriodWindow {
            start_date: parse_grading_period_date(start).unwrap(),
            end_date: parse_grading_period_date(end).unwrap(),
            close_date: parse_grading_period_date(close).unwrap(),
        };

        assert!(validate_period_window(&window("2024-01-01", "2024-03-01", "2024-03-08")).is_ok());
        // close_date defaults to end_date in the handler, so equal dates are valid.
        assert!(validate_period_window(&window("2024-01-01", "2024-03-01", "2024-03-01")).is_ok());
        assert!(validate_period_window(&window("2024-03-02", "2024-03-01", "2024-03-01")).is_err());
        assert!(validate_period_window(&window("2024-01-01", "2024-03-01", "2024-02-01")).is_err());
    }

    #[test]
    fn test_windows_overlap_allows_touching_boundaries() {
        let window = |start: &str, end: &str| GradingPeriodWindow {
            start_date: parse_grading_period_date(start).unwrap(),
            end_date: parse_grading_period_date(end).unwrap(),
            close_date: parse_grading_period_date(end).unwrap(),
        };

        let first = window("2024-01-01", "2024-02-01");
        let second = window("2024-02-01", "2024-03-01");
        let overlapping = window("2024-01-15", "2024-02-15");

        assert!(!windows_overlap(&first, &second));
        assert!(!windows_overlap(&second, &first));
        assert!(windows_overlap(&first, &overlapping));
        assert!(windows_overlap(&overlapping, &second));
    }
}
