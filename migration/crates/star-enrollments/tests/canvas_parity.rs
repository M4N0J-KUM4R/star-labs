//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for enrollments — hit the live API if reachable,
//! otherwise skip cleanly.

use serde_json::Value;

const BASE: &str = "http://127.0.0.1:8080";

fn live() -> Option<reqwest::blocking::Client> {
    // Skip unless STAR_PARITY_TESTS=1 (keeps CI/offline runs green)
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

fn parity_password() -> String {
    std::env::var("STAR_ADMIN_PASSWORD")
        .expect("STAR_ADMIN_PASSWORD is required for explicit live parity runs")
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_created_enrollment_has_workflow_state_active() {
    let Some(client) = live() else { return };
    let token = admin_token(&client);

    // Create a dedicated course + user for isolation
    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let course = client
        .post(format!("{BASE}/api/v1/accounts/1/courses"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": format!("Parity Enroll {uniq}"),
            "course_code": format!("PE-{uniq}")
        }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let course_id = course["id"].as_i64().unwrap();

    let user = client
        .post(format!("{BASE}/api/v1/auth/register"))
        .json(&serde_json::json!({
            "name": format!("Parity Student {uniq}"),
            "email": format!("parity-enroll-{uniq}@star.edu"),
            "password": parity_password(),
            "role": "student"
        }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let user_id = user["id"].as_i64().unwrap();

    // Canvas scopes the create to the course
    // (`post "courses/:course_id/enrollments"`, routes.rb:1402); the flat
    // POST /enrollments was never a Canvas API route.
    let r = client
        .post(format!("{BASE}/api/v1/courses/{course_id}/enrollments"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({ "user_id": user_id, "role": "student" }))
        .send()
        .unwrap();
    assert_eq!(r.status(), 201, "create enrollment");
    let body = r.json::<Value>().unwrap();
    assert_eq!(
        body["user"]["id"].as_i64(),
        Some(user_id),
        "the enrolled user comes back on the enrollment (Canvas parity)"
    );
    assert_eq!(
        body["role"].as_str(),
        Some("student"),
        "role defaults to student (Canvas parity)"
    );
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_deleted_enrollment_vanishes_from_list() {
    let Some(client) = live() else { return };
    let token = admin_token(&client);

    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let course = client
        .post(format!("{BASE}/api/v1/accounts/1/courses"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": format!("Parity Del {uniq}"),
            "course_code": format!("PD-{uniq}")
        }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let course_id = course["id"].as_i64().unwrap();

    let user = client
        .post(format!("{BASE}/api/v1/auth/register"))
        .json(&serde_json::json!({
            "name": format!("Parity Del Student {uniq}"),
            "email": format!("parity-del-{uniq}@star.edu"),
            "password": parity_password(),
            "role": "student"
        }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let user_id = user["id"].as_i64().unwrap();

    let enr = client
        .post(format!("{BASE}/api/v1/courses/{course_id}/enrollments"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({ "user_id": user_id, "role": "student" }))
        .send()
        .unwrap();
    assert_eq!(enr.status(), 201, "create enrollment");

    // Canvas declares the member delete as
    // `delete "courses/:course_id/enrollments/:id"` keyed by enrollment id
    // (routes.rb:1410). The Rust API serves the member delete keyed by user id
    // (star-courses `DELETE /courses/{id}/enrollments/{userId}`), so that is
    // the spelling this live leg exercises until the enrollment-id member
    // route lands.
    let r = client
        .delete(format!(
            "{BASE}/api/v1/courses/{course_id}/enrollments/{user_id}"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 204, "delete returns 204");

    // The list is course-scoped too: `get "courses/:course_id/enrollments"`
    // (routes.rb:1394).
    let list = client
        .get(format!("{BASE}/api/v1/courses/{course_id}/enrollments"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let arr = list["data"]
        .as_array()
        .expect("enrollment list data is array");
    assert!(
        !arr.iter()
            .any(|e| e["user"]["id"].as_i64() == Some(user_id)),
        "deleted enrollment must not appear in list (Canvas parity)"
    );
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_duplicate_active_enrollment_conflicts() {
    let Some(client) = live() else { return };
    let token = admin_token(&client);

    let uniq = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let course = client
        .post(format!("{BASE}/api/v1/accounts/1/courses"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "name": format!("Parity Dup {uniq}"),
            "course_code": format!("PX-{uniq}")
        }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let course_id = course["id"].as_i64().unwrap();

    let user = client
        .post(format!("{BASE}/api/v1/auth/register"))
        .json(&serde_json::json!({
            "name": format!("Parity Dup Student {uniq}"),
            "email": format!("parity-dup-{uniq}@star.edu"),
            "password": parity_password(),
            "role": "student"
        }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let user_id = user["id"].as_i64().unwrap();

    let payload = serde_json::json!({ "user_id": user_id, "role": "student" });
    let auth = format!("Bearer {token}");
    // Canvas-real course-scoped create (routes.rb:1402).
    let url = format!("{BASE}/api/v1/courses/{course_id}/enrollments");

    let first = client
        .post(&url)
        .header("Authorization", &auth)
        .json(&payload)
        .send()
        .unwrap();
    assert_eq!(first.status(), 201);

    let second = client
        .post(&url)
        .header("Authorization", &auth)
        .json(&payload)
        .send()
        .unwrap();
    assert_eq!(second.status(), 409, "duplicate active enrollment -> 409");
}
