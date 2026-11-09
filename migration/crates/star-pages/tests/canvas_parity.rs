//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for star-pages — run against the live API.
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

/// Create a course to host test pages/modules; returns its id.
async fn create_course(client: &Client, token: &str, name: &str) -> i64 {
    let resp = client
        .post(format!("{}/api/v1/courses", api_base()))
        .bearer_auth(token)
        .json(&json!({
            "name": name,
            "course_code": format!("PARITY-{}", uuid_like()),
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
async fn parity_created_page_returns_url_slug_matching_title() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(&client, &token, &format!("Parity Slug {}", uuid_like())).await;

    let resp = client
        .post(format!("{}/api/v1/courses/{}/pages", api_base(), course_id))
        .bearer_auth(&token)
        .json(&json!({"title": "Canvas Parity Slug Page", "body": "<p>hi</p>"}))
        .send()
        .await
        .expect("create page request");
    assert!(
        resp.status().is_success(),
        "create page failed: {}",
        resp.status()
    );
    let page: Value = resp.json().await.expect("page json");

    // Canvas wiki_page.rb derives the url from the title.
    assert_eq!(
        page["url"].as_str(),
        Some("canvas-parity-slug-page"),
        "POST response should echo an auto-generated url slug: {page}"
    );
    assert_eq!(page["title"].as_str(), Some("Canvas Parity Slug Page"));
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_unpublished_page_echoes_workflow_state() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(&client, &token, &format!("Parity Unpub {}", uuid_like())).await;

    // Create with workflow_state = unpublished
    let resp = client
        .post(format!("{}/api/v1/courses/{}/pages", api_base(), course_id))
        .bearer_auth(&token)
        .json(&json!({
            "title": "Unpublished Parity Page",
            "body": "draft",
            "workflow_state": "unpublished"
        }))
        .send()
        .await
        .expect("create page request");
    assert!(
        resp.status().is_success(),
        "create page failed: {}",
        resp.status()
    );
    let page: Value = resp.json().await.expect("page json");
    assert_eq!(
        page["workflow_state"].as_str(),
        Some("unpublished"),
        "create response must echo workflow_state=unpublished: {page}"
    );

    // List shows it, with the same state
    let resp = client
        .get(format!("{}/api/v1/courses/{}/pages", api_base(), course_id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list pages request");
    assert!(
        resp.status().is_success(),
        "list pages failed: {}",
        resp.status()
    );
    let list: Vec<Value> = resp.json().await.expect("page list json");
    let found = list
        .iter()
        .find(|p| p["id"].as_i64() == page["id"].as_i64())
        .expect("created page present in list");
    assert_eq!(found["workflow_state"].as_str(), Some("unpublished"));
    // Canvas pages#index omits body
    assert!(
        found.get("body").is_none(),
        "list summaries must omit body: {found}"
    );

    // Single GET (by id) still returns body + state
    let id = page["id"].as_i64().expect("page id");
    let resp = client
        .get(format!("{}/api/v1/pages/{}", api_base(), id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get page request");
    assert!(
        resp.status().is_success(),
        "get page failed: {}",
        resp.status()
    );
    let detail: Value = resp.json().await.expect("page json");
    assert_eq!(detail["workflow_state"].as_str(), Some("unpublished"));
    assert_eq!(detail["body"].as_str(), Some("draft"));
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_page_lookup_by_url_slug() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(&client, &token, &format!("Parity Lookup {}", uuid_like())).await;

    let resp = client
        .post(format!("{}/api/v1/courses/{}/pages", api_base(), course_id))
        .bearer_auth(&token)
        .json(&json!({"title": "Slug Lookup Target", "body": "b"}))
        .send()
        .await
        .expect("create page request");
    let page: Value = resp.json().await.expect("page json");

    // GET by url slug returns the same page
    let slug = page["url"].as_str().expect("url slug");
    let resp = client
        .get(format!("{}/api/v1/pages/{}", api_base(), slug))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get page by slug request");
    assert!(
        resp.status().is_success(),
        "get by slug failed: {}",
        resp.status()
    );
    let by_slug: Value = resp.json().await.expect("page json");
    assert_eq!(by_slug["id"].as_i64(), page["id"].as_i64());
    assert_eq!(by_slug["body"].as_str(), Some("b"));
}

#[tokio::test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
async fn parity_deleted_page_excluded_from_list() {
    let Ok((client, token)) = authed().await else {
        eprintln!("skipping: live API unreachable");
        return;
    };
    let course_id = create_course(&client, &token, &format!("Parity Del {}", uuid_like())).await;

    let resp = client
        .post(format!("{}/api/v1/courses/{}/pages", api_base(), course_id))
        .bearer_auth(&token)
        .json(&json!({"title": "Doomed Page"}))
        .send()
        .await
        .expect("create page request");
    let page: Value = resp.json().await.expect("page json");
    let id = page["id"].as_i64().expect("page id");

    let resp = client
        .delete(format!("{}/api/v1/pages/{}", api_base(), id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("delete page request");
    assert!(
        resp.status().is_success(),
        "delete page failed: {}",
        resp.status()
    );

    let resp = client
        .get(format!("{}/api/v1/courses/{}/pages", api_base(), course_id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("list pages request");
    let list: Vec<Value> = resp.json().await.expect("page list json");
    assert!(
        list.iter().all(|p| p["id"].as_i64() != Some(id)),
        "deleted page must be excluded from list: {list:?}"
    );

    // ...and no longer fetchable by id
    let resp = client
        .get(format!("{}/api/v1/pages/{}", api_base(), id))
        .bearer_auth(&token)
        .send()
        .await
        .expect("get deleted page request");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND,
        "deleted page should 404"
    );
}
