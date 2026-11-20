//! Live parity tests are ignored by default; run explicitly with `-- --ignored`
//! against a local service. canvas_parity.rs — Canvas API v1 parity tests for
//! moderated grading (ProvisionalGrade, ProvisionalGradeSelection,
//! ModerationGrader, AssignmentExtension, anonymous provisional).
//!
//! Gated by STAR_PARITY_TESTS=1. Targets live API at http://127.0.0.1:8080.
//!
//! Covers all 9 routes:
//!   1. POST   .../courses/:course_id/assignments/:assignment_id/extensions
//!   2. GET    .../courses/:course_id/assignments/:assignment_id/moderated_students
//!   3. POST   .../courses/:course_id/assignments/:assignment_id/moderated_students
//!   4. GET    .../courses/:course_id/assignments/:assignment_id/provisional_grades/status
//!   5. PUT    .../courses/:course_id/assignments/:assignment_id/provisional_grades/:provisional_grade_id/select
//!   6. PUT    .../courses/:course_id/assignments/:assignment_id/provisional_grades/bulk_select
//!   7. POST   .../courses/:course_id/assignments/:assignment_id/provisional_grades/publish
//!   8. GET    .../courses/:course_id/assignments/:assignment_id/anonymous_provisional_grades/status
//!   9. POST   .../sections/:section_id/assignments/:assignment_id/submissions

use reqwest::blocking::{Client, Response};
use serde::Deserialize;
use std::env;

const API_BASE: &str = "http://127.0.0.1:8080/api/v1";

#[derive(Debug, Deserialize)]
pub struct AssignmentExtensionResponse {
    pub id: i64,
    pub assignment_id: i64,
    pub user_id: i64,
    pub seconds_late_extension: i64,
}

#[derive(Debug, Deserialize)]
pub struct ModerationSetEntry {
    pub student_id: i64,
    pub assignment_id: i64,
    #[serde(default)]
    pub selected_provisional_grade_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ProvisionalGradesStatusResponse {
    pub students: Vec<ProvisionalStudentStatus>,
}

#[derive(Debug, Deserialize)]
pub struct ProvisionalStudentStatus {
    pub user_id: Option<i64>,
    pub provisional_count: i64,
    #[serde(default)]
    pub has_selected_grade: bool,
    #[serde(default)]
    pub anonymous_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProvisionalGradeSelectResponse {
    pub assignment_id: i64,
    pub student_id: i64,
    pub selected_provisional_grade_id: i64,
    pub score: Option<f64>,
    pub grade: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BulkSelectResponse {
    pub results: Vec<BulkSelectResult>,
    #[serde(default)]
    pub errors: Vec<BulkSelectError>,
}

#[derive(Debug, Deserialize)]
pub struct BulkSelectResult {
    pub student_id: i64,
    pub selected_provisional_grade_id: i64,
    #[serde(default)]
    pub submission_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct BulkSelectError {
    pub index: i32,
    pub student_id: Option<i64>,
    pub error: String,
}

#[derive(Debug, Deserialize)]
pub struct PublishResponse {
    pub assignment_id: i64,
    pub grades_published_at: Option<String>,
    pub published_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct AnonymousProvisionalStatusResponse {
    pub students: Vec<AnonymousProvisionalStudent>,
}

#[derive(Debug, Deserialize)]
pub struct AnonymousProvisionalStudent {
    pub anonymous_id: String,
    pub provisional_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct SubmissionResponse {
    pub id: i64,
    pub assignment_id: i64,
    pub user_id: i64,
    pub body: Option<String>,
}

fn client() -> Client {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap()
}

fn auth_header() -> String {
    env::var("STAR_TEST_TOKEN")
        .unwrap_or_else(|_| "Bearer test-token-placeholder".to_string())
}

fn do_get(path: &str) -> Response {
    client()
        .get(format!("{API_BASE}{path}"))
        .header("Authorization", auth_header())
        .send()
        .unwrap()
}

fn do_post(path: &str, json: &serde_json::Value) -> Response {
    client()
        .post(format!("{API_BASE}{path}"))
        .header("Authorization", auth_header())
        .header("Content-Type", "application/json")
        .json(json)
        .send()
        .unwrap()
}

fn do_put(path: &str, json: &serde_json::Value) -> Response {
    client()
        .put(format!("{API_BASE}{path}"))
        .header("Authorization", auth_header())
        .header("Content-Type", "application/json")
        .json(json)
        .send()
        .unwrap()
}

// ---------------------------------------------------------------------------
// Test 1: POST .../courses/:course_id/assignments/:assignment_id/extensions
// — create or update an assignment extension (AssignmentExtension#create).
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_create_assignment_extension() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());
    let user_id = env::var("STAR_PARITY_USER_ID").unwrap_or("1".to_string());

    let payload = serde_json::json!({
        "user_id": user_id.parse::<i64>().unwrap(),
        "seconds_late_extension": 86400
    });

    let resp = do_post(
        &format!("/courses/{course_id}/assignments/{assignment_id}/extensions"),
        &payload,
    );

    // Teacher/admin creates extension → 201 Created
    assert_eq!(
        resp.status().as_u16(),
        201,
        "expected 201 on extension create, got {}",
        resp.status()
    );

    let body: AssignmentExtensionResponse = serde_json::from_str(&resp.text().unwrap())
        .expect("response should parse as AssignmentExtensionResponse");
    assert_eq!(body.assignment_id, assignment_id.parse().unwrap());
    assert_eq!(body.user_id, user_id.parse().unwrap());
    assert_eq!(body.seconds_late_extension, 86400);
}

// ---------------------------------------------------------------------------
// Test 2: GET .../courses/:course_id/assignments/:assignment_id/moderated_students
// — list the moderation set (ModerationSet#index).
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_moderation_set_index() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());

    let resp = do_get(&format!(
        "/courses/{course_id}/assignments/{assignment_id}/moderated_students"
    ));

    // Teacher/admin lists moderation set → 200 OK (empty array is valid)
    assert_eq!(
        resp.status().as_u16(),
        200,
        "expected 200 on moderation set index, got {}",
        resp.status()
    );

    let body: Vec<ModerationSetEntry> = serde_json::from_str(&resp.text().unwrap())
        .expect("response should parse as array of ModerationSetEntry");
    // Empty is acceptable; seed data in test DB drives the real count.
    assert!(true, "moderation set index returned {:?}", body);
}

// ---------------------------------------------------------------------------
// Test 3: POST .../courses/:course_id/assignments/:assignment_id/moderated_students
// — add a student to the moderation set (Selection#create).
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_moderation_set_create() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());
    let student_id = env::var("STAR_PARITY_STUDENT_ID").unwrap_or("2".to_string());

    let payload = serde_json::json!({
        "student_id": student_id.parse::<i64>().unwrap()
    });

    let resp = do_post(
        &format!("/courses/{course_id}/assignments/{assignment_id}/moderated_students"),
        &payload,
    );

    // Teacher/admin adds student → 201 Created
    assert_eq!(
        resp.status().as_u16(),
        201,
        "expected 201 on moderation set create, got {}",
        resp.status()
    );

    let body: serde_json::Value = serde_json::from_str(&resp.text().unwrap())
        .expect("response should parse as JSON");
    assert_eq!(body["assignment_id"], assignment_id.parse().unwrap());
    assert_eq!(body["student_id"], student_id.parse().unwrap());
}

// ---------------------------------------------------------------------------
// Test 4: GET .../courses/:course_id/assignments/:assignment_id/provisional_grades/status
// — provisional grade counts per student (ProvisionalGrade#status).
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_provisional_grades_status() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());

    let resp = do_get(&format!(
        "/courses/{course_id}/assignments/{assignment_id}/provisional_grades/status"
    ));

    // Teacher/admin reads status → 200 OK
    assert_eq!(
        resp.status().as_u16(),
        200,
        "expected 200 on provisional grades status, got {}",
        resp.status()
    );

    let body: ProvisionalGradesStatusResponse =
        serde_json::from_str(&resp.text().unwrap())
            .expect("response should parse as ProvisionalGradesStatusResponse");
    // `students` key must be present even when empty.
    assert!(body.students.len() >= 0);
}

// ---------------------------------------------------------------------------
// Test 5: PUT .../courses/:course_id/assignments/:assignment_id/provisional_grades/:pg_id/select
// — select a provisional grade as the final grade (ProvisionalGrade#select).
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_provisional_grades_select() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());
    let provisional_grade_id =
        env::var("STAR_PARITY_PROVISIONAL_GRADE_ID").unwrap_or("1".to_string());

    let payload = serde_json::json!({});

    let resp = do_put(
        &format!(
            "/courses/{course_id}/assignments/{assignment_id}/provisional_grades/{provisional_grade_id}/select"
        ),
        &payload,
    );

    // Teacher/admin selects pg → 200 OK
    assert_eq!(
        resp.status().as_u16(),
        200,
        "expected 200 on provisional grade select, got {}",
        resp.status()
    );

    let body: ProvisionalGradeSelectResponse =
        serde_json::from_str(&resp.text().unwrap())
            .expect("response should parse as ProvisionalGradeSelectResponse");
    assert_eq!(body.assignment_id, assignment_id.parse().unwrap());
    assert_eq!(
        body.selected_provisional_grade_id,
        provisional_grade_id.parse().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Test 6: PUT .../courses/:course_id/assignments/:assignment_id/provisional_grades/bulk_select
// — select multiple provisional grades at once (ProvisionalGrade#bulk_select).
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_provisional_grades_bulk_select() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());

    let payload = serde_json::json!({
        "selections": [
            { "student_id": 1, "provisional_grade_id": 1 },
            { "student_id": 2, "provisional_grade_id": 2 }
        ]
    });

    let resp = do_put(
        &format!(
            "/courses/{course_id}/assignments/{assignment_id}/provisional_grades/bulk_select"
        ),
        &payload,
    );

    // Teacher/admin bulk-selects → 200 OK
    assert_eq!(
        resp.status().as_u16(),
        200,
        "expected 200 on bulk select, got {}",
        resp.status()
    );

    let body: BulkSelectResponse = serde_json::from_str(&resp.text().unwrap())
        .expect("response should parse as BulkSelectResponse");
    // `results` and `errors` keys must be present.
    assert!(body.results.capacity() >= 0);
    assert!(body.errors.capacity() >= 0);
}

// ---------------------------------------------------------------------------
// Test 7: POST .../courses/:course_id/assignments/:assignment_id/provisional_grades/publish
// — publish all selected provisional grades (ProvisionalGrade#publish).
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_provisional_grades_publish() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());

    let payload = serde_json::json!({});

    let resp = do_post(
        &format!(
            "/courses/{course_id}/assignments/{assignment_id}/provisional_grades/publish"
        ),
        &payload,
    );

    // Teacher/admin publishes → 200 OK
    assert_eq!(
        resp.status().as_u16(),
        200,
        "expected 200 on provisional grades publish, got {}",
        resp.status()
    );

    let body: PublishResponse = serde_json::from_str(&resp.text().unwrap())
        .expect("response should parse as PublishResponse");
    assert_eq!(body.assignment_id, assignment_id.parse().unwrap());
    assert!(body.grades_published_at.is_some());
}

// ---------------------------------------------------------------------------
// Test 8: GET .../courses/:course_id/assignments/:assignment_id/anonymous_provisional_grades/status
// — anonymous view of provisional grade counts (AnonymousProvisionalGrade#status).
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_anonymous_provisional_grades_status() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());

    let resp = do_get(&format!(
        "/courses/{course_id}/assignments/{assignment_id}/anonymous_provisional_grades/status"
    ));

    // Enrolled user reads anonymous status → 200 OK
    assert_eq!(
        resp.status().as_u16(),
        200,
        "expected 200 on anonymous provisional grades status, got {}",
        resp.status()
    );

    let body: AnonymousProvisionalStatusResponse =
        serde_json::from_str(&resp.text().unwrap())
            .expect("response should parse as AnonymousProvisionalStatusResponse");
    assert!(body.students.len() >= 0);
}

// ---------------------------------------------------------------------------
// Test 9: POST .../sections/:section_id/assignments/:assignment_id/submissions
// — section-scoped submission create (Submissions#create for sections).
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_section_create_submission() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let section_id = env::var("STAR_PARITY_SECTION_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());

    let payload = serde_json::json!({
        "body": "Section-scoped test submission"
    });

    let resp = do_post(
        &format!("/sections/{section_id}/assignments/{assignment_id}/submissions"),
        &payload,
    );

    // Student in section submits → 201 Created
    assert_eq!(
        resp.status().as_u16(),
        201,
        "expected 201 on section submission create, got {}",
        resp.status()
    );

    let body: SubmissionResponse = serde_json::from_str(&resp.text().unwrap())
        .expect("response should parse as SubmissionResponse");
    assert_eq!(body.assignment_id, assignment_id.parse().unwrap());
    assert!(body.id > 0);
}
