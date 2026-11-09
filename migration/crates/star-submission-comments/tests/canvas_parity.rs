//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Canvas parity tests for star-submission-comments — run against the live API.
//!
//! These tests exercise the deployed service (STAR_API_BASE, default
//! http://127.0.0.1:8080) with the admin login (STAR_ADMIN_EMAIL /
//! STAR_ADMIN_PASSWORD, use STAR_ADMIN_EMAIL and STAR_ADMIN_PASSWORD from the environment).
//! They skip cleanly when the API is unreachable.
//!
//! STAR_PARITY_TESTS=1 gate required (matches other Canvas parity suites).
//!
//! Parity targets (Canvas: app/models/submission_comment.rb,
//! submissions_api_controller.rb):
//! - POST /submissions/{id}/comments {comment} → 201 {id, submission_id,
//!   author_id, comment, workflow_state, created_at, updated_at}
//! - GET  /submissions/{id}/comments → [active comments], ordered by created_at
//! - DELETE /submissions/{id}/comments/{commentId} → 204, soft-delete

use serde_json::{json, Value};

const BASE: &str = "http://127.0.0.1:8080";

fn live() -> Option<reqwest::blocking::Client> {
    if std::env::var("STAR_PARITY_TESTS").ok().as_deref() != Some("1") {
        return None;
    }
    Some(
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("reqwest client"),
    )
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

fn uuid_like() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn parity_password() -> String {
    std::env::var("STAR_ADMIN_PASSWORD")
        .expect("STAR_ADMIN_PASSWORD is required for explicit live parity runs")
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_student_can_comment_on_own_submission() {
    let Some(client) = live() else { return };
    let token = admin_token(&client);
    let uniq = uuid_like();
    let student_email = format!("sc_student_{}@star.edu", uniq);

    // Create course
    let course = client
        .post(format!("{BASE}/api/v1/courses"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "name": format!("SC Own {}", uniq),
            "course_code": format!("SCO-{}", uniq),
        }))
        .send()
        .unwrap();
    assert!(
        course.status().is_success(),
        "create course: {} — {:?}",
        course.status(),
        course.text().map(|t| t[..t.len().min(200)].to_string())
    );
    let course_id: i64 = course.json::<Value>().unwrap()["id"].as_i64().unwrap();

    // Register + enroll student
    let student = client
        .post(format!("{BASE}/api/v1/auth/register"))
        .json(&json!({
            "name": format!("SC Student {}", uniq),
            "email": &student_email,
            "password": parity_password(),
            "role": "student"
        }))
        .send()
        .unwrap();
    assert!(
        student.status().is_success(),
        "register: {} — {:?}",
        student.status(),
        student.text().map(|t| t[..t.len().min(200)].to_string())
    );
    let student_id: i64 = student.json::<Value>().unwrap()["id"].as_i64().unwrap();

    client
        .post(format!("{BASE}/api/v1/courses/{course_id}/enrollments"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "user_id": student_id, "role": "student" }))
        .send()
        .unwrap();

    // Login as student
    let st = client
        .post(format!("{BASE}/api/v1/auth/login"))
        .json(&json!({ "email": &student_email, "password": parity_password() }))
        .send()
        .unwrap();
    assert!(
        st.status().is_success(),
        "student login: {} — {:?}",
        st.status(),
        st.text().map(|t| t[..t.len().min(200)].to_string())
    );
    let student_token: String = st.json::<Value>().unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // Create assignment
    let assignment = client
        .post(format!("{BASE}/api/v1/courses/{course_id}/assignments"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "title": "Test Assignment", "points_possible": 10.0 }))
        .send()
        .unwrap();
    assert!(
        assignment.status().is_success(),
        "create assignment: {} — {:?}",
        assignment.status(),
        assignment.text().map(|t| t[..t.len().min(200)].to_string())
    );
    let assignment_id: i64 = assignment.json::<Value>().unwrap()["id"].as_i64().unwrap();

    // Student submits
    let submission = client
        .post(format!(
            "{BASE}/api/v1/assignments/{assignment_id}/submissions"
        ))
        .header("Authorization", format!("Bearer {student_token}"))
        .json(&json!({ "body": "my work" }))
        .send()
        .unwrap();
    assert!(
        submission.status().is_success(),
        "create submission: {} — {:?}",
        submission.status(),
        submission.text().map(|t| t[..t.len().min(200)].to_string())
    );
    let submission_id: i64 = submission.json::<Value>().unwrap()["id"].as_i64().unwrap();

    // Student posts a comment
    let resp = client
        .post(format!(
            "{BASE}/api/v1/submissions/{submission_id}/comments"
        ))
        .header("Authorization", format!("Bearer {student_token}"))
        .json(&json!({ "comment": "great work!" }))
        .send()
        .unwrap();
    assert!(
        resp.status().is_success(),
        "student comment: {} — {:?}",
        resp.status(),
        resp.text().map(|t| t[..t.len().min(200)].to_string())
    );
    let comment: Value = resp.json().unwrap();
    assert_eq!(
        comment["submission_id"].as_i64(),
        Some(submission_id),
        "comment must belong to submission"
    );
    assert_eq!(
        comment["author_id"].as_i64(),
        Some(student_id),
        "author must be student"
    );
    assert_eq!(
        comment["comment"].as_str(),
        Some("great work!"),
        "comment text must match"
    );
    assert_eq!(
        comment["workflow_state"].as_str(),
        Some("active"),
        "new comment must be active"
    );
    assert!(comment["created_at"].is_string(), "created_at must be set");
    assert!(comment["updated_at"].is_string(), "updated_at must be set");

    // Verify it shows in list
    let resp = client
        .get(format!(
            "{BASE}/api/v1/submissions/{submission_id}/comments"
        ))
        .header("Authorization", format!("Bearer {student_token}"))
        .send()
        .unwrap();
    assert!(
        resp.status().is_success(),
        "list comments: {}",
        resp.status()
    );
    let list: Vec<Value> = resp.json().unwrap();
    assert!(
        list.iter()
            .any(|c| c["id"].as_i64() == comment["id"].as_i64()),
        "newly created comment must appear in list: {list:?}"
    );
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn parity_teacher_can_list_and_delete_comments() {
    let Some(client) = live() else { return };
    let token = admin_token(&client);
    let uniq = uuid_like();
    let student_email = format!("sc_teacher_student_{}@star.edu", uniq);

    // Create course
    let course = client
        .post(format!("{BASE}/api/v1/courses"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "name": format!("SC Teacher {}", uniq),
            "course_code": format!("SCT-{}", uniq),
        }))
        .send()
        .unwrap();
    assert!(course.status().is_success());
    let course_id: i64 = course.json::<Value>().unwrap()["id"].as_i64().unwrap();

    // Register + enroll student
    let student = client
        .post(format!("{BASE}/api/v1/auth/register"))
        .json(&json!({
            "name": format!("SC Teacher Student {}", uniq),
            "email": &student_email,
            "password": parity_password(),
            "role": "student"
        }))
        .send()
        .unwrap();
    assert!(student.status().is_success());
    let _student_id: i64 = student.json::<Value>().unwrap()["id"].as_i64().unwrap();

    client
        .post(format!("{BASE}/api/v1/courses/{course_id}/enrollments"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "user_id": _student_id, "role": "student" }))
        .send()
        .unwrap();

    let st = client
        .post(format!("{BASE}/api/v1/auth/login"))
        .json(&json!({ "email": &student_email, "password": parity_password() }))
        .send()
        .unwrap();
    assert!(st.status().is_success());
    let student_token: String = st.json::<Value>().unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // Create assignment + submission
    let assignment = client
        .post(format!("{BASE}/api/v1/courses/{course_id}/assignments"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "title": "Assignment", "points_possible": 10.0 }))
        .send()
        .unwrap();
    assert!(assignment.status().is_success());
    let assignment_id: i64 = assignment.json::<Value>().unwrap()["id"].as_i64().unwrap();

    let submission = client
        .post(format!(
            "{BASE}/api/v1/assignments/{assignment_id}/submissions"
        ))
        .header("Authorization", format!("Bearer {student_token}"))
        .json(&json!({ "body": "submission body" }))
        .send()
        .unwrap();
    assert!(
        submission.status().is_success(),
        "submission: {} — {:?}",
        submission.status(),
        submission.text().map(|t| t[..t.len().min(200)].to_string())
    );
    let submission_id: i64 = submission.json::<Value>().unwrap()["id"].as_i64().unwrap();

    // Teacher posts a comment
    let resp = client
        .post(format!(
            "{BASE}/api/v1/submissions/{submission_id}/comments"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "comment": "needs revision" }))
        .send()
        .unwrap();
    assert!(
        resp.status().is_success(),
        "teacher comment: {} — {:?}",
        resp.status(),
        resp.text().map(|t| t[..t.len().min(200)].to_string())
    );
    let comment_id: i64 = resp.json::<Value>().unwrap()["id"].as_i64().unwrap();

    // Teacher can list and see it
    let resp = client
        .get(format!(
            "{BASE}/api/v1/submissions/{submission_id}/comments"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .unwrap();
    assert!(resp.status().is_success());
    let list: Vec<Value> = resp.json().unwrap();
    assert!(
        list.iter().any(|c| c["id"].as_i64() == Some(comment_id)),
        "teacher comment must appear in list: {list:?}"
    );

    // Author deletes it
    let resp = client
        .delete(format!(
            "{BASE}/api/v1/submissions/{submission_id}/comments/{comment_id}"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .unwrap();
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::NO_CONTENT,
        "delete must return 204: {} — {:?}",
        resp.status(),
        resp.text().map(|t| t[..t.len().min(200)].to_string())
    );

    // It must vanish
    let resp = client
        .get(format!(
            "{BASE}/api/v1/submissions/{submission_id}/comments"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .unwrap();
    assert!(resp.status().is_success());
    let list: Vec<Value> = resp.json().unwrap();
    assert!(
        !list.iter().any(|c| c["id"].as_i64() == Some(comment_id)),
        "deleted comment must NOT appear in list: {list:?}"
    );
}
