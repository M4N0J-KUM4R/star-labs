//! Integration tests for the Course/User Canvas field-parity slice
//! (issue #28).
//!
//! Same harness and conventions as `graphql_integration.rs`: real Postgres
//! via DATABASE_URL (SKIP-clean without it), unique run prefix, full
//! teardown, raw JSON `{ "query", "variables" }` POSTs with Bearer tokens.
//!
//! Covers:
//! - `course(id)` field shape parity — camelCase names, ISO timestamps,
//!   nullable `syllabusBody`/`startAt`/`endAt`, string ids, `account`;
//! - `Course.permissions` computed per role (teacher full, student
//!   sendMessages-only, account admin becomeUser);
//! - `Course.usersConnection` gated: teacher/admin see the roster,
//!   enrolled student gets null (Canvas hides, not errors);
//! - `user(id)` shape parity — `sortableName`, `avatarUrl`;
//! - `User.enrollmentsConnection` visibility: self, admin, shared-course
//!   teacher; unrelated caller null;
//! - regressions: existing Course/User fields still resolve.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness (copied from tests/graphql_integration.rs)
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-cu-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the GraphQL router
/// mounted the same way star-http mounts it. Returns None when DATABASE_URL
/// is unset so callers can skip cleanly.
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
        jwt_secret: "test-jwt-secret".to_string(),
    };

    // Same nesting as star-http main.rs: /api/graphql outside /api/v1.
    let app = axum::Router::new()
        .nest("/api/graphql", star_graphql::router())
        .with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Create a user directly in the DB with the given role. Returns user id.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Create a user directly in the DB with an explicit sortable_name.
/// Returns user id.
async fn seed_user_sortable(pool: &PgPool, prefix: &str, name: &str, sortable: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role, sortable_name)
        VALUES ($1, $2, $3, 'student', $4)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(sortable)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Give a user a user_profiles row with an avatar_url. Returns profile id.
async fn seed_profile(pool: &PgPool, user_id: i64, avatar_url: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO user_profiles (user_id, avatar_url)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(avatar_url)
    .fetch_one(pool)
    .await
    .expect("seed profile")
}

/// Create a course directly in the DB with the full Canvas field set.
/// Returns course id.
async fn seed_course_full(
    pool: &PgPool,
    prefix: &str,
    name: &str,
    syllabus: Option<&str>,
    start_at: Option<chrono::DateTime<chrono::Utc>>,
    end_at: Option<chrono::DateTime<chrono::Utc>>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code, syllabus_body, start_at, end_at)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .bind(syllabus)
    .bind(start_at)
    .bind(end_at)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

/// Create a plain course (no syllabus/dates), like the foundation harness.
async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    seed_course_full(pool, prefix, name, None, None, None).await
}

/// Give a user an active enrollment in a course with the given role.
async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO enrollments (user_id, course_id, role, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

/// Create an account directly in the DB. Returns account id.
async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO accounts (name)
        VALUES ($1)
        RETURNING id
        "#,
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

/// Make a user an account admin (account_users row). Returns row id.
async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO account_users (account_id, user_id)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed account admin")
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run: account_users, enrollments,
/// user_profiles + users, courses, accounts.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM user_profiles WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

/// POST a raw JSON GraphQL request `{ query, variables }` with the given
/// bearer token. Returns the full JSON body (data + errors).
async fn gql_post(server: &TestServer, token: &str, query: &str, variables: Value) -> Value {
    let resp = server
        .post("/api/graphql")
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "query": query, "variables": variables }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "graphql post should succeed; body: {}",
        resp.text()
    );
    resp.json()
}

// ---------------------------------------------------------------------------
// course(id): Canvas field shape parity for the #28 additions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_returns_canvas_field_names_for_issue_28_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let start = chrono::Utc::now();
    let end = start + chrono::Duration::days(90);
    let course = seed_course_full(
        &pool,
        &prefix,
        "ParityCourse",
        Some("<p>Read the syllabus</p>"),
        Some(start),
        Some(end),
    )
    .await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { id name courseCode syllabusBody state \
         startAt endAt createdAt updatedAt } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let node = &body["data"]["course"];
    assert_eq!(node["name"], "ParityCourse");
    assert_eq!(node["courseCode"], format!("{prefix}-ParityCourse"));
    assert_eq!(node["syllabusBody"], "<p>Read the syllabus</p>");
    assert_eq!(node["state"], "available");
    // ISO timestamps, not null: seeded startAt/endAt.
    assert!(
        node["startAt"].as_str().is_some_and(|v| !v.is_empty()),
        "startAt should be the seeded ISO timestamp: {body}"
    );
    assert!(
        node["endAt"].as_str().is_some_and(|v| !v.is_empty()),
        "endAt should be the seeded ISO timestamp: {body}"
    );
    assert!(
        node["createdAt"].as_str().is_some_and(|v| !v.is_empty()),
        "createdAt should be an ISO timestamp: {body}"
    );
    assert!(
        node["updatedAt"].as_str().is_some_and(|v| !v.is_empty()),
        "updatedAt should be an ISO timestamp: {body}"
    );

    // Nullables serialize as JSON null when unset (Canvas shape parity).
    let bare = seed_course_full(&pool, &prefix, "BareCourse", None, None, None).await;
    seed_enrollment(&pool, teacher, bare, "teacher").await;
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { courseCode syllabusBody startAt endAt } }",
        json!({ "id": bare.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let node = &body["data"]["course"];
    assert_eq!(node["courseCode"], format!("{prefix}-BareCourse"));
    assert!(node["syllabusBody"].is_null(), "body: {body}");
    assert!(node["startAt"].is_null(), "body: {body}");
    assert!(node["endAt"].is_null(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Course.account: owning account resolvable by an enrolled caller
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_account_returns_owning_account_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "OwnerAccount").await;
    let course = seed_course_full(&pool, &prefix, "AccountCourse", None, None, None).await;
    // Explicit owning account.
    sqlx::query("UPDATE courses SET account_id = $1 WHERE id = $2")
        .bind(account)
        .bind(course)
        .execute(&pool)
        .await
        .expect("set account_id");
    seed_enrollment(&pool, student, course, "student").await;

    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { course(id: $id) { account { id name state createdAt updatedAt } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let acct = &body["data"]["course"]["account"];
    assert_eq!(acct["id"], account.to_string());
    assert_eq!(acct["name"], format!("{prefix}-OwnerAccount"));
    assert_eq!(acct["state"], "active");
    assert!(
        acct["createdAt"].as_str().is_some_and(|v| !v.is_empty()),
        "createdAt should be an ISO timestamp: {body}"
    );
    assert!(
        acct["updatedAt"].as_str().is_some_and(|v| !v.is_empty()),
        "updatedAt should be an ISO timestamp: {body}"
    );

    // A course with no owning account resolves null, not an error
    // (courses.account_id is nullable).
    sqlx::query("UPDATE courses SET account_id = NULL WHERE id = $1")
        .bind(course)
        .execute(&pool)
        .await
        .expect("clear account_id");
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { course(id: $id) { account { id } } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "missing account is null, not an error: {body}"
    );
    assert!(body["data"]["course"]["account"].is_null(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Course.permissions: computed per role
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_permissions_computed_per_role() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "PermAccount").await;
    let account_admin = seed_user(&pool, &prefix, "AccountAdmin", "student").await;
    seed_account_admin(&pool, account, account_admin).await;
    let course = seed_course_full(&pool, &prefix, "PermCourse", None, None, None).await;
    sqlx::query("UPDATE courses SET account_id = $1 WHERE id = $2")
        .bind(account)
        .bind(course)
        .execute(&pool)
        .await
        .expect("set account_id");
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    // The account admin is enrolled as a student so `course(id)` resolves
    // for them; the becomeUser flag must still light up via account_users.
    seed_enrollment(&pool, account_admin, course, "student").await;

    let query = "query ($id: ID!) { course(id: $id) { permissions { manageGrades \
                 sendMessages viewAllGrades viewAnalytics becomeUser } } }";
    let variables = json!({ "id": course.to_string() });

    // Teacher: full management rights, no account-admin becomeUser.
    let body = gql_post(&server, &auth(teacher, "teacher"), query, variables.clone()).await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let perms = &body["data"]["course"]["permissions"];
    assert_eq!(perms["manageGrades"], true, "body: {body}");
    assert_eq!(perms["sendMessages"], true, "body: {body}");
    assert_eq!(perms["viewAllGrades"], true, "body: {body}");
    assert_eq!(perms["viewAnalytics"], true, "body: {body}");
    assert_eq!(
        perms["becomeUser"], false,
        "course teacher without account_users row: {body}"
    );

    // Student: sendMessages only (they are enrolled), everything else false.
    let body = gql_post(&server, &auth(student, "student"), query, variables.clone()).await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let perms = &body["data"]["course"]["permissions"];
    assert_eq!(perms["manageGrades"], false, "body: {body}");
    assert_eq!(perms["sendMessages"], true, "body: {body}");
    assert_eq!(perms["viewAllGrades"], false, "body: {body}");
    assert_eq!(perms["viewAnalytics"], false, "body: {body}");
    assert_eq!(perms["becomeUser"], false, "body: {body}");

    // Global admin: everything true.
    let body = gql_post(&server, &auth(admin, "admin"), query, variables.clone()).await;
    let perms = &body["data"]["course"]["permissions"];
    assert_eq!(perms["manageGrades"], true, "body: {body}");
    assert_eq!(perms["sendMessages"], true, "body: {body}");
    assert_eq!(perms["viewAllGrades"], true, "body: {body}");
    assert_eq!(perms["viewAnalytics"], true, "body: {body}");
    assert_eq!(perms["becomeUser"], true, "body: {body}");

    // Account admin enrolled as a student: becomeUser via the account chain,
    // but no management rights (they are not a teacher here).
    let body = gql_post(
        &server,
        &auth(account_admin, "student"),
        query,
        variables.clone(),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let perms = &body["data"]["course"]["permissions"];
    assert_eq!(perms["manageGrades"], false, "body: {body}");
    assert_eq!(perms["sendMessages"], true, "body: {body}");
    assert_eq!(
        perms["becomeUser"], true,
        "account admin on the course's account chain: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Course.usersConnection: gated + paginated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn users_connection_returns_canvas_field_names() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user_sortable(&pool, &prefix, "Ada Lovelace", "Lovelace, Ada").await;
    let student = seed_user(&pool, &prefix, "Grace Hopper", "student").await;
    seed_profile(&pool, student, "https://cdn.example.test/avatar.png").await;
    let course = seed_course(&pool, &prefix, "UsersConnCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { usersConnection(first: 10) \
         { edges { cursor node { id name email sortableName avatarUrl createdAt } } \
         pageInfo { hasNextPage endCursor } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let conn = &body["data"]["course"]["usersConnection"];
    let edges = conn["edges"].as_array().expect("edges array");
    assert_eq!(edges.len(), 2, "teacher sees the whole roster: {body}");
    let find = |name: &str| {
        edges
            .iter()
            .find(|e| e["node"]["name"] == name)
            .unwrap_or_else(|| panic!("{} in roster: {body}", name))
    };
    let ada = find("Ada Lovelace");
    assert_eq!(
        ada["node"]["email"],
        format!("{prefix}-Ada Lovelace@test.local")
    );
    assert_eq!(ada["node"]["sortableName"], "Lovelace, Ada");
    assert!(
        ada["node"]["avatarUrl"].is_null(),
        "Ada has no profile row: {body}"
    );
    assert!(
        ada["node"]["createdAt"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "createdAt should be an ISO timestamp: {body}"
    );
    let grace = find("Grace Hopper");
    // sortable_name coalesces onto name when the column is NULL.
    assert_eq!(grace["node"]["sortableName"], "Grace Hopper");
    assert_eq!(
        grace["node"]["avatarUrl"],
        "https://cdn.example.test/avatar.png"
    );
    assert!(
        edges[0]["cursor"].as_str().is_some_and(|c| !c.is_empty()),
        "edge cursor must be present: {body}"
    );
    assert_eq!(conn["pageInfo"]["hasNextPage"], false, "body: {body}");
    assert_eq!(
        conn["pageInfo"]["endCursor"],
        edges.last().unwrap()["cursor"],
        "endCursor equals the last edge's cursor: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn users_connection_hidden_from_unauthorized_callers() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "GatedUsersCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let query = "query ($id: ID!) { course(id: $id) { usersConnection(first: 10) \
                 { edges { node { id } } } } }";
    let variables = json!({ "id": course.to_string() });

    // Enrolled student resolves the course but NOT the roster connection:
    // Canvas hides the field (null), never an error.
    let body = gql_post(&server, &auth(student, "student"), query, variables.clone()).await;
    assert!(
        body.get("errors").is_none(),
        "unauthorized field access is null, not an error: {body}"
    );
    assert!(
        body["data"]["course"]["usersConnection"].is_null(),
        "enrolled student must not see the roster connection: {body}"
    );

    // Teacher and admin see it.
    for token in [auth(teacher, "teacher"), auth(admin, "admin")] {
        let body = gql_post(&server, &token, query, variables.clone()).await;
        assert!(
            !body["data"]["course"]["usersConnection"].is_null(),
            "teacher/admin see the roster: {body}"
        );
    }

    // Outsider cannot even resolve the course (existing gate, regression).
    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        query,
        variables.clone(),
    )
    .await;
    assert!(body["data"]["course"].is_null(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn users_connection_paginates_across_two_pages() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PagedUsersCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // Three students, enrolled in id order (ORDER BY u.id ascending).
    let mut students = vec![];
    for n in 1..=3 {
        let student = seed_user(&pool, &prefix, &format!("PagedUser{n}"), "student").await;
        seed_enrollment(&pool, student, course, "student").await;
        students.push(student);
    }

    let query = "query ($id: ID!, $first: Int, $after: String) \
                 { course(id: $id) { usersConnection(first: $first, after: $after) \
                 { edges { cursor node { id } } pageInfo { hasNextPage endCursor } } } }";

    // Page 1: first=2 over the 4-user roster -> 2 edges + next.
    let page1 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2 }),
    )
    .await;
    let conn = &page1["data"]["course"]["usersConnection"];
    let edges = conn["edges"].as_array().expect("page1 edges");
    assert_eq!(edges.len(), 2, "page1: {page1}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], true, "page1: {page1}");
    let end_cursor = conn["pageInfo"]["endCursor"]
        .as_str()
        .expect("page1 endCursor")
        .to_string();

    // Page 2: after the first page's cursor -> remaining 2 users.
    let page2 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2, "after": end_cursor }),
    )
    .await;
    let conn = &page2["data"]["course"]["usersConnection"];
    let edges2 = conn["edges"].as_array().expect("page2 edges");
    assert_eq!(edges2.len(), 2, "page2: {page2}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], false, "page2: {page2}");

    // The two pages together cover the roster exactly once, in id order.
    let mut all: Vec<&str> = edges
        .iter()
        .chain(edges2.iter())
        .map(|e| e["node"]["id"].as_str().expect("user id"))
        .collect();
    assert_eq!(all.len(), 4, "no overlap between pages: {all:?}");
    all.sort_unstable();
    let mut expected: Vec<String> = std::iter::once(teacher)
        .chain(students.iter().copied())
        .map(|id| id.to_string())
        .collect();
    expected.sort_unstable();
    assert_eq!(
        all,
        expected.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "pages cover the roster exactly once"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// user(id): Canvas field shape parity for the #28 additions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_returns_canvas_field_names_for_issue_28_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let caller = seed_user(&pool, &prefix, "Caller", "student").await;
    let target = seed_user_sortable(&pool, &prefix, "Marie Curie", "Curie, Marie").await;
    seed_profile(&pool, target, "https://cdn.example.test/marie.png").await;

    let body = gql_post(
        &server,
        &auth(caller, "student"),
        "query ($id: ID!) { user(id: $id) { id name email sisUserId sortableName \
         avatarUrl createdAt } }",
        json!({ "id": target.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let node = &body["data"]["user"];
    assert_eq!(node["id"], target.to_string());
    assert_eq!(node["name"], "Marie Curie");
    assert_eq!(node["sortableName"], "Curie, Marie");
    assert_eq!(node["avatarUrl"], "https://cdn.example.test/marie.png");
    assert!(node["sisUserId"].is_null(), "no SIS columns yet: {body}");
    assert!(
        node["createdAt"].as_str().is_some_and(|v| !v.is_empty()),
        "createdAt should be an ISO timestamp: {body}"
    );

    // Users without a profile row resolve a null avatarUrl, and the
    // sortable_name coalesces onto name (never null).
    let bare = seed_user(&pool, &prefix, "Bare User", "student").await;
    let body = gql_post(
        &server,
        &auth(caller, "student"),
        "query ($id: ID!) { user(id: $id) { name sortableName avatarUrl } }",
        json!({ "id": bare.to_string() }),
    )
    .await;
    let node = &body["data"]["user"];
    assert_eq!(node["name"], "Bare User");
    assert_eq!(node["sortableName"], "Bare User");
    assert!(
        node["avatarUrl"].is_null(),
        "no profile row means null avatarUrl: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// User.enrollmentsConnection visibility: shared-course teacher reads it
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_enrollments_connection_visible_to_shared_course_teacher() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let unrelated = seed_user(&pool, &prefix, "Unrelated", "teacher").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "SharedCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let query = "query ($id: ID!) { user(id: $id) { enrollmentsConnection(first: 10) \
                 { edges { node { courseId role state } } } } }";
    let variables = json!({ "id": student.to_string() });

    // Teacher of the shared course reads the student's enrollments.
    let body = gql_post(&server, &auth(teacher, "teacher"), query, variables.clone()).await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let edges = body["data"]["user"]["enrollmentsConnection"]["edges"]
        .as_array()
        .expect("teacher edges");
    assert_eq!(edges.len(), 1, "body: {body}");
    assert_eq!(edges[0]["node"]["courseId"], course.to_string());

    // Self still sees it (regression).
    let body = gql_post(&server, &auth(student, "student"), query, variables.clone()).await;
    assert!(
        !body["data"]["user"]["enrollmentsConnection"].is_null(),
        "self sees own enrollments: {body}"
    );

    // Admin still sees it (regression).
    let body = gql_post(&server, &auth(admin, "admin"), query, variables.clone()).await;
    assert!(
        !body["data"]["user"]["enrollmentsConnection"].is_null(),
        "admin sees the user's enrollments: {body}"
    );

    // A teacher with no shared course gets null (Canvas hides, no error).
    let body = gql_post(
        &server,
        &auth(unrelated, "teacher"),
        query,
        variables.clone(),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unauthorized field access is null, not an error: {body}"
    );
    assert!(
        body["data"]["user"]["enrollmentsConnection"].is_null(),
        "unrelated teacher must not see the enrollments: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Regression: the pre-existing Course/User fields still resolve
// ---------------------------------------------------------------------------

#[tokio::test]
async fn existing_course_and_user_fields_still_resolve() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "RegressionCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Course: original field set plus both existing connections.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { id name state createdAt updatedAt \
         enrollmentsConnection(first: 10) { edges { cursor node { userId role state } } \
         pageInfo { hasNextPage endCursor } } \
         assignmentsConnection(first: 10) { edges { cursor node { id name } } \
         pageInfo { hasNextPage endCursor } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let node = &body["data"]["course"];
    assert_eq!(node["id"], course.to_string());
    assert_eq!(node["name"], "RegressionCourse");
    assert_eq!(node["state"], "available");
    let roster = node["enrollmentsConnection"]["edges"]
        .as_array()
        .expect("roster edges");
    assert_eq!(
        roster.len(),
        2,
        "teacher still sees the full roster: {body}"
    );
    assert!(
        node["assignmentsConnection"]["edges"]
            .as_array()
            .is_some_and(|e| e.is_empty()),
        "empty assignments connection still resolves: {body}"
    );

    // Student scoping on enrollmentsConnection is unchanged.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { course(id: $id) { enrollmentsConnection(first: 10) \
         { edges { node { userId } } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    let edges = body["data"]["course"]["enrollmentsConnection"]["edges"]
        .as_array()
        .expect("student edges");
    assert_eq!(edges.len(), 1, "student sees only own enrollment: {body}");
    assert_eq!(edges[0]["node"]["userId"], student.to_string());

    // User: original field set (sisUserId parity already asserted above).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { user(id: $id) { id name email createdAt } }",
        json!({ "id": student.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    assert_eq!(body["data"]["user"]["name"], "Student");
    assert_eq!(
        body["data"]["user"]["email"],
        format!("{prefix}-Student@test.local")
    );

    cleanup(&pool, &prefix).await;
}
