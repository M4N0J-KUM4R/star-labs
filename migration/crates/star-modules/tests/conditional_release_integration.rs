//! Conditional release rules (Canvas `ConditionalRelease::RulesController`
//! API v1) integration tests.
//!
//! Covers the five Canvas routes under
//! `/api/v1/courses/:course_id/mastery_paths/rules`: index, show, create,
//! update and destroy.
//!
//! The module is wired into the crate root, so the test drives the crate
//! router.
//!
//! Fleet rule: dedicated users, accounts, courses and assignments per test,
//! every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is
//! unset, and skipped cleanly while the `conditional_release_*` tables are
//! not migrated yet.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "cr-{}-{}-{}",
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

    let tables: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_name IN ('conditional_release_rules', \
           'conditional_release_scoring_ranges', \
           'conditional_release_assignment_sets', \
           'conditional_release_assignment_set_associations')",
    )
    .fetch_one(&pool)
    .await
    .expect("count the conditional release tables");
    if tables.0 < 4 {
        println!(
            "SKIP conditional_release_integration: the conditional_release_* tables are not migrated yet"
        );
        return None;
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_modules::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
    )
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id, workflow_state) \
         VALUES ($1, NULL, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id, workflow_state) \
         VALUES ($1, $2, $3, 'available') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active') RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO assignments (course_id, title) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(title)
        .fetch_one(pool)
        .await
        .expect("seed assignment")
}

/// The fixture of the conditional release tests.
struct Fixture {
    course_id: i64,
    other_course_id: i64,
    teacher_id: i64,
    student_id: i64,
    outsider_id: i64,
    trigger_assignment_id: i64,
    other_trigger_assignment_id: i64,
    assignment_ids: Vec<i64>,
}

async fn seed_fixture(pool: &PgPool, prefix: &str) -> Fixture {
    let account_id = seed_account(pool, prefix, "Account").await;
    let course_id = seed_course(pool, prefix, "Course", account_id).await;
    let other_course_id = seed_course(pool, prefix, "OtherCourse", account_id).await;

    let teacher_id = seed_user(pool, prefix, "Teacher", "teacher").await;
    let student_id = seed_user(pool, prefix, "Student", "student").await;
    let outsider_id = seed_user(pool, prefix, "Outsider", "user").await;
    seed_enrollment(pool, teacher_id, course_id, "teacher").await;
    seed_enrollment(pool, student_id, course_id, "student").await;
    seed_enrollment(pool, teacher_id, other_course_id, "teacher").await;

    let trigger_assignment_id = seed_assignment(pool, course_id, "Trigger").await;
    let other_trigger_assignment_id = seed_assignment(pool, course_id, "Other trigger").await;
    let foreign_assignment_id = seed_assignment(pool, other_course_id, "Foreign").await;
    let assignment_one = seed_assignment(pool, course_id, "One").await;
    let assignment_two = seed_assignment(pool, course_id, "Two").await;

    Fixture {
        course_id,
        other_course_id,
        teacher_id,
        student_id,
        outsider_id,
        trigger_assignment_id,
        other_trigger_assignment_id,
        assignment_ids: vec![assignment_one, assignment_two, foreign_assignment_id],
    }
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
    let statements = [
        "DELETE FROM conditional_release_assignment_set_associations WHERE assignment_set_id IN ( \
           SELECT s.id FROM conditional_release_assignment_sets s \
           JOIN conditional_release_scoring_ranges r ON r.id = s.scoring_range_id \
           JOIN conditional_release_rules ru ON ru.id = r.rule_id \
           WHERE ru.course_id IN (SELECT id FROM courses WHERE course_code LIKE $1))",
        "DELETE FROM conditional_release_assignment_sets WHERE scoring_range_id IN ( \
           SELECT r.id FROM conditional_release_scoring_ranges r \
           JOIN conditional_release_rules ru ON ru.id = r.rule_id \
           WHERE ru.course_id IN (SELECT id FROM courses WHERE course_code LIKE $1))",
        "DELETE FROM conditional_release_scoring_ranges WHERE rule_id IN ( \
           SELECT id FROM conditional_release_rules \
           WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1))",
        "DELETE FROM conditional_release_rules \
           WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM enrollments \
           WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM assignments \
           WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM courses WHERE course_code LIKE $1",
        "DELETE FROM accounts WHERE name LIKE $1",
        "DELETE FROM users WHERE email LIKE $1",
    ];
    for statement in statements {
        sqlx::query(statement)
            .bind(&pattern)
            .execute(pool)
            .await
            .ok();
    }
}

/// The rule body of the Canvas create request.
fn create_body(trigger_assignment_id: i64, assignment_ids: &[i64]) -> Value {
    json!({
        "trigger_assignment_id": trigger_assignment_id,
        "scoring_ranges": [
            {
                "lower_bound": 65,
                "upper_bound": 95,
                "assignment_sets": [
                    {
                        "assignment_set_associations": [
                            { "assignment_id": assignment_ids[0], "position": 1 },
                            { "assignment_id": assignment_ids[1], "position": 2 }
                        ]
                    }
                ]
            }
        ]
    })
}

#[tokio::test]
async fn the_rules_of_a_course_are_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let rules = body.as_array().expect("an array of rules");
    assert_eq!(rules.len(), 0, "body: {body}");

    // Canvas does not render the nested collections unless they are asked
    // for.
    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&create_body(
            fixture.trigger_assignment_id,
            &fixture.assignment_ids,
        ))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rule: Value = resp.json();
    assert_eq!(rule["course_id"].as_i64(), Some(fixture.course_id));
    assert_eq!(
        rule["trigger_assignment_id"].as_i64(),
        Some(fixture.trigger_assignment_id)
    );
    assert!(rule["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(rule["updated_at"].as_str().unwrap().ends_with('Z'));
    assert!(rule.get("root_account_id").is_none());
    assert!(rule.get("deleted_at").is_none());

    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let rules = body.as_array().expect("an array of rules");
    assert_eq!(rules.len(), 1, "body: {body}");
    assert_eq!(rules[0]["id"].as_i64(), Some(rule["id"].as_i64().unwrap()));
    assert!(rules[0].get("scoring_ranges").is_none(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_nested_collections_are_rendered_when_they_are_asked_for() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&create_body(
            fixture.trigger_assignment_id,
            &fixture.assignment_ids,
        ))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules?include=all",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let rules = body.as_array().expect("an array of rules");
    assert_eq!(rules.len(), 1, "body: {body}");
    let ranges = rules[0]["scoring_ranges"].as_array().expect("the ranges");
    assert_eq!(ranges.len(), 1, "body: {body}");
    assert_eq!(
        ranges[0]["lower_bound"].as_f64(),
        Some(65.0),
        "body: {body}"
    );
    assert_eq!(
        ranges[0]["upper_bound"].as_f64(),
        Some(95.0),
        "body: {body}"
    );
    assert_eq!(ranges[0]["position"].as_i64(), Some(1), "body: {body}");
    let sets = ranges[0]["assignment_sets"].as_array().expect("the sets");
    assert_eq!(sets.len(), 1, "body: {body}");
    assert_eq!(sets[0]["position"].as_i64(), Some(1), "body: {body}");
    let associations = sets[0]["assignment_set_associations"]
        .as_array()
        .expect("the associations");
    assert_eq!(associations.len(), 2, "body: {body}");
    assert_eq!(
        associations[0]["assignment_id"].as_i64(),
        Some(fixture.assignment_ids[0]),
        "body: {body}"
    );
    assert_eq!(
        associations[1]["assignment_id"].as_i64(),
        Some(fixture.assignment_ids[1]),
        "body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_active_filter_keeps_the_rules_with_assignments() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    // A rule without any assignment.
    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "trigger_assignment_id": fixture.trigger_assignment_id,
            "scoring_ranges": [{ "lower_bound": 0, "upper_bound": 1 }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let empty_rule: Value = resp.json();

    // A rule with assignments.
    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&create_body(
            fixture.other_trigger_assignment_id,
            &fixture.assignment_ids,
        ))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().expect("rules").len(), 2, "body: {body}");

    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules?active=true",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let rules = body.as_array().expect("rules");
    assert_eq!(rules.len(), 1, "body: {body}");
    assert_ne!(
        rules[0]["id"].as_i64(),
        empty_rule["id"].as_i64(),
        "body: {body}"
    );

    // The rules can be filtered by their trigger assignment.
    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules?trigger_assignment_id={}",
            fixture.course_id, fixture.other_trigger_assignment_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().expect("rules").len(), 1, "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_user_outside_the_course_may_not_read_the_rules() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.outsider_id, "user"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_may_read_the_rules_but_not_write_them() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.student_id, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.student_id, "student"),
        )
        .json(&create_body(
            fixture.trigger_assignment_id,
            &fixture.assignment_ids,
        ))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_unknown_course_is_a_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .get("/courses/999999999/mastery_paths/rules")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_rule_without_a_trigger_assignment_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({ "scoring_ranges": [{ "lower_bound": 0, "upper_bound": 1 }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["trigger_assignment_id"][0].as_str(),
        Some("can't be blank"),
        "body: {body}"
    );
    assert_eq!(
        body["trigger_assignment"][0].as_str(),
        Some("must exist"),
        "body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_trigger_assignment_of_another_course_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "trigger_assignment_id": fixture.assignment_ids[2],
            "scoring_ranges": [{ "lower_bound": 0, "upper_bound": 1 }]
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["trigger_assignment_id"][0].as_str(),
        Some("invalid trigger assignment"),
        "body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_scoring_range_without_bounds_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "trigger_assignment_id": fixture.trigger_assignment_id,
            "scoring_ranges": [{ "foo": 3 }]
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["scoring_ranges.base"][0].as_str(),
        Some("one bound must exist"),
        "body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_assignment_set_association_without_an_assignment_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "trigger_assignment_id": fixture.trigger_assignment_id,
            "scoring_ranges": [{
                "lower_bound": 65,
                "upper_bound": 95,
                "assignment_sets": [{ "assignment_set_associations": [{ "foo": 3 }] }]
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["scoring_ranges.assignment_sets.assignment_set_associations.assignment_id"][0]
            .as_str(),
        Some("can't be blank"),
        "body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_rule_is_shown_and_a_missing_rule_is_a_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&create_body(
            fixture.trigger_assignment_id,
            &fixture.assignment_ids,
        ))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rule: Value = resp.json();
    let rule_id = rule["id"].as_i64().expect("the rule id");

    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules/{}",
            fixture.course_id, rule_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(rule_id), "body: {body}");
    assert!(body.get("scoring_ranges").is_none(), "body: {body}");

    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules/{}",
            fixture.course_id,
            rule_id + 1_000_000
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_trigger_assignment_of_a_rule_is_updated() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&create_body(
            fixture.trigger_assignment_id,
            &fixture.assignment_ids,
        ))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rule: Value = resp.json();
    let rule_id = rule["id"].as_i64().expect("the rule id");

    let resp = server
        .put(&format!(
            "/courses/{}/mastery_paths/rules/{}",
            fixture.course_id, rule_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({ "trigger_assignment_id": fixture.other_trigger_assignment_id }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["trigger_assignment_id"].as_i64(),
        Some(fixture.other_trigger_assignment_id),
        "body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_scoring_range_the_request_does_not_mention_is_destroyed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "trigger_assignment_id": fixture.trigger_assignment_id,
            "scoring_ranges": [
                { "lower_bound": 0, "upper_bound": 0.5 },
                { "lower_bound": 0.5, "upper_bound": 1 }
            ]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rule: Value = resp.json();
    let rule_id = rule["id"].as_i64().expect("the rule id");
    let ranges = rule["scoring_ranges"].as_array().expect("the ranges");
    assert_eq!(ranges.len(), 2, "body: {rule}");
    assert_eq!(ranges.len(), 2, "rule: {rule}");
    let second_range_id = ranges[1]["id"].as_i64().expect("the range id");

    // Only the second range is sent, so the first one is destroyed.
    let resp = server
        .put(&format!(
            "/courses/{}/mastery_paths/rules/{}",
            fixture.course_id, rule_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "scoring_ranges": [
                { "id": second_range_id, "lower_bound": 0.5, "upper_bound": 1 }
            ]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let ranges = body["scoring_ranges"].as_array().expect("the ranges");
    assert_eq!(ranges.len(), 1, "body: {body}");
    assert_eq!(
        ranges[0]["id"].as_i64(),
        Some(second_range_id),
        "body: {body}"
    );
    // The positions are renumbered from one.
    assert_eq!(ranges[0]["position"].as_i64(), Some(1), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_rule_is_destroyed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&create_body(
            fixture.trigger_assignment_id,
            &fixture.assignment_ids,
        ))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rule: Value = resp.json();
    let rule_id = rule["id"].as_i64().expect("the rule id");

    let resp = server
        .delete(&format!(
            "/courses/{}/mastery_paths/rules/{}",
            fixture.course_id, rule_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["success"].as_bool(), Some(true), "body: {body}");

    // The rule is soft deleted, so it is gone from the course.
    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().expect("rules").len(), 0, "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_explicit_null_bound_clears_the_bound() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "trigger_assignment_id": fixture.trigger_assignment_id,
            "scoring_ranges": [{ "lower_bound": 0, "upper_bound": 1 }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rule: Value = resp.json();
    let rule_id = rule["id"].as_i64().expect("the rule id");
    let range_id = rule["scoring_ranges"][0]["id"]
        .as_i64()
        .expect("the range id");

    // A `null` bound is assigned to the attribute, it does not keep the
    // bound of the request that is missing.
    let resp = server
        .put(&format!(
            "/courses/{}/mastery_paths/rules/{}",
            fixture.course_id, rule_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "scoring_ranges": [
                { "id": range_id, "lower_bound": null, "upper_bound": 1 }
            ]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let ranges = body["scoring_ranges"].as_array().expect("the ranges");
    assert_eq!(ranges.len(), 1, "body: {body}");
    assert_eq!(ranges[0]["lower_bound"], Value::Null, "body: {body}");
    assert_eq!(ranges[0]["upper_bound"].as_f64(), Some(1.0), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_rule_of_another_course_is_a_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/mastery_paths/rules",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&create_body(
            fixture.trigger_assignment_id,
            &fixture.assignment_ids,
        ))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rule: Value = resp.json();
    let rule_id = rule["id"].as_i64().expect("the rule id");

    // The rule belongs to another course, so it is not found there.
    let resp = server
        .get(&format!(
            "/courses/{}/mastery_paths/rules/{}",
            fixture.other_course_id, rule_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
