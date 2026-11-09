//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for star-quizzes — run against the live API.
//!
//! These tests exercise the deployed service (STAR_API_BASE, default
//! http://127.0.0.1:8080) with the admin login (STAR_ADMIN_EMAIL /
//! STAR_ADMIN_PASSWORD, use STAR_ADMIN_EMAIL and STAR_ADMIN_PASSWORD from the environment).
//! They skip cleanly when the API is unreachable.
//!
//! Parity targets (Canvas: app/models/quizzes/quiz.rb): quiz_type must be one
//! of assignment|practice|survey|graded_survey; responses use shuffle_answers,
//! allowed_attempts, published.

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
    // token may be under "token" or "access_token" depending on deployment
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

/// Create a course to host test quizzes; returns its id.
async fn create_course(client: &Client, token: &str, name: &str) -> i64 {
    let resp = client
        .post(format!("{}/api/v1/courses", api_base()))
        .bearer_auth(token)
        .json(&json!({
            "name": name,
            "course_code": format!("PARITY-QUIZ-{}", uuid_like()),
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

/// Best-effort unique suffix (no uuid crate; nanos are enough).
fn uuid_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_quiz_type_practice_accepted_with_canvas_fields() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(
        &client,
        &token,
        &format!("Parity Quiz Practice {}", uuid_like()),
    )
    .await;

    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/quizzes",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Canvas Parity Practice Quiz",
            "description": "<p>parity</p>",
            "quiz_type": "practice",
            "time_limit": 30,
            "shuffle_answers": true,
            "allowed_attempts": 2,
            "published": false
        }))
        .send()
        .await
        .expect("create quiz request");
    assert!(
        resp.status().is_success(),
        "quiz with quiz_type 'practice' must be accepted: {}",
        resp.status()
    );
    let quiz: Value = resp.json().await.expect("quiz json");
    assert_eq!(quiz["quiz_type"].as_str(), Some("practice"));
    assert_eq!(quiz["shuffle_answers"].as_bool(), Some(true));
    assert_eq!(quiz["allowed_attempts"].as_i64(), Some(2));
    assert_eq!(quiz["published"].as_bool(), Some(false));
    assert_eq!(quiz["time_limit"].as_i64(), Some(30));
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_quiz_type_bogus_rejected_422() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(
        &client,
        &token,
        &format!("Parity Quiz Bogus {}", uuid_like()),
    )
    .await;

    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/quizzes",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Bogus Quiz Type",
            "quiz_type": "bogus"
        }))
        .send()
        .await
        .expect("create quiz request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "quiz_type 'bogus' must be rejected with 422"
    );

    // Update path validates too: create a valid quiz then PUT a bogus type.
    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/quizzes",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({"title": "Valid Quiz", "quiz_type": "assignment"}))
        .send()
        .await
        .expect("create quiz request");
    assert!(resp.status().is_success(), "valid quiz create failed");
    let quiz: Value = resp.json().await.expect("quiz json");
    let id = quiz["id"].as_i64().expect("quiz id");

    let resp = client
        .put(format!("{}/api/v1/quizzes/{}", api_base(), id))
        .bearer_auth(&token)
        .json(&json!({"quiz_type": "bogus"}))
        .send()
        .await
        .expect("update quiz request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        "PUT with quiz_type 'bogus' must be rejected with 422"
    );
}
