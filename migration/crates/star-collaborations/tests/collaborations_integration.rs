//! Integration tests for star-collaborations (issue #91, spec
//! `specs/p5-rest-collaborations.md`).
//!
//! Pinned behavior:
//! - api_index: 401 anonymous; 404 unknown course/group; 403 non-member
//!   (student not in the course); tab-disabled 404 body for a course without
//!   a seeded collaborations tab (star-courses DEFAULT_COURSE_TABS has none);
//!   bare `collaborations` array envelope with meta.pagination; non-managers
//!   only see rows they authored / are user-collaborator on / are in a
//!   collaborating group; group twin always tab-enabled.
//! - members: 404 unknown collaboration; 403 non-collaborator; paginated
//!   flat `collaborators` array; user collaborators (type "user",
//!   sortable_name, user id) and group collaborators (type "group", group
//!   name, group id).
//! - potential_collaborators: course → distinct enrolled users; group →
//!   members + parent-course active teacher/ta/designer; 403 outsider;
//!   `[User]` shape.
//! - destroy: 200 + row JSON (Canvas renders the record, not 204); soft
//!   delete (workflow_state 'deleted', deleted_at stamped); destroyed rows
//!   invisible in api_index; cross-context id → 404; non-author
//!   non-manager → 403; `delete_doc` accepted (stubbed boundary).
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix plus
//! full teardown keeps shared-DB runs safe.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "collab_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
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

    let server =
        TestServer::new(star_collaborations::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role, sortable_name) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .bind(format!("{name}, {prefix}"))
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

async fn seed_group_member(pool: &PgPool, group_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state) \
         VALUES ($1, $2, 'accepted') RETURNING id",
    )
    .bind(group_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed group membership")
}

/// Seed a collaboration (snapshot 0223 column layout); returns its id.
async fn seed_collaboration(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    user_id: i64,
    title: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO collaborations \
         (collaboration_type, user_id, context_id, context_type, title, description, url, \
          type, workflow_state, created_at, updated_at) \
         VALUES ('ExternalToolCollaboration', $1, $2, $3, $4, $5, $6, \
                 'ExternalToolCollaboration', 'active', now(), now()) RETURNING id",
    )
    .bind(user_id)
    .bind(context_id)
    .bind(context_type)
    .bind(title)
    .bind("integration collaboration")
    .bind(format!("https://collab.example.test/{title}"))
    .fetch_one(pool)
    .await
    .expect("seed collaboration")
}

/// Seed a user-collaborator row (Canvas Collaborator: user_id set).
async fn seed_user_collaborator(pool: &PgPool, collaboration_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO collaborators (collaboration_id, user_id, created_at, updated_at) \
         VALUES ($1, $2, now(), now()) RETURNING id",
    )
    .bind(collaboration_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed user collaborator")
}

/// Seed a group-collaborator row (Canvas Collaborator: group_id set).
async fn seed_group_collaborator(pool: &PgPool, collaboration_id: i64, group_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO collaborators (collaboration_id, group_id, created_at, updated_at) \
         VALUES ($1, $2, now(), now()) RETURNING id",
    )
    .bind(collaboration_id)
    .bind(group_id)
    .fetch_one(pool)
    .await
    .expect("seed group collaborator")
}

/// Enable the collaborations tab on a course (Canvas TAB_COLLABORATIONS).
async fn seed_course_collaborations_tab(pool: &PgPool, course_id: i64) {
    sqlx::query(
        "INSERT INTO course_tabs (course_id, tab_id, position, hidden) \
         VALUES ($1, 'collaborations', 12, false) \
         ON CONFLICT (course_id, tab_id) DO NOTHING",
    )
    .bind(course_id)
    .execute(pool)
    .await
    .expect("seed collaborations tab");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // collaborators/collaborations first (context ids in this run's
    // courses/groups), then group_memberships → groups → courses → users.
    let _ = sqlx::query(
        "DELETE FROM collaborators WHERE collaboration_id IN ( \
           SELECT id FROM collaborations WHERE title LIKE $1 || '-%')",
    )
    .bind(prefix)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM collaborations WHERE title LIKE $1 || '-%'")
        .bind(prefix)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM group_memberships WHERE group_id IN ( \
           SELECT id FROM groups WHERE name LIKE $1 || '-%')",
    )
    .bind(prefix)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM groups WHERE name LIKE $1 || '-%'")
        .bind(prefix)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM course_tabs WHERE course_id IN ( \
           SELECT id FROM courses WHERE course_code LIKE $1 || '-%')",
    )
    .bind(prefix)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1 || '-%'")
        .bind(prefix)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1 || '%@test.local'")
        .bind(prefix)
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_index_course_lifecycle_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_course_collaborations_tab(&pool, course).await;

    let own = seed_collaboration(&pool, "Course", course, teacher, &format!("{prefix}-own")).await;
    seed_collaboration(&pool, "Course", course, teacher, &format!("{prefix}-other")).await;
    // deleted rows are invisible (Canvas scope :active).
    let deleted =
        seed_collaboration(&pool, "Course", course, teacher, &format!("{prefix}-gone")).await;
    sqlx::query(
        "UPDATE collaborations SET workflow_state = 'deleted', deleted_at = now() WHERE id = $1",
    )
    .bind(deleted)
    .execute(&pool)
    .await
    .expect("soft-delete seeded row");

    // anonymous → 401
    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .await;
    assert_eq!(resp.status_code(), 401);

    // unknown course → 404
    let resp = server
        .get("/courses/999999999/collaborations")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // non-member → 403
    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // teacher (manager) sees both active rows (items + meta.pagination).
    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2, "deleted row must be invisible: {body}");
    assert!(body["meta"]["pagination"]["total"].as_i64().unwrap() >= 2);
    let first = &items[0];
    assert_eq!(first["id"].as_i64().unwrap(), own);
    assert_eq!(
        first["collaboration_type"].as_str().unwrap(),
        "ExternalToolCollaboration"
    );
    assert_eq!(first["context_id"].as_i64().unwrap(), course);
    assert_eq!(first["context_type"].as_str().unwrap(), "Course");
    assert!(first["title"]
        .as_str()
        .unwrap()
        .starts_with(&format!("{prefix}-")));
    assert!(first["user_name"].as_str().unwrap().contains("Teacher"));
    assert!(first["permissions"]["delete"].as_bool().unwrap());

    // student (non-manager) sees only rows they collaborate on.
    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(
        body["items"].as_array().unwrap().is_empty(),
        "student sees none yet: {body}"
    );

    // add the student as a user-collaborator → visible to them.
    seed_user_collaborator(&pool, own, student).await;
    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn api_index_course_tab_disabled_is_404_body() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    // NO collaborations tab seeded — star-courses DEFAULT_COURSE_TABS has
    // none, so the tab is disabled and Canvas renders the tab-disabled 404.
    let course = seed_course(&pool, &prefix, "NoTab").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_collaboration(
        &pool,
        "Course",
        course,
        teacher,
        &format!("{prefix}-tabcheck"),
    )
    .await;

    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["message"].as_str().unwrap(),
        "That page has been disabled for this course"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn api_index_group_twin_always_enabled() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let group = seed_group(&pool, course, &format!("{prefix}-Group")).await;
    seed_group_member(&pool, group, teacher).await;
    seed_collaboration(&pool, "Group", group, teacher, &format!("{prefix}-grouped")).await;

    // group twin: no tab seeding needed (star-groups always renders the
    // collaborations tabs).
    let resp = server
        .get(&format!("/groups/{group}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["context_type"].as_str().unwrap(), "Group");
    assert_eq!(items[0]["context_id"].as_i64().unwrap(), group);

    // unknown group → 404
    let resp = server
        .get("/groups/999999999/collaborations")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn members_pagination_and_mixed_collaborators() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let group = seed_group(&pool, course, &format!("{prefix}-Group")).await;
    let collab =
        seed_collaboration(&pool, "Course", course, owner, &format!("{prefix}-members")).await;

    // 12 user collaborators (page 2 at per_page=10) + 1 group collaborator.
    let mut user_ids = Vec::new();
    for i in 0..12 {
        let uid = seed_user(&pool, &prefix, &format!("Member{i}"), "student").await;
        seed_user_collaborator(&pool, collab, uid).await;
        user_ids.push(uid);
    }
    seed_group_collaborator(&pool, collab, group).await;

    // author reads page 1
    let resp = server
        .get(&format!("/collaborations/{collab}/members"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let page1 = body["items"].as_array().expect("items array");
    assert_eq!(page1.len(), 10, "Canvas PER_PAGE default 10: {body}");
    assert_eq!(body["meta"]["pagination"]["total"].as_i64().unwrap(), 13);
    let first = &page1[0];
    assert_eq!(first["type"].as_str().unwrap(), "user");
    assert!(first["name"].as_str().unwrap().contains("Member"));
    assert!(first["collaborator_id"].as_i64().unwrap() > 0);

    // page 2: 2 users + 1 group
    let resp = server
        .get(&format!("/collaborations/{collab}/members?page=2"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    let page2 = body["items"].as_array().unwrap();
    assert_eq!(page2.len(), 3);
    let group_item = page2.iter().find(|i| i["type"] == "user").is_none();
    assert!(
        group_item,
        "page 2 must contain the group collaborator: {body}"
    );
    let group_entry = page2
        .iter()
        .find(|i| i["type"] == "group")
        .expect("group entry");
    assert!(group_entry["name"].as_str().unwrap().contains("Group"));
    assert_eq!(group_entry["collaborator_id"].as_i64().unwrap(), group);

    // unknown collaboration → 404
    let resp = server
        .get("/collaborations/999999999/members")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // anonymous → 401
    let resp = server
        .get(&format!("/collaborations/{collab}/members"))
        .await;
    assert_eq!(resp.status_code(), 401);

    // a collaborating-group member can read (Canvas :read policy)
    let member_user = seed_user(&pool, &prefix, "Groupie", "student").await;
    seed_group_member(&pool, group, member_user).await;
    let resp = server
        .get(&format!("/collaborations/{collab}/members"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(member_user, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "group member read: {}",
        resp.text()
    );

    // an unrelated user → 403
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let resp = server
        .get(&format!("/collaborations/{collab}/members"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn potential_collaborators_course_and_group() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let ta = seed_user(&pool, &prefix, "Ta", "ta").await;
    let s1 = seed_user(&pool, &prefix, "StudentA", "student").await;
    let s2 = seed_user(&pool, &prefix, "StudentB", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, ta, course, "ta").await;
    seed_enrollment(&pool, s1, course, "student").await;
    seed_enrollment(&pool, s2, course, "student").await;

    // course: distinct enrolled users, ordered by sortable_name.
    let resp = server
        .get(&format!("/courses/{course}/potential_collaborators"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let users = body["items"].as_array().expect("items array");
    assert_eq!(users.len(), 4, "teacher+ta+2 students: {body}");
    for u in users {
        assert!(u["sortable_name"].is_string());
        assert!(u["email"].as_str().unwrap().contains("@test.local"));
    }

    // outsider → 403
    let resp = server
        .get(&format!("/courses/{course}/potential_collaborators"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // unknown course → 404; anonymous → 401
    let resp = server
        .get("/courses/999999999/potential_collaborators")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .get(&format!("/courses/{course}/potential_collaborators"))
        .await;
    assert_eq!(resp.status_code(), 401);

    // group: members + parent-course active admins (teacher/ta, not students).
    let group = seed_group(&pool, course, &format!("{prefix}-Group")).await;
    seed_group_member(&pool, group, s1).await;
    let resp = server
        .get(&format!("/groups/{group}/potential_collaborators"))
        .add_header(axum::http::header::AUTHORIZATION, auth(s1, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let users = body["items"].as_array().expect("items array");
    let mut ids: Vec<i64> = users.iter().map(|u| u["id"].as_i64().unwrap()).collect();
    ids.sort();
    let mut expected = vec![teacher, ta, s1];
    expected.sort();
    assert_eq!(ids, expected, "member + parent admins: {body}");

    // unknown group → 404
    let resp = server
        .get("/groups/999999999/potential_collaborators")
        .add_header(axum::http::header::AUTHORIZATION, auth(s1, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn destroy_soft_delete_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let author = seed_user(&pool, &prefix, "Author", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_course_collaborations_tab(&pool, course).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, author, course, "student").await;
    seed_enrollment(&pool, stranger, course, "student").await;

    let own =
        seed_collaboration(&pool, "Course", course, author, &format!("{prefix}-doomed")).await;
    let other_course = seed_course(&pool, &prefix, "Other").await;
    let foreign = seed_collaboration(
        &pool,
        "Course",
        other_course,
        author,
        &format!("{prefix}-foreign"),
    )
    .await;

    // anonymous → 401
    let resp = server
        .delete(&format!("/courses/{course}/collaborations/{own}"))
        .await;
    assert_eq!(resp.status_code(), 401);

    // stranger (non-author, non-manager) → 403
    let resp = server
        .delete(&format!("/courses/{course}/collaborations/{own}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // cross-context id → 404 (scoped @context.collaborations.find)
    let resp = server
        .delete(&format!("/courses/{course}/collaborations/{foreign}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // author destroys own row with delete_doc → 200 + row JSON (not 204)
    let resp = server
        .delete(&format!(
            "/courses/{course}/collaborations/{own}?delete_doc=true"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(author, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64().unwrap(), own);
    assert_eq!(body["title"].as_str().unwrap(), format!("{prefix}-doomed"));

    // soft-deleted: row remains but workflow_state flipped.
    let (state, deleted_at): (String, Option<chrono::NaiveDateTime>) =
        sqlx::query_as("SELECT workflow_state, deleted_at FROM collaborations WHERE id = $1")
            .bind(own)
            .fetch_one(&pool)
            .await
            .expect("row still exists");
    assert_eq!(state, "deleted");
    assert!(deleted_at.is_some());

    // invisible in api_index afterwards.
    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let body: Value = resp.json();
    assert!(
        !body["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"].as_i64() == Some(own)),
        "soft-deleted collaboration must vanish from the index: {body}"
    );

    // teacher can destroy another author's row (content manager).
    let teacher_doomed = seed_collaboration(
        &pool,
        "Course",
        course,
        author,
        &format!("{prefix}-teacherdestroys"),
    )
    .await;
    let resp = server
        .delete(&format!(
            "/courses/{course}/collaborations/{teacher_doomed}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // group twin destroy.
    let group = seed_group(&pool, course, &format!("{prefix}-Group")).await;
    let group_collab = seed_collaboration(
        &pool,
        "Group",
        group,
        author,
        &format!("{prefix}-groupdoom"),
    )
    .await;
    let resp = server
        .delete(&format!("/groups/{group}/collaborations/{group_collab}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(author, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // unknown course → 404
    let resp = server
        .delete(&format!("/courses/999999999/collaborations/{foreign}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}
