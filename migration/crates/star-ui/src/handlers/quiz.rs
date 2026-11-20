//! Quiz-taking journey handlers (issue #109, canvas-reference.md §8).
//!
//! Server-rendered Canvas `quizzes` journey: list, teacher new-quiz form,
//! show, take (all questions in ONE form), bulk submit with server-side
//! grading, and results. All routes are course-gated through
//! `star_courses::load_course_for_viewer` (404-before-403) — the shell cannot
//! bypass `/api/v1` authorization.
//!
//! Deviations from Canvas (documented in the PR):
//! - Canvas answers each question via `POST /quiz_submissions/:id/answer`
//!   (missing in star-quizzes); the shell bulk-submits the whole form to
//!   `POST .../submissions/{sid}/submit` and grades server-side in one go.
//! - Canvas shows a Due date from assignment overrides; star-quizzes has no
//!   `due_at` (due dates live in `learning_object_dates`), so the show page
//!   renders only Available / Until.
//! - The REST `list_course_quizzes` returns every quiz to any
//!   enrolled-or-admin caller; the UI layer additionally hides unpublished
//!   quizzes from students (Canvas index behavior).
//! - Question types with no phase-1 renderer (file upload, calculated,
//!   fill-in-multiple-blanks) render a "not renderable in phase 1" note and
//!   force the submission into `pending_review`.

use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_common::{ApiError, AppState};

use crate::handlers::error::from_api_error;
use crate::session::{check_csrf, ensure_csrf_cookie, SessionUser};
use crate::{assets, env::JsEnv, queries};

// ---------------------------------------------------------------------------
// View models
// ---------------------------------------------------------------------------

/// One answer option of a question (`quiz_questions.answers` JSON entry).
#[derive(Debug, Clone)]
pub struct AnswerView {
    pub id: i64,
    pub text: String,
    pub weight: f64,
}

/// A question rendered on the take page / results breakdown.
#[derive(Debug, Clone)]
pub struct QuestionView {
    pub id: i64,
    pub position: i32,
    pub name: String,
    pub question_type: String,
    pub text: String,
    pub points: f64,
    /// False for types with no phase-1 renderer (note is shown instead).
    pub renderable: bool,
    /// Answer options (radio/checkbox labels; left side of a match).
    pub answers: Vec<AnswerView>,
    /// Right-side options for matching/missing-word/multiple-dropdowns
    /// selects (phase 1: the same answers array, graded by answer_weight).
    pub select_options: Vec<AnswerView>,
}

/// One row of the quizzes index.
#[derive(Debug, Clone)]
pub struct QuizListView {
    pub id: i64,
    pub title: String,
    pub published: bool,
    pub points: f64,
    pub question_count: i64,
    /// "Available", "Not available until <date>", "Closed <date>"...
    pub availability: String,
    pub locked: bool,
}

/// One row of the teacher's submissions summary table.
#[derive(Debug, Clone)]
pub struct SubmissionRowView {
    pub id: i64,
    pub student_name: String,
    pub attempt: i32,
    pub score: Option<f64>,
    pub workflow_state: String,
    pub started_at: String,
}

/// Per-question grading outcome for the results page.
#[derive(Debug, Clone)]
pub struct ResultQuestionView {
    pub id: i64,
    pub name: String,
    pub question_type: String,
    pub text: String,
    pub points: f64,
    /// "correct" | "incorrect" | "manual" (needs a human grader).
    pub outcome: String,
    /// The student's answer, human readable.
    pub your_answer: String,
    /// The correct answer, human readable (auto-graded types only).
    pub correct_answer: Option<String>,
    pub score: Option<f64>,
}

// ---------------------------------------------------------------------------
// Queries (thin, shell-specific — mirroring the REST payloads)
// ---------------------------------------------------------------------------

const QUIZ_LIST_SQL: &str = r#"
    SELECT q.id, q.title, q.published, q.points_possible::float8 AS points,
           (SELECT COUNT(*) FROM quiz_questions qq
             WHERE qq.quiz_id = q.id AND qq.workflow_state <> 'deleted') AS question_count,
           q.available_from, q.available_until
    FROM quizzes q
    WHERE q.course_id = $1
    ORDER BY q.created_at ASC, q.id ASC
"#;

/// Row behind [`QUIZ_LIST_SQL`].
struct QuizListRow {
    id: i64,
    title: String,
    published: bool,
    points: f64,
    question_count: i64,
    available_from: Option<chrono::DateTime<chrono::Utc>>,
    available_until: Option<chrono::DateTime<chrono::Utc>>,
}

/// Load the course's quizzes (same ordering as the REST list handler).
async fn load_quiz_list(db: &PgPool, course_id: i64) -> Result<Vec<QuizListRow>, ApiError> {
    Ok(sqlx::query_as(QUIZ_LIST_SQL)
        .bind(course_id)
        .fetch_all(db)
        .await?)
}

/// Availability label for the index/show pages (Canvas shared/_available_dates
/// analogue; phase 1 has no due date — see module docs).
fn availability_label(
    from: &Option<chrono::DateTime<chrono::Utc>>,
    until: &Option<chrono::DateTime<chrono::Utc>>,
) -> (String, bool) {
    let now = chrono::Utc::now();
    let mut locked = false;
    let label = match (from, until) {
        (Some(from), Some(until)) if now < *from => {
            locked = true;
            format!("Not available until {}", format_ts(from))
        }
        (Some(_), Some(until)) if now > *until => {
            locked = true;
            format!("Closed {}", format_ts(until))
        }
        (Some(from), None) if now < *from => {
            locked = true;
            format!("Not available until {}", format_ts(from))
        }
        (None, Some(until)) if now > *until => {
            locked = true;
            format!("Closed {}", format_ts(until))
        }
        (Some(from), Some(until)) => format!("Available {} – {}", format_ts(from), format_ts(until)),
        (Some(from), None) => format!("Available {}", format_ts(from)),
        (None, Some(until)) => format!("Available until {}", format_ts(until)),
        (None, None) => "Always available".to_string(),
    };
    (label, locked)
}

/// Timestamp rendering for labels (Canvas' `strftime`-style short form).
fn format_ts(ts: &chrono::DateTime<chrono::Utc>) -> String {
    ts.format("%b %-d, %Y %H:%M").to_string()
}

/// Load one quiz (id, course check, fields the pages need).
struct QuizRow {
    id: i64,
    course_id: i64,
    title: String,
    description: Option<String>,
    quiz_type: String,
    published: bool,
    points_possible: f64,
    allowed_attempts: i32,
    available_from: Option<chrono::DateTime<chrono::Utc>>,
    available_until: Option<chrono::DateTime<chrono::Utc>>,
}

async fn load_quiz(db: &PgPool, quiz_id: i64) -> Result<Option<QuizRow>, ApiError> {
    Ok(sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, quiz_type, published,
               points_possible::float8 AS points_possible, allowed_attempts,
               available_from, available_until
        FROM quizzes WHERE id = $1
        "#,
    )
    .bind(quiz_id)
    .fetch_optional(db)
    .await?)
}

/// Load the quiz's questions in position order (same filter/order as the
/// REST question index), parsed into renderable view models.
async fn load_question_views(db: &PgPool, quiz_id: i64) -> Result<Vec<QuestionView>, ApiError> {
    let rows: Vec<(i64, i32, String, String, String, f64, String)> = sqlx::query_as(
        r#"
        SELECT id, position, question_name, question_type, question_text,
               points_possible::float8, answers
        FROM quiz_questions
        WHERE quiz_id = $1 AND workflow_state <> 'deleted'
        ORDER BY position, id
        "#,
    )
    .bind(quiz_id)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, position, name, question_type, text, points, answers)| {
            let parsed = parse_answers(&answers);
            let renderable = renderable_type(&question_type);
            QuestionView {
                id,
                position,
                name,
                question_type: question_type.clone(),
                text,
                points,
                renderable,
                // Phase 1: matching/missing-word selects draw their options
                // from the same answers array (see module docs).
                select_options: parsed.clone(),
                answers: parsed,
            }
        })
        .collect())
}

/// Parse the `quiz_questions.answers` JSON array (`{id, answer_text,
/// answer_weight}` entries; weight 100 marks the correct option).
fn parse_answers(raw: &str) -> Vec<AnswerView> {
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let id = entry.get("id").and_then(Value::as_i64)?;
            let text = entry
                .get("answer_text")
                .or_else(|| entry.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let weight = entry
                .get("answer_weight")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            Some(AnswerView { id, text, weight })
        })
        .collect()
}

/// Question types the take page can render in phase 1.
fn renderable_type(question_type: &str) -> bool {
    matches!(
        question_type,
        "multiple_choice_question"
            | "true_false_question"
            | "multiple_answers_question"
            | "short_answer_question"
            | "numerical_question"
            | "essay_question"
            | "matching_question"
            | "missing_word_question"
            | "multiple_dropdowns_question"
    )
}

/// The submission fields the journey needs (star-quizzes' QuizSubmissionRow
/// is handler-private, so the shell keeps its own minimal row).
struct SubmissionRow {
    id: i64,
    quiz_id: i64,
    user_id: i64,
    attempt: i32,
    score: Option<f64>,
    workflow_state: String,
    submission_data: String,
    started_at: chrono::DateTime<chrono::Utc>,
}

async fn load_submission(
    db: &PgPool,
    quiz_id: i64,
    submission_id: i64,
) -> Result<Option<SubmissionRow>, ApiError> {
    Ok(sqlx::query_as(
        r#"
        SELECT id, quiz_id, user_id, attempt, score::float8 AS score,
               workflow_state, submission_data, started_at
        FROM quiz_submissions
        WHERE id = $1 AND quiz_id = $2
        "#,
    )
    .bind(submission_id)
    .bind(quiz_id)
    .fetch_optional(db)
    .await?)
}

/// The caller's own submission for a quiz, if any.
async fn load_my_submission(
    db: &PgPool,
    quiz_id: i64,
    user_id: i64,
) -> Result<Option<SubmissionRow>, ApiError> {
    Ok(sqlx::query_as(
        r#"
        SELECT id, quiz_id, user_id, attempt, score::float8 AS score,
               workflow_state, submission_data, started_at
        FROM quiz_submissions
        WHERE quiz_id = $1 AND user_id = $2
        "#,
    )
    .bind(quiz_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?)
}

// ---------------------------------------------------------------------------
// Grading (phase 1; star-quizzes has no grading helpers — see module docs)
// ---------------------------------------------------------------------------

/// A student's decoded answer for one question.
#[derive(Debug, Clone)]
enum StudentAnswer {
    /// Radio / single select: the chosen answer id.
    Choice(i64),
    /// Checkbox (multiple answers): the checked answer ids.
    Choices(Vec<i64>),
    /// Text / essay / numerical: the raw text.
    Text(String),
    /// Per-blank selects (matching / missing word / dropdowns): blank answer
    /// id -> chosen option answer id.
    Blanks(Vec<(i64, i64)>),
    Unanswered,
}

/// Outcome of grading one question.
struct GradedQuestion {
    /// "correct" | "incorrect" | "manual"
    outcome: &'static str,
    /// Auto-earned points (None for manual questions).
    score: Option<f64>,
    /// Human-readable "your answer" for the results page.
    your_answer: String,
    /// Human-readable correct answer (auto-graded types only).
    correct_answer: Option<String>,
}

/// Grade one answered question against its answer keys
/// (`answer_weight == 100` marks correct options).
fn grade_question(question: &QuestionView, answer: &StudentAnswer) -> GradedQuestion {
    let correct_ids: Vec<i64> = question
        .answers
        .iter()
        .filter(|a| a.weight == 100.0)
        .map(|a| a.id)
        .collect();
    let answer_text = |id: i64| {
        question
            .answers
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.text.clone())
            .unwrap_or_else(|| id.to_string())
    };

    match (&*question.question_type, answer) {
        // Manual grading: essays and unrenderable types never auto-score.
        ("essay_question", _) => GradedQuestion {
            outcome: "manual",
            score: None,
            your_answer: match answer {
                StudentAnswer::Text(text) if !text.trim().is_empty() => text.clone(),
                _ => "No answer".to_string(),
            },
            correct_answer: None,
        },
        (qtype, _) if !renderable_type(qtype) => GradedQuestion {
            outcome: "manual",
            score: None,
            your_answer: "Not renderable in phase 1 (manual review required)".to_string(),
            correct_answer: None,
        },
        // Single choice: the selected option is the weight-100 one.
        ("multiple_choice_question", StudentAnswer::Choice(id))
        | ("true_false_question", StudentAnswer::Choice(id)) => {
            let correct = correct_ids.contains(id);
            GradedQuestion {
                outcome: if correct { "correct" } else { "incorrect" },
                score: Some(if correct { question.points } else { 0.0 }),
                your_answer: answer_text(*id),
                correct_answer: correct_ids.first().map(|id| answer_text(*id)),
            }
        }
        // Multiple answers: all-or-nothing on the exact checked set.
        ("multiple_answers_question", StudentAnswer::Choices(ids)) => {
            let mut selected = ids.clone();
            selected.sort_unstable();
            selected.dedup();
            let mut keys = correct_ids.clone();
            keys.sort_unstable();
            let correct = selected == keys;
            GradedQuestion {
                outcome: if correct { "correct" } else { "incorrect" },
                score: Some(if correct { question.points } else { 0.0 }),
                your_answer: if selected.is_empty() {
                    "No answer".to_string()
                } else {
                    selected.iter().map(|id| answer_text(*id)).collect::<Vec<_>>().join(", ")
                },
                correct_answer: Some(
                    keys.iter().map(|id| answer_text(*id)).collect::<Vec<_>>().join(", "),
                ),
            }
        }
        // Short answer: exact, trimmed text match against weight-100 texts.
        ("short_answer_question", StudentAnswer::Text(text)) => {
            let keys: Vec<&str> = question
                .answers
                .iter()
                .filter(|a| a.weight == 100.0)
                .map(|a| a.text.trim())
                .collect();
            let correct = !keys.is_empty() && keys.contains(&text.trim());
            GradedQuestion {
                outcome: if correct { "correct" } else { "incorrect" },
                score: Some(if correct { question.points } else { 0.0 }),
                your_answer: if text.trim().is_empty() {
                    "No answer".to_string()
                } else {
                    text.clone()
                },
                correct_answer: correct_ids.first().map(|id| answer_text(*id)),
            }
        }
        // Numerical: trimmed numeric equality against weight-100 values.
        ("numerical_question", StudentAnswer::Text(text)) => {
            let value = text.trim().parse::<f64>().ok();
            let correct = value.is_some_and(|v| {
                question.answers.iter().filter(|a| a.weight == 100.0).any(|a| {
                    a.text.trim().parse::<f64>().is_ok_and(|key| (key - v).abs() < f64::EPSILON)
                })
            });
            GradedQuestion {
                outcome: if correct { "correct" } else { "incorrect" },
                score: Some(if correct { question.points } else { 0.0 }),
                your_answer: if text.trim().is_empty() {
                    "No answer".to_string()
                } else {
                    text.clone()
                },
                correct_answer: correct_ids.first().map(|id| answer_text(*id)),
            }
        }
        // Per-blank selects: every blank must land on a weight-100 option.
        ("matching_question", StudentAnswer::Blanks(blanks))
        | ("missing_word_question", StudentAnswer::Blanks(blanks))
        | ("multiple_dropdowns_question", StudentAnswer::Blanks(blanks)) => {
            let correct = !blanks.is_empty()
                && blanks.iter().all(|(_, chosen)| correct_ids.contains(chosen));
            GradedQuestion {
                outcome: if correct { "correct" } else { "incorrect" },
                score: Some(if correct { question.points } else { 0.0 }),
                your_answer: if blanks.is_empty() {
                    "No answer".to_string()
                } else {
                    blanks
                        .iter()
                        .map(|(blank, chosen)| {
                            format!("{} → {}", answer_text(*blank), answer_text(*chosen))
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                },
                correct_answer: Some(
                    question
                        .answers
                        .iter()
                        .filter(|a| a.weight == 100.0)
                        .map(|a| a.text.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            }
        }
        // Renderable type but the student left it blank (or a malformed
        // field arrived): wrong, except we cannot distinguish "blank text"
        // from "not answered" — both score 0.
        (_, answer) => GradedQuestion {
            outcome: "incorrect",
            score: Some(0.0),
            your_answer: match answer {
                StudentAnswer::Text(text) if !text.trim().is_empty() => text.clone(),
                _ => "No answer".to_string(),
            },
            correct_answer: correct_ids.first().map(|id| answer_text(*id)),
        },
    }
}

/// Decode the raw take-form pairs into per-question answers.
///
/// DOM contract (canvas-reference.md §8): radio `question_<id>`; checkbox
/// `question_<id>_answer_<aid>` (with a hidden 0); text/essay
/// `question_<id>`; matching/missing-word/dropdown selects
/// `question_<id>_answer_<aid>`. Field names are dynamic, so the pairs are
/// decoded with `url::form_urlencoded` rather than a typed form struct.
fn decode_answers(
    pairs: &[(String, String)],
    questions: &[QuestionView],
) -> Vec<(i64, StudentAnswer)> {
    let get = |name: &str| -> Option<&str> {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };

    questions
        .iter()
        .map(|question| {
            let id = question.id;
            let base = format!("question_{id}");
            let answer = match &*question.question_type {
                "multiple_choice_question" | "true_false_question" => match get(&base) {
                    Some(value) => value.parse::<i64>().ok().map(StudentAnswer::Choice),
                    None => None,
                },
                "multiple_answers_question" => {
                    let checked: Vec<i64> = question
                        .answers
                        .iter()
                        .filter(|a| {
                            get(&format!("{base}_answer_{}", a.id))
                                .is_some_and(|v| v != "0")
                        })
                        .map(|a| a.id)
                        .collect();
                    Some(StudentAnswer::Choices(checked))
                }
                "short_answer_question" | "numerical_question" | "essay_question" => {
                    get(&base).map(|v| StudentAnswer::Text(v.to_string()))
                }
                "matching_question" | "missing_word_question" | "multiple_dropdowns_question" => {
                    let blanks: Vec<(i64, i64)> = question
                        .answers
                        .iter()
                        .filter_map(|a| {
                            get(&format!("{base}_answer_{}", a.id))
                                .and_then(|v| v.parse::<i64>().ok())
                                .map(|chosen| (a.id, chosen))
                        })
                        .collect();
                    Some(StudentAnswer::Blanks(blanks))
                }
                _ => None,
            }
            .unwrap_or(StudentAnswer::Unanswered);
            (id, answer)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Shared shell plumbing
// ---------------------------------------------------------------------------

/// Build the AuthUser view of a session user for the domain-crate loaders.
fn auth_of(user: &SessionUser) -> star_auth::AuthUser {
    star_auth::AuthUser {
        id: user.id,
        role: user.role.clone(),
    }
}

/// Common course-page context (gates + tabs + ENV + CSRF), shared by every
/// quiz handler. `Err` carries the error-page response.
struct CourseContext {
    course_name: String,
    course_id: i64,
    user_name: String,
    tabs: Vec<star_courses::CourseTab>,
    env: JsEnv,
    csrf_token: String,
    jar: CookieJar,
}

async fn course_context(
    app: &AppState,
    course_id: i64,
    user: &SessionUser,
    jar: CookieJar,
    active_tab: &str,
) -> Result<CourseContext, Response> {
    let origin = crate::request_origin();

    let course: star_courses::Course =
        match star_courses::load_course_for_viewer(&app.db, course_id, &auth_of(user)).await {
            Ok(course) => course,
            Err(e) => return Err(from_api_error(&e, &origin).into_response()),
        };

    let tabs = match star_courses::load_course_tabs(&app.db, course_id, &auth_of(user)).await {
        Ok(tabs) => tabs,
        Err(e) => return Err(from_api_error(&e, &origin).into_response()),
    };

    let (preferences, use_high_contrast) = match queries::load_preferences(&app.db, user.id).await {
        Ok(loaded) => loaded,
        Err(e) => return Err(from_api_error(&e, &origin).into_response()),
    };
    let user_name = match queries::load_user_name(&app.db, user.id).await {
        Ok(name) => name.unwrap_or_default(),
        Err(e) => return Err(from_api_error(&e, &origin).into_response()),
    };

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        preferences,
        use_high_contrast,
        &origin,
        Some(active_tab),
        &context_asset_string,
        Vec::new(),
    );
    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    Ok(CourseContext {
        course_name: course.name,
        course_id,
        user_name,
        tabs,
        env,
        csrf_token,
        jar,
    })
}

/// Render an HTML page with the shell content type.
fn html_response(template_result: Result<String, askama::Error>, page: &str) -> Response {
    match template_result {
        Ok(html) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html,
        )
            .into_response(),
        Err(err) => {
            ApiError::Db(format!("{page} template render failed: {err}")).into_response()
        }
    }
}

/// Load the quiz and verify it belongs to the path course (404 otherwise,
/// mirroring the REST nested find).
async fn quiz_in_course(
    db: &PgPool,
    course_id: i64,
    quiz_id: i64,
) -> Result<QuizRow, Response> {
    match load_quiz(db, quiz_id).await {
        Ok(Some(quiz)) if quiz.course_id == course_id => Ok(quiz),
        Ok(_) => Err(from_api_error(&ApiError::NotFound, &crate::request_origin()).into_response()),
        Err(e) => Err(from_api_error(&e, &crate::request_origin()).into_response()),
    }
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// GET /courses/{id}/quizzes — the index page.
#[derive(Template)]
#[template(path = "quizzes.html")]
pub struct QuizzesPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub quizzes: Vec<QuizListView>,
    pub is_teacher: bool,
}

/// GET /courses/{id}/quizzes/new — the teacher-only creation form.
#[derive(Template)]
#[template(path = "quiz_new.html")]
pub struct QuizNewPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub error: String,
}

/// GET /courses/{id}/quizzes/{qid} — the show page (both roles).
#[derive(Template)]
#[template(path = "quiz_show.html")]
pub struct QuizShowPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub quiz_id: i64,
    pub quiz_title: String,
    pub quiz_type: String,
    pub instructions_html: String,
    pub published: bool,
    pub points: f64,
    pub question_count: i64,
    pub availability: String,
    pub locked: bool,
    pub allowed_attempts: i32,
    pub is_teacher: bool,
    pub take_url: String,
    pub can_take: bool,
    /// The student's existing submission, when one exists.
    pub has_submission: bool,
    pub submission_state: String,
    pub submission_score: Option<f64>,
    pub results_url: Option<String>,
    pub submissions: Vec<SubmissionRowView>,
}

/// GET /courses/{id}/quizzes/{qid}/take — the one-form take page.
#[derive(Template)]
#[template(path = "quiz_take.html")]
pub struct QuizTakePage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub quiz_id: i64,
    pub submission_id: i64,
    pub quiz_title: String,
    pub instructions_html: String,
    pub questions: Vec<QuestionView>,
    pub submit_url: String,
}

/// GET /courses/{id}/quizzes/{qid}/submissions/{sid} — the results page.
#[derive(Template)]
#[template(path = "quiz_results.html")]
pub struct QuizResultsPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub quiz_id: i64,
    pub quiz_title: String,
    pub student_name: String,
    pub attempt: i32,
    pub score: Option<f64>,
    pub points_possible: f64,
    pub workflow_state: String,
    pub questions: Vec<ResultQuestionView>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{id}/quizzes — quiz index.
///
/// Students see published quizzes only (the REST list returns everything to
/// any enrolled user; Canvas' HTML index filters — documented deviation).
/// Teachers additionally get the "+ Quiz" creation link.
pub async fn quiz_list(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let ctx = match course_context(&app, course_id, &user, jar, "quizzes").await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let is_teacher =
        star_courses::check_teacher_or_admin(&app.db, user.id, course_id, &auth_of(&user))
            .await
            .unwrap_or(false);

    let rows = match load_quiz_list(&app.db, course_id).await {
        Ok(rows) => rows,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    let quizzes: Vec<QuizListView> = rows
        .into_iter()
        .filter(|row| is_teacher || row.published)
        .map(|row| {
            let (availability, locked) =
                availability_label(&row.available_from, &row.available_until);
            QuizListView {
                id: row.id,
                title: row.title,
                published: row.published,
                points: row.points,
                question_count: row.question_count,
                availability,
                locked,
            }
        })
        .collect();

    let page = QuizzesPage {
        title: format!("{} - Quizzes", ctx.course_name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} quizzes"
        ),
        css_url: assets::app_css_url(),
        env_json: ctx.env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (
                ctx.course_name.clone(),
                format!("/courses/{course_id}"),
            ),
        ],
        csrf_token: ctx.csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "quizzes".to_string(),
        course_name: ctx.course_name,
        course_id,
        quizzes,
        is_teacher,
    };
    (ctx.jar, html_response(page.render(), "quiz list")).into_response()
}

/// GET /courses/{id}/quizzes/new — teacher-only minimal creation form.
///
/// Phase 1 has NO question editor: questions are added through the seed or
/// the REST question endpoints (documented deviation).
pub async fn quiz_new(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let ctx = match course_context(&app, course_id, &user, jar, "quizzes").await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let is_teacher = match star_courses::check_teacher_or_admin(
        &app.db,
        user.id,
        course_id,
        &auth_of(&user),
    )
    .await
    {
        Ok(teacher) => teacher,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };
    if !is_teacher {
        return from_api_error(&ApiError::Forbidden, &crate::request_origin()).into_response();
    }

    let page = QuizNewPage {
        title: format!("{} - New Quiz", ctx.course_name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} quizzes"
        ),
        css_url: assets::app_css_url(),
        env_json: ctx.env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course_name.clone(), format!("/courses/{course_id}")),
            ("Quizzes".to_string(), format!("/courses/{course_id}/quizzes")),
        ],
        csrf_token: ctx.csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "quizzes".to_string(),
        course_name: ctx.course_name,
        course_id,
        error: String::new(),
    };
    (ctx.jar, html_response(page.render(), "quiz new")).into_response()
}

#[derive(Deserialize)]
pub struct QuizNewForm {
    pub _csrf: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub points_possible: String,
    #[serde(default)]
    pub available_from: String,
    #[serde(default)]
    pub available_until: String,
    #[serde(default)]
    pub published: String,
}

/// POST /courses/{id}/quizzes/new — create from the minimal form (teacher
/// only). Mirrors the REST create's validation (non-blank title, default
/// points 0) and redirects 303 to the new quiz's show page.
pub async fn quiz_create(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<QuizNewForm>,
) -> Response {
    if !check_csrf(&jar, &form._csrf) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
        )
            .into_response();
    }

    let is_teacher = match star_courses::check_teacher_or_admin(
        &app.db,
        user.id,
        course_id,
        &auth_of(&user),
    )
    .await
    {
        Ok(teacher) => teacher,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };
    if !is_teacher {
        return from_api_error(&ApiError::Forbidden, &crate::request_origin()).into_response();
    }

    // Same validation as the REST create handler.
    if form.title.trim().is_empty() {
        let ctx = match course_context(&app, course_id, &user, jar, "quizzes").await {
            Ok(ctx) => ctx,
            Err(response) => return response,
        };        let page = QuizNewPage {
            title: format!("{} - New Quiz", ctx.course_name),
            body_class: format!(
                "with-left-side course-menu-expanded context-course_{course_id} quizzes"
            ),
            css_url: assets::app_css_url(),
            env_json: ctx.env.to_json_string(),
            notices: Vec::new(),
            user_name: ctx.user_name,
            active_path: "courses",
            breadcrumb: vec![
                ("Dashboard".to_string(), "/dashboard".to_string()),
                (ctx.course_name.clone(), format!("/courses/{course_id}")),
                ("Quizzes".to_string(), format!("/courses/{course_id}/quizzes")),
            ],
            csrf_token: ctx.csrf_token,
            tabs: ctx.tabs,
            active_context_tab: "quizzes".to_string(),
            course_name: ctx.course_name,
            course_id,
            error: "Title is required.".to_string(),
        };
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            html_response(page.render(), "quiz new"),
        )
            .into_response();
    }

    let points: f64 = form.points_possible.trim().parse().unwrap_or(0.0);
    let parse_ts = |raw: &str| -> Option<chrono::DateTime<chrono::Utc>> {
        if raw.trim().is_empty() {
            return None;
        }
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(raw.trim()) {
            return Some(ts.with_timezone(&chrono::Utc));
        }
        // datetime-local form values carry no offset; treat them as UTC.
        chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%dT%H:%M")
            .ok()
            .map(|naive| naive.and_utc())
    };
    let published = matches!(form.published.as_str(), "true" | "1" | "on" | "yes");

    let new_id: i64 = match sqlx::query_scalar(
        r#"
        INSERT INTO quizzes
            (course_id, title, description, quiz_type, points_possible,
             published, available_from, available_until)
        VALUES ($1, $2, NULLIF($3, ''), 'assignment', $4, $5, $6, $7)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(form.title.trim())
    .bind(form.description.trim())
    .bind(points)
    .bind(published)
    .bind(parse_ts(&form.available_from))
    .bind(parse_ts(&form.available_until))
    .fetch_one(&app.db)
    .await
    {
        Ok(id) => id,
        Err(e) => return from_api_error(&ApiError::from(e), &crate::request_origin()).into_response(),
    };

    (
        StatusCode::SEE_OTHER,
        [("location", format!("/courses/{course_id}/quizzes/{new_id}").as_str())],
    )
        .into_response()
}

/// GET /courses/{id}/quizzes/{qid} — the show page.
///
/// Students: quiz details + "Take Quiz" (or results link when already
/// submitted). Teachers: details + the submissions summary table.
pub async fn quiz_show(
    State(app): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let ctx = match course_context(&app, course_id, &user, jar, "quizzes").await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let quiz = match quiz_in_course(&app.db, course_id, quiz_id).await {
        Ok(quiz) => quiz,
        Err(response) => return response,
    };

    let is_teacher = match star_courses::check_teacher_or_admin(
        &app.db,
        user.id,
        course_id,
        &auth_of(&user),
    )
    .await
    {
        Ok(teacher) => teacher,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    // Students cannot open unpublished quizzes (404, not 403 — matches the
    // course gate's 404-before-403 posture).
    if !is_teacher && !quiz.published {
        return from_api_error(&ApiError::NotFound, &crate::request_origin()).into_response();
    }

    let question_count = match load_question_views(&app.db, quiz_id).await {
        Ok(questions) => questions.len() as i64,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    let (availability, locked) = availability_label(&quiz.available_from, &quiz.available_until);

    let take_url = format!("/courses/{course_id}/quizzes/{quiz_id}/take");

    let (has_submission, submission_state, submission_score, results_url, can_take, submissions) =
        if is_teacher {
            let rows = match load_submission_rows(&app.db, quiz_id).await {
                Ok(rows) => rows,
                Err(e) => {
                    return from_api_error(&e, &crate::request_origin()).into_response()
                }
            };
            let submissions: Vec<SubmissionRowView> = rows
                .into_iter()
                .map(|row| SubmissionRowView {
                    id: row.id,
                    student_name: row.student_name,
                    attempt: row.attempt,
                    score: row.score,
                    workflow_state: row.workflow_state,
                    started_at: format_ts(&row.started_at),
                })
                .collect();
            (false, String::new(), None, None, false, submissions)
        } else {
            let mine = match load_my_submission(&app.db, quiz_id, user.id).await {
                Ok(row) => row,
                Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
            };
            match mine {
                Some(sub) => (
                    true,
                    sub.workflow_state.clone(),
                    sub.score,
                    Some(format!(
                        "/courses/{course_id}/quizzes/{quiz_id}/submissions/{}",
                        sub.id
                    )),
                    false,
                    Vec::new(),
                ),
                None => (false, String::new(), None, None, !locked, Vec::new()),
            }
        };

    let page = QuizShowPage {
        title: format!("{} - {}", ctx.course_name, quiz.title),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} quizzes"
        ),
        css_url: assets::app_css_url(),
        env_json: ctx.env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course_name.clone(), format!("/courses/{course_id}")),
            ("Quizzes".to_string(), format!("/courses/{course_id}/quizzes")),
        ],
        csrf_token: ctx.csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "quizzes".to_string(),
        course_name: ctx.course_name,
        course_id,
        quiz_id,
        quiz_title: quiz.title,
        quiz_type: quiz.quiz_type,
        instructions_html: quiz.description.unwrap_or_default(),
        published: quiz.published,
        points: quiz.points_possible,
        question_count,
        availability,
        locked,
        allowed_attempts: quiz.allowed_attempts,
        is_teacher,
        take_url,
        can_take,
        has_submission,
        submission_state,
        submission_score,
        results_url,
        submissions,
    };
    (ctx.jar, html_response(page.render(), "quiz show")).into_response()
}

/// Teacher's submissions summary row.
struct SubmissionWithUserRow {
    id: i64,
    student_name: String,
    attempt: i32,
    score: Option<f64>,
    workflow_state: String,
    started_at: chrono::DateTime<chrono::Utc>,
}

async fn load_submission_rows(
    db: &PgPool,
    quiz_id: i64,
) -> Result<Vec<SubmissionWithUserRow>, ApiError> {
    Ok(sqlx::query_as(
        r#"
        SELECT qs.id, u.name AS student_name, qs.attempt,
               qs.score::float8 AS score, qs.workflow_state, qs.started_at
        FROM quiz_submissions qs
        JOIN users u ON u.id = qs.user_id
        WHERE qs.quiz_id = $1
        ORDER BY qs.started_at DESC
        "#,
    )
    .bind(quiz_id)
    .fetch_all(db)
    .await?)
}

#[derive(Deserialize)]
pub struct TakeStartForm {
    #[allow(dead_code)]
    pub _csrf: String,
}

/// POST /courses/{id}/quizzes/{qid}/take — start (or resume) the attempt and
/// land on the take page (Canvas' `link_to_take_or_retake_poll` flow).
///
/// Students only (star-quizzes' gate: admins get no bypass), redirected to
/// the results page when their submission is already complete.
pub async fn quiz_take_start(
    State(app): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<TakeStartForm>,
) -> Response {
    if !check_csrf(&jar, &form._csrf) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
        )
            .into_response();
    }

    if let Err(response) = quiz_in_course(&app.db, course_id, quiz_id).await {
        return response;
    }

    let submission = match star_quizzes::start_or_resume_submission(&app.db, quiz_id, user.id).await
    {
        Ok(submission) => submission,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    (
        StatusCode::SEE_OTHER,
        [(
            "location",
            format!("/courses/{course_id}/quizzes/{quiz_id}/take").as_str(),
        )],
    )
        .into_response()
}

/// GET /courses/{id}/quizzes/{qid}/take — render every question in ONE form.
///
/// Requires an in-progress (own) submission: no submission means the student
/// must go through POST take (Canvas' start-then-take flow); a completed
/// submission redirects to the results page.
pub async fn quiz_take(
    State(app): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let ctx = match course_context(&app, course_id, &user, jar, "quizzes").await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let quiz = match quiz_in_course(&app.db, course_id, quiz_id).await {
        Ok(quiz) => quiz,
        Err(response) => return response,
    };

    let submission = match load_my_submission(&app.db, quiz_id, user.id).await {
        Ok(Some(sub)) => sub,
        Ok(None) => {
            // No attempt yet: send the student back to the show page, whose
            // "Take Quiz" button POSTs start (no GET-start loop).
            (
                StatusCode::SEE_OTHER,
                [(
                    "location",
                    format!("/courses/{course_id}/quizzes/{quiz_id}").as_str(),
                )],
            )
                .into_response()
        }
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    if matches!(submission.workflow_state.as_str(), "complete" | "pending_review") {
        return (
            StatusCode::SEE_OTHER,
            [(
                "location",
                format!(
                    "/courses/{course_id}/quizzes/{quiz_id}/submissions/{}",
                    submission.id
                )
                .as_str(),
            )],
        )
            .into_response();
    }

    let questions = match load_question_views(&app.db, quiz_id).await {
        Ok(questions) => questions,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    let page = QuizTakePage {
        title: format!("{} - Take: {}", ctx.course_name, quiz.title),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} quizzes"
        ),
        css_url: assets::app_css_url(),
        env_json: ctx.env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course_name.clone(), format!("/courses/{course_id}")),
            ("Quizzes".to_string(), format!("/courses/{course_id}/quizzes")),
            (quiz.title.clone(), format!("/courses/{course_id}/quizzes/{quiz_id}")),
        ],
        csrf_token: ctx.csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "quizzes".to_string(),
        course_name: ctx.course_name,
        course_id,
        quiz_id,
        submission_id: submission.id,
        quiz_title: quiz.title,
        instructions_html: quiz.description.unwrap_or_default(),
        questions,
        submit_url: format!(
            "/courses/{course_id}/quizzes/{quiz_id}/submissions/{}/submit",
            submission.id
        ),
    };
    (ctx.jar, html_response(page.render(), "quiz take")).into_response()
}

/// POST /courses/{id}/quizzes/{qid}/submissions/{sid}/submit — bulk submit.
///
/// The Canvas API's `POST /quiz_submissions/:id/answer` is not implemented in
/// star-quizzes, so the shell grades the whole form server-side (documented
/// deviation): answers are recorded in `submission_data.questions`, then the
/// submission flips to `complete` (with score) or `pending_review` (essay or
/// unrenderable questions present), mirroring `complete_quiz_submission`'s
/// timestamp stamps. 303 to the results page.
pub async fn quiz_submit(
    State(app): State<AppState>,
    Path((course_id, quiz_id, submission_id)): Path<(i64, i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
    body: Bytes,
) -> Response {
    // CSRF double-submit on the raw body (field names are dynamic).
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(&body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let form_token = pairs
        .iter()
        .find(|(key, _)| key == "_csrf")
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    if !check_csrf(&jar, &form_token) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
        )
            .into_response();
    }

    if let Err(response) = quiz_in_course(&app.db, course_id, quiz_id).await {
        return response;
    }

    let submission = match load_submission(&app.db, quiz_id, submission_id).await {
        Ok(Some(sub)) => sub,
        Ok(None) => {
            return from_api_error(&ApiError::NotFound, &crate::request_origin()).into_response()
        }
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    // Only the owning student may submit (Canvas quiz_submission_allowed?).
    if submission.user_id != user.id {
        return from_api_error(&ApiError::Forbidden, &crate::request_origin()).into_response();
    }

    // One shot per attempt: submitting a finished attempt goes to results.
    if matches!(submission.workflow_state.as_str(), "complete" | "pending_review") {
        return (
            StatusCode::SEE_OTHER,
            [(
                "location",
                format!(
                    "/courses/{course_id}/quizzes/{quiz_id}/submissions/{submission_id}"
                )
                .as_str(),
            )],
        )
            .into_response();
    }

    let questions = match load_question_views(&app.db, quiz_id).await {
        Ok(questions) => questions,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    let answers = decode_answers(&pairs, &questions);

    // Record answers + grade in one pass.
    let mut questions_data = Map::new();
    let mut total_score = 0.0;
    let mut needs_review = false;
    for (question, (_, answer)) in questions.iter().zip(answers.iter()) {
        let graded = grade_question(question, answer);
        let mut entry = Map::new();
        entry.insert("answer".to_string(), student_answer_json(answer));
        match graded.score {
            Some(score) => {
                entry.insert("score".to_string(), json!(score));
                total_score += score;
            }
            None => needs_review = true,
        }
        questions_data.insert(question.id.to_string(), Value::Object(entry));
    }

    let mut submission_data = serde_json::from_str::<Value>(&submission.submission_data)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    submission_data.insert("questions".to_string(), Value::Object(questions_data));
    let submission_data_json = Value::Object(submission_data).to_string();

    let workflow_state = if needs_review { "pending_review" } else { "complete" };
    // Partial score: the auto-graded portion is stored even while essay /
    // unrenderable questions await manual review (Canvas shows the running
    // total; `pending_review` marks the attempt as not final).
    let score = total_score;

    if let Err(e) = sqlx::query(
        r#"
        UPDATE quiz_submissions
        SET submission_data = $2,
            workflow_state = $3,
            score = $4,
            kept_score = $4,
            completed_at = now(),
            finished_at = now(),
            end_at = COALESCE(end_at, now()),
            time_spent = EXTRACT(EPOCH FROM (now() - started_at))::int
                + COALESCE(extra_time, 0),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(submission_id)
    .bind(submission_data_json)
    .bind(workflow_state)
    .bind(score)
    .execute(&app.db)
    .await
    {
        return from_api_error(&ApiError::from(e), &crate::request_origin()).into_response();
    }

    (
        StatusCode::SEE_OTHER,
        [(
            "location",
            format!("/courses/{course_id}/quizzes/{quiz_id}/submissions/{submission_id}").as_str(),
        )],
    )
        .into_response()
}

/// JSON snapshot of a student's answer for `submission_data.questions`.
fn student_answer_json(answer: &StudentAnswer) -> Value {
    match answer {
        StudentAnswer::Choice(id) => json!({ "answer_id": id }),
        StudentAnswer::Choices(ids) => json!({ "answer_ids": ids }),
        StudentAnswer::Text(text) => json!({ "text": text }),
        StudentAnswer::Blanks(blanks) => {
            json!({ "blanks": blanks.iter().map(|(b, c)| json!({"answer_id": b, "chosen": c})).collect::<Vec<_>>() })
        }
        StudentAnswer::Unanswered => Value::Null,
    }
}

/// GET /courses/{id}/quizzes/{qid}/submissions/{sid} — results (owner or
/// teacher): "Score for this attempt: X / Y" plus the per-question breakdown.
pub async fn quiz_results(
    State(app): State<AppState>,
    Path((course_id, quiz_id, submission_id)): Path<(i64, i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let ctx = match course_context(&app, course_id, &user, jar, "quizzes").await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let quiz = match quiz_in_course(&app.db, course_id, quiz_id).await {
        Ok(quiz) => quiz,
        Err(response) => return response,
    };

    let submission = match load_submission(&app.db, quiz_id, submission_id).await {
        Ok(Some(sub)) => sub,
        Ok(None) => {
            return from_api_error(&ApiError::NotFound, &crate::request_origin()).into_response()
        }
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    let is_teacher = match star_courses::check_teacher_or_admin(
        &app.db,
        user.id,
        course_id,
        &auth_of(&user),
    )
    .await
    {
        Ok(teacher) => teacher,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };
    if !is_teacher && submission.user_id != user.id {
        return from_api_error(&ApiError::Forbidden, &crate::request_origin()).into_response();
    }

    let student_name: Option<String> = sqlx::query_scalar("SELECT name FROM users WHERE id = $1")
        .bind(submission.user_id)
        .fetch_optional(&app.db)
        .await
        .ok()
        .flatten();

    let questions = match load_question_views(&app.db, quiz_id).await {
        Ok(questions) => questions,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };

    // Rebuild the per-question breakdown from the recorded answers.
    let stored: Value = serde_json::from_str(&submission.submission_data).unwrap_or(json!({}));
    let empty = Map::new();
    let stored_questions = stored.get("questions").and_then(Value::as_object).unwrap_or(&empty);

    let mut breakdown = Vec::with_capacity(questions.len());
    for question in &questions {
        let stored_answer = stored_questions.get(&question.id.to_string());
        let answer = stored_answer.map(decode_stored_answer).unwrap_or(StudentAnswer::Unanswered);
        let graded = grade_question(question, &answer);
        let stored_score = stored_answer
            .and_then(|entry| entry.get("score"))
            .and_then(Value::as_f64);
        breakdown.push(ResultQuestionView {
            id: question.id,
            name: question.name.clone(),
            question_type: question.question_type.clone(),
            text: question.text.clone(),
            points: question.points,
            outcome: graded.outcome.to_string(),
            your_answer: graded.your_answer,
            correct_answer: graded.correct_answer,
            score: stored_score.or(graded.score),
        });
    }

    let page = QuizResultsPage {
        title: format!("{} - Results: {}", ctx.course_name, quiz.title),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} quizzes"
        ),
        css_url: assets::app_css_url(),
        env_json: ctx.env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course_name.clone(), format!("/courses/{course_id}")),
            ("Quizzes".to_string(), format!("/courses/{course_id}/quizzes")),
            (quiz.title.clone(), format!("/courses/{course_id}/quizzes/{quiz_id}")),
        ],
        csrf_token: ctx.csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "quizzes".to_string(),
        course_name: ctx.course_name,
        course_id,
        quiz_id,
        quiz_title: quiz.title,
        student_name: student_name.unwrap_or_else(|| "Student".to_string()),
        attempt: submission.attempt,
        score: submission.score,
        points_possible: quiz.points_possible,
        workflow_state: submission.workflow_state,
        questions: breakdown,
    };
    (ctx.jar, html_response(page.render(), "quiz results")).into_response()
}

/// Rehydrate a `StudentAnswer` from the stored `submission_data` snapshot.
fn decode_stored_answer(entry: &Value) -> StudentAnswer {
    if let Some(id) = entry.get("answer_id").and_then(Value::as_i64) {
        return StudentAnswer::Choice(id);
    }
    if let Some(ids) = entry.get("answer_ids").and_then(Value::as_array) {
        return StudentAnswer::Choices(
            ids.iter().filter_map(Value::as_i64).collect::<Vec<_>>(),
        );
    }
    if let Some(text) = entry.get("text").and_then(Value::as_str) {
        return StudentAnswer::Text(text.to_string());
    }
    if let Some(blanks) = entry.get("blanks").and_then(Value::as_array) {
        let pairs = blanks
            .iter()
            .filter_map(|blank| {
                let from = blank.get("answer_id").and_then(Value::as_i64)?;
                let to = blank.get("chosen").and_then(Value::as_i64)?;
                Some((from, to))
            })
            .collect::<Vec<_>>();
        return StudentAnswer::Blanks(pairs);
    }
    StudentAnswer::Unanswered
}

