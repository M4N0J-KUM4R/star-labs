//! Live parity tests are ignored by default; run explicitly with `-- --ignored`
//! against a local service. canvas_parity.rs — Canvas API v1 parity tests for
//! the assignment-extensions and section-submissions routes this branch adds.
//!
//! Gated by STAR_PARITY_TESTS=1. Targets live API at http://127.0.0.1:8080.
//!
//! The other moderated-grading routes this branch originally carried
//! (moderated_students, provisional_grades status/select/bulk_select/publish,
//! anonymous_provisional_grades/status) already exist on main and are tested
//! there; only these two routes are new here:
//!   1. POST .../courses/:course_id/assignments/:assignment_id/extensions
//!   2. POST .../sections/:section_id/assignments/:assignment_id/submissions

use reqwest::blocking::{Client, Response};
use serde::Deserialize;
use std::env;

const API_BASE: &str = "http://127.0.0.1:8080/api/v1";

#[derive(Debug, Deserialize)]
pub struct AssignmentExtensionsResponse {
    pub assignment_extensions: Vec<AssignmentExtensionApplied>,
}

#[derive(Debug, Deserialize)]
pub struct AssignmentExtensionApplied {
    pub user_id: i64,
    pub extra_attempts: i64,
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
    env::var("STAR_TEST_TOKEN").unwrap_or_else(|_| "Bearer test-token-placeholder".to_string())
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

// ---------------------------------------------------------------------------
// Test 1: POST .../courses/:course_id/assignments/:assignment_id/extensions
// — Canvas AssignmentExtensionsController#create: array body
//   {assignment_extensions: [{user_id, extra_attempts}]}, extra attempts are
//   stored on the submission row, 200 (not 201) with the applied list.
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
        "assignment_extensions": [{
            "user_id": user_id.parse::<i64>().unwrap(),
            "extra_attempts": 2
        }]
    });

    let resp = do_post(
        &format!("/courses/{course_id}/assignments/{assignment_id}/extensions"),
        &payload,
    );

    // Canvas renders 200 with the applied extensions (controller docs).
    assert_eq!(
        resp.status().as_u16(),
        200,
        "expected 200 on extension create, got {}",
        resp.status()
    );

    let body: AssignmentExtensionsResponse = serde_json::from_str(&resp.text().unwrap())
        .expect("response should parse as AssignmentExtensionsResponse");
    assert_eq!(body.assignment_extensions.len(), 1);
    assert_eq!(
        body.assignment_extensions[0].user_id,
        user_id.parse().unwrap()
    );
    assert_eq!(body.assignment_extensions[0].extra_attempts, 2);
}

// ---------------------------------------------------------------------------
// Test 1b: invalid extension payloads are 400s (missing array key, entry
// missing user_id/extra_attempts) — Canvas reject! paths.
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_PARITY_TESTS=1 and a live Star API at http://127.0.0.1:8080"]
fn test_create_assignment_extension_invalid_payloads() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let assignment_id = env::var("STAR_PARITY_ASSIGNMENT_ID").unwrap_or("1".to_string());

    // Missing :assignment_extensions key entirely.
    let resp = do_post(
        &format!("/courses/{course_id}/assignments/{assignment_id}/extensions"),
        &serde_json::json!({ "user_id": 1 }),
    );
    assert_eq!(
        resp.status().as_u16(),
        400,
        "missing assignment_extensions key must 400, got {}",
        resp.status()
    );

    // Entry missing extra_attempts.
    let resp = do_post(
        &format!("/courses/{course_id}/assignments/{assignment_id}/extensions"),
        &serde_json::json!({ "assignment_extensions": [{ "user_id": 1 }] }),
    );
    assert_eq!(
        resp.status().as_u16(),
        400,
        "entry missing extra_attempts must 400, got {}",
        resp.status()
    );

    // Negative extra_attempts violates the Submission numericality.
    let resp = do_post(
        &format!("/courses/{course_id}/assignments/{assignment_id}/extensions"),
        &serde_json::json!({ "assignment_extensions": [{ "user_id": 1, "extra_attempts": -1 }] }),
    );
    assert_eq!(
        resp.status().as_u16(),
        400,
        "negative extra_attempts must 400, got {}",
        resp.status()
    );
}

// ---------------------------------------------------------------------------
// Test 2: POST .../sections/:section_id/assignments/:assignment_id/submissions
// — section twin of submissions#create (routes.rb context loop).
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
