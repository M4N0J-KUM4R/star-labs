//! Integration tests for the LTI AGS Scores/Results and NRPS services
//! (issue #94): `POST /api/lti/courses/{id}/line_items/{id}/scores`,
//! `GET .../results[/{id}]`, `GET /api/lti/courses/{id}/names_and_roles`,
//! `GET /api/lti/groups/{id}/names_and_roles`.
//!
//! Same conventions as `lti_advantage_mount_integration.rs`: the advantage
//! router mounted at the app root (paths carry `/api/lti`), axum-test
//! against a real Postgres, SKIP when DATABASE_URL is unset, unique
//! per-run prefix, full teardown.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

const SCOPE_SCORE: &str = "https://purl.imsglobal.org/spec/lti-ags/scope/score";
const SCOPE_RESULT: &str = "https://purl.imsglobal.org/spec/lti-ags/scope/result.readonly";
const SCOPE_NRPS: &str =
    "https://purl.imsglobal.org/spec/lti-nrps/scope/contextmembership.readonly";

fn run_prefix() -> String {
    // The counter makes the prefix unique inside the process: two tests
    // that start in the same millisecond would otherwise seed the same
    // email and collide on the users' unique key.
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "lti-ags-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
        n
    )
}

async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };

    let app = star_lti::lti_advantage_router().with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

// ---------------------------------------------------------------------------
// Seed helpers
// ---------------------------------------------------------------------------

async fn seed_user(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, 'x', 'student') RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// A developer key + an LTI registration bound to it (the `verify_tool`
/// gate needs the registration row). Returns the developer key id.
async fn seed_tool(pool: &PgPool, prefix: &str) -> i64 {
    let key_id: i64 = sqlx::query_scalar(
        "INSERT INTO developer_keys (name, api_key, workflow_state) \
         VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-key"))
    .bind(format!("{prefix}-secret"))
    .fetch_one(pool)
    .await
    .expect("seed developer key");

    sqlx::query("INSERT INTO lti_ims_registrations (guid, developer_key_id) VALUES ($1, $2)")
        .bind(format!("{prefix}-guid"))
        .bind(key_id)
        .execute(pool)
        .await
        .expect("seed registration");
    key_id
}

/// A line item (+ its backing assignment), active.
async fn seed_line_item(
    pool: &PgPool,
    course_id: i64,
    label: &str,
    score_max: f64,
    published: bool,
) -> (i64, Option<i64>) {
    // assignments have no `published` column — Canvas Assignment#published?
    // maps to workflow_state ('published' | 'unpublished'), like PR #166's
    // line_items.rs:230.
    let assignment_id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible, workflow_state) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(course_id)
    .bind(label)
    .bind(score_max)
    .bind(if published {
        "published"
    } else {
        "unpublished"
    })
    .fetch_one(pool)
    .await
    .ok();

    let line_item_id: i64 = sqlx::query_scalar(
        "INSERT INTO lti_line_items (course_id, assignment_id, label, score_maximum) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(course_id)
    .bind(assignment_id)
    .bind(label)
    .bind(score_max)
    .fetch_one(pool)
    .await
    .expect("seed line item");
    (line_item_id, assignment_id)
}

/// Mint an Advantage service token for the developer key.
fn service_token(client_id: i64, scopes: &[&str]) -> String {
    star_lti::mint_service_token_for_tests(JWT_SECRET, client_id, scopes)
}

fn auth_header(token: &str) -> String {
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // courses cascade line items/assignments/enrollments; results cascade
    // from line items via the 0227 FK.
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM lti_ims_registrations WHERE guid LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM developer_keys WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM groups WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Scores
// -------------------------------------------------------------------

#[tokio::test]
async fn scores_auth_chain_in_canvas_order() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix, "c1").await;
    let (line_item, _) = seed_line_item(&pool, course, "li", 100.0, true).await;
    let key = seed_tool(&pool, &prefix).await;
    let url = format!("/api/lti/courses/{course}/line_items/{line_item}/scores");
    let body = json!({
        "userId": "999999", "activityProgress": "Completed",
        "gradingProgress": "FullyGraded", "timestamp": "2026-01-01T00:00:00Z",
    });

    // 1. no token → 401 Missing access token.
    let resp = server.post(&url).json(&body).await;
    assert_eq!(resp.status_code(), 401, "no token must 401");
    assert!(
        resp.text().contains("Missing access token"),
        "{}",
        resp.text()
    );

    // 2. unknown developer key → 401.
    let bad_key = service_token(999999, &[SCOPE_SCORE]);
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&bad_key))
        .json(&body)
        .await;
    assert_eq!(resp.status_code(), 401);
    assert!(
        resp.text().contains("Unknown or inactive Developer Key"),
        "{}",
        resp.text()
    );

    // 3. wrong scope → 401 Insufficient permissions.
    let wrong_scope = service_token(key, &[SCOPE_RESULT]);
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&wrong_scope))
        .json(&body)
        .await;
    assert_eq!(resp.status_code(), 401);
    assert!(
        resp.text().contains("Insufficient permissions"),
        "{}",
        resp.text()
    );

    // 4. unknown course → 404 Context is deleted or not found.
    let token = service_token(key, &[SCOPE_SCORE]);
    let resp = server
        .post(&format!(
            "/api/lti/courses/987654/line_items/{line_item}/scores"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&body)
        .await;
    assert_eq!(resp.status_code(), 404);
    assert!(
        resp.text().contains("Context is deleted or not found"),
        "{}",
        resp.text()
    );

    // 5. line item in another course → 404.
    let other_course = seed_course(&pool, &prefix, "c2").await;
    let resp = server
        .post(&format!(
            "/api/lti/courses/{other_course}/line_items/{line_item}/scores"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&body)
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn scores_validation_order_and_happy_path() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix, "c1").await;
    let student = seed_user(&pool, &prefix, "student").await;
    seed_enrollment(&pool, student, course, "student").await;
    let (line_item, assignment) = seed_line_item(&pool, course, "li", 100.0, true).await;
    let key = seed_tool(&pool, &prefix).await;
    let token = service_token(key, &[SCOPE_SCORE]);
    let url = format!("/api/lti/courses/{course}/line_items/{line_item}/scores");

    // userId not a student of the course → 422 (before required params).
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&json!({
            "timestamp": "not-even-checked",
        }))
        .await;
    assert_eq!(resp.status_code(), 422);
    assert!(
        resp.text()
            .contains("User not found in course or is not a student"),
        "{}",
        resp.text()
    );

    // missing required params (student ok) → 400.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&json!({ "userId": student }))
        .await;
    assert_eq!(resp.status_code(), 400, "{}", resp.text());

    // unpublished assignment → 422.
    let (unpub_li, _) = seed_line_item(&pool, course, "unpub", 10.0, false).await;
    let resp = server
        .post(&format!(
            "/api/lti/courses/{course}/line_items/{unpub_li}/scores"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&json!({
            "userId": student, "activityProgress": "Completed",
            "gradingProgress": "FullyGraded", "timestamp": "2026-01-01T00:00:00Z",
        }))
        .await;
    assert_eq!(resp.status_code(), 422);
    assert!(resp.text().contains("still unpublished"), "{}", resp.text());

    // invalid timestamp → 400.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&json!({
            "userId": student, "activityProgress": "Completed",
            "gradingProgress": "FullyGraded", "timestamp": "nope",
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert!(
        resp.text().contains("not a valid timestamp"),
        "{}",
        resp.text()
    );

    // scoreGiven without scoreMaximum → 422.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&json!({
            "userId": student, "activityProgress": "Completed",
            "gradingProgress": "FullyGraded", "timestamp": "2026-01-01T00:00:00Z",
            "scoreGiven": 50,
        }))
        .await;
    assert_eq!(resp.status_code(), 422);
    assert!(
        resp.text().contains("ScoreMaximum not supplied"),
        "{}",
        resp.text()
    );

    // negative scoreMaximum → 422.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&json!({
            "userId": student, "activityProgress": "Completed",
            "gradingProgress": "FullyGraded", "timestamp": "2026-01-01T00:00:00Z",
            "scoreGiven": 50, "scoreMaximum": -1,
        }))
        .await;
    assert_eq!(resp.status_code(), 422);
    assert!(
        resp.text()
            .contains("ScoreMaximum must be greater than or equal to 0"),
        "{}",
        resp.text()
    );

    // future submittedAt → 400.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&json!({
            "userId": student, "activityProgress": "Completed",
            "gradingProgress": "FullyGraded", "timestamp": "2026-01-01T00:00:00Z",
            "scoreGiven": 50, "scoreMaximum": 100,
            "submittedAt": "2099-01-01T00:00:00Z",
        }))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert!(resp.text().contains("in the future"), "{}", resp.text());

    // happy path: 201 + resultUrl, result row, submission score passback.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&json!({
            "userId": student.to_string(), "activityProgress": "Completed",
            "gradingProgress": "FullyGraded", "timestamp": "2026-01-01T00:00:00Z",
            "scoreGiven": 83, "scoreMaximum": 100, "comment": "nice",
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "{}", resp.text());
    let text = resp.text();
    let payload: Value = serde_json::from_str(&text).expect("json body");
    let result_url = payload["resultUrl"]
        .as_str()
        .expect("resultUrl")
        .to_string();
    assert!(result_url.contains(&format!(
        "/api/lti/courses/{course}/line_items/{line_item}/results/"
    )));
    assert_eq!(
        payload["https://canvas.instructure.com/lti/submission"]["content_items"],
        json!([])
    );

    let (score,): (Option<f64>,) = sqlx::query_as(
        "SELECT result_score FROM lti_results WHERE lti_line_item_id = $1 AND user_id = $2",
    )
    .bind(line_item)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("result row");
    assert_eq!(score, Some(83.0));

    if let Some(assignment_id) = assignment {
        let (sub_score,): (Option<f64>,) = sqlx::query_as(
            "SELECT score::float8 FROM submissions WHERE assignment_id = $1 AND user_id = $2",
        )
        .bind(assignment_id)
        .bind(student)
        .fetch_one(&pool)
        .await
        .expect("submission row");
        assert_eq!(
            sub_score,
            Some(83.0),
            "grade passback must sync the submission score"
        );
    }

    // second call updates the same result (upsert) and rejects stale timestamps.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .json(&json!({
            "userId": student.to_string(), "activityProgress": "Completed",
            "gradingProgress": "FullyGraded", "timestamp": "2025-06-01T00:00:00Z",
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "stale timestamp must 400: {}",
        resp.text()
    );
    assert!(resp.text().contains("before last updated timestamp"));

    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM lti_results WHERE lti_line_item_id = $1")
            .bind(line_item)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(count, 1, "upsert must not duplicate the result row");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Results
// -------------------------------------------------------------------

#[tokio::test]
async fn results_index_show_and_filters() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix, "c1").await;
    let s1 = seed_user(&pool, &prefix, "s1").await;
    let s2 = seed_user(&pool, &prefix, "s2").await;
    seed_enrollment(&pool, s1, course, "student").await;
    seed_enrollment(&pool, s2, course, "student").await;
    let (line_item, _) = seed_line_item(&pool, course, "li", 100.0, true).await;
    let key = seed_tool(&pool, &prefix).await;
    let score_token = service_token(key, &[SCOPE_SCORE]);
    let result_token = service_token(key, &[SCOPE_RESULT]);

    for (uid, score) in [(s1, 50.0), (s2, 70.0)] {
        let resp = server
            .post(&format!(
                "/api/lti/courses/{course}/line_items/{line_item}/scores"
            ))
            .add_header(axum::http::header::AUTHORIZATION, auth_header(&score_token))
            .json(&json!({
                "userId": uid, "activityProgress": "Completed",
                "gradingProgress": "FullyGraded", "timestamp": "2026-01-01T00:00:00Z",
                "scoreGiven": score, "scoreMaximum": 100,
            }))
            .await;
        assert_eq!(resp.status_code(), 201, "{}", resp.text());
    }

    // index (no filter).
    let resp = server
        .get(&format!(
            "/api/lti/courses/{course}/line_items/{line_item}/results"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(&result_token),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let data: Vec<Value> = serde_json::from_str(&resp.text()).expect("array");
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["userId"], json!(s1.to_string()));
    assert_eq!(data[0]["resultScore"], json!(50.0));
    assert_eq!(
        data[0]["scoreOf"],
        json!(format!("/api/lti/courses/{course}/line_items/{line_item}"))
    );
    assert!(data[0]["id"].as_str().unwrap().ends_with(&format!(
        "/results/{}",
        data[0]["id"].as_str().unwrap().rsplit('/').next().unwrap()
    )));

    // user_id filter.
    let resp = server
        .get(&format!(
            "/api/lti/courses/{course}/line_items/{line_item}/results?user_id={s2}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(&result_token),
        )
        .await;
    assert_eq!(resp.status_code(), 200);
    let data: Vec<Value> = serde_json::from_str(&resp.text()).expect("array");
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["userId"], json!(s2.to_string()));

    // limit.
    let resp = server
        .get(&format!(
            "/api/lti/courses/{course}/line_items/{line_item}/results?limit=1"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(&result_token),
        )
        .await;
    let data: Vec<Value> = serde_json::from_str(&resp.text()).expect("array");
    assert_eq!(data.len(), 1);

    // show.
    let result_id = data[0]["id"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let resp = server
        .get(&format!(
            "/api/lti/courses/{course}/line_items/{line_item}/results/{result_id}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(&result_token),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let one: Value = serde_json::from_str(&resp.text()).expect("obj");
    assert_eq!(one["resultMaximum"], json!(100.0));

    // show for a result of another line item → 404.
    let (other_li, _) = seed_line_item(&pool, course, "li2", 10.0, true).await;
    let resp = server
        .get(&format!(
            "/api/lti/courses/{course}/line_items/{other_li}/results/{result_id}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(&result_token),
        )
        .await;
    assert_eq!(resp.status_code(), 404);

    // wrong scope on index → 401.
    let resp = server
        .get(&format!(
            "/api/lti/courses/{course}/line_items/{line_item}/results"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&score_token))
        .await;
    assert_eq!(resp.status_code(), 401);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// NRPS
// -------------------------------------------------------------------

#[tokio::test]
async fn nrps_course_memberships_role_filter_and_scoping() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix, "c1").await;
    let student = seed_user(&pool, &prefix, "student").await;
    let teacher = seed_user(&pool, &prefix, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let key = seed_tool(&pool, &prefix).await;
    let token = service_token(key, &[SCOPE_NRPS]);
    let url = format!("/api/lti/courses/{course}/names_and_roles");

    // no token → 401.
    let resp = server.get(&url).await;
    assert_eq!(resp.status_code(), 401);

    // wrong scope → 401.
    let wrong = service_token(key, &[SCOPE_SCORE]);
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&wrong))
        .await;
    assert_eq!(resp.status_code(), 401);

    // unknown course → 404.
    let resp = server
        .get("/api/lti/courses/987654/names_and_roles")
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .await;
    assert_eq!(resp.status_code(), 404);

    // full membership.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let payload: Value = serde_json::from_str(&resp.text()).expect("obj");
    assert_eq!(
        payload["id"],
        json!(format!("/api/lti/courses/{course}/names_and_roles"))
    );
    assert_eq!(payload["context"]["id"], json!(course.to_string()));
    let members = payload["members"].as_array().expect("members");
    assert_eq!(members.len(), 2);
    let by_role: Vec<&Value> = members
        .iter()
        .filter(|m| {
            m["roles"].as_array().unwrap().contains(&json!(
                "http://purl.imsglobal.org/vocab/lis/v2/membership#Learner"
            ))
        })
        .collect();
    assert_eq!(by_role.len(), 1);
    assert_eq!(by_role[0]["status"], json!("Active"));
    assert_eq!(by_role[0]["user_id"], json!(student.to_string()));
    assert_eq!(
        by_role[0]["lti11_legacy_user_id"],
        json!(student.to_string())
    );

    // role filter: Instructor keeps only the teacher.
    let resp = server
        .get(&format!(
            "{url}?role=http://purl.imsglobal.org/vocab/lis/v2/membership%23Instructor"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .await;
    let payload: Value = serde_json::from_str(&resp.text()).expect("obj");
    let members = payload["members"].as_array().expect("members");
    assert_eq!(members.len(), 1, "role filter keeps only the teacher");
    assert_eq!(members[0]["user_id"], json!(teacher.to_string()));

    // nonsense role filters out everybody.
    let resp = server
        .get(&format!("{url}?role=urn:nonsense"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .await;
    let payload: Value = serde_json::from_str(&resp.text()).expect("obj");
    assert_eq!(payload["members"].as_array().unwrap().len(), 0);

    // limit.
    let resp = server
        .get(&format!("{url}?limit=1"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .await;
    let payload: Value = serde_json::from_str(&resp.text()).expect("obj");
    assert_eq!(payload["members"].as_array().unwrap().len(), 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nrps_group_memberships() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let member = seed_user(&pool, &prefix, "member").await;
    let group: i64 = sqlx::query_scalar(
        "INSERT INTO groups (name, workflow_state) VALUES ($1, 'available') RETURNING id",
    )
    .bind(format!("{prefix}-group"))
    .fetch_one(&pool)
    .await
    .expect("seed group");
    sqlx::query(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state) \
         VALUES ($1, $2, 'accepted')",
    )
    .bind(group)
    .bind(member)
    .execute(&pool)
    .await
    .expect("seed membership");

    let key = seed_tool(&pool, &prefix).await;
    let token = service_token(key, &[SCOPE_NRPS]);

    // unknown group → 404.
    let resp = server
        .get("/api/lti/groups/987654/names_and_roles")
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .await;
    assert_eq!(resp.status_code(), 404);

    let url = format!("/api/lti/groups/{group}/names_and_roles");
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let payload: Value = serde_json::from_str(&resp.text()).expect("obj");
    let members = payload["members"].as_array().expect("members");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["user_id"], json!(member.to_string()));
    assert!(members[0]["roles"].as_array().unwrap().contains(&json!(
        "http://purl.imsglobal.org/vocab/lis/v2/membership#Member"
    )));

    // role filter with #Member keeps the member.
    let resp = server
        .get(&format!(
            "{url}?role=http://purl.imsglobal.org/vocab/lis/v2/membership%23Member"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth_header(&token))
        .await;
    let payload: Value = serde_json::from_str(&resp.text()).expect("obj");
    assert_eq!(payload["members"].as_array().unwrap().len(), 1);

    cleanup(&pool, &prefix).await;
}
