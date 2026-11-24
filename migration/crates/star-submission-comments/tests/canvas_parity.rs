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
//! submissions_api_controller.rb).  Canvas declares no comments collection
//! index/create in the API route set (routes.rb:1580-1583 declare only
//! create_file, PUT and DELETE on the nested member path), so the thread is
//! exercised through the Canvas-real pair:
//! - PUT  .../courses/{id}/assignments/{assignmentId}/submissions/{userId}
//!   {comment} writes the comment (routes.rb:1551)
//! - GET  same path with `include[]=submission_comments` → [active comments],
//!   ordered by created_at, each {id, submission_id, author_id, comment,
//!   workflow_state, created_at, updated_at}
//! - DELETE /courses/{id}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId} → 204, soft-delete

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

/// The id of a comment carrying `text`, read through Canvas'
/// `include[]=submission_comments` on the submission
/// (submissions_api_controller.rb:231); `None` once the comment is gone from
/// the thread (Canvas soft-deletes keep the row but drop it from the include).
fn thread_comment_id(
    client: &reqwest::blocking::Client,
    token: &str,
    course_id: i64,
    assignment_id: i64,
    student_id: i64,
    text: &str,
) -> Option<i64> {
    let resp = client
        .get(format!(
            "{BASE}/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/{student_id}?include[]=submission_comments"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .unwrap();
    assert!(
        resp.status().is_success(),
        "submission read: {}",
        resp.status()
    );
    let body: Value = resp.json().unwrap();
    let list = body["submission_comments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    list.iter()
        .find(|c| c["comment"].as_str() == Some(text))
        .and_then(|c| c["id"].as_i64())
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
        .post(format!("{BASE}/api/v1/accounts/1/courses"))
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
            "{BASE}/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions"
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

    // Canvas writes the thread through the submission update
    // (routes.rb:1551, `comment` argument); there is no comments collection
    // create route in the API route set (routes.rb:1580-1583).
    let resp = client
        .put(format!(
            "{BASE}/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/{student_id}"
        ))
        .header("Authorization", format!("Bearer {student_token}"))
        .json(&json!({ "comment": "great work!" }))
        .send()
        .unwrap();
    assert!(
        resp.status().is_success(),
        "student comment write: {} — {:?}",
        resp.status(),
        resp.text().map(|t| t[..t.len().min(200)].to_string())
    );

    // The thread is read back as the submission's
    // `include[]=submission_comments` (submissions_api_controller.rb:231),
    // ordered by created_at, active comments only.
    let resp = client
        .get(format!(
            "{BASE}/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/{student_id}?include[]=submission_comments"
        ))
        .header("Authorization", format!("Bearer {student_token}"))
        .send()
        .unwrap();
    assert!(
        resp.status().is_success(),
        "submission read: {}",
        resp.status()
    );
    let submission_json: Value = resp.json().unwrap();
    let list: Vec<Value> = submission_json["submission_comments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let comment = list
        .iter()
        .find(|c| c["comment"].as_str() == Some("great work!"))
        .unwrap_or_else(|| panic!("newly written comment must appear in the include: {list:?}"));
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
        comment["workflow_state"].as_str(),
        Some("active"),
        "new comment must be active"
    );
    assert!(comment["created_at"].is_string(), "created_at must be set");
    assert!(comment["updated_at"].is_string(), "updated_at must be set");
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
        .post(format!("{BASE}/api/v1/accounts/1/courses"))
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
            "{BASE}/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions"
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
    let _submission_id: i64 = submission.json::<Value>().unwrap()["id"].as_i64().unwrap();

    // Teacher writes the comment through the submission update
    // (routes.rb:1551) — Canvas has no comments collection create.
    let resp = client
        .put(format!(
            "{BASE}/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/{_student_id}"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({ "comment": "needs revision" }))
        .send()
        .unwrap();
    assert!(
        resp.status().is_success(),
        "teacher comment write: {} — {:?}",
        resp.status(),
        resp.text().map(|t| t[..t.len().min(200)].to_string())
    );

    // The teacher reads the thread back through the submission include.
    let comment_id: i64 = thread_comment_id(
        &client,
        &token,
        course_id,
        assignment_id,
        _student_id,
        "needs revision",
    )
    .unwrap_or_else(|| panic!("teacher comment must appear in the include"));

    // Author deletes it through the nested member route (routes.rb:1583).
    let resp = client
        .delete(format!(
            "{BASE}/api/v1/courses/{course_id}/assignments/{assignment_id}/submissions/{_student_id}/comments/{comment_id}"
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

    // It must vanish from the thread (soft-deleted, so `workflow_state`
    // 'deleted' rows are not part of the include).
    assert_eq!(
        thread_comment_id(
            &client,
            &token,
            course_id,
            assignment_id,
            _student_id,
            "needs revision",
        ),
        None,
        "deleted comment must NOT appear in the include"
    );
}
