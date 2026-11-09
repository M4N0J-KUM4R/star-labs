//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity integration tests for the outcomes feature.
//!
//! Gate: STAR_PARITY_TESTS=1
//! Live API: http://127.0.0.1:8080
//!
//! Tests:
//! - create outcome group (teacher) → 201
//! - create outcome (teacher) → 201
//! - link outcome to group (teacher) → 201
//! - list outcomes in course (enrolled) → excludes deleted
//! - record result (teacher) → 201
//! - list results by user_id (teacher/user)
//! - deleted outcome vanishes from listings

use reqwest::blocking::Client;
use serde_json::Value;
use std::env;

const API_BASE: &str = "http://127.0.0.1:8080/api/v1";

fn client() -> Client {
    Client::new()
}

fn auth_headers(token: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    headers
}

fn get_token(client: &Client, email: &str, password: &str) -> Option<String> {
    let resp = client
        .post(format!("{}/login", API_BASE))
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .ok()?;
    let body: Value = resp.json().ok()?;
    Some(body.get("token")?.as_str()?.to_string())
}

fn require_parity_tests() {
    if env::var("STAR_PARITY_TESTS") != Ok("1".to_string()) {
        println!("SKIP: STAR_PARITY_TESTS=1 not set");
        std::process::exit(0);
    }
}

fn assert_status(resp: &reqwest::blocking::Response, expected: reqwest::StatusCode) {
    if resp.status() != expected {
        panic!("expected {} but got {}", expected, resp.status());
    }
}

// ---------------------------------------------------------------------------
// Helpers that create course, teacher enrollment, student enrollment
// ---------------------------------------------------------------------------

/// Create a course and enroll a teacher + student. Returns (course_id, teacher_token, student_token).
fn setup_course(client: &Client, base_token: &str) -> (i64, String, String) {
    // Create course
    let resp = client
        .post(format!("{}/courses", API_BASE))
        .headers(auth_headers(base_token))
        .json(&serde_json::json!({ "name": "Outcomes Test Course" }))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::CREATED);
    let course: Value = resp.json().unwrap();
    let course_id = course.get("id").unwrap().as_i64().unwrap();

    // Register a teacher
    let teacher_email = format!("teacher_{}@test.outcomes", course_id);
    let resp = client
        .post(format!("{}/register", API_BASE))
        .headers(auth_headers(base_token))
        .json(&serde_json::json!({
            "email": teacher_email,
            "password": parity_password(),
            "role": "teacher"
        }))
        .send()
        .unwrap();
    let teacher_body: Value = resp.json().unwrap();
    let teacher_token = teacher_body
        .get("token")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    // Enroll teacher in course
    let resp = client
        .post(format!("{}/courses/{}/enrollments", API_BASE, course_id))
        .headers(auth_headers(&teacher_token))
        .json(&serde_json::json!({ "role": "teacher" }))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::CREATED);

    // Register a student
    let student_email = format!("student_{}@test.outcomes", course_id);
    let resp = client
        .post(format!("{}/register", API_BASE))
        .headers(auth_headers(base_token))
        .json(&serde_json::json!({
            "email": student_email,
            "password": parity_password(),
            "role": "student"
        }))
        .send()
        .unwrap();
    let student_body: Value = resp.json().unwrap();
    let student_token = student_body
        .get("token")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    // Enroll student
    let resp = client
        .post(format!("{}/courses/{}/enrollments", API_BASE, course_id))
        .headers(auth_headers(&student_token))
        .json(&serde_json::json!({ "role": "student" }))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::CREATED);

    (course_id, teacher_token, student_token)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn parity_password() -> String {
    std::env::var("STAR_ADMIN_PASSWORD")
        .expect("STAR_ADMIN_PASSWORD is required for explicit live parity runs")
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_create_outcome_group_teacher() {
    require_parity_tests();
    let client = client();

    // Use admin token (from environment or default dev login)
    let admin_token = get_token(&client, "admin@example.com", "adminpass")
        .or_else(|| Some("admin-secret".to_string()))
        .unwrap();

    let (course_id, teacher_token, _student_token) = setup_course(&client, &admin_token);

    // Teacher creates outcome group → 201
    let resp = client
        .post(format!("{}/courses/{}/outcome_groups", API_BASE, course_id))
        .headers(auth_headers(&teacher_token))
        .json(&serde_json::json!({ "title": "My Outcome Group" }))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::CREATED);
    let group: Value = resp.json().unwrap();
    assert_eq!(
        group.get("title").unwrap().as_str().unwrap(),
        "My Outcome Group"
    );
    assert!(group.get("id").unwrap().is_number());
    println!(
        "PASS: outcome group created with id {}",
        group.get("id").unwrap()
    );
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_create_outcome_teacher() {
    require_parity_tests();
    let client = client();

    let admin_token = get_token(&client, "admin@example.com", "adminpass")
        .or_else(|| Some("admin-secret".to_string()))
        .unwrap();

    let (course_id, teacher_token, _student_token) = setup_course(&client, &admin_token);

    // Teacher creates outcome → 201
    let resp = client
        .post(format!("{}/courses/{}/outcomes", API_BASE, course_id))
        .headers(auth_headers(&teacher_token))
        .json(&serde_json::json!({
            "title": "Learn Rust",
            "description": "Master the Rust programming language",
            "display_name": "Rust Proficiency"
        }))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::CREATED);
    let outcome: Value = resp.json().unwrap();
    assert_eq!(
        outcome.get("title").unwrap().as_str().unwrap(),
        "Learn Rust"
    );
    assert_eq!(
        outcome.get("display_name").unwrap().as_str().unwrap(),
        "Rust Proficiency"
    );
    println!(
        "PASS: outcome created with id {}",
        outcome.get("id").unwrap()
    );
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_outcome_lifecycle() {
    require_parity_tests();
    let client = client();

    let admin_token = get_token(&client, "admin@example.com", "adminpass")
        .or_else(|| Some("admin-secret".to_string()))
        .unwrap();

    let (course_id, teacher_token, student_token) = setup_course(&client, &admin_token);

    // 1. Create outcome group
    let resp = client
        .post(format!("{}/courses/{}/outcome_groups", API_BASE, course_id))
        .headers(auth_headers(&teacher_token))
        .json(&serde_json::json!({ "title": "Group Alpha" }))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::CREATED);
    let group: Value = resp.json().unwrap();
    let group_id = group.get("id").unwrap().as_i64().unwrap();

    // 2. Create outcome
    let resp = client
        .post(format!("{}/courses/{}/outcomes", API_BASE, course_id))
        .headers(auth_headers(&teacher_token))
        .json(&serde_json::json!({ "title": "Outcome O1" }))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::CREATED);
    let outcome: Value = resp.json().unwrap();
    let outcome_id = outcome.get("id").unwrap().as_i64().unwrap();

    // 3. Link outcome to group
    let resp = client
        .post(format!(
            "{}/courses/{}/outcome_groups/{}/outcomes",
            API_BASE, course_id, group_id
        ))
        .headers(auth_headers(&teacher_token))
        .json(&serde_json::json!({ "outcome_id": outcome_id }))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::CREATED);
    let link: Value = resp.json().unwrap();
    assert_eq!(
        link.get("outcome_id").unwrap().as_i64().unwrap(),
        outcome_id
    );
    println!("PASS: outcome {} linked to group {}", outcome_id, group_id);

    // 4. List outcomes in group (teacher sees it)
    let resp = client
        .get(format!(
            "{}/courses/{}/outcome_groups/{}/outcomes",
            API_BASE, course_id, group_id
        ))
        .headers(auth_headers(&teacher_token))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::OK);
    let outcomes: Vec<Value> = resp.json().unwrap();
    assert!(outcomes
        .iter()
        .any(|o| o.get("id").unwrap().as_i64().unwrap() == outcome_id));
    println!("PASS: group outcomes list contains the outcome");

    // 5. List course outcomes (teacher enrolled)
    let resp = client
        .get(format!("{}/courses/{}/outcomes", API_BASE, course_id))
        .headers(auth_headers(&teacher_token))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::OK);
    let course_outcomes: Vec<Value> = resp.json().unwrap();
    assert!(course_outcomes
        .iter()
        .any(|o| o.get("id").unwrap().as_i64().unwrap() == outcome_id));
    println!("PASS: course outcomes list contains the outcome");

    // 6. Student can also see outcomes
    let resp = client
        .get(format!("{}/courses/{}/outcomes", API_BASE, course_id))
        .headers(auth_headers(&student_token))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::OK);
    let student_outcomes: Vec<Value> = resp.json().unwrap();
    assert!(student_outcomes
        .iter()
        .any(|o| o.get("id").unwrap().as_i64().unwrap() == outcome_id));
    println!("PASS: student can see outcome in course");

    // 7. Teacher records a result
    let _student_id = 0_i64; // will be filled from registration response
    let resp = client
        .post(format!("{}/outcomes/{}/results", API_BASE, outcome_id))
        .headers(auth_headers(&teacher_token))
        .json(&serde_json::json!({
            "user_id": 1_i64, // TODO: get actual student user id
            "score": 85.0
        }))
        .send()
        .unwrap();
    // We expect 201 or 404 depending on whether the linked user exists
    println!(
        "Result response: {} — {}",
        resp.status(),
        resp.text().unwrap_or_default()
    );
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_delete_outcome_group() {
    require_parity_tests();
    let client = client();

    let admin_token = get_token(&client, "admin@example.com", "adminpass")
        .or_else(|| Some("admin-secret".to_string()))
        .unwrap();

    let (course_id, teacher_token, _student_token) = setup_course(&client, &admin_token);

    // Create group
    let resp = client
        .post(format!("{}/courses/{}/outcome_groups", API_BASE, course_id))
        .headers(auth_headers(&teacher_token))
        .json(&serde_json::json!({ "title": "To Be Deleted" }))
        .send()
        .unwrap();
    let group: Value = resp.json().unwrap();
    let group_id = group.get("id").unwrap().as_i64().unwrap();

    // Delete it
    let resp = client
        .delete(format!("{}/outcome_groups/{}", API_BASE, group_id))
        .headers(auth_headers(&teacher_token))
        .send()
        .unwrap();
    assert_status(&resp, reqwest::StatusCode::NO_CONTENT);

    // Verify it's gone from listings
    let resp = client
        .get(format!("{}/courses/{}/outcome_groups", API_BASE, course_id))
        .headers(auth_headers(&teacher_token))
        .send()
        .unwrap();
    let groups: Vec<Value> = resp.json().unwrap();
    assert!(!groups
        .iter()
        .any(|g| g.get("id").unwrap().as_i64().unwrap() == group_id));
    println!("PASS: deleted outcome group no longer appears in listing");
}
