//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for star-modules — run against the live API.
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

/// Create a course to host test modules; returns its id.
async fn create_course(client: &Client, token: &str, name: &str) -> i64 {
    let resp = client
        .post(format!("{}/api/v1/courses", api_base()))
        .bearer_auth(token)
        .json(&json!({
            "name": name,
            "course_code": format!("PARITY-M{}", uuid_like()),
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
async fn parity_module_response_includes_workflow_state() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(&client, &token, &format!("Mod Parity {}", uuid_like())).await;

    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/modules",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({"name": "Week 1", "position": 1}))
        .send()
        .await
        .expect("create module request");
    assert!(
        resp.status().is_success(),
        "create module failed: {}",
        resp.status()
    );
    let module: Value = resp.json().await.expect("module json");

    // Canvas context_module_json includes workflow_state
    assert_eq!(
        module["workflow_state"].as_str(),
        Some("active"),
        "module response should include workflow_state=active: {module}"
    );

    // Single GET also carries workflow_state
    let id = module["id"].as_i64().expect("module id");
    let resp = client
        .get(format!("{}/api/v1/modules/{}", api_base(), id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get module request");
    assert!(
        resp.status().is_success(),
        "get module failed: {}",
        resp.status()
    );
    let detail: Value = resp.json().await.expect("module json");
    assert_eq!(detail["workflow_state"].as_str(), Some("active"));
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_deleted_module_excluded_from_list() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(&client, &token, &format!("Mod Del {}", uuid_like())).await;

    // Create two modules, delete the first.
    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/modules",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({"name": "Doomed Module", "position": 1}))
        .send()
        .await
        .expect("create module request");
    assert!(
        resp.status().is_success(),
        "create module failed: {}",
        resp.status()
    );
    let doomed: Value = resp.json().await.expect("module json");
    let doomed_id = doomed["id"].as_i64().expect("module id");

    let resp = client
        .post(format!(
            "{}/api/v1/courses/{}/modules",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .json(&json!({"name": "Survivor Module", "position": 2}))
        .send()
        .await
        .expect("create module request");
    assert!(
        resp.status().is_success(),
        "create module failed: {}",
        resp.status()
    );
    let survivor: Value = resp.json().await.expect("module json");
    let survivor_id = survivor["id"].as_i64().expect("module id");

    // DELETE -> soft delete
    let resp = client
        .delete(format!("{}/api/v1/modules/{}", api_base(), doomed_id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("delete module request");
    assert!(
        resp.status().is_success(),
        "delete module failed: {}",
        resp.status()
    );

    // List excludes the deleted module
    let resp = client
        .get(format!(
            "{}/api/v1/courses/{}/modules",
            api_base(),
            course_id
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list modules request");
    assert!(
        resp.status().is_success(),
        "list modules failed: {}",
        resp.status()
    );
    let list: Vec<Value> = resp.json().await.expect("module list json");
    assert!(
        list.iter().all(|m| m["id"].as_i64() != Some(doomed_id)),
        "deleted module must be excluded from list: {list:?}"
    );
    assert!(
        list.iter().any(|m| m["id"].as_i64() == Some(survivor_id)),
        "surviving module still listed: {list:?}"
    );

    // Single GET of the deleted module 404s
    let resp = client
        .get(format!("{}/api/v1/modules/{}", api_base(), doomed_id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get deleted module request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "deleted module should 404"
    );
}
