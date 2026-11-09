//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for star-rubrics — run against the live API.
//!
//! These tests exercise the deployed service (STAR_API_BASE, default
//! http://127.0.0.1:8080) with the admin login (STAR_ADMIN_EMAIL /
//! STAR_ADMIN_PASSWORD, use STAR_ADMIN_EMAIL and STAR_ADMIN_PASSWORD from the environment).
//! They skip cleanly when the API is unreachable.

use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

fn api_base() -> String {
    std::env::var("STAR_API_BASE").unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("reqwest client")
}

/// Try to log in and return a bearer token. None => API unreachable, skip.
async fn login(client: &Client) -> Option<String> {
    let resp = client
        .post(format!("{}/api/v1/auth/login", api_base()))
        .json(&json!({
            "email": std::env::var("STAR_ADMIN_EMAIL")
                .unwrap_or_else(|_| "admin@star.edu".to_string()),
            "password": std::env::var("STAR_ADMIN_PASSWORD").expect("STAR_ADMIN_PASSWORD is required for explicit live parity runs")
        }))
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }
    let body: Value = resp.json().await.ok()?;
    let token = body["token"]
        .as_str()
        .or_else(|| body["access_token"].as_str())
        .map(str::to_string)?;
    Some(token)
}

/// Authenticated helper. Returns Err(skip-message) if API is unreachable.
async fn authed() -> Result<(Client, String), String> {
    let client = client();
    match login(&client).await {
        Some(token) => Ok((client, token)),
        None => Err("live API unreachable — skipping Canvas parity tests".to_string()),
    }
}

/// Create a course to host test rubrics; returns its id.
async fn create_course(client: &Client, token: &str, name: &str) -> i64 {
    let resp = client
        .post(format!("{}/api/v1/courses", api_base()))
        .bearer_auth(token)
        .json(&json!({
            "name": name,
            "course_code": format!("PARITY-RUB-{}", uuid_like()),
        }))
        .send()
        .await
        .expect("create course request");
    assert!(
        resp.status().is_success(),
        "create course failed: {}",
        resp.status()
    );
    let body: Value = resp.json().await.expect("course json");
    body["id"].as_i64().expect("course id")
}

/// Best-effort unique suffix.
fn uuid_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_rubric_create_returns_nested_criteria_and_ratings() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(
        &client,
        &token,
        &format!("Parity Rubric Nested {}", uuid_like()),
    )
    .await;

    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/rubrics",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Project Rubric",
            "points_possible": 100.0,
            "criteria": [
                {
                    "description": "Content quality",
                    "points": 50.0,
                    "ratings": [
                        { "description": "Excellent", "points": 50.0 },
                        { "description": "Good", "points": 40.0 },
                        { "description": "Fair", "points": 30.0 },
                        { "description": "Poor", "points": 10.0 }
                    ]
                },
                {
                    "description": "Presentation",
                    "points": 50.0,
                    "ratings": [
                        { "description": "Clear and organized", "points": 50.0 },
                        { "description": "Somewhat clear", "points": 35.0 },
                        { "description": "Unclear", "points": 15.0 }
                    ]
                }
            ]
        }))
        .send()
        .await
        .expect("create rubric request");

    assert!(
        resp.status().is_success(),
        "rubric create must succeed: {}",
        resp.status()
    );

    let rubric: Value = resp.json().await.expect("rubric json");
    assert!(rubric["id"].is_i64(), "rubric must have numeric id");
    assert_eq!(rubric["title"].as_str(), Some("Project Rubric"));
    assert_eq!(rubric["points_possible"].as_f64(), Some(100.0));

    // Verify nested criteria are returned
    let criteria = rubric["criteria"]
        .as_array()
        .expect("criteria must be an array");
    assert_eq!(criteria.len(), 2, "must have 2 criteria");

    // First criterion has ratings
    let c0 = &criteria[0];
    assert_eq!(c0["description"].as_str(), Some("Content quality"));
    assert_eq!(c0["points"].as_f64(), Some(50.0));
    let ratings0 = c0["ratings"]
        .as_array()
        .expect("criterion 0 ratings must be an array");
    assert_eq!(ratings0.len(), 4, "criterion 0 must have 4 ratings");

    // Second criterion has ratings
    let c1 = &criteria[1];
    assert_eq!(c1["description"].as_str(), Some("Presentation"));
    let ratings1 = c1["ratings"]
        .as_array()
        .expect("criterion 1 ratings must be an array");
    assert_eq!(ratings1.len(), 3, "criterion 1 must have 3 ratings");
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_rubric_assessment_records_and_lists() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(
        &client,
        &token,
        &format!("Parity Assessment {}", uuid_like()),
    )
    .await;

    // Create an assignment for the submission
    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/assignments",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Graded Assignment",
            "points_possible": 100.0
        }))
        .send()
        .await
        .expect("create assignment request");
    assert!(
        resp.status().is_success(),
        "create assignment failed: {}",
        resp.status()
    );
    let assignment: Value = resp.json().await.expect("assignment json");
    let assignment_id = assignment["id"].as_i64().expect("assignment id");

    // Enroll a student to create a submission
    let _student_resp = client
        .post(format!(
            "{}/api/v1/courses/{}/enrollments",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "user_id": 2,  // assume student user id 2 exists
            "type": "student"
        }))
        .send()
        .await;
    // May fail if user doesn't exist — that's fine for this test

    // Create a submission for the assignment
    let sub_resp = match client
        .post(format!(
            "{}/api/v1/assignments/{}/submissions",
            api_base(),
            assignment_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "body": "My submission text"
        }))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            eprintln!("skipping assessment test: submission request failed: {err}");
            return;
        }
    };
    // May fail if no enrolled student — skip if so
    if !sub_resp.status().is_success() {
        eprintln!("skipping assessment test: could not create submission");
        return;
    }
    let submission: Value = sub_resp.json().await.expect("submission json");
    let submission_id = submission["id"].as_i64().expect("submission id");

    // Create a rubric
    let rubric_resp = client
        .post(format!(
            "{}/api/v1/courses/{}/rubrics",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Grading Rubric",
            "points_possible": 100.0,
            "criteria": [
                {
                    "description": "Depth",
                    "points": 60.0,
                    "ratings": [
                        { "description": "Excellent", "points": 60.0 },
                        { "description": "Good", "points": 45.0 },
                        { "description": "Fair", "points": 30.0 }
                    ]
                },
                {
                    "description": "Clarity",
                    "points": 40.0,
                    "ratings": [
                        { "description": "Clear", "points": 40.0 },
                        { "description": "Okay", "points": 25.0 },
                        { "description": "Poor", "points": 10.0 }
                    ]
                }
            ]
        }))
        .send()
        .await
        .expect("create rubric request");
    assert!(rubric_resp.status().is_success(), "rubric create failed");
    let rubric: Value = rubric_resp.json().await.expect("rubric json");
    let rubric_id = rubric["id"].as_i64().expect("rubric id");

    // Get criterion IDs from the rubric
    let criteria = rubric["criteria"].as_array().expect("criteria array");
    let criterion0_id = criteria[0]["id"].as_i64().expect("criterion 0 id");
    let rating0_id = criteria[0]["ratings"][0]["id"]
        .as_i64()
        .expect("rating 0 id");

    // Create a rubric assessment
    let assessment_resp = client
        .post(format!("{}/api/v1/rubrics/{}/assessments", api_base(), rubric_id))
        .bearer_auth(&token)
        .json(&json!({
            "submission_id": submission_id,
            "score": 85.0,
            "data": [
                { "criterion_id": criterion0_id, "rating_id": rating0_id, "points": 60.0, "comments": "Great work" },
                { "criterion_id": criteria[1]["id"].as_i64().unwrap(), "rating_id": criteria[1]["ratings"][0]["id"].as_i64().unwrap(), "points": 25.0, "comments": "Okay" }
            ]
        }))
        .send()
        .await
        .expect("create assessment request");

    assert_eq!(
        assessment_resp.status(),
        reqwest::StatusCode::CREATED,
        "assessment create must return 201"
    );
    let assessment: Value = assessment_resp.json().await.expect("assessment json");
    assert!(assessment["id"].is_i64(), "assessment must have id");
    assert_eq!(assessment["rubric_id"].as_i64(), Some(rubric_id));
    assert_eq!(assessment["submission_id"].as_i64(), Some(submission_id));
    assert_eq!(assessment["score"].as_f64(), Some(85.0));

    // List assessments for the rubric
    let list_resp = client
        .get(format!(
            "{}/api/v1/rubrics/{}/assessments?submission_id={}",
            api_base(),
            rubric_id,
            submission_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list assessments request");
    assert!(
        list_resp.status().is_success(),
        "list assessments must succeed"
    );
    let assessments: Vec<Value> = list_resp.json().await.expect("assessments list");
    assert!(
        !assessments.is_empty(),
        "should have at least one assessment"
    );
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_deleted_rubric_vanishes_from_list() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(&client, &token, &format!("Parity Delete {}", uuid_like())).await;

    // Create a rubric
    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/rubrics",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Temp Rubric",
            "points_possible": 50.0,
            "criteria": [
                { "description": "Criterion 1", "points": 50.0 }
            ]
        }))
        .send()
        .await
        .expect("create rubric request");
    assert!(resp.status().is_success(), "create failed");
    let rubric: Value = resp.json().await.expect("rubric json");
    let rubric_id = rubric["id"].as_i64().expect("rubric id");

    // Verify it appears in list
    let list_resp = client
        .get(format!(
            "{}/api/v1/courses/{}/rubrics",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list rubrics request");
    assert!(list_resp.status().is_success());
    let list: Vec<Value> = list_resp.json().await.expect("rubrics list");
    assert!(
        list.iter().any(|r| r["id"].as_i64() == Some(rubric_id)),
        "rubric must be in list before delete"
    );

    // Delete it
    let del_resp = client
        .delete(format!("{}/api/v1/rubrics/{}", api_base(), rubric_id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("delete rubric request");
    assert_eq!(
        del_resp.status(),
        reqwest::StatusCode::NO_CONTENT,
        "delete must return 204"
    );

    // Verify it's gone from the course rubric list
    let list_resp2 = client
        .get(format!(
            "{}/api/v1/courses/{}/rubrics",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list rubrics after delete");
    assert!(list_resp2.status().is_success());
    let list2: Vec<Value> = list_resp2.json().await.expect("rubrics list after delete");
    assert!(
        !list2.iter().any(|r| r["id"].as_i64() == Some(rubric_id)),
        "deleted rubric must NOT appear in list"
    );

    // Verify GET returns 404
    let get_resp = client
        .get(format!("{}/api/v1/rubrics/{}", api_base(), rubric_id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get deleted rubric");
    assert_eq!(
        get_resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "deleted rubric GET must return 404"
    );
}
