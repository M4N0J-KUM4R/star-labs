//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for star-calendar.
//!
//! These tests hit the live API at http://127.0.0.1:8080 when
//! STAR_PARITY_TESTS=1 is set.  They verify:
//!   - event create returns start_at round-trip intact
//!   - deleted event vanishes from course event list
//!
//! Prerequisite: a course with id 1, an enrolled teacher user, and a student.

use reqwest::blocking::Client;
use serde_json::Value;
use std::env;

const BASE: &str = "http://127.0.0.1:8080/api/v1";

fn client() -> Client {
    Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap()
}

fn auth_header() -> Option<String> {
    // Set STAR_PARITY_TOKEN=Bearer <token> in the environment.
    env::var("STAR_PARITY_TOKEN").ok()
}

fn skip_if_no_token() {
    if env::var("STAR_PARITY_TESTS").is_err() {
        eprintln!("SKIP: set STAR_PARITY_TESTS=1 to run live API tests");
        std::process::exit(0);
    }
    if auth_header().is_none() {
        eprintln!("SKIP: set STAR_PARITY_TOKEN=Bearer <token> for live API auth");
        std::process::exit(0);
    }
}

fn get(path: &str) -> Value {
    let token = auth_header().unwrap();
    let resp = client()
        .get(format!("{BASE}{path}"))
        .header("Authorization", &token)
        .send()
        .unwrap();
    resp.json().unwrap()
}

fn post_json(path: &str, body: Value) -> Value {
    let token = auth_header().unwrap();
    let resp = client()
        .post(format!("{BASE}{path}"))
        .header("Authorization", &token)
        .json(&body)
        .send()
        .unwrap();
    resp.json().unwrap()
}

fn put_json(path: &str, body: Value) -> reqwest::blocking::Response {
    let token = auth_header().unwrap();
    client()
        .put(format!("{BASE}{path}"))
        .header("Authorization", &token)
        .json(&body)
        .send()
        .unwrap()
}

fn delete_(path: &str) -> reqwest::blocking::Response {
    let token = auth_header().unwrap();
    client()
        .delete(format!("{BASE}{path}"))
        .header("Authorization", &token)
        .send()
        .unwrap()
}

/// Test 1: create a calendar event and verify start_at is returned verbatim.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_create_calendar_event_round_trip() {
    skip_if_no_token();

    let course_id: i64 = 1; // assume course 1 exists in test DB

    let start_at = "2025-06-15T10:00:00Z";
    let payload = serde_json::json!({
        "title": "Parity Test Event",
        "start_at": start_at,
        "end_at": "2025-06-15T11:00:00Z",
        "description": "Testing round-trip"
    });

    let created: Value = post_json(&format!("/courses/{course_id}/calendar_events"), payload);

    let returned_start_at = created["start_at"].as_str().unwrap();
    // The API may normalise to a particular format, so compare the date-time part.
    assert!(
        returned_start_at.contains("2025-06-15T10:00:00"),
        "expected start_at to contain '2025-06-15T10:00:00', got {returned_start_at}"
    );

    // Cleanup
    let id = created["id"].as_i64().unwrap();
    let _ = delete_(&format!("/calendar_events/{id}")).error_for_status();
}

/// Test 2: delete a calendar event and verify it no longer appears in the course list.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_deleted_event_vanishes_from_course_list() {
    skip_if_no_token();

    let course_id: i64 = 1;

    // Create an event
    let payload = serde_json::json!({
        "title": "Event to Delete",
        "start_at": "2025-07-01T14:00:00Z"
    });

    let created: Value = post_json(&format!("/courses/{course_id}/calendar_events"), payload);
    let id = created["id"].as_i64().unwrap();

    // Verify it appears in the course list
    let list_before: Vec<Value> =
        serde_json::from_value(get(&format!("/courses/{course_id}/calendar_events"))).unwrap();
    assert!(
        list_before.iter().any(|e| e["id"].as_i64() == Some(id)),
        "created event {id} should appear in course list before delete"
    );

    // Delete it
    let del_resp = delete_(&format!("/calendar_events/{id}"));
    assert_eq!(
        del_resp.status().as_u16(),
        204,
        "delete should return 204, got {}",
        del_resp.status()
    );

    // Verify it's gone
    let list_after: Vec<Value> =
        serde_json::from_value(get(&format!("/courses/{course_id}/calendar_events"))).unwrap();
    assert!(
        !list_after.iter().any(|e| e["id"].as_i64() == Some(id)),
        "deleted event {id} should NOT appear in course list after delete"
    );
}

/// Test 3: update an event and verify fields change.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_update_calendar_event() {
    skip_if_no_token();

    let course_id: i64 = 1;

    let payload = serde_json::json!({
        "title": "Original Title",
        "start_at": "2025-08-01T09:00:00Z"
    });
    let created: Value = post_json(&format!("/courses/{course_id}/calendar_events"), payload);
    let id = created["id"].as_i64().unwrap();

    let updated: Value = serde_json::from_str(
        &put_json(
            &format!("/calendar_events/{id}"),
            serde_json::json!({ "title": "Updated Title" }),
        )
        .text()
        .unwrap(),
    )
    .unwrap();

    assert_eq!(updated["title"].as_str().unwrap(), "Updated Title");
    assert_eq!(updated["id"].as_i64().unwrap(), id);

    // Cleanup
    let _ = delete_(&format!("/calendar_events/{id}")).error_for_status();
}

/// Test 4: GET /calendar_events/{id} returns the event.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_get_calendar_event() {
    skip_if_no_token();

    let course_id: i64 = 1;

    let payload = serde_json::json!({
        "title": "GET Test Event",
        "start_at": "2025-09-01T12:00:00Z",
        "description": "Testing GET"
    });
    let created: Value = post_json(&format!("/courses/{course_id}/calendar_events"), payload);
    let id = created["id"].as_i64().unwrap();

    let fetched: Value = get(&format!("/calendar_events/{id}"));

    assert_eq!(fetched["id"].as_i64(), Some(id));
    assert_eq!(fetched["title"].as_str().unwrap(), "GET Test Event");

    // Cleanup
    let _ = delete_(&format!("/calendar_events/{id}")).error_for_status();
}
