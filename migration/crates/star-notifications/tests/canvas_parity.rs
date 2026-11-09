//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for notifications — hit the live API if reachable,
//! otherwise skip cleanly.

use serde_json::Value;

const BASE: &str = "http://127.0.0.1:8080";

fn live() -> Option<reqwest::blocking::Client> {
    if std::env::var("STAR_PARITY_TESTS").ok().as_deref() != Some("1") {
        return None;
    }
    Some(reqwest::blocking::Client::new())
}

fn admin_token(client: &reqwest::blocking::Client) -> String {
    let r = client
        .post(format!("{BASE}/api/v1/auth/login"))
        .json(&serde_json::json!({
            "email": "admin@star.edu",
            "password": parity_password()
        }))
        .send()
        .expect("login request");
    assert_eq!(r.status(), 200, "admin login");
    r.json::<Value>().expect("login json")["token"]
        .as_str()
        .expect("token")
        .to_string()
}

/// Creates a test user and returns their token + user_id.
fn create_test_user(
    client: &reqwest::blocking::Client,
    admin_token: &str,
    suffix: &str,
) -> (String, i64) {
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user = client
        .post(format!("{BASE}/api/v1/auth/register"))
        .header("Authorization", format!("Bearer {admin_token}"))
        .json(&serde_json::json!({
            "name": format!("Parity Notif User {suffix} {uniq}"),
            "email": format!("parity-notif-{suffix}-{uniq}@star.edu"),
            "password": parity_password(),
            "role": "student"
        }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let user_id = user["id"].as_i64().unwrap();

    // Log in as this user to get their token.
    let login = client
        .post(format!("{BASE}/api/v1/auth/login"))
        .json(&serde_json::json!({
            "email": format!("parity-notif-{suffix}-{uniq}@star.edu"),
            "password": parity_password()
        }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let user_token = login["token"].as_str().expect("user token").to_string();
    (user_token, user_id)
}

// --------------------------------------------------------------------------
// Tests — Notification Preferences (no seed data needed)
// --------------------------------------------------------------------------

/// PUT /users/self/notification_preferences upsert round-trips correctly.
fn parity_password() -> String {
    std::env::var("STAR_ADMIN_PASSWORD")
        .expect("STAR_ADMIN_PASSWORD is required for explicit live parity runs")
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_notification_preference_upsert_round_trips() {
    let Some(client) = live() else {
        eprintln!("STAR_PARITY_TESTS not set; skipping");
        return;
    };
    let admin_token = admin_token(&client);
    let (user_token, _user_id) = create_test_user(&client, &admin_token, "pref");

    let payload = serde_json::json!({
        "notification_category": "announcement",
        "frequency": "daily"
    });

    let r = client
        .put(format!("{BASE}/api/v1/users/self/notification_preferences"))
        .header("Authorization", format!("Bearer {user_token}"))
        .json(&payload)
        .send()
        .unwrap();
    assert_eq!(r.status(), 200, "upsert preference should return 200");
    let body: Value = r.json().unwrap();

    assert_eq!(
        body["notification_preferences"][0]["notification_category"].as_str(),
        Some("announcement"),
        "notification_category must match"
    );
    assert_eq!(
        body["notification_preferences"][0]["frequency"].as_str(),
        Some("daily"),
        "frequency must match"
    );

    let pref_id = body["notification_preferences"][0]["id"]
        .as_i64()
        .expect("id must be present");

    // Upsert again — should update, not create a new row.
    let payload2 = serde_json::json!({
        "notification_category": "announcement",
        "frequency": "weekly"
    });
    let r2 = client
        .put(format!("{BASE}/api/v1/users/self/notification_preferences"))
        .header("Authorization", format!("Bearer {user_token}"))
        .json(&payload2)
        .send()
        .unwrap();
    assert_eq!(r2.status(), 200, "second upsert should return 200");
    let body2: Value = r2.json().unwrap();
    assert_eq!(
        body2["notification_preferences"][0]["id"].as_i64(),
        Some(pref_id),
        "id must be same (upsert not insert)"
    );
    assert_eq!(
        body2["notification_preferences"][0]["frequency"].as_str(),
        Some("weekly"),
        "frequency must be updated"
    );

    // Verify list returns the updated preference.
    let r3 = client
        .get(format!("{BASE}/api/v1/users/self/notification_preferences"))
        .header("Authorization", format!("Bearer {user_token}"))
        .send()
        .unwrap();
    assert_eq!(r3.status(), 200, "list preferences");
    let list: Value = r3.json().unwrap();
    let arr = list["notification_preferences"]
        .as_array()
        .expect("notification_preferences is array");
    let found = arr.iter().find(|p| p["id"].as_i64() == Some(pref_id));
    assert!(
        found.is_some(),
        "preference must appear in list after upsert"
    );
    assert_eq!(
        found.unwrap()["frequency"].as_str(),
        Some("weekly"),
        "listed frequency must match updated value"
    );
}

/// Preference frequency validation rejects invalid values.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_notification_preference_rejects_invalid_frequency() {
    let Some(client) = live() else {
        eprintln!("STAR_PARITY_TESTS not set; skipping");
        return;
    };
    let admin_token = admin_token(&client);
    let (user_token, _user_id) = create_test_user(&client, &admin_token, "invfreq");

    let payload = serde_json::json!({
        "notification_category": "announcement",
        "frequency": "invalid_frequency"
    });

    let r = client
        .put(format!("{BASE}/api/v1/users/self/notification_preferences"))
        .header("Authorization", format!("Bearer {user_token}"))
        .json(&payload)
        .send()
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        422,
        "invalid frequency must return 422"
    );
}

// --------------------------------------------------------------------------
// Tests — Notifications (require seeded data)
// --------------------------------------------------------------------------

/// GET /notifications/unread_count returns the correct count.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_unread_count_returns_ok() {
    let Some(client) = live() else {
        eprintln!("STAR_PARITY_TESTS not set; skipping");
        return;
    };
    let admin_token = admin_token(&client);
    let (user_token, _user_id) = create_test_user(&client, &admin_token, "unread");

    let r = client
        .get(format!("{BASE}/api/v1/notifications/unread_count"))
        .header("Authorization", format!("Bearer {user_token}"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200, "unread_count endpoint");
    let body: Value = r.json().unwrap();
    assert!(
        body["count"].is_number(),
        "unread_count must have a number 'count' field"
    );
}

/// GET /users/self/notifications returns 200 with a list (may be empty for fresh user).
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_list_notifications_returns_ok() {
    let Some(client) = live() else {
        eprintln!("STAR_PARITY_TESTS not set; skipping");
        return;
    };
    let admin_token = admin_token(&client);
    let (user_token, _user_id) = create_test_user(&client, &admin_token, "list");

    let r = client
        .get(format!("{BASE}/api/v1/users/self/notifications"))
        .header("Authorization", format!("Bearer {user_token}"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200, "list notifications");
    let body: Value = r.json().unwrap();
    assert!(
        body.is_array(),
        "notification list must be an array (may be empty)"
    );
}

/// DELETE /notifications/{id} for a non-existent notification returns 404.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_delete_nonexistent_notification_returns_404() {
    let Some(client) = live() else {
        eprintln!("STAR_PARITY_TESTS not set; skipping");
        return;
    };
    let admin_token = admin_token(&client);
    let (user_token, _user_id) = create_test_user(&client, &admin_token, "del");

    let r = client
        .delete(format!("{BASE}/api/v1/notifications/999999"))
        .header("Authorization", format!("Bearer {user_token}"))
        .send()
        .unwrap();
    assert_eq!(r.status().as_u16(), 404, "delete non-existent notification");
}

/// Unauthenticated request to notification endpoints returns 401.
#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_notifications_require_auth() {
    let Some(client) = live() else {
        eprintln!("STAR_PARITY_TESTS not set; skipping");
        return;
    };

    let r = client
        .get(format!("{BASE}/api/v1/users/self/notifications"))
        .send()
        .unwrap();
    assert_eq!(
        r.status().as_u16(),
        401,
        "unauthenticated list notifications"
    );

    let r2 = client
        .get(format!("{BASE}/api/v1/notifications/unread_count"))
        .send()
        .unwrap();
    assert_eq!(r2.status().as_u16(), 401, "unauthenticated unread_count");
}
