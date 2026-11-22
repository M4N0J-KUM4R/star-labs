//! Canvas quiz submission questions API — port of
//! `canvas-src/app/controllers/quizzes/quiz_submission_questions_controller.rb`
//! together with `canvas-src/lib/api/v1/quiz_submission_question.rb` and the
//! `Quizzes::QuizQuestion::AnswerSerializers` family
//! (`canvas-src/app/models/quizzes/quiz_question/answer_serializers/*.rb`).
//!
//! Routes served here (the coordinator mounts this router under `/api/v1`):
//!
//! | Method | Path                                                                  | Canvas action        |
//! |--------|-----------------------------------------------------------------------|----------------------|
//! | GET    | `/quiz_submissions/{quiz_submission_id}/questions`                       | `index`              |
//! | POST   | `/quiz_submissions/{quiz_submission_id}/questions`                       | `answer`             |
//! | GET    | `/quiz_submissions/{quiz_submission_id}/questions/{id}/formatted_answer` | `formatted_answer`   |
//! | GET    | `/quiz_submissions/{quiz_submission_id}/questions/{id}`                  | `show`               |
//! | PUT    | `/quiz_submissions/{quiz_submission_id}/questions/{id}/flag`             | `flag`               |
//! | PUT    | `/quiz_submissions/{quiz_submission_id}/questions/{id}/unflag`           | `unflag`             |
//!
//! Wiring (coordinator): declare `pub mod quiz_submission_questions_api;` in
//! `src/lib.rs` and merge `quiz_submission_questions_api::router()` into the
//! crate router — the integration test builds its server from that router.
//!
//! Dependency (coordinator): the crate needs `md-5 = { workspace = true }` —
//! Canvas stores the answer of a blank under the MD5 digest of the blank name
//! (`AssessmentQuestion.variable_id`).
//!
//! Faithfulness notes:
//!
//! * The controller's `before_action` chain is reproduced in its order:
//!   `require_quiz_submission` (404), `require_question` (404), the lockdown
//!   browser check (403) and only then the action itself. A missing resource is
//!   therefore always a 404 *before* a missing permission is a 403.
//! * `reject!` raises a `RequestError` in Canvas, which the API renders as
//!   `{"status": "<rails symbol>", "message": "..."}` with the requested HTTP
//!   status. The 404 of a missing record is Canvas'
//!   `{"errors": [{"message": "The specified resource does not exist."}]}` and
//!   the 403 of `authorized_action` is
//!   `{"status": "unauthorized", "errors": [{"message": "user not authorized
//!   to perform that action"}]}`.
//! * `GET .../questions/{id}` is routed by Canvas but the controller has no
//!   `show` action, so Rails answers `ActionNotFound` — a 404. The route is
//!   registered here with that behaviour instead of inventing an endpoint.
//! * The answers live in `quiz_submissions.submission_data` under the Canvas
//!   keys (`question_<id>`, `question_<id>_marked`,
//!   `question_<id>_answer_<answer id>`), exactly like Canvas' ungraded
//!   submission data. A submission whose `submission_data` is a JSON *array* is
//!   a graded submission (`QuizSubmission#graded?`) and is rendered with the
//!   `correct` flag instead of the answer.
//! * `censored` follows `Api::V1::QuizQuestion#censor`: a student only sees
//!   the allow listed question fields and never the answer weights.
//!
//! DDL the coordinator has to add to a `star-db` migration (the module reads
//! the quiz and the question rows with `to_jsonb`, so it also works without
//! these columns — they only add fidelity):
//!
//! ```sql
//! ALTER TABLE quizzes
//!     ADD COLUMN IF NOT EXISTS one_question_at_a_time BOOLEAN NOT NULL DEFAULT false,
//!     ADD COLUMN IF NOT EXISTS hide_results TEXT,
//!     ADD COLUMN IF NOT EXISTS one_time_results BOOLEAN NOT NULL DEFAULT false,
//!     ADD COLUMN IF NOT EXISTS require_lockdown_browser BOOLEAN NOT NULL DEFAULT false,
//!     ADD COLUMN IF NOT EXISTS cant_go_back BOOLEAN NOT NULL DEFAULT false;
//!
//! ALTER TABLE quiz_questions
//!     ADD COLUMN IF NOT EXISTS question_data JSONB;
//! ```
//!
//! `quiz_questions.question_data` holds the Canvas `question_data` hash
//! (`answers`, `matches`, `variables`, `formulas`, `blank_id`, ...). Without
//! it the module falls back to the scalar columns of `quiz_questions`.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Constants ported from the Ruby
// ---------------------------------------------------------------------------

/// Canvas `Quizzes::QuizQuestion::AnswerSerializers::Util::MaxTextualAnswerLength`.
const MAX_TEXTUAL_ANSWER_LENGTH: usize = 16 * 1024;

/// Canvas `Api::V1::QuizSubmissionQuestion::INCLUDABLES`.
const INCLUDABLES: [&str; 1] = ["quiz_question"];

/// Canvas `Api::V1::QuizQuestion::API_ALLOWED_QUESTION_DATA_OUTPUT_FIELDS`.
const ALLOWED_QUESTION_DATA_OUTPUT_FIELDS: [&str; 18] = [
    "question_name",
    "question_type",
    "question_text",
    "position",
    "points_possible",
    "correct_comments",
    "incorrect_comments",
    "neutral_comments",
    "correct_comments_html",
    "incorrect_comments_html",
    "neutral_comments_html",
    "answers",
    "variables",
    "formulas",
    "answer_tolerance",
    "formula_decimal_places",
    "matches",
    "matching_answer_incorrect_matches",
];

/// Canvas `Api::V1::QuizQuestion::API_CENSOR_ATTR_ALLOWLIST`.
const CENSOR_ATTR_ALLOWLIST: [&str; 15] = [
    "id",
    "position",
    "quiz_group_id",
    "quiz_id",
    "assessment_question_id",
    "assessment_question",
    "question_name",
    "question_type",
    "question_text",
    "answers",
    "matches",
    "formulas",
    "variables",
    "answer_tolerance",
    "formula_decimal_places",
];

/// Canvas `Api::V1::QuizQuestion::API_CENSOR_QUESTION_TYPE_ANSWERS_ALLOWLIST`.
const CENSOR_ANSWERS_QUESTION_TYPES: [&str; 6] = [
    "multiple_choice_question",
    "true_false_question",
    "multiple_answers_question",
    "matching_question",
    "multiple_dropdowns_question",
    "calculated_question",
];

/// Canvas `Api::V1::QuizQuestion::API_CENSOR_ANSWER_ATTRS_ALLOWLIST`.
const CENSOR_ANSWER_ATTRS_ALLOWLIST: [&str; 5] = ["id", "text", "html", "blank_id", "variables"];

/// Canvas `Quizzes::Quiz.non_shuffled_questions`.
const NON_SHUFFLED_QUESTION_TYPES: [&str; 3] = [
    "true_false_question",
    "matching_question",
    "fill_in_multiple_blanks_question",
];

/// The HTML tags the small sanitizer of this module keeps. Canvas uses the
/// `sanitize` gem with `CanvasSanitize::SANITIZE`, star-lms has no HTML
/// sanitizer dependency.
const ALLOWED_HTML_TAGS: [&str; 12] = [
    "p", "br", "b", "strong", "i", "em", "u", "ul", "ol", "li", "span", "div",
];

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// The failures of the quiz submission questions routes.
///
/// Canvas renders a rejected request (`reject!`) as
/// `{"status": "<rails symbol>", "message": "..."}`, a missing record as
/// `{"errors": [{"message": "The specified resource does not exist."}]}` and a
/// failed `authorized_action` as
/// `{"status": "unauthorized", "errors": [{"message": "user not authorized to
/// perform that action"}]}`.
#[derive(Debug)]
enum QsqError {
    /// Canvas `reject!(message, status)`.
    Reject(StatusCode, String),
    /// Canvas `ActiveRecord::RecordNotFound`.
    NotFound,
    /// Canvas `ActionController::ActionNotFound` — a routed action the
    /// controller does not implement.
    ActionNotFound,
    /// Canvas `render_json_unauthorized` for a logged in user.
    NotAuthorized,
    /// The house error (database failures, missing credentials).
    Api(ApiError),
}

impl From<ApiError> for QsqError {
    fn from(error: ApiError) -> Self {
        QsqError::Api(error)
    }
}

impl From<sqlx::Error> for QsqError {
    fn from(error: sqlx::Error) -> Self {
        QsqError::Api(ApiError::Db(error.to_string()))
    }
}

impl IntoResponse for QsqError {
    fn into_response(self) -> Response {
        match self {
            QsqError::Reject(status, message) => (
                status,
                Json(json!({ "status": rails_status(status), "message": message })),
            )
                .into_response(),
            QsqError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "errors": [{ "message": "The specified resource does not exist." }]
                })),
            )
                .into_response(),
            QsqError::ActionNotFound => (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "errors": [{ "message": "An error occurred.", "error_code": "not_found" }]
                })),
            )
                .into_response(),
            QsqError::NotAuthorized => (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "status": "unauthorized",
                    "errors": [{ "message": "user not authorized to perform that action" }]
                })),
            )
                .into_response(),
            QsqError::Api(error) => error.into_response(),
        }
    }
}

/// The Rails status symbol of an HTTP status (`Rack::Utils`).
fn rails_status(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 => "bad_request",
        401 => "unauthorized",
        403 => "forbidden",
        404 => "not_found",
        409 => "conflict",
        422 => "unprocessable_entity",
        501 => "not_implemented",
        _ => "internal_server_error",
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// The `quiz_submissions` row the controller works with.
#[derive(Debug, sqlx::FromRow)]
struct SubmissionRow {
    id: i64,
    quiz_id: i64,
    user_id: i64,
    attempt: i32,
    workflow_state: String,
    submission_data: String,
    validation_token: Option<String>,
    end_at: Option<DateTime<Utc>>,
    has_seen_results: bool,
}

impl SubmissionRow {
    /// Canvas `QuizSubmission#completed?` — `complete? || pending_review?`.
    fn completed(&self) -> bool {
        self.workflow_state == "complete" || self.workflow_state == "pending_review"
    }

    /// Canvas `QuizSubmission#untaken?`.
    fn untaken(&self) -> bool {
        self.workflow_state == "untaken"
    }

    /// Canvas `QuizSubmission#valid_token?`.
    fn valid_token(&self, token: Option<&str>) -> bool {
        match self.validation_token.as_deref() {
            None => true,
            Some(stored) => stored.is_empty() || Some(stored) == token,
        }
    }

    /// Canvas `QuizSubmission#overdue?` (without the strict five minutes).
    fn overdue(&self) -> bool {
        match self.end_at {
            Some(end_at) => end_at < Utc::now(),
            None => false,
        }
    }
}

/// The quiz of a submission (`@quiz = @quiz_submission.quiz`).
///
/// The row is read with `to_jsonb` so that the Canvas only settings columns
/// (`hide_results`, `one_question_at_a_time`, ...) stay optional: a column the
/// database does not have yet simply reads as null.
#[derive(Debug, Clone)]
struct QuizView {
    id: i64,
    course_id: i64,
    published: bool,
    allowed_attempts: i32,
    access_code: Option<String>,
    shuffle_answers: bool,
    one_question_at_a_time: bool,
    hide_results: Option<String>,
    one_time_results: bool,
    require_lockdown_browser: bool,
    cant_go_back: bool,
}

impl QuizView {
    fn from_row(row: &Value) -> QuizView {
        QuizView {
            id: row.get("id").and_then(json_i64).unwrap_or_default(),
            course_id: row.get("course_id").and_then(json_i64).unwrap_or_default(),
            published: row.get("published").and_then(json_bool).unwrap_or(false),
            allowed_attempts: row
                .get("allowed_attempts")
                .and_then(json_i64)
                .unwrap_or_default() as i32,
            access_code: row.get("access_code").and_then(json_string),
            shuffle_answers: row
                .get("shuffle_answers")
                .and_then(json_bool)
                .unwrap_or(false),
            one_question_at_a_time: row
                .get("one_question_at_a_time")
                .and_then(json_bool)
                .unwrap_or(false),
            hide_results: row.get("hide_results").and_then(json_string),
            one_time_results: row
                .get("one_time_results")
                .and_then(json_bool)
                .unwrap_or(false),
            require_lockdown_browser: row
                .get("require_lockdown_browser")
                .and_then(json_bool)
                .unwrap_or(false),
            cant_go_back: row.get("cant_go_back").and_then(json_bool).unwrap_or(false),
        }
    }
}

/// A quiz question with the Canvas `question_data` hash it carries.
#[derive(Debug, Clone)]
struct QuestionView {
    id: i64,
    quiz_id: i64,
    position: i32,
    quiz_group_id: Option<i64>,
    assessment_question_bank_id: Option<i64>,
    created_at: Option<String>,
    data: Map<String, Value>,
}

impl QuestionView {
    fn from_row(row: &Value) -> QuestionView {
        let id = row.get("id").and_then(json_i64).unwrap_or_default();
        let quiz_id = row.get("quiz_id").and_then(json_i64).unwrap_or_default();
        let position = row.get("position").and_then(json_i64).unwrap_or_default() as i32;

        // The Canvas question data is the source of the question fields. It is
        // either stored in `question_data` or synthesized from the columns of
        // `quiz_questions`.
        let mut data = match row.get("question_data") {
            Some(Value::Object(stored)) => stored.clone(),
            _ => Map::new(),
        };
        data.entry("question_name".to_string()).or_insert_with(|| {
            row.get("question_name")
                .and_then(json_string)
                .unwrap_or_default()
                .into()
        });
        data.entry("question_type".to_string()).or_insert_with(|| {
            row.get("question_type")
                .and_then(json_string)
                .unwrap_or_default()
                .into()
        });
        data.entry("question_text".to_string()).or_insert_with(|| {
            row.get("question_text")
                .and_then(json_string)
                .unwrap_or_default()
                .into()
        });
        data.entry("points_possible".to_string())
            .or_insert_with(|| row.get("points_possible").cloned().unwrap_or(json!(0)));
        data.entry("correct_comments".to_string())
            .or_insert_with(|| row.get("correct_comments").cloned().unwrap_or(Value::Null));
        data.entry("incorrect_comments".to_string())
            .or_insert_with(|| {
                row.get("incorrect_comments")
                    .cloned()
                    .unwrap_or(Value::Null)
            });
        data.entry("neutral_comments".to_string())
            .or_insert_with(|| row.get("neutral_comments").cloned().unwrap_or(Value::Null));
        data.entry("answers".to_string())
            .or_insert_with(|| parse_answers(row.get("answers")).unwrap_or_else(|| json!([])));
        data.entry("position".to_string())
            .or_insert(json!(position));

        QuestionView {
            id,
            quiz_id,
            position,
            quiz_group_id: row.get("quiz_group_id").and_then(json_i64),
            assessment_question_bank_id: row.get("assessment_question_bank_id").and_then(json_i64),
            created_at: row.get("created_at").and_then(json_string),
            data,
        }
    }

    fn question_type(&self) -> &str {
        self.data
            .get("question_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// All answer records of the question.
    fn answers(&self) -> Vec<Value> {
        self.data
            .get("answers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The answer pool: for a matching question the records that carry a
    /// `match_id` are the matches, not the answers.
    fn answer_records(&self) -> Vec<Value> {
        let answers = self.answers();
        if self.data.get("matches").and_then(Value::as_array).is_some() {
            return answers;
        }
        answers
            .into_iter()
            .filter(|answer| answer.get("match_id").is_none())
            .collect()
    }

    /// The match pool of a matching question.
    fn match_records(&self) -> Vec<Value> {
        if let Some(matches) = self.data.get("matches").and_then(Value::as_array) {
            return matches.clone();
        }
        self.answers()
            .into_iter()
            .filter(|answer| answer.get("match_id").is_some())
            .collect()
    }

    fn answer_ids(&self) -> Vec<i64> {
        self.answer_records().iter().filter_map(answer_id).collect()
    }

    fn answer_available(&self, id: i64) -> bool {
        self.answer_ids().contains(&id)
    }

    fn match_ids(&self) -> Vec<i64> {
        self.match_records()
            .iter()
            .filter_map(|record| record.get("match_id").and_then(json_i64))
            .collect()
    }

    fn match_available(&self, id: i64) -> bool {
        self.match_ids().contains(&id)
    }

    /// The blanks of a fill in the blanks / multiple dropdowns question.
    fn blanks(&self) -> Vec<String> {
        self.answer_records()
            .iter()
            .filter_map(|answer| {
                answer
                    .get("blank_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }

    fn blank_available(&self, blank: &str) -> bool {
        self.blanks().iter().any(|known| known == blank)
    }

    /// Canvas `Quizzes::Quiz.shuffleable_question_type?`.
    fn shuffleable(&self) -> bool {
        !NON_SHUFFLED_QUESTION_TYPES.contains(&self.question_type())
    }
}

// ---------------------------------------------------------------------------
// The answer serializers
// ---------------------------------------------------------------------------

/// The Canvas answer serializer of a question type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnswerKind {
    /// `MultipleChoice` (also used for `TrueFalse`).
    MultipleChoice,
    /// `MultipleAnswers`.
    MultipleAnswers,
    /// `Numerical` (also used for `Calculated`).
    Numerical,
    /// `ShortAnswer`.
    ShortAnswer,
    /// `Essay`.
    Essay,
    /// `FillInMultipleBlanks`.
    FillInMultipleBlanks,
    /// `MultipleDropdowns`.
    MultipleDropdowns,
    /// `Matching`.
    Matching,
    /// `FileUpload`.
    FileUpload,
    /// `TextOnly` and `Unknown` — they have no answer.
    None,
}

/// The serializer Canvas picks for a question type.
fn answer_kind(question_type: &str) -> AnswerKind {
    match question_type {
        "multiple_choice_question" | "true_false_question" => AnswerKind::MultipleChoice,
        "multiple_answers_question" => AnswerKind::MultipleAnswers,
        "numerical_question" | "calculated_question" => AnswerKind::Numerical,
        "short_answer_question" => AnswerKind::ShortAnswer,
        "essay_question" => AnswerKind::Essay,
        "fill_in_multiple_blanks_question" => AnswerKind::FillInMultipleBlanks,
        "multiple_dropdowns_question" => AnswerKind::MultipleDropdowns,
        "matching_question" => AnswerKind::Matching,
        "file_upload_question" => AnswerKind::FileUpload,
        _ => AnswerKind::None,
    }
}

/// The key of a question answer inside `submission_data`.
fn question_key(question_id: i64) -> String {
    format!("question_{question_id}")
}

/// The key of the flag of a question inside `submission_data`.
fn marked_key(question_id: i64) -> String {
    format!("question_{question_id}_marked")
}

/// The key of a single answer of a question inside `submission_data`.
fn answer_key(question_id: i64, answer_id: i64) -> String {
    format!("question_{question_id}_answer_{answer_id}")
}

/// The key of a blank answer of a question inside `submission_data`.
///
/// Canvas uses `AssessmentQuestion.variable_id(blank)`, the MD5 digest of
/// `"dropdown,<blank>,instructure-key"`.
fn blank_key(question_id: i64, blank: &str) -> String {
    format!("question_{question_id}_{}", blank_id(blank))
}

/// Canvas `AssessmentQuestion.variable_id` — the digest of the blank name.
fn blank_id(blank: &str) -> String {
    use md5::{Digest, Md5};

    let payload = format!("dropdown,{blank},instructure-key");
    let digest = Md5::digest(payload.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Canvas `Quizzes::QuizQuestion::AnswerSerializers#serialize` — turn the
/// answer of the request into the records of `submission_data`.
fn serialize_answer(
    question: &QuestionView,
    answer: &Value,
) -> Result<Map<String, Value>, QsqError> {
    let mut record = Map::new();
    let key = question_key(question.id);

    match answer_kind(question.question_type()) {
        AnswerKind::MultipleChoice => {
            let Some(answer_id) = ruby_to_integer(answer) else {
                return Err(invalid_type("answer", "Integer"));
            };
            if !question.answer_available(answer_id) {
                return Err(unknown_answer(answer_id));
            }
            record.insert(key, json!(answer_id.to_string()));
        }
        AnswerKind::MultipleAnswers => {
            let Some(items) = answer.as_array() else {
                return Err(invalid_type("answer", "Array"));
            };
            let selection: Vec<Value> = if items.len() == 1 && items[0] == json!("") {
                Vec::new()
            } else {
                items.clone()
            };
            let mut chosen = Vec::new();
            for (index, entry) in selection.iter().enumerate() {
                let Some(answer_id) = ruby_to_integer(entry) else {
                    return Err(invalid_type(&format!("answer[{index}]"), "Integer"));
                };
                if !question.answer_available(answer_id) {
                    return Err(unknown_answer(answer_id));
                }
                chosen.push(answer_id);
            }
            for answer_id in question.answer_ids() {
                let selected = chosen.contains(&answer_id);
                record.insert(
                    answer_key(question.id, answer_id),
                    json!(if selected { "1" } else { "0" }),
                );
            }
        }
        AnswerKind::Numerical => {
            record.insert(key, json!(answer_to_decimal(answer)));
        }
        AnswerKind::ShortAnswer => {
            let Some(text) = answer.as_str() else {
                return Err(invalid_type("answer", "String"));
            };
            if text.len() >= MAX_TEXTUAL_ANSWER_LENGTH {
                return Err(QsqError::Reject(
                    StatusCode::BAD_REQUEST,
                    "Text is too long.".to_string(),
                ));
            }
            record.insert(key, json!(sanitize_text(text)));
        }
        AnswerKind::Essay => {
            let Some(text) = answer.as_str() else {
                return Err(invalid_type("answer", "String"));
            };
            let sanitized = sanitize_html(text);
            if sanitized.len() >= MAX_TEXTUAL_ANSWER_LENGTH {
                return Err(QsqError::Reject(
                    StatusCode::BAD_REQUEST,
                    "Text is too long.".to_string(),
                ));
            }
            record.insert(key, json!(sanitized));
        }
        AnswerKind::FillInMultipleBlanks => {
            let Some(blanks) = answer.as_object() else {
                return Err(invalid_type("answer", "Hash"));
            };
            for (blank, value) in blanks {
                if !question.blank_available(blank) {
                    return Err(QsqError::Reject(
                        StatusCode::BAD_REQUEST,
                        format!("Unknown blank '{blank}'"),
                    ));
                }
                let Some(text) = value.as_str() else {
                    return Err(invalid_type(&format!("{blank}.answer"), "String"));
                };
                if text.len() >= MAX_TEXTUAL_ANSWER_LENGTH {
                    return Err(QsqError::Reject(
                        StatusCode::BAD_REQUEST,
                        "Text is too long.".to_string(),
                    ));
                }
                record.insert(blank_key(question.id, blank), json!(sanitize_text(text)));
            }
        }
        AnswerKind::MultipleDropdowns => {
            let Some(blanks) = answer.as_object() else {
                return Err(invalid_type("answer", "Hash"));
            };
            for (blank, value) in blanks {
                if !question.blank_available(blank) {
                    return Err(QsqError::Reject(
                        StatusCode::BAD_REQUEST,
                        format!("Unknown blank '{blank}'"),
                    ));
                }
                let Some(answer_id) = ruby_to_integer(value) else {
                    return Err(invalid_type(&format!("answer.{blank}"), "Integer"));
                };
                if !question.answer_available(answer_id) {
                    return Err(unknown_answer(answer_id));
                }
                record.insert(blank_key(question.id, blank), json!(answer_id.to_string()));
            }
        }
        AnswerKind::Matching => {
            let Some(pairings) = answer.as_array() else {
                return Err(invalid_type("answer", "Array"));
            };
            for (index, entry) in pairings.iter().enumerate() {
                let Some(pairing) = entry.as_object() else {
                    return Err(invalid_type(&format!("answer[{index}]"), "Hash"));
                };
                if !pairing.contains_key("answer_id") {
                    return Err(QsqError::Reject(
                        StatusCode::BAD_REQUEST,
                        "Matching pair is missing parameter \"answer_id\"".to_string(),
                    ));
                }
                if !pairing.contains_key("match_id") {
                    return Err(QsqError::Reject(
                        StatusCode::BAD_REQUEST,
                        "Matching pair is missing parameter \"match_id\"".to_string(),
                    ));
                }
                let Some(answer_id) = ruby_to_integer(&pairing["answer_id"]) else {
                    return Err(invalid_type("answer_id", "Integer"));
                };
                if !question.answer_available(answer_id) {
                    return Err(unknown_answer(answer_id));
                }
                let Some(match_id) = ruby_to_integer(&pairing["match_id"]) else {
                    return Err(invalid_type("match_id", "Integer"));
                };
                if !question.match_available(match_id) {
                    return Err(QsqError::Reject(
                        StatusCode::BAD_REQUEST,
                        format!("Unknown match '{match_id}'"),
                    ));
                }
                record.insert(
                    answer_key(question.id, answer_id),
                    json!(match_id.to_string()),
                );
            }
        }
        AnswerKind::FileUpload => {
            // Canvas `FileUpload#serialize(*args)` stores the arguments of the
            // serializer, so the record holds the answer in an array.
            record.insert(key, json!([answer.clone()]));
        }
        AnswerKind::None => {}
    }

    Ok(record)
}

/// Canvas `Quizzes::QuizQuestion::AnswerSerializers#deserialize` — the answer
/// of a question as it is shown to the user.
fn deserialize_answer(question: &QuestionView, data: &Map<String, Value>) -> Value {
    let key = question_key(question.id);

    match answer_kind(question.question_type()) {
        AnswerKind::MultipleChoice => data.get(&key).cloned().unwrap_or(Value::Null),
        AnswerKind::MultipleAnswers => {
            let mut selected = Vec::new();
            for answer_id in question.answer_ids() {
                let field = answer_key(question.id, answer_id);
                if value_to_boolean(data.get(&field)).unwrap_or(false) {
                    selected.push(json!(answer_id.to_string()));
                }
            }
            Value::Array(selected)
        }
        AnswerKind::Numerical => match data.get(&key) {
            Some(Value::String(text)) => json_number(i18n_to_decimal(text)),
            Some(value) => value.clone(),
            None => Value::Null,
        },
        AnswerKind::ShortAnswer | AnswerKind::Essay => match data.get(&key) {
            Some(Value::String(text)) if !text.is_empty() => Value::String(text.clone()),
            _ => Value::Null,
        },
        AnswerKind::FillInMultipleBlanks | AnswerKind::MultipleDropdowns => {
            let mut blanks = Map::new();
            for blank in question.blanks() {
                let field = blank_key(question.id, &blank);
                match data.get(&field) {
                    Some(Value::String(text)) if !text.is_empty() => {
                        blanks.insert(blank, Value::String(text.clone()));
                    }
                    Some(value) if !value.is_null() => {
                        blanks.insert(blank, value.clone());
                    }
                    _ => {
                        blanks.insert(blank, Value::Null);
                    }
                }
            }
            Value::Object(blanks)
        }
        AnswerKind::Matching => {
            let mut pairs = Vec::new();
            for answer_id in question.answer_ids() {
                let field = answer_key(question.id, answer_id);
                let match_id = data.get(&field).filter(|value| !blank_value(value));
                pairs.push(json!({
                    "answer_id": answer_id.to_string(),
                    "match_id": match_id.cloned().unwrap_or(Value::Null),
                }));
            }
            Value::Array(pairs)
        }
        AnswerKind::FileUpload => match data.get(&key) {
            Some(Value::Array(items)) => items
                .iter()
                .find(|item| !blank_value(item))
                .cloned()
                .unwrap_or(Value::Null),
            Some(value) if !blank_value(value) => value.clone(),
            _ => Value::Null,
        },
        AnswerKind::None => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// Ruby helpers
// ---------------------------------------------------------------------------

/// Canvas `SerializedAnswer::ERROR_CODES[:invalid_type]`.
fn invalid_type(param: &str, expected: &str) -> QsqError {
    QsqError::Reject(
        StatusCode::BAD_REQUEST,
        format!("{param} must be of type {expected}"),
    )
}

/// Canvas `SerializedAnswer::ERROR_CODES[:unknown_answer]`.
fn unknown_answer(id: i64) -> QsqError {
    QsqError::Reject(StatusCode::BAD_REQUEST, format!("Unknown answer '{id}'"))
}

/// The id of an answer record (`answer[:id]`).
fn answer_id(answer: &Value) -> Option<i64> {
    answer.get("id").and_then(json_i64)
}

/// Ruby's `Integer(value, exception: false)`.
fn ruby_to_integer(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float.trunc() as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Ruby's `String#to_i` — the leading integer of the string, 0 when there is
/// none.
fn ruby_to_i(text: &str) -> i64 {
    let trimmed = text.trim_start();
    let mut end = 0;
    for (index, character) in trimmed.char_indices() {
        if character.is_ascii_digit() || (index == 0 && (character == '-' || character == '+')) {
            end = index + character.len_utf8();
        } else {
            break;
        }
    }
    trimmed[..end].parse::<i64>().unwrap_or(0)
}

/// Canvas `Canvas::Plugin.value_to_boolean`.
fn value_to_boolean(value: Option<&Value>) -> Option<bool> {
    match value {
        None | Some(Value::Null) => Some(false),
        Some(Value::Bool(flag)) => Some(*flag),
        Some(Value::String(text)) => {
            let lowered = text.to_lowercase();
            if ["yes", "y", "true", "t", "on", "1"].contains(&lowered.as_str()) {
                Some(true)
            } else if ["no", "n", "false", "f", "off", "0"].contains(&lowered.as_str()) {
                Some(false)
            } else {
                Some(ruby_to_i(&lowered) != 0)
            }
        }
        Some(Value::Number(number)) => {
            if let Some(integer) = number.as_i64() {
                let text = integer.to_string();
                if ["yes", "y", "true", "t", "on", "1"].contains(&text.as_str()) {
                    Some(true)
                } else if ["no", "n", "false", "f", "off", "0"].contains(&text.as_str()) {
                    Some(false)
                } else {
                    Some(integer != 0)
                }
            } else {
                // Canvas returns nil for a value it does not recognize.
                None
            }
        }
        Some(_) => None,
    }
}

/// Canvas `Util.sanitize_text` — stripped and lower cased.
fn sanitize_text(text: &str) -> String {
    text.trim().to_lowercase()
}

/// Canvas `Util.i18n_to_decimal` — the en locale strips the thousands
/// delimiter (",") and uses "." as the decimal separator.
fn i18n_to_decimal(text: &str) -> f64 {
    text.replace(',', "").parse::<f64>().unwrap_or(0.0)
}

/// Canvas `Util.to_decimal` — everything that is not a number becomes 0.
fn to_decimal(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap_or(0.0),
        Value::String(text) => text.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// The decimal of an answer — a string is localized, everything else is cast.
fn answer_to_decimal(answer: &Value) -> f64 {
    match answer {
        Value::String(text) => i18n_to_decimal(text),
        other => to_decimal(other),
    }
}

/// Whether a stored answer value counts as blank (Ruby `blank?`).
fn blank_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

/// A JSON number of a float.
fn json_number(value: f64) -> Value {
    serde_json::Number::from_f64(value)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// The small HTML sanitizer of this module.
///
/// Canvas uses the `sanitize` gem; star-lms has no HTML sanitizer. Only the
/// tags below survive, every other tag is dropped (with its content for
/// `script` and `style`).
fn sanitize_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] != b'<' {
            let character = input[index..].chars().next().unwrap_or('\u{fffd}');
            out.push(character);
            index += character.len_utf8();
            continue;
        }

        let Some(relative_end) = input[index..].find('>') else {
            out.push_str(&input[index..]);
            break;
        };
        let end = index + relative_end;
        let inner = &input[index + 1..end];

        if inner.starts_with("!--") {
            match input[end + 1..].find("-->") {
                Some(comment_end) => {
                    index = end + 1 + comment_end + 3;
                }
                None => {
                    index = input.len();
                }
            }
            continue;
        }

        let name = inner
            .trim_start_matches('/')
            .split(|character: char| {
                character.is_whitespace() || character == '/' || character == '>'
            })
            .next()
            .unwrap_or_default()
            .to_lowercase();

        if name == "script" || name == "style" {
            let rest = &input[end + 1..];
            let lowered = rest.to_ascii_lowercase();
            match lowered.find(&format!("</{name}")) {
                Some(closing) => match rest[closing..].find('>') {
                    Some(tag_end) => {
                        index = end + 1 + closing + tag_end + 1;
                    }
                    None => {
                        index = input.len();
                    }
                },
                None => {
                    index = input.len();
                }
            }
            continue;
        }

        if ALLOWED_HTML_TAGS.contains(&name.as_str()) {
            out.push('<');
            if inner.starts_with('/') {
                out.push('/');
            }
            out.push_str(&name);
            out.push('>');
        }

        index = end + 1;
    }

    out
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

/// The integer of a JSON value (a numeric string counts as an integer).
fn json_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float.trunc() as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// The string of a JSON value.
fn json_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The boolean of a JSON value.
fn json_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        _ => None,
    }
}

/// The answer array of a `quiz_questions` row (the column holds JSON text).
fn parse_answers(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::String(text)) => serde_json::from_str(text).ok(),
        Some(other) => Some(other.clone()),
        None => None,
    }
}

/// The `submission_data` of a submission as a JSON value.
fn parse_submission_data(submission_data: &str) -> Value {
    serde_json::from_str(submission_data).unwrap_or_else(|_| json!({}))
}

/// The `submission_data` of a submission as a JSON object.
fn submission_object(submission_data: &str) -> Map<String, Value> {
    match parse_submission_data(submission_data) {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Ruby's `Hash#deep_merge`.
fn deep_merge(base: &mut Map<String, Value>, other: Map<String, Value>) {
    for (key, value) in other {
        let existing = match base.get(&key) {
            Some(Value::Object(existing)) => Some(existing.clone()),
            _ => None,
        };
        match (existing, value) {
            (Some(mut existing), Value::Object(incoming)) => {
                deep_merge(&mut existing, incoming);
                base.insert(key, Value::Object(existing));
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// `require_quiz_submission` — the submission of the path, or the one of the
/// current user for the path `self`. A missing submission is a 404.
async fn load_submission(
    pool: &PgPool,
    quiz_submission_id: &str,
    auth: &AuthUser,
) -> Result<SubmissionRow, QsqError> {
    let row: Option<SubmissionRow> = if quiz_submission_id == "self" {
        sqlx::query_as(
            r#"
            SELECT id, quiz_id, user_id, attempt, workflow_state, submission_data,
                   validation_token, end_at, has_seen_results
            FROM quiz_submissions
            WHERE user_id = $1
            ORDER BY id
            LIMIT 1
            "#,
        )
        .bind(auth.id)
        .fetch_optional(pool)
        .await?
    } else {
        let quiz_submission_id = ruby_to_i(quiz_submission_id);
        sqlx::query_as(
            r#"
            SELECT id, quiz_id, user_id, attempt, workflow_state, submission_data,
                   validation_token, end_at, has_seen_results
            FROM quiz_submissions
            WHERE id = $1
            "#,
        )
        .bind(quiz_submission_id)
        .fetch_optional(pool)
        .await?
    };

    row.ok_or(QsqError::NotFound)
}

/// `export_scopes` — the quiz of the submission.
async fn load_quiz(pool: &PgPool, quiz_id: i64) -> Result<QuizView, QsqError> {
    let row: Option<Value> =
        sqlx::query_scalar("SELECT to_jsonb(q) FROM quizzes q WHERE q.id = $1")
            .bind(quiz_id)
            .fetch_optional(pool)
            .await?;

    row.as_ref()
        .map(QuizView::from_row)
        .ok_or(QsqError::NotFound)
}

/// The questions of a quiz (`@quiz.quiz_questions`).
async fn load_questions(pool: &PgPool, quiz_id: i64) -> Result<Vec<QuestionView>, QsqError> {
    let rows: Vec<Value> = sqlx::query_scalar(
        r#"
        SELECT to_jsonb(qq)
        FROM quiz_questions qq
        WHERE qq.quiz_id = $1 AND qq.workflow_state <> 'deleted'
        ORDER BY qq.position, qq.id
        "#,
    )
    .bind(quiz_id)
    .fetch_all(pool)
    .await?;

    Ok(rows.iter().map(QuestionView::from_row).collect())
}

/// `require_question` — a question of the quiz, a missing one is a 404.
async fn require_question(
    pool: &PgPool,
    quiz_id: i64,
    question_id: i64,
) -> Result<QuestionView, QsqError> {
    let row: Option<Value> = sqlx::query_scalar(
        r#"
        SELECT to_jsonb(qq)
        FROM quiz_questions qq
        WHERE qq.quiz_id = $1 AND qq.id = $2
        "#,
    )
    .bind(quiz_id)
    .bind(question_id)
    .fetch_optional(pool)
    .await?;

    row.as_ref()
        .map(QuestionView::from_row)
        .ok_or(QsqError::NotFound)
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Canvas `context.grants_right?(user, :manage_grades)` /
/// `:manage_assignments_edit` — a teacher of the course or a site admin.
async fn is_course_staff(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM enrollments
        WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
          AND workflow_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `QuizSubmission`'s policy: an observer of the owner of the
/// submission may read it.
async fn is_observer(
    pool: &PgPool,
    observer_id: i64,
    observee_id: i64,
    course_id: i64,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM observer_enrollments
        WHERE observer_id = $1 AND user_id = $2 AND course_id = $3
        LIMIT 1
        "#,
    )
    .bind(observer_id)
    .bind(observee_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `QuizSubmission#results_visible?`.
fn results_visible(quiz: &QuizView, submission: &SubmissionRow, can_review_grades: bool) -> bool {
    if can_review_grades {
        return true;
    }
    if quiz.one_time_results && submission_has_seen_results(submission) {
        return false;
    }
    match quiz.hide_results.as_deref() {
        Some("always") => false,
        Some("until_after_last_attempt") => {
            quiz.allowed_attempts < 1
                || i64::from(submission.attempt) > i64::from(quiz.allowed_attempts)
                || (submission.completed()
                    && i64::from(submission.attempt) >= i64::from(quiz.allowed_attempts))
        }
        _ => true,
    }
}

/// Canvas `QuizSubmission#has_seen_results?` — the flag is a column of
/// `quiz_submissions`.
fn submission_has_seen_results(submission: &SubmissionRow) -> bool {
    submission.has_seen_results
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::QuizQuestion#question_json`.
fn question_json(question: &QuestionView, censored: bool, shuffle: bool) -> Value {
    let mut hash = Map::new();
    hash.insert("id".to_string(), json!(question.id));
    hash.insert("quiz_id".to_string(), json!(question.quiz_id));
    hash.insert("position".to_string(), json!(question.position));
    hash.insert(
        "regrade_option".to_string(),
        question
            .data
            .get("regrade_option")
            .cloned()
            .unwrap_or(Value::Null),
    );
    hash.insert("assessment_question_id".to_string(), Value::Null);
    hash.insert(
        "assessment_question_bank_id".to_string(),
        json!(question.assessment_question_bank_id),
    );
    hash.insert("quiz_group_id".to_string(), json!(question.quiz_group_id));
    hash.insert(
        "created_at".to_string(),
        match &question.created_at {
            Some(created_at) => json!(created_at.replace("+00:00", "Z")),
            None => Value::Null,
        },
    );

    for field in ALLOWED_QUESTION_DATA_OUTPUT_FIELDS {
        hash.insert(
            field.to_string(),
            question.data.get(field).cloned().unwrap_or(Value::Null),
        );
    }

    if shuffle && question.shuffleable() {
        if let Some(Value::Array(answers)) = hash.get_mut("answers") {
            shuffle_answers(answers, question.id);
        }
    }

    if censored {
        censor(&mut hash);
    }

    Value::Object(hash)
}

/// Canvas `Api::V1::QuizQuestion#censor`.
fn censor(hash: &mut Map<String, Value>) {
    hash.retain(|key, _| CENSOR_ATTR_ALLOWLIST.contains(&key.as_str()));

    let question_type = hash
        .get("question_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !CENSOR_ANSWERS_QUESTION_TYPES.contains(&question_type.as_str()) {
        hash.remove("answers");
    }

    if let Some(Value::Array(answers)) = hash.get_mut("answers") {
        for answer in answers.iter_mut() {
            if let Value::Object(record) = answer {
                record.retain(|key, _| CENSOR_ANSWER_ATTRS_ALLOWLIST.contains(&key.as_str()));
            }
        }
    }
}

/// A deterministic shuffle of the answers of a question.
///
/// Canvas shuffles with `Array#shuffle!` (a new order per request). star-lms
/// shuffles deterministically from the question id so that the same question
/// always has the same order.
fn shuffle_answers(answers: &mut [Value], question_id: i64) {
    let mut state = (question_id as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1);
    for index in (1..answers.len()).rev() {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let pick = ((state >> 33) as usize) % (index + 1);
        answers.swap(index, pick);
    }
}

/// Canvas `Api::V1::QuizSubmissionQuestion#quiz_submission_questions_json`.
fn quiz_submission_questions_json(
    questions: &[QuestionView],
    submission: &SubmissionRow,
    censored: bool,
    includes: &[String],
    shuffle: bool,
) -> Value {
    let data = parse_submission_data(&submission.submission_data);
    let graded = data.is_array();
    let answers = submission_object(&submission.submission_data);

    let mut payload = Map::new();
    let mut items: Vec<Value> = Vec::new();
    for question in questions {
        let mut item = match question_json(question, censored, shuffle) {
            Value::Object(item) => item,
            other => {
                items.push(other);
                continue;
            }
        };
        if graded {
            let entry = data.as_array().and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.get("question_id").and_then(json_i64) == Some(question.id))
            });
            // Canvas returns the question only when the submission holds a
            // graded entry for it.
            let Some(entry) = entry else {
                continue;
            };
            item.insert("flagged".to_string(), json!(false));
            item.insert(
                "correct".to_string(),
                entry.get("correct").cloned().unwrap_or(Value::Null),
            );
        } else {
            item.insert(
                "flagged".to_string(),
                json!(value_to_boolean(answers.get(&marked_key(question.id))).unwrap_or(false)),
            );
            item.insert("answer".to_string(), deserialize_answer(question, &answers));
        }
        items.push(Value::Object(item));
    }

    payload.insert("quiz_submission_questions".to_string(), Value::Array(items));

    if includes.iter().any(|include| include == "quiz_question") {
        payload.insert(
            "quiz_questions".to_string(),
            Value::Array(
                questions
                    .iter()
                    .map(|question| question_json(question, censored, shuffle))
                    .collect(),
            ),
        );
    }

    if !includes.is_empty() {
        payload.insert(
            "meta".to_string(),
            json!({ "primaryCollection": "quiz_submission_questions" }),
        );
    }

    Value::Object(payload)
}

// ---------------------------------------------------------------------------
// The action parameters
// ---------------------------------------------------------------------------

/// The parameters of an action: the JSON body and the query string merged,
/// the body wins.
#[derive(Debug, Default)]
struct ActionParams {
    body: Map<String, Value>,
    query: HashMap<String, String>,
}

impl ActionParams {
    fn new(body: Option<Value>, query: HashMap<String, String>) -> ActionParams {
        let body = match body {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        ActionParams { body, query }
    }

    /// The raw value of a parameter.
    fn value(&self, key: &str) -> Option<Value> {
        if let Some(value) = self.body.get(key) {
            return Some(value.clone());
        }
        self.query
            .get(key)
            .map(|value| Value::String(value.clone()))
    }

    /// The string value of a parameter.
    fn string(&self, key: &str) -> Option<String> {
        self.value(key).and_then(|value| match value {
            Value::Null => None,
            other => json_string(&other),
        })
    }

    /// The integer value of a parameter (Ruby's `Integer(value)`).
    fn integer(&self, key: &str) -> Option<i64> {
        self.value(key).and_then(|value| ruby_to_integer(&value))
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /quiz_submissions/{quiz_submission_id}/questions
async fn index(
    State(state): State<AppState>,
    Path(quiz_submission_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, QsqError> {
    let submission = load_submission(&state.db, &quiz_submission_id, &auth).await?;
    let quiz = load_quiz(&state.db, submission.quiz_id).await?;

    let staff = is_course_staff(&state.db, &auth, quiz.course_id).await?;
    // Canvas `censored?` — `!@quiz.grants_right?(@current_user, session, :update)`.
    let censored = !staff;

    // `retrieve_quiz_submission_attempt!` — the attempt of the request has to
    // exist. star-lms keeps one submission per quiz and user, so only the
    // current attempt is known.
    if let Some(attempt) = query.get("quiz_submission_attempt") {
        let attempt = ruby_to_i(attempt);
        if attempt != i64::from(submission.attempt) {
            return Err(QsqError::NotFound);
        }
    }

    if quiz.one_question_at_a_time && censored {
        return Err(QsqError::Reject(
            StatusCode::UNAUTHORIZED,
            "Cannot receive one question at a time questions in the API".to_string(),
        ));
    }

    if submission.completed() && !results_visible(&quiz, &submission, staff) {
        return Err(QsqError::Reject(
            StatusCode::UNAUTHORIZED,
            "Cannot view questions due to quiz settings".to_string(),
        ));
    }

    // `authorized_action(@quiz_submission, @current_user, :read)`.
    let readable = submission.user_id == auth.id
        || staff
        || is_observer(&state.db, auth.id, submission.user_id, quiz.course_id).await?;
    if !readable {
        return Err(QsqError::NotAuthorized);
    }

    let questions = load_questions(&state.db, quiz.id).await?;
    let includes = extract_includes(&query);
    let shuffle = quiz.shuffle_answers && !staff;

    Ok(Json(quiz_submission_questions_json(
        &questions,
        &submission,
        censored,
        &includes,
        shuffle,
    )))
}

/// The `include[]` parameters of the request.
fn extract_includes(query: &HashMap<String, String>) -> Vec<String> {
    let mut includes: Vec<String> = Vec::new();
    for (key, value) in query {
        if key == "include[]" || key == "include" {
            let value = value.trim();
            if !value.is_empty() && INCLUDABLES.contains(&value) {
                includes.push(value.to_string());
            }
        }
    }
    includes
}

/// POST /quiz_submissions/{quiz_submission_id}/questions
async fn answer(
    State(state): State<AppState>,
    Path(quiz_submission_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, QsqError> {
    let submission = load_submission(&state.db, &quiz_submission_id, &auth).await?;
    let quiz = load_quiz(&state.db, submission.quiz_id).await?;

    // `validate_ldb_status!`
    if quiz.require_lockdown_browser {
        return Err(QsqError::Reject(
            StatusCode::FORBIDDEN,
            "this quiz requires the lockdown browser".to_string(),
        ));
    }

    // `@quiz_submission.grants_right?(@service.participant.user, :update)` —
    // the own submission of an untaken quiz.
    if submission.user_id != auth.id || !submission.untaken() {
        return Err(QsqError::Reject(
            StatusCode::FORBIDDEN,
            "you are not allowed to update questions for this quiz submission".to_string(),
        ));
    }

    let params = ActionParams::new(body.map(|Json(value)| value), query);

    // The answers of the request: `params[:quiz_questions]`.
    let requested = params
        .body
        .get("quiz_questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut answers: HashMap<i64, Value> = HashMap::new();
    for entry in &requested {
        let Some(object) = entry.as_object() else {
            continue;
        };
        let Some(id) = object.get("id") else {
            continue;
        };
        if blank_value(id) {
            continue;
        }
        let question_id = ruby_to_i(&json_string(id).unwrap_or_default());
        let answer = match object.get("answer") {
            Some(Value::Null) | None => json!([]),
            Some(answer) => answer.clone(),
        };
        answers.insert(question_id, answer);
    }

    // The questions of the quiz that are answered.
    let questions = load_questions(&state.db, quiz.id).await?;
    let answered: Vec<QuestionView> = questions
        .into_iter()
        .filter(|question| answers.contains_key(&question.id))
        .collect();

    // The serialization of the answers — a failure is a 400.
    let mut record = Map::new();
    for question in &answered {
        let answer = answers.get(&question.id).cloned().unwrap_or(json!([]));
        let serialized = serialize_answer(question, &answer)?;
        for (key, value) in serialized {
            record.insert(key, value);
        }
    }

    update_question(&state.db, &submission, &quiz, &params, record, true).await?;

    let submission = load_submission(&state.db, &quiz_submission_id, &auth).await?;
    Ok(Json(quiz_submission_questions_json(
        &answered,
        &submission,
        true,
        &[],
        false,
    )))
}

/// `Quizzes::QuizSubmissionService#update_question`.
async fn update_question(
    pool: &PgPool,
    submission: &SubmissionRow,
    quiz: &QuizView,
    params: &ActionParams,
    record: Map<String, Value>,
    snapshot: bool,
) -> Result<(), QsqError> {
    if submission.completed() {
        return Err(QsqError::Reject(
            StatusCode::BAD_REQUEST,
            "quiz submission is already complete".to_string(),
        ));
    }
    if submission.overdue() {
        return Err(QsqError::Reject(
            StatusCode::BAD_REQUEST,
            "quiz submission is overdue".to_string(),
        ));
    }

    assert_takeability(quiz, params)?;

    // `validate_token!`
    let token = params.string("validation_token");
    if !submission.valid_token(token.as_deref()) {
        return Err(QsqError::Reject(
            StatusCode::FORBIDDEN,
            "invalid token".to_string(),
        ));
    }

    // `ensure_latest_attempt!`
    let Some(attempt) = params.integer("attempt") else {
        return Err(QsqError::Reject(
            StatusCode::BAD_REQUEST,
            "invalid attempt".to_string(),
        ));
    };
    if attempt != i64::from(submission.attempt) {
        return Err(QsqError::Reject(
            StatusCode::BAD_REQUEST,
            format!("attempt {attempt} can not be modified"),
        ));
    }

    backup_submission_data(pool, submission, params, record, snapshot).await
}

/// `Quizzes::QuizSubmissionService#assert_takeability!`.
fn assert_takeability(quiz: &QuizView, params: &ActionParams) -> Result<(), QsqError> {
    // The "can't go back" quizzes are not supported by the API.
    if quiz.cant_go_back {
        return Err(QsqError::Reject(
            StatusCode::NOT_IMPLEMENTED,
            "that type of quizzes is not supported yet".to_string(),
        ));
    }

    // The access code of the quiz has to match.
    if let Some(access_code) = quiz.access_code.as_deref() {
        if !access_code.is_empty() && params.string("access_code").as_deref() != Some(access_code) {
            return Err(QsqError::Reject(
                StatusCode::FORBIDDEN,
                "invalid access code".to_string(),
            ));
        }
    }

    // A student may only take a published quiz.
    if !quiz.published {
        return Err(QsqError::Reject(
            StatusCode::BAD_REQUEST,
            "quiz is locked".to_string(),
        ));
    }

    Ok(())
}

/// `QuizSubmission#backup_submission_data` — merge the records into the
/// submission data of the attempt.
async fn backup_submission_data(
    pool: &PgPool,
    submission: &SubmissionRow,
    params: &ActionParams,
    record: Map<String, Value>,
    snapshot: bool,
) -> Result<(), QsqError> {
    let stored = parse_submission_data(&submission.submission_data);
    let same_attempt = submission_object(&submission.submission_data)
        .get("attempt")
        .and_then(json_i64)
        == Some(i64::from(submission.attempt));

    let mut merged = match stored {
        Value::Object(map) if same_attempt => map,
        _ => Map::new(),
    };
    deep_merge(&mut merged, record);

    // `update_question` merges the validation token and the snapshot counter
    // into the records before they are stored.
    merged.insert(
        "validation_token".to_string(),
        match params.string("validation_token") {
            Some(token) => json!(token),
            None => Value::Null,
        },
    );
    let counter_seed: i64 = if snapshot { 5 } else { 1 };
    merged.insert("cnt".to_string(), json!(counter_seed));

    merged.insert("attempt".to_string(), json!(submission.attempt));

    // `new_params[:cnt] = (new_params[:cnt].to_i + 1) % 5`.
    let counter = merged
        .get("cnt")
        .and_then(json_i64)
        .unwrap_or(0)
        .wrapping_add(1)
        .rem_euclid(5);
    merged.insert("cnt".to_string(), json!(counter));

    let payload = serde_json::to_string(&Value::Object(merged))
        .map_err(|error| ApiError::Db(error.to_string()))?;

    sqlx::query(
        r#"
        UPDATE quiz_submissions
        SET submission_data = $2, updated_at = now()
        WHERE id = $1 AND workflow_state NOT IN ('complete', 'pending_review')
        "#,
    )
    .bind(submission.id)
    .bind(payload)
    .execute(pool)
    .await?;

    Ok(())
}

/// GET /quiz_submissions/{quiz_submission_id}/questions/{id}/formatted_answer
async fn formatted_answer(
    State(state): State<AppState>,
    Path((quiz_submission_id, question_id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, QsqError> {
    let submission = load_submission(&state.db, &quiz_submission_id, &auth).await?;
    let quiz = load_quiz(&state.db, submission.quiz_id).await?;
    let question = require_question(&state.db, quiz.id, ruby_to_i(&question_id)).await?;

    let student_answer = query.get("answer").cloned().unwrap_or_default();
    if student_answer.trim().is_empty() {
        return Err(QsqError::Reject(
            StatusCode::UNPROCESSABLE_ENTITY,
            "answer param is required".to_string(),
        ));
    }

    let formatted = if question.question_type() == "numerical_question" {
        let has_precision_answers = question.answers().iter().any(|answer| {
            answer.get("numerical_answer_type").and_then(Value::as_str) == Some("precision_answer")
        });
        json!(number_with_precision(
            &student_answer,
            if has_precision_answers { 16 } else { 4 },
            has_precision_answers,
        ))
    } else {
        Value::String(student_answer.clone())
    };

    Ok(Json(json!({ "formatted_answer": formatted })))
}

/// Rails `number_with_precision` with `strip_insignificant_zeros`.
///
/// A value that is not a number is returned unchanged, exactly like Rails'
/// `ActiveSupport::DecimalHelper`.
fn number_with_precision(value: &str, precision: usize, significant: bool) -> String {
    let Ok(number) = value.trim().parse::<f64>() else {
        return value.to_string();
    };
    if !number.is_finite() {
        return value.to_string();
    }

    if significant {
        if number == 0.0 {
            return "0".to_string();
        }
        let exponent = number.abs().log10().floor() as i32;
        let decimals = (precision as i32 - 1 - exponent).max(0) as usize;
        strip_insignificant_zeros(&format!("{:.*}", decimals, number))
    } else {
        strip_insignificant_zeros(&format!("{:.*}", precision, number))
    }
}

/// Rails' `strip_insignificant_zeros`.
fn strip_insignificant_zeros(value: &str) -> String {
    if !value.contains('.') {
        return value.to_string();
    }
    let trimmed = value.trim_end_matches('0');
    let trimmed = trimmed.strip_suffix('.').unwrap_or(trimmed);
    trimmed.to_string()
}

/// GET /quiz_submissions/{quiz_submission_id}/questions/{id}
///
/// Canvas routes this path but the controller has no `show` action, so Rails
/// answers `ActionNotFound` before any `before_action` runs: a 404 with
/// `{"errors": [{"message": "An error occurred.", "error_code": "not_found"}]}`.
/// The answer does not depend on the caller, so the route needs no
/// authentication either — Rails raises before the login filter.
async fn show(
    Path((_quiz_submission_id, _question_id)): Path<(String, String)>,
) -> Result<Json<Value>, QsqError> {
    Err(QsqError::ActionNotFound)
}

/// PUT /quiz_submissions/{quiz_submission_id}/questions/{id}/flag
async fn flag(
    State(state): State<AppState>,
    Path((quiz_submission_id, question_id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, QsqError> {
    toggle_flag(
        state,
        auth,
        FlagRequest {
            quiz_submission_id,
            question_id: ruby_to_i(&question_id),
            body,
            query,
            flagged: true,
        },
    )
    .await
}

/// PUT /quiz_submissions/{quiz_submission_id}/questions/{id}/unflag
async fn unflag(
    State(state): State<AppState>,
    Path((quiz_submission_id, question_id)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, QsqError> {
    toggle_flag(
        state,
        auth,
        FlagRequest {
            quiz_submission_id,
            question_id: ruby_to_i(&question_id),
            body,
            query,
            flagged: false,
        },
    )
    .await
}

/// The request of the `flag` and `unflag` actions.
struct FlagRequest {
    quiz_submission_id: String,
    question_id: i64,
    body: Option<Json<Value>>,
    query: HashMap<String, String>,
    flagged: bool,
}

/// The shared body of `flag` and `unflag`.
async fn toggle_flag(
    state: AppState,
    auth: AuthUser,
    request: FlagRequest,
) -> Result<Json<Value>, QsqError> {
    let submission = load_submission(&state.db, &request.quiz_submission_id, &auth).await?;
    let quiz = load_quiz(&state.db, submission.quiz_id).await?;
    let question = require_question(&state.db, quiz.id, request.question_id).await?;

    // `validate_ldb_status!`
    if quiz.require_lockdown_browser {
        return Err(QsqError::Reject(
            StatusCode::FORBIDDEN,
            "this quiz requires the lockdown browser".to_string(),
        ));
    }

    // `@quiz_submission.grants_right?(@service.participant.user, :update)`
    if submission.user_id != auth.id || !submission.untaken() {
        return Err(QsqError::Reject(
            StatusCode::FORBIDDEN,
            "you are not allowed to update questions for this quiz submission".to_string(),
        ));
    }

    let params = ActionParams::new(request.body.map(|Json(value)| value), request.query);
    let mut record = Map::new();
    record.insert(marked_key(question.id), json!(request.flagged));

    update_question(&state.db, &submission, &quiz, &params, record, false).await?;

    let submission = load_submission(&state.db, &request.quiz_submission_id, &auth).await?;
    let questions = vec![question];
    Ok(Json(quiz_submission_questions_json(
        &questions,
        &submission,
        true,
        &[],
        false,
    )))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The routes of the Canvas quiz submission questions API.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/quiz_submissions/{quiz_submission_id}/questions",
            get(index).post(answer),
        )
        .route(
            "/quiz_submissions/{quiz_submission_id}/questions/{id}/formatted_answer",
            get(formatted_answer),
        )
        .route(
            "/quiz_submissions/{quiz_submission_id}/questions/{id}",
            get(show),
        )
        .route(
            "/quiz_submissions/{quiz_submission_id}/questions/{id}/flag",
            put(flag),
        )
        .route(
            "/quiz_submissions/{quiz_submission_id}/questions/{id}/unflag",
            put(unflag),
        )
}
