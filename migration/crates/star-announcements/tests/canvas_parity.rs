//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for star-announcements — run against the live API.
//!
//! These tests exercise the deployed service (STAR_API_BASE, default
//! http://127.0.0.1:8080) with the admin login (STAR_ADMIN_EMAIL /
//! STAR_ADMIN_PASSWORD, use STAR_ADMIN_EMAIL and STAR_ADMIN_PASSWORD from the environment).
//! They skip cleanly when the API is unreachable.
//!
//! Parity targets (Canvas: app/models/announcement.rb, a DiscussionTopic
//! subclass): announcement responses include workflow_state + posted_at;
//! DELETE soft-deletes (workflow_state='deleted') and deleted announcements
//! are excluded from lists.

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

/// Create a course to host test announcements; returns its id.
async fn create_course(client: &Client, token: &str, name: &str) -> i64 {
    let resp = client
        .post(format!("{}/api/v1/courses", api_base()))
        .bearer_auth(token)
        .json(&json!({
            "name": name,
            "course_code": format!("PARITY-ANN-{}", uuid_like()),
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
async fn parity_announcement_includes_workflow_state_and_posted_at() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(
        &client,
        &token,
        &format!("Parity Ann Fields {}", uuid_like()),
    )
    .await;

    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/announcements",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Canvas Parity Announcement",
            "message": "<p>hello</p>"
        }))
        .send()
        .await
        .expect("create announcement request");
    assert!(
        resp.status().is_success(),
        "create announcement failed: {}",
        resp.status()
    );
    let announcement: Value = resp.json().await.expect("announcement json");

    // Canvas: Announcement subclasses DiscussionTopic → workflow_state present,
    // posted_at set when posted.
    assert_eq!(
        announcement["workflow_state"].as_str(),
        Some("active"),
        "create response must include workflow_state='active': {announcement}"
    );
    assert!(
        announcement["posted_at"].is_string(),
        "create response must include posted_at timestamp: {announcement}"
    );

    // GET one shows the same fields.
    let id = announcement["id"].as_i64().expect("announcement id");
    let resp = client
        .get(format!("{}/api/v1/announcements/{}", api_base(), id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get announcement request");
    assert!(
        resp.status().is_success(),
        "get announcement failed: {}",
        resp.status()
    );
    let detail: Value = resp.json().await.expect("announcement json");
    assert_eq!(detail["workflow_state"].as_str(), Some("active"));
    assert!(
        detail["posted_at"].is_string(),
        "GET must include posted_at: {detail}"
    );
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_deleted_announcement_excluded_from_list() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id =
        create_course(&client, &token, &format!("Parity Ann Del {}", uuid_like())).await;

    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/announcements",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Doomed Announcement",
            "message": "to be soft-deleted"
        }))
        .send()
        .await
        .expect("create announcement request");
    assert!(
        resp.status().is_success(),
        "create announcement failed: {}",
        resp.status()
    );
    let announcement: Value = resp.json().await.expect("announcement json");
    let id = announcement["id"].as_i64().expect("announcement id");

    // Canvas parity: DELETE soft-deletes (workflow_state='deleted'), 204.
    let resp = client
        .delete(format!("{}/api/v1/announcements/{}", api_base(), id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("delete announcement request");
    assert!(
        resp.status().is_success(),
        "delete announcement failed: {}",
        resp.status()
    );

    // Excluded from the course list.
    let resp = client
        .get(format!(
            "{}/api/v1/courses/{}/announcements",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list announcements request");
    assert!(
        resp.status().is_success(),
        "list announcements failed: {}",
        resp.status()
    );
    let list: Vec<Value> = resp.json().await.expect("announcement list json");
    assert!(
        list.iter().all(|a| a["id"].as_i64() != Some(id)),
        "soft-deleted announcement must be excluded from list: {list:?}"
    );

    // ...and no longer fetchable by id.
    let resp = client
        .get(format!("{}/api/v1/announcements/{}", api_base(), id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get deleted announcement request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "soft-deleted announcement should 404"
    );
}
