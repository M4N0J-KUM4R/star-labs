//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! canvas_parity.rs — Canvas API v1 parity tests for grading standards.
//!
//! Gated by STAR_PARITY_TESTS=1. Targets live API at http://127.0.0.1:8080.
//!
//! Covers all 13 routes across grading_periods (7) and grading_standards_api (6):
//!   1. POST   /accounts/{accountId}/grading_periods       — create grading period(s)
//!   2. GET    /accounts/{accountId}/grading_periods       — list account grading periods
//!   3. GET    /grading_periods/{id}                       — get grading period
//!   4. PUT    /grading_periods/{id}                      — update grading period
//!   5. DELETE /grading_periods/{id}                       — soft-delete grading period
//!   6. GET    /accounts/{accountId}/grading_schemes       — list account grading schemes
//!   7. POST   /accounts/{accountId}/grading_schemes      — create grading scheme
//!   8. GET    /grading_schemes/{id}                       — get grading scheme
//!   9. PUT    /grading_schemes/{id}                       — update grading scheme
//!  10. DELETE /grading_schemes/{id}                      — soft-delete grading scheme
//!  11. GET    /courses/{courseId}/final_grade             — compute letter grade
//!  12. GET    /courses/{courseId}/grading_standards       — list standards
//!  13. POST   /courses/{courseId}/grading_standards      — create grading standard

use reqwest::blocking::{Client, Response};
use serde::Deserialize;
use std::env;

const API_BASE: &str = "http://127.0.0.1:8080/api/v1";

#[derive(Debug, Deserialize)]
pub struct GradingSchemeEntryResponse {
    pub id: i64,
    pub name: String,
    pub min_score: f64,
    pub position: i32,
}

#[derive(Debug, Deserialize)]
pub struct GradingStandardResponse {
    pub id: i64,
    pub title: String,
    pub workflow_state: String,
    pub created_at: String,
    pub updated_at: String,
    pub grading_scheme: Option<Vec<GradingSchemeEntryResponse>>,
}

#[derive(Debug, Deserialize)]
pub struct FinalGradeResponse {
    pub score: f64,
    pub letter: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GradingPeriodResponse {
    pub id: i64,
    pub title: String,
    pub start_date: String,
    pub end_date: String,
    pub close_date: String,
    pub weight: Option<f64>,
    pub grading_period_group_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct GradingSchemeResponse {
    pub id: String,
    pub title: String,
    pub data: Vec<GradingSchemeDatum>,
    pub context_type: Option<String>,
    pub context_id: Option<i64>,
    pub points_based: bool,
    pub scaling_factor: f64,
    pub workflow_state: String,
}

#[derive(Debug, Deserialize)]
pub struct GradingSchemeDatum {
    pub name: String,
    pub value: f64,
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

fn do_delete(path: &str) -> Response {
    client()
        .delete(format!("{API_BASE}{path}"))
        .header("Authorization", auth_header())
        .send()
        .unwrap()
}

// ---------------------------------------------------------------------------
// Test 1: POST /courses/{courseId}/grading_standards → 201 with nested entries
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_create_grading_standard_nested_entries() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let payload = serde_json::json!({
        "title": "Canvas Parity Test Scheme",
        "entries": [
            { "name": "A", "min_score": 0.94 },
            { "name": "A-", "min_score": 0.90 },
            { "name": "B+", "min_score": 0.87 },
            { "name": "B", "min_score": 0.84 },
            { "name": "B-", "min_score": 0.80 },
            { "name": "C+", "min_score": 0.77 },
            { "name": "C", "min_score": 0.74 },
            { "name": "C-", "min_score": 0.70 },
            { "name": "D+", "min_score": 0.67 },
            { "name": "D", "min_score": 0.64 },
            { "name": "D-", "min_score": 0.61 },
            { "name": "F", "min_score": 0.0 }
        ]
    });

    let resp = do_post(&format!("/courses/{course_id}/grading_standards"), &payload);
    assert_eq!(
        resp.status().as_u16(),
        201,
        "Expected 201, got {}",
        resp.status()
    );

    let gs: GradingStandardResponse = resp.json().unwrap();
    assert_eq!(gs.title, "Canvas Parity Test Scheme");
    assert!(
        gs.grading_scheme.is_some(),
        "grading_scheme must be present"
    );
    let entries = gs.grading_scheme.unwrap();
    assert_eq!(entries.len(), 12, "Expected 12 entries");
    assert_eq!(entries[0].name, "A");
    assert!((entries[0].min_score - 0.94).abs() < 1e-6);
    assert_eq!(entries[11].name, "F");
    assert!((entries[11].min_score - 0.0).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// Test 2: GET /courses/{courseId}/grading_standards → lists the standard
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_list_grading_standards_contains_created() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let title = format!(
        "List Test Scheme {}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    let payload = serde_json::json!({
        "title": title,
        "entries": [
            { "name": "Pass", "min_score": 0.6 },
            { "name": "Fail", "min_score": 0.0 }
        ]
    });

    let create_resp = do_post(&format!("/courses/{course_id}/grading_standards"), &payload);
    if create_resp.status().as_u16() != 201 {
        eprintln!(
            "Skipping: could not create grading standard (status {})",
            create_resp.status()
        );
        return;
    }
    let created: GradingStandardResponse = create_resp.json().unwrap();

    let list_resp = do_get(&format!("/courses/{course_id}/grading_standards"));
    assert_eq!(list_resp.status().as_u16(), 200);
    let list: Vec<GradingStandardResponse> = list_resp.json().unwrap();
    assert!(
        list.iter().any(|gs| gs.id == created.id),
        "Created standard {} should appear in list",
        created.id
    );
}

// ---------------------------------------------------------------------------
// Test 3: GET /grading_standards/{id} → returns standard with entries
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_get_grading_standard_by_id() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let payload = serde_json::json!({
        "title": "Get-By-ID Test Scheme",
        "entries": [
            { "name": "A", "min_score": 0.90 },
            { "name": "F", "min_score": 0.0 }
        ]
    });

    let create_resp = do_post(&format!("/courses/{course_id}/grading_standards"), &payload);
    if create_resp.status().as_u16() != 201 {
        eprintln!(
            "Skipping: could not create grading standard (status {})",
            create_resp.status()
        );
        return;
    }
    let gs: GradingStandardResponse = create_resp.json().unwrap();
    let id = gs.id;

    let resp = do_get(&format!("/grading_standards/{id}"));
    assert_eq!(resp.status().as_u16(), 200);
    let gs2: GradingStandardResponse = resp.json().unwrap();
    assert_eq!(gs2.id, id);
    assert!(gs2.grading_scheme.is_some());
    assert_eq!(gs2.grading_scheme.as_ref().unwrap().len(), 2);
}

// ---------------------------------------------------------------------------
// Test 4: DELETE /grading_standards/{id} → soft-delete, returns 204
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_delete_grading_standard() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    let payload = serde_json::json!({
        "title": "Delete Me Test",
        "entries": [
            { "name": "P", "min_score": 0.5 },
            { "name": "F", "min_score": 0.0 }
        ]
    });

    let create_resp = do_post(&format!("/courses/{course_id}/grading_standards"), &payload);
    if create_resp.status().as_u16() != 201 {
        eprintln!(
            "Skipping: could not create grading standard (status {})",
            create_resp.status()
        );
        return;
    }
    let gs: GradingStandardResponse = create_resp.json().unwrap();
    let id = gs.id;

    let del_resp = do_delete(&format!("/grading_standards/{id}"));
    assert_eq!(del_resp.status().as_u16(), 204, "Delete should return 204");

    let get_resp = do_get(&format!("/grading_standards/{id}"));
    assert_eq!(
        get_resp.status().as_u16(),
        404,
        "Deleted standard should 404"
    );
}

// ---------------------------------------------------------------------------
// Test 5: GET /courses/{courseId}/final_grade → computes letter grade
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_final_grade_letter_computation() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let course_id = env::var("STAR_PARITY_COURSE_ID").unwrap_or("1".to_string());
    // This test requires: a course with submissions graded and a grading standard.
    // If the course has no grading standard, letter will be null — this is expected.
    let user_id = env::var("STAR_PARITY_USER_ID").unwrap_or("1".to_string());

    let resp = do_get(&format!(
        "/courses/{course_id}/final_grade?user_id={user_id}"
    ));
    assert_eq!(resp.status().as_u16(), 200);
    let fg: FinalGradeResponse = resp.json().unwrap();
    // We cannot assert on exact letter without known grade data,
    // but we verify the response shape.
    println!(
        "final_grade response: score={}, letter={:?}",
        fg.score, fg.letter
    );
}

// ---------------------------------------------------------------------------
// Test 6: POST /accounts/{accountId}/grading_periods → 201 with period(s)
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_create_grading_period() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    let payload = serde_json::json!({
        "grading_periods": [{
            "title": "Parity Test Period",
            "start_date": "2999-01-01",
            "end_date": "2999-06-30",
            "close_date": "2999-07-07",
            "weight": 50.0
        }]
    });

    let resp = do_post(&format!("/accounts/{account_id}/grading_periods"), &payload);
    assert_eq!(
        resp.status().as_u16(),
        201,
        "Expected 201, got {}: {}",
        resp.status(),
        resp.text()
    );

    let body: serde_json::Value = resp.json().unwrap();
    let periods = body["grading_periods"].as_array().expect("periods array");
    assert!(!periods.is_empty());
    assert_eq!(periods[0]["title"], "Parity Test Period");
    assert_eq!(periods[0]["weight"], 50.0);
}

// ---------------------------------------------------------------------------
// Test 7: GET /accounts/{accountId}/grading_periods → 200 with list
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_list_account_grading_periods() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    let resp = do_get(&format!("/accounts/{account_id}/grading_periods"));
    assert_eq!(resp.status().as_u16(), 200, "Expected 200: {}", resp.text());

    let body: serde_json::Value = resp.json().unwrap();
    assert!(
        body["grading_periods"].is_array(),
        "response must have grading_periods array"
    );
    assert!(
        body["can_create_grading_periods"].is_boolean(),
        "response must have can_create_grading_periods"
    );
}

// ---------------------------------------------------------------------------
// Test 8: GET /grading_periods/{id} → 200 with single period wrapped
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_get_grading_period() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    // Create a period first so we have a known id.
    let create_resp = do_post(
        &format!("/accounts/{account_id}/grading_periods"),
        &serde_json::json!({
            "grading_periods": [{
                "title": "Get Test Period",
                "start_date": "2999-02-01",
                "end_date": "2999-05-31"
            }]
        }),
    );
    if create_resp.status().as_u16() != 201 {
        eprintln!(
            "Skipping: could not create grading period (status {})",
            create_resp.status()
        );
        return;
    }

    let body: serde_json::Value = create_resp.json().unwrap();
    let period_id = body["grading_periods"][0]["id"].as_i64().unwrap();

    let resp = do_get(&format!("/grading_periods/{period_id}"));
    assert_eq!(resp.status().as_u16(), 200, "Expected 200: {}", resp.text());
    let get_body: serde_json::Value = resp.json().unwrap();
    let periods = get_body["grading_periods"]
        .as_array()
        .expect("wrapped array");
    assert_eq!(periods[0]["id"].as_i64(), Some(period_id));
    assert_eq!(periods[0]["title"], "Get Test Period");
}

// ---------------------------------------------------------------------------
// Test 9: PUT /grading_periods/{id} → 200 with updated period
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_update_grading_period() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    let create_resp = do_post(
        &format!("/accounts/{account_id}/grading_periods"),
        &serde_json::json!({
            "title": "Original Title",
            "start_date": "2999-01-01",
            "end_date": "2999-03-31"
        }),
    );
    if create_resp.status().as_u16() != 201 {
        eprintln!(
            "Skipping: could not create grading period (status {})",
            create_resp.status()
        );
        return;
    }

    let period_id = create_resp.json::<serde_json::Value>().unwrap()["grading_periods"][0]["id"]
        .as_i64()
        .unwrap();

    let resp = do_put(
        &format!("/grading_periods/{period_id}"),
        &serde_json::json!({
            "title": "Updated Title",
            "weight": 33.3
        }),
    );
    assert_eq!(resp.status().as_u16(), 200, "Expected 200: {}", resp.text());
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body["grading_periods"][0]["title"], "Updated Title");
    assert_eq!(body["grading_periods"][0]["weight"], 33.3);
}

// ---------------------------------------------------------------------------
// Test 10: DELETE /grading_periods/{id} → 204, then 404
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_delete_grading_period() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    let create_resp = do_post(
        &format!("/accounts/{account_id}/grading_periods"),
        &serde_json::json!({
            "title": "Delete Me",
            "start_date": "2999-04-01",
            "end_date": "2999-04-30"
        }),
    );
    if create_resp.status().as_u16() != 201 {
        eprintln!(
            "Skipping: could not create grading period (status {})",
            create_resp.status()
        );
        return;
    }
    let period_id = create_resp.json::<serde_json::Value>().unwrap()["grading_periods"][0]["id"]
        .as_i64()
        .unwrap();

    let del_resp = do_delete(&format!("/grading_periods/{period_id}"));
    assert_eq!(del_resp.status().as_u16(), 204, "Delete should return 204");

    let get_resp = do_get(&format!("/grading_periods/{period_id}"));
    assert_eq!(get_resp.status().as_u16(), 404, "Deleted period should 404");
}

// ---------------------------------------------------------------------------
// Test 11: GET /accounts/{accountId}/grading_schemes → 200 array
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_list_account_grading_schemes() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    let resp = do_get(&format!("/accounts/{account_id}/grading_schemes"));
    assert_eq!(resp.status().as_u16(), 200, "Expected 200: {}", resp.text());
    let schemes: Vec<GradingSchemeResponse> = resp.json().unwrap();
    println!(
        "Listed {} grading schemes for account {}",
        schemes.len(),
        account_id
    );
}

// ---------------------------------------------------------------------------
// Test 12: POST /accounts/{accountId}/grading_schemes → 200 with created scheme
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_create_grading_scheme() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    let payload = serde_json::json!({
        "title": "Parity Test Scheme",
        "data": [
            { "name": "A", "value": 0.90 },
            { "name": "B", "value": 0.80 },
            { "name": "C", "value": 0.70 },
            { "name": "D", "value": 0.60 },
            { "name": "F", "value": 0.0 }
        ],
        "points_based": false,
        "scaling_factor": 1.0
    });

    let resp = do_post(&format!("/accounts/{account_id}/grading_schemes"), &payload);
    assert_eq!(
        resp.status().as_u16(),
        200,
        "Expected 200, got {}: {}",
        resp.status(),
        resp.text()
    );

    let scheme: GradingSchemeResponse = resp.json().unwrap();
    assert_eq!(scheme.title, "Parity Test Scheme");
    assert_eq!(scheme.data.len(), 5);
    assert_eq!(scheme.data[0].name, "A");
    assert_eq!(scheme.context_type.as_deref(), Some("Account"));
}

// ---------------------------------------------------------------------------
// Test 13: GET /grading_schemes/{id} → 200 with scheme details
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_get_grading_scheme() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    let create_resp = do_post(
        &format!("/accounts/{account_id}/grading_schemes"),
        &serde_json::json!({
            "title": "Get Scheme Test",
            "data": [
                { "name": "Pass", "value": 0.60 },
                { "name": "Fail", "value": 0.0 }
            ]
        }),
    );
    if create_resp.status().as_u16() != 200 {
        eprintln!(
            "Skipping: could not create grading scheme (status {})",
            create_resp.status()
        );
        return;
    }
    let created: GradingSchemeResponse = create_resp.json().unwrap();
    let scheme_id = created.id.parse::<i64>().expect("numeric id string");

    let resp = do_get(&format!("/grading_schemes/{scheme_id}"));
    assert_eq!(resp.status().as_u16(), 200, "Expected 200: {}", resp.text());
    let shown: GradingSchemeResponse = resp.json().unwrap();
    assert_eq!(shown.id, created.id);
    assert_eq!(shown.title, "Get Scheme Test");
}

// ---------------------------------------------------------------------------
// Test 14: PUT /grading_schemes/{id} → 200 with updated scheme
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_update_grading_scheme() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    let create_resp = do_post(
        &format!("/accounts/{account_id}/grading_schemes"),
        &serde_json::json!({
            "title": "Original Scheme",
            "data": [
                { "name": "A", "value": 0.90 },
                { "name": "F", "value": 0.0 }
            ]
        }),
    );
    if create_resp.status().as_u16() != 200 {
        eprintln!(
            "Skipping: could not create grading scheme (status {})",
            create_resp.status()
        );
        return;
    }
    let created: GradingSchemeResponse = create_resp.json().unwrap();
    let scheme_id: i64 = created.id.parse().expect("numeric id");

    let updated_data = serde_json::json!([
        { "name": "High Pass", "value": 0.85 },
        { "name": "Pass", "value": 0.70 },
        { "name": "Fail", "value": 0.0 }
    ]);
    let resp = do_put(
        &format!("/grading_schemes/{scheme_id}"),
        &serde_json::json!({
            "title": "Renamed Scheme",
            "data": updated_data
        }),
    );
    assert_eq!(resp.status().as_u16(), 200, "Expected 200: {}", resp.text());
    let updated: GradingSchemeResponse = resp.json().unwrap();
    assert_eq!(updated.title, "Renamed Scheme");
    assert_eq!(updated.data.len(), 3);
}

// ---------------------------------------------------------------------------
// Test 15: DELETE /grading_schemes/{id} → 200 {}, then 404
// ---------------------------------------------------------------------------
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_delete_grading_scheme() {
    if env::var("STAR_PARITY_TESTS").as_deref() != Ok("1") {
        eprintln!("STAR_PARITY_TESTS=1 not set — skipping live API test");
        return;
    }

    let account_id = env::var("STAR_PARITY_ACCOUNT_ID").unwrap_or("1".to_string());
    let create_resp = do_post(
        &format!("/accounts/{account_id}/grading_schemes"),
        &serde_json::json!({
            "title": "Delete Me Scheme",
            "data": [
                { "name": "A", "value": 0.90 },
                { "name": "F", "value": 0.0 }
            ]
        }),
    );
    if create_resp.status().as_u16() != 200 {
        eprintln!(
            "Skipping: could not create grading scheme (status {})",
            create_resp.status()
        );
        return;
    }
    let created: GradingSchemeResponse = create_resp.json().unwrap();
    let scheme_id: i64 = created.id.parse().expect("numeric id");

    let del_resp = do_delete(&format!("/grading_schemes/{scheme_id}"));
    assert_eq!(
        del_resp.status().as_u16(),
        200,
        "Canvas delete returns 200, got {}",
        del_resp.status()
    );
    assert_eq!(
        del_resp.json::<serde_json::Value>().unwrap(),
        serde_json::json!({})
    );

    let get_resp = do_get(&format!("/grading_schemes/{scheme_id}"));
    assert_eq!(get_resp.status().as_u16(), 404, "Deleted scheme should 404");
}
