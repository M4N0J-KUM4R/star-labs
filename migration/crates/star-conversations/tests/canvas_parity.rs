//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for star-conversations.
//!
//! Run with:
//!   STAR_PARITY_TESTS=1 cargo test -p star-conversations
//!
//! These tests hit a live API at http://127.0.0.1:8080.

use serde_json::json;

const API_BASE: &str = "http://127.0.0.1:8080/api/v1";

/// Get a JWT token for a test user (creates if needed, returns email + token).
fn get_auth(email: &str) -> (String, String) {
    let client = reqwest::blocking::Client::new();

    // Try to register (ignore if already exists).
    let _ = client
        .post(format!("{}/auth/register", API_BASE))
        .json(&json!({ "name": email, "email": email, "password": "test123" }))
        .send();

    // Login.
    let resp = client
        .post(format!("{}/auth/login", API_BASE))
        .json(&json!({ "email": email, "password": "test123" }))
        .send()
        .expect("login request failed");

    let body: serde_json::Value = resp.json().expect("login response not JSON");
    let token = body["token"].as_str().unwrap().to_string();
    (email.to_string(), token)
}

fn auth_header(token: &str) -> String {
    format!("Bearer {}", token)
}

/// Wait for the service to be ready.
fn wait_ready() {
    let client = reqwest::blocking::Client::new();
    for _i in 0..30 {
        if client
            .get("http://127.0.0.1:8080/health")
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    panic!("service did not become ready in 30s");
}

#[test]
#[ignore]
fn test_parity_create_conversation_returns_participants_and_messages() {
    if std::env::var("STAR_PARITY_TESTS").is_err() {
        return;
    }
    wait_ready();

    let client = reqwest::blocking::Client::new();

    // Create two users.
    let (_email1, token1) = get_auth("conv_parity_test_1@example.com");
    let (_email2, _token2) = get_auth("conv_parity_test_2@example.com");

    // User 1 creates a conversation with User 2.
    let resp = client
        .post(format!("{}/conversations", API_BASE))
        .header("Authorization", auth_header(&token1))
        .json(&json!({
            "recipients": [],
            "subject": "Hello World",
            "body": "This is my first message"
        }))
        .send()
        .expect("create conversation request failed");

    assert_eq!(resp.status().as_u16(), 201, "create returned 201");

    let body: serde_json::Value = resp.json().expect("response JSON");
    println!("CREATE RESPONSE: {:#}", body);

    assert!(body["id"].is_number(), "conversation has id");
    assert_eq!(body["subject"].as_str().unwrap(), "Hello World");
    assert_eq!(body["workflow_state"].as_str().unwrap(), "active");

    // The creating user should be the only participant initially.
    let participants = body["participants"].as_array().unwrap();
    assert!(!participants.is_empty(), "has participants");

    // Should have exactly 1 message.
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "has one initial message");
    assert_eq!(
        messages[0]["body"].as_str().unwrap(),
        "This is my first message"
    );
    assert!(messages[0]["author_id"].is_number());
    assert!(messages[0]["created_at"].is_string());
}

#[test]
#[ignore]
fn test_parity_add_second_message_append() {
    if std::env::var("STAR_PARITY_TESTS").is_err() {
        return;
    }
    wait_ready();

    let client = reqwest::blocking::Client::new();

    let (_email1, token1) = get_auth("conv_parity_test_msg2@example.com");
    let (_email2, _token2) = get_auth("conv_parity_test_msg2b@example.com");

    // Create conversation.
    let resp = client
        .post(format!("{}/conversations", API_BASE))
        .header("Authorization", auth_header(&token1))
        .json(&json!({
            "recipients": [],
            "subject": "Thread Test",
            "body": "First message"
        }))
        .send()
        .expect("create conversation failed");

    assert_eq!(resp.status().as_u16(), 201);
    let body: serde_json::Value = resp.json().expect("response JSON");
    let conv_id = body["id"].as_i64().unwrap();
    let initial_msg_count = body["messages"].as_array().unwrap().len();

    // Add a second message.
    let resp2 = client
        .post(format!(
            "{}/conversations/{}/add_message",
            API_BASE, conv_id
        ))
        .header("Authorization", auth_header(&token1))
        .json(&json!({ "body": "Second message added" }))
        .send()
        .expect("add_message request failed");

    assert_eq!(resp2.status().as_u16(), 200, "add_message returned 200");
    let msg2_body: serde_json::Value = resp2.json().expect("add_message JSON");
    assert_eq!(msg2_body["body"].as_str().unwrap(), "Second message added");
    assert!(msg2_body["id"].is_number());

    // Get full conversation and verify both messages present.
    let resp3 = client
        .get(format!("{}/conversations/{}", API_BASE, conv_id))
        .header("Authorization", auth_header(&token1))
        .send()
        .expect("get conversation failed");

    assert_eq!(resp3.status().as_u16(), 200);
    let detail: serde_json::Value = resp3.json().expect("detail JSON");
    let messages = detail["messages"].as_array().unwrap();
    assert_eq!(
        messages.len(),
        initial_msg_count + 1,
        "message was appended"
    );
}

#[test]
#[ignore]
fn test_parity_deleted_conversation_hidden_from_list() {
    if std::env::var("STAR_PARITY_TESTS").is_err() {
        return;
    }
    wait_ready();

    let client = reqwest::blocking::Client::new();

    let (_email1, token1) = get_auth("conv_parity_del@example.com");
    let (_email2, _token2) = get_auth("conv_parity_del2@example.com");

    // Create a conversation.
    let resp = client
        .post(format!("{}/conversations", API_BASE))
        .header("Authorization", auth_header(&token1))
        .json(&json!({
            "recipients": [],
            "subject": "Will be deleted",
            "body": "Please delete me"
        }))
        .send()
        .expect("create failed");
    assert_eq!(resp.status().as_u16(), 201);
    let body: serde_json::Value = resp.json().expect("create JSON");
    let conv_id = body["id"].as_i64().unwrap();

    // List conversations - should contain it.
    let resp_list = client
        .get(format!("{}/conversations", API_BASE))
        .header("Authorization", auth_header(&token1))
        .send()
        .expect("list failed");
    let list_body: serde_json::Value = resp_list.json().expect("list JSON");
    let count_before = list_body.as_array().unwrap().len();

    // Delete the conversation for this user.
    let resp_del = client
        .delete(format!("{}/conversations/{}", API_BASE, conv_id))
        .header("Authorization", auth_header(&token1))
        .send()
        .expect("delete failed");
    assert_eq!(resp_del.status().as_u16(), 204, "delete returned 204");

    // List again - should NOT contain it.
    let resp_list2 = client
        .get(format!("{}/conversations", API_BASE))
        .header("Authorization", auth_header(&token1))
        .send()
        .expect("list 2 failed");
    let list_body2: serde_json::Value = resp_list2.json().expect("list 2 JSON");
    let count_after = list_body2.as_array().unwrap().len();
    assert_eq!(
        count_after,
        count_before - 1,
        "deleted conversation hidden from list"
    );
}

#[test]
#[ignore]
fn test_parity_conversation_participants_add_recipients_and_read() {
    if std::env::var("STAR_PARITY_TESTS").is_err() {
        return;
    }
    wait_ready();

    let client = reqwest::blocking::Client::new();

    let (_email1, token1) = get_auth("conv_parity_part_1@example.com");
    let (_email2, token2) = get_auth("conv_parity_part_2@example.com");

    // Resolve user 2's id.
    let resp = client
        .get(format!("{}/users/self", API_BASE))
        .header("Authorization", auth_header(&token2))
        .send()
        .expect("users/self failed");
    let self_body: serde_json::Value = resp.json().expect("users/self JSON");
    let user2_id = self_body["id"].as_i64().expect("user 2 id");

    // User 1 creates a conversation with no recipients.
    let resp = client
        .post(format!("{}/conversations", API_BASE))
        .header("Authorization", auth_header(&token1))
        .json(&json!({
            "recipients": [],
            "subject": "Participants parity",
            "body": "First message"
        }))
        .send()
        .expect("create conversation failed");
    assert_eq!(resp.status().as_u16(), 201);
    let created: serde_json::Value = resp.json().expect("create JSON");
    let conv_id = created["id"].as_i64().unwrap();

    // GET participants returns Canvas user entries.
    let resp = client
        .get(format!(
            "{}/conversations/{}/participants",
            API_BASE, conv_id
        ))
        .header("Authorization", auth_header(&token1))
        .send()
        .expect("participants failed");
    assert_eq!(resp.status().as_u16(), 200);
    let participants: serde_json::Value = resp.json().expect("participants JSON");
    let participants = participants.as_array().expect("participants array");
    assert!(!participants.is_empty());
    assert!(participants[0]["id"].is_number());
    assert!(participants[0]["name"].is_string());
    assert!(participants[0]["full_name"].is_string());

    // Add user 2 as a recipient.
    let resp = client
        .post(format!(
            "{}/conversations/{}/add_recipients",
            API_BASE, conv_id
        ))
        .header("Authorization", auth_header(&token1))
        .json(&json!({ "recipients": [user2_id] }))
        .send()
        .expect("add_recipients failed");
    assert_eq!(resp.status().as_u16(), 200, "add_recipients returned 200");
    let detail: serde_json::Value = resp.json().expect("add_recipients JSON");
    let audience = detail["participants"].as_array().unwrap();
    assert!(audience.iter().any(|id| id.as_i64() == Some(user2_id)));
    assert_eq!(
        detail["messages"].as_array().unwrap().len(),
        1,
        "only the latest message is returned"
    );

    // Empty recipients is a 400.
    let resp = client
        .post(format!(
            "{}/conversations/{}/add_recipients",
            API_BASE, conv_id
        ))
        .header("Authorization", auth_header(&token1))
        .json(&json!({ "recipients": [] }))
        .send()
        .expect("add_recipients empty failed");
    assert_eq!(resp.status().as_u16(), 400, "empty recipients returned 400");

    // Mark the conversation read for user 2.
    let resp = client
        .put(format!("{}/conversations/{}/read", API_BASE, conv_id))
        .header("Authorization", auth_header(&token2))
        .send()
        .expect("mark read failed");
    assert_eq!(resp.status().as_u16(), 200, "read returned 200");

    // User 2 is now a participant and sees the conversation.
    let resp = client
        .get(format!("{}/conversations/{}", API_BASE, conv_id))
        .header("Authorization", auth_header(&token2))
        .send()
        .expect("get conversation as user 2 failed");
    assert_eq!(resp.status().as_u16(), 200);
}
