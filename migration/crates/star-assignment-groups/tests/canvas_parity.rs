//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for assignment groups.
//!
//! These tests call the live API at STAR_PARITY_API_URL (default: http://127.0.0.1:8080).
//! Set STAR_PARITY_TESTS=1 to enable them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct AssignmentGroup {
    pub id: i64,
    pub course_id: i64,
    pub name: String,
    pub position: i32,
    pub group_weight: f64,
    pub workflow_state: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateAssignmentGroupRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_weight: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateAssignmentGroupRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_weight: Option<f64>,
}

fn api_url() -> String {
    std::env::var("STAR_PARITY_API_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
}

fn auth_token() -> String {
    std::env::var("STAR_PARITY_TOKEN").unwrap_or_else(|_| {
        // Default dev token for local testing
        "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOjEsInJvbGUiOiJhZG1pbiIsImV4cCI6OTk5OTk5OTk5OX0.foobar"
            .to_string()
    })
}

fn course_id() -> i64 {
    std::env::var("STAR_PARITY_COURSE_ID")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap_or(1)
}

fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap()
}

/// Test: group create returns workflow_state available.
/// POST /courses/{courseId}/assignment_groups → 201, workflow_state = "available"
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_create_returns_workflow_state_available() {
    if std::env::var("STAR_PARITY_TESTS").is_err() {
        return;
    }

    let client = http_client();
    let url = format!(
        "{}/api/v1/courses/{}/assignment_groups",
        api_url(),
        course_id()
    );

    let request = CreateAssignmentGroupRequest {
        name: "Test Group Parity".to_string(),
        group_weight: Some(25.0),
        position: Some(1),
    };

    let resp = client
        .post(&url)
        .header("Authorization", auth_token())
        .json(&request)
        .send()
        .unwrap();

    assert_eq!(
        resp.status().as_u16(),
        201,
        "create should return 201, got {}: {:?}",
        resp.status().as_u16(),
        resp.text()
    );

    let group: AssignmentGroup = resp.json().unwrap();
    assert_eq!(
        group.workflow_state, "available",
        "newly created group should have workflow_state=available, got {}",
        group.workflow_state
    );

    // Cleanup: delete the group
    let delete_url = format!(
        "{}/api/v1/courses/{}/assignment_groups/{}",
        api_url(),
        course_id(),
        group.id
    );
    let _ = client
        .delete(&delete_url)
        .header("Authorization", auth_token())
        .send();
}

/// Test: deleted group vanishes from list.
/// DELETE /courses/{courseId}/assignment_groups/{id} → 204, then GET list excludes it.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_deleted_group_vanishes_from_list() {
    if std::env::var("STAR_PARITY_TESTS").is_err() {
        return;
    }

    let client = http_client();
    let course_id = course_id();
    let url = format!(
        "{}/api/v1/courses/{}/assignment_groups",
        api_url(),
        course_id
    );

    // Create a group
    let request = CreateAssignmentGroupRequest {
        name: "Delete Test Group".to_string(),
        group_weight: None,
        position: None,
    };

    let resp = client
        .post(&url)
        .header("Authorization", auth_token())
        .json(&request)
        .send()
        .unwrap();

    assert_eq!(resp.status().as_u16(), 201);
    let group: AssignmentGroup = resp.json().unwrap();
    let group_id = group.id;

    // Delete it
    let delete_url = format!(
        "{}/api/v1/courses/{}/assignment_groups/{}",
        api_url(),
        course_id,
        group_id
    );
    let resp = client
        .delete(&delete_url)
        .header("Authorization", auth_token())
        .send()
        .unwrap();

    assert_eq!(
        resp.status().as_u16(),
        204,
        "delete should return 204, got {}",
        resp.status().as_u16()
    );

    // List should not include it
    let resp = client
        .get(&url)
        .header("Authorization", auth_token())
        .send()
        .unwrap();

    let groups: Vec<AssignmentGroup> = resp.json().unwrap();
    assert!(
        !groups.iter().any(|g| g.id == group_id),
        "deleted group {} should not appear in list",
        group_id
    );
}

/// Test: group list ordered by position ASC.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn test_group_list_ordered_by_position() {
    if std::env::var("STAR_PARITY_TESTS").is_err() {
        return;
    }

    let client = http_client();
    let course_id = course_id();
    let url = format!(
        "{}/api/v1/courses/{}/assignment_groups",
        api_url(),
        course_id
    );

    // Create groups with specific positions
    let positions = [3, 1, 2];
    let mut created_ids = vec![];

    for pos in positions {
        let request = CreateAssignmentGroupRequest {
            name: format!("Position Test {}", pos),
            group_weight: None,
            position: Some(pos),
        };

        let resp = client
            .post(&url)
            .header("Authorization", auth_token())
            .json(&request)
            .send()
            .unwrap();

        if resp.status().as_u16() == 201 {
            let group: AssignmentGroup = resp.json().unwrap();
            created_ids.push(group.id);
        }
    }

    // Get list
    let resp = client
        .get(&url)
        .header("Authorization", auth_token())
        .send()
        .unwrap();

    let groups: Vec<AssignmentGroup> = resp.json().unwrap();

    // Verify ordering by position (ASC)
    for window in groups.windows(2) {
        assert!(
            window[0].position <= window[1].position,
            "groups should be ordered by position ASC, got {} before {}",
            window[0].position,
            window[1].position
        );
    }

    // Cleanup
    for id in created_ids {
        let delete_url = format!(
            "{}/api/v1/courses/{}/assignment_groups/{}",
            api_url(),
            course_id,
            id
        );
        let _ = client
            .delete(&delete_url)
            .header("Authorization", auth_token())
            .send();
    }
}
