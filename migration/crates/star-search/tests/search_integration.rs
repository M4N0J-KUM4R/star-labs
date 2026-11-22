//! Integration tests for star-search (issue #91, spec
//! `specs/p5-rest-search-analytics.md` §7).
//!
//! Pinned behavior:
//! - recipients: AND-term matching, shared-course/group visibility
//!   (AddressBook semantics — no cross-course leaks), context scoping
//!   (course_N/group_N, invalid context dropped), exclude[] (plain user ids
//!   and course_/group_ asset strings), user_id exact single result,
//!   from_conversation_id widening, permissions[] per context,
//!   messageable_only, per_page, anonymous 401 (both paths).
//! - all_courses: PUBLIC — anonymous 200 with the catalog on (available
//!   courses of the root account, created_at order, search/public_only/
//!   open_enrollment_only filters), 401 JSON envelope with it off.
//! - rubrics: teacher-managed + public pool, double title filter, bare
//!   array, teacher-only management contexts, student sees only the public
//!   pool.
//! - smartsearch: 404 unknown course, 401 feature off / non-reader, missing
//!   q → 400 exact body, results over pages/assignments/announcements/
//!   discussion_topics with title/body/html_url/distance/relevance, filter[]
//!   narrowing, include[]=status, index_status transitions (indexing →
//!   indexed on first search touch), log 204 for arbitrary params.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix plus
//! full teardown keeps shared-DB runs safe. NEVER touches account 1's
//! settings beyond a guarded update it restores afterwards (catalog tests
//! flip the root account setting because the catalog is an account-wide
//! gate; the original value is saved and restored).

use axum_test::TestServer;
use serde_json::{json, Value};
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
        "srch_{}_{}",
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
        TestServer::new(star_search::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
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
        "INSERT INTO group_memberships (group_id, user_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(group_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed group member")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let emails = format!("{prefix}%@test.local");
    let names = format!("{prefix}-%");
    // smart_search_indexes / rubrics cascade with their courses; groups
    // cascade with courses too; conversations cascade from participants.
    let _ = sqlx::query("DELETE FROM courses WHERE name LIKE $1")
        .bind(&names)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}-account"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(&emails)
        .execute(pool)
        .await;
}

async fn get_ok(server: &TestServer, bearer: &str, path: &str) -> Value {
    let resp = server
        .get(path)
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}

async fn get_ok_anon(server: &TestServer, path: &str) -> Value {
    let resp = server.get(path).await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}

fn ids_of(items: &Value) -> Vec<String> {
    items
        .as_array()
        .expect("array")
        .iter()
        .map(|i| i["id"].to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// recipients
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recipients_and_term_context_and_excludes() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // Caller + two shared courses + a group; outsider with no overlap.
    let caller = seed_user(&pool, &prefix, "Caller Carol", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice Anderson", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob Barker", "student").await;
    let carol = seed_user(&pool, &prefix, "Carol Crenshaw", "student").await;
    let outsider = seed_user(&pool, &prefix, "Zed Zeta", "student").await;

    let course1 = seed_course(&pool, &prefix, "Biology").await;
    let course2 = seed_course(&pool, &prefix, "Chemistry").await;
    seed_enrollment(&pool, caller, course1, "teacher").await;
    seed_enrollment(&pool, caller, course2, "teacher").await;
    seed_enrollment(&pool, alice, course1, "student").await;
    seed_enrollment(&pool, bob, course1, "student").await;
    seed_enrollment(&pool, bob, course2, "student").await;
    seed_enrollment(&pool, carol, course2, "student").await;
    // outsider: no shared context.
    let group = seed_group(&pool, course1, "Study Group").await;
    seed_group_member(&pool, group, caller).await;
    seed_group_member(&pool, group, carol).await;

    let bearer = auth(caller, "teacher");

    // Single term: name substring across shared users.
    let body = get_ok(&server, &bearer, "/search/recipients?search=anderson").await;
    assert_eq!(ids_of(&body), vec![alice.to_string()], "{body}");

    // AND terms: both must match. "bob barker" matches Bob; "bob chemistry"
    // also matches (name term + shared course context in common_courses is
    // NOT part of term matching — Canvas matches terms on the name only).
    let body = get_ok(&server, &bearer, "/search/recipients?search=bob%20bar").await;
    assert_eq!(ids_of(&body), vec![bob.to_string()], "{body}");
    // A term that matches no name excludes the user even if the other does.
    let body = get_ok(&server, &bearer, "/search/recipients?search=bob%20zzz").await;
    assert_eq!(ids_of(&body), Vec::<String>::new(), "{body}");

    // No search: contexts only (courses + group), users skipped (Canvas
    // builds user collections only with search/context).
    let body = get_ok(&server, &bearer, "/search/recipients").await;
    let found = ids_of(&body);
    assert!(found.contains(&format!("\"course_{course1}\"")), "{body}");
    assert!(found.contains(&format!("\"course_{course2}\"")), "{body}");
    assert!(found.contains(&format!("\"group_{group}\"")), "{body}");
    assert!(
        !found.contains(&caller.to_string()),
        "no user entries without search/context: {body}"
    );

    // Contexts carry user_count and permissions ({} unless requested).
    let ctx = body
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(format!("course_{course1}")))
        .expect("course context");
    assert_eq!(ctx["type"], "context");
    assert_eq!(ctx["name"], format!("{prefix}-Biology"));
    assert_eq!(ctx["user_count"], 3, "caller + alice + bob: {ctx}");
    assert_eq!(ctx["permissions"], json!({}));

    // permissions[]=send_messages reflected per context.
    let body = get_ok(
        &server,
        &bearer,
        "/search/recipients?permissions[]=send_messages",
    )
    .await;
    let ctx = body
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == json!(format!("course_{course1}")))
        .expect("course context");
    assert_eq!(ctx["permissions"], json!({"send_messages": true}));

    // messageable_only: contexts without send_messages are dropped (all
    // ours grant it, so the set is unchanged).
    let body = get_ok(&server, &bearer, "/search/recipients?messageable_only=true").await;
    assert_eq!(ids_of(&body).len(), 3, "{body}");

    // Context scoping: course_2 → only that course (and its groups).
    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?context=course_{course2}"),
    )
    .await;
    let found = ids_of(&body);
    assert!(found.contains(&format!("\"course_{course2}\"")), "{body}");
    assert!(!found.contains(&format!("\"course_{course1}\"")), "{body}");
    assert!(!found.contains(&format!("\"group_{group}\"")), "{body}");
    // users in scope: bob + carol (course2), not alice.
    assert!(found.contains(&bob.to_string()), "{body}");
    assert!(found.contains(&carol.to_string()), "{body}");
    assert!(!found.contains(&alice.to_string()), "{body}");

    // Group context: the group's course scope — carol via group membership.
    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?context=group_{group}"),
    )
    .await;
    let found = ids_of(&body);
    assert!(
        found.contains(&carol.to_string()),
        "group member visible: {body}"
    );

    // An invalid context is dropped (Canvas nulls it), not a 404: results
    // are the unscoped set.
    let body = get_ok(
        &server,
        &bearer,
        "/search/recipients?context=course_999999999",
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 3, "{body}");

    // exclude[]: plain user id and course_ prefixed asset string.
    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?search=a&exclude[]={alice}"),
    )
    .await;
    assert!(!ids_of(&body).contains(&alice.to_string()), "{body}");

    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?exclude[]=course_{course1}"),
    )
    .await;
    let found = ids_of(&body);
    assert!(!found.contains(&format!("\"course_{course1}\"")), "{body}");
    assert!(found.contains(&format!("\"course_{course2}\"")), "{body}");
    // scoping into the excluded course now yields only users.
    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?context=course_{course1}&exclude[]=course_{course1}"),
    )
    .await;
    let found = ids_of(&body);
    assert!(!found.contains(&format!("\"course_{course1}\"")), "{body}");
    assert!(
        found.contains(&bob.to_string()),
        "users still listed: {body}"
    );

    // exclude group_ asset string.
    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?exclude[]=group_{group}"),
    )
    .await;
    assert!(
        !ids_of(&body).contains(&format!("\"group_{group}\"")),
        "{body}"
    );

    // common_courses/common_groups hashes on user entries.
    let body = get_ok(&server, &bearer, "/search/recipients?search=barker").await;
    let user = &body.as_array().unwrap()[0];
    assert_eq!(user["id"], bob);
    assert_eq!(user["name"], format!("{prefix}-Bob Barker"));
    assert_eq!(user["full_name"], format!("{prefix}-Bob Barker"));
    assert!(user["avatar_url"].is_null());
    let cc = user["common_courses"].as_object().unwrap();
    assert!(
        cc.contains_key(&course1.to_string()) && cc.contains_key(&course2.to_string()),
        "{user}"
    );
    assert_eq!(cc[&course1.to_string()], json!(["student"]));

    // Group-shared carol carries common_groups.
    let body = get_ok(&server, &bearer, "/search/recipients?search=crenshaw").await;
    let user = &body.as_array().unwrap()[0];
    assert_eq!(
        user["common_groups"],
        json!({ group.to_string(): ["Member"] }),
        "{user}"
    );

    // No cross-course leaks: the outsider is invisible.
    let body = get_ok(&server, &bearer, "/search/recipients?search=zeta").await;
    assert_eq!(ids_of(&body), Vec::<String>::new(), "{body}");

    // per_page bounds the user collection.
    let body = get_ok(&server, &bearer, "/search/recipients?search=a&per_page=1").await;
    assert_eq!(body.as_array().unwrap().len(), 1, "{body}");
    // per_page=0 → unset (Canvas nil) → default 50. "a" matches the
    // caller (Canvas "self" collection), alice, bob and carol.
    let body = get_ok(&server, &bearer, "/search/recipients?search=a&per_page=0").await;
    assert_eq!(body.as_array().unwrap().len(), 4, "a-term users: {body}");

    // Anonymous: 401 on both paths.
    for path in ["/search/recipients", "/conversations/find_recipients"] {
        let resp = server.get(path).await;
        assert_eq!(resp.status_code(), 401, "GET {path} anonymous");
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn recipients_user_id_exact_and_from_conversation() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let caller = seed_user(&pool, &prefix, "Caller", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, caller, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    // stranger shares nothing.

    let bearer = auth(caller, "teacher");

    // user_id of a shared user: exactly one result, ignoring search.
    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?user_id={alice}&search=zzz"),
    )
    .await;
    let items = body.as_array().unwrap();
    assert_eq!(items.len(), 1, "{body}");
    assert_eq!(items[0]["id"], alice);
    assert_eq!(
        items[0]["common_courses"],
        json!({course.to_string(): ["student"]})
    );

    // user_id of a stranger: no result, NOT an error.
    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?user_id={stranger}"),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 0, "{body}");

    // from_conversation_id: a conversation both are in makes the stranger
    // known (Canvas side-conversation semantics).
    let (conv,): (i64,) =
        sqlx::query_as("INSERT INTO conversations (subject) VALUES ($1) RETURNING id")
            .bind(format!("{prefix}-subject"))
            .fetch_one(&pool)
            .await
            .expect("seed conversation");
    for uid in [caller, stranger] {
        sqlx::query(
            "INSERT INTO conversation_participants (conversation_id, user_id) VALUES ($1, $2)",
        )
        .bind(conv)
        .bind(uid)
        .execute(&pool)
        .await
        .expect("seed participant");
    }
    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?user_id={stranger}&from_conversation_id={conv}"),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 1, "{body}");

    // A conversation the caller is NOT in does not widen anything.
    let (conv2,): (i64,) =
        sqlx::query_as("INSERT INTO conversations (subject) VALUES ($1) RETURNING id")
            .bind(format!("{prefix}-other"))
            .fetch_one(&pool)
            .await
            .expect("seed conversation 2");
    sqlx::query("INSERT INTO conversation_participants (conversation_id, user_id) VALUES ($1, $2)")
        .bind(conv2)
        .bind(stranger)
        .execute(&pool)
        .await
        .expect("seed participant 2");
    let body = get_ok(
        &server,
        &bearer,
        &format!("/search/recipients?user_id={stranger}&from_conversation_id={conv2}"),
    )
    .await;
    assert_eq!(body.as_array().unwrap().len(), 0, "{body}");

    // Invalid user_id → 400.
    let resp = server
        .get("/search/recipients?user_id=abc")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// all_courses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn all_courses_public_catalog_gating_and_filters() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // A DEDICATED root: flip account 1's setting is forbidden; instead this
    // test uses the real root account's settings with a guarded save/
    // restore (the catalog gate is defined on the root account; Canvas
    // semantics — there is exactly one root).
    let (orig_settings,): (Value,) = sqlx::query_as("SELECT settings FROM accounts WHERE id = 1")
        .fetch_one(&pool)
        .await
        .expect("load root settings");
    let restore = || async {
        sqlx::query("UPDATE accounts SET settings = $1 WHERE id = 1")
            .bind(&orig_settings)
            .execute(&pool)
            .await
            .expect("restore root settings");
    };

    // Catalog OFF: anonymous caller gets the pinned 401 envelope.
    sqlx::query("UPDATE accounts SET settings = '{}'::jsonb WHERE id = 1")
        .execute(&pool)
        .await
        .expect("clear catalog setting");
    let resp = server.get("/search/all_courses").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "unauthenticated");
    assert_eq!(body["errors"][0]["message"], "user authorization required");

    // Catalog ON: seed available courses of account 1 (name LIKE prefix),
    // a non-available course (excluded), an unindexed course (excluded via
    // settings map), a public one and an open-enrollment one.
    let c_ok = seed_course(&pool, &prefix, "AOk").await;
    let c_public = seed_course(&pool, &prefix, "BPublic").await;
    let c_open = seed_course(&pool, &prefix, "COpen").await;
    let c_unpublished: i64 = sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, workflow_state) VALUES ($1, $2, 'unpublished') RETURNING id",
    )
    .bind(format!("{prefix}-Unpub"))
    .bind(format!("{prefix}-Unpub"))
    .fetch_one(&pool)
    .await
    .expect("seed unpublished");
    let c_unindexed = seed_course(&pool, &prefix, "DUnindexed").await;
    // Re-point the seeded courses at the root account (default account_id=1
    // already; ensure explicitly) and set catalog flags via settings map.
    for cid in [c_ok, c_public, c_open, c_unpublished, c_unindexed] {
        sqlx::query("UPDATE courses SET account_id = 1 WHERE id = $1")
            .bind(cid)
            .execute(&pool)
            .await
            .expect("set account");
    }
    let catalog_cfg = json!({
        c_public.to_string(): { "is_public": true },
        c_open.to_string(): { "open_enrollment": true },
        c_unindexed.to_string(): { "indexed": false },
    });
    sqlx::query("UPDATE accounts SET settings = $1 WHERE id = 1")
        .bind(json!({ "enable_course_catalog": true, "course_catalog": catalog_cfg }))
        .execute(&pool)
        .await
        .expect("enable catalog");

    // Anonymous 200: available + indexed, created_at order.
    let resp = server.get("/search/all_courses").await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("bare array");
    let ids: Vec<i64> = items.iter().map(|c| c["id"].as_i64().unwrap()).collect();
    assert_eq!(
        ids,
        vec![c_ok, c_public, c_open],
        "available+indexed only: {body}"
    );
    let first = &items[0];
    assert_eq!(first["name"], format!("{prefix}-AOk"));
    assert_eq!(first["course_code"], format!("{prefix}-AOk"));
    assert_eq!(first["account_id"], 1);
    assert_eq!(first["is_public"], false);
    assert!(first["created_at"].is_string());

    // search filter: name substring.
    let body = get_ok_anon(
        &server,
        &format!("/search/all_courses?search={prefix}%20AOk"),
    )
    .await;
    let ids: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![c_ok], "{body}");

    // public_only.
    let body = get_ok_anon(&server, "/search/all_courses?public_only=true").await;
    let ids: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![c_public], "{body}");

    // open_enrollment_only.
    let body = get_ok_anon(&server, "/search/all_courses?open_enrollment_only=true").await;
    let ids: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![c_open], "{body}");

    restore().await;
    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// rubrics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rubrics_matching_and_bare_array() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // NOTE: the publicly-reusable pool (Canvas Rubric.publicly_reusable) is
    // represented by account-level rubrics (course_id IS NULL), but
    // rubrics.course_id is NOT NULL in this workspace — the pool is
    // structurally empty (documented deviation); the managed-context arm is
    // what these tests exercise.
    let seed_rubric = |title: &str| {
        let prefix = prefix.clone();
        let pool = pool.clone();
        async move {
            sqlx::query_scalar(
                "INSERT INTO rubrics (course_id, title) VALUES ($1, $2) RETURNING id",
            )
            .bind(course)
            .bind(format!("{prefix}-{title}"))
            .fetch_one(&pool)
            .await
            .expect("seed rubric")
        }
    };
    let r_match = seed_rubric("Essay Rubric").await;
    let r_other = seed_rubric("Lab Rubric").await;

    let bearer = auth(teacher, "teacher");

    // Blank q: managed contexts, bare array.
    let body = get_ok(&server, &bearer, "/search/rubrics").await;
    let items = body.as_array().expect("bare array");
    let ids: Vec<i64> = items.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(ids.len(), 2, "{body}");
    assert!(ids.contains(&r_match) && ids.contains(&r_other), "{body}");
    let item = items.iter().find(|r| r["id"] == json!(r_match)).unwrap();
    assert_eq!(item["title"], format!("{prefix}-Essay Rubric"));
    assert_eq!(item["course_id"], course);
    assert!(item["points_possible"].is_number());
    assert_eq!(item["workflow_state"], "active");
    assert!(item["created_at"].is_string());

    // q filters by title (the verified double filter collapses to downcase
    // containment); non-matching excluded.
    let body = get_ok(&server, &bearer, "/search/rubrics?q=essay").await;
    let ids: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![r_match], "{body}");

    // Case-insensitive match.
    let body = get_ok(&server, &bearer, "/search/rubrics?q=ESSAY").await;
    let ids: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![r_match], "{body}");

    // A student (no management contexts) sees nothing (Canvas
    // management_contexts rescue → []).
    let body = get_ok(
        &server,
        &auth(student, "student"),
        "/search/rubrics?q=essay",
    )
    .await;
    assert_eq!(body, json!([]), "{body}");

    // Anonymous: 401.
    let resp = server.get("/search/rubrics?q=essay").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// smartsearch
// ---------------------------------------------------------------------------

async fn enable_smart_search(pool: &PgPool, course_id: i64) {
    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state) \
         VALUES ('Course', $1, 'smart_search', 'IgniteAI Search for Courses', 'on') \
         ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'on'",
    )
    .bind(course_id)
    .execute(pool)
    .await
    .expect("enable smart_search");
}

#[tokio::test]
async fn smartsearch_guards_and_results() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let (page_id,): (i64,) = sqlx::query_as(
        "INSERT INTO pages (course_id, title, body) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course)
    .bind("Nicolaus Copernicus")
    .bind("Renaissance-era mathematician and astronomer")
    .fetch_one(&pool)
    .await
    .expect("seed page");
    sqlx::query("INSERT INTO assignments (course_id, title, description) VALUES ($1, $2, $3)")
        .bind(course)
        .bind("Kepler Essay")
        .bind("Write about planetary motion")
        .execute(&pool)
        .await
        .expect("seed assignment");
    sqlx::query("INSERT INTO announcements (course_id, title, message) VALUES ($1, $2, $3)")
        .bind(course)
        .bind("Welcome Announcement")
        .bind("Welcome to astronomy class")
        .execute(&pool)
        .await
        .expect("seed announcement");
    sqlx::query("INSERT INTO discussion_topics (course_id, title, message) VALUES ($1, $2, $3)")
        .bind(course)
        .bind("Galileo Discussion")
        .bind("Telescope observations")
        .execute(&pool)
        .await
        .expect("seed discussion");

    let bearer = auth(teacher, "teacher");
    let path = format!("/courses/{course}/smartsearch");

    // Unknown course → 404 (before ability checks).
    let resp = server
        .get("/courses/999999999/smartsearch?q=x")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Feature off → 401 anonymous, 403 authenticated (Canvas
    // render_unauthorized_action: :unauthenticated vs :forbidden).
    for b in [&bearer, &auth(student, "student")] {
        let resp = server
            .get(&format!("{path}?q=copernicus"))
            .add_header(axum::http::header::AUTHORIZATION, b.clone())
            .await;
        assert_eq!(
            resp.status_code(),
            403,
            "feature off, authenticated: {}",
            resp.text()
        );
    }
    let resp = server.get(&format!("{path}?q=x")).await;
    assert_eq!(resp.status_code(), 401, "anonymous: {}", resp.text());

    enable_smart_search(&pool, course).await;

    // Non-reader (stranger) → 403 even with the feature on.
    let resp = server
        .get(&format!("{path}?q=copernicus"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "non-reader: {}", resp.text());

    // Missing q → 400 exact body.
    let resp = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({"error": "missing 'q' param"}));

    // Blank q: 200 with empty results (Canvas skips the search when blank).
    let resp = server
        .get(&format!("{path}?q="))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["results"], json!([]));

    // First search touch: index incomplete → status + indexing_progress, and
    // the row flips to indexed/100.
    let body = get_ok(&server, &bearer, &format!("{path}?q=astronomer")).await;
    assert_eq!(body["status"], "index_incomplete", "{body}");
    assert_eq!(body["indexing_progress"], 0, "{body}");
    let (status, progress): (String, f64) =
        sqlx::query_as("SELECT status, progress FROM smart_search_indexes WHERE course_id = $1")
            .bind(course)
            .fetch_one(&pool)
            .await
            .expect("index row");
    assert_eq!(status, "indexed");
    assert_eq!(progress, 100.0);

    // Second search: no incomplete marker.
    let body = get_ok(&server, &bearer, &format!("{path}?q=copernicus")).await;
    assert!(body.get("status").is_none(), "{body}");
    let results = body["results"].as_array().expect("results");
    assert_eq!(results.len(), 1, "{body}");
    let r = &results[0];
    assert_eq!(r["content_id"], page_id);
    assert_eq!(r["content_type"], "WikiPage");
    assert_eq!(r["readable_type"], "Page");
    assert_eq!(r["title"], "Nicolaus Copernicus");
    assert_eq!(r["body"], "Renaissance-era mathematician and astronomer");
    assert_eq!(
        r["html_url"],
        format!("/courses/{course}/pages/nicolaus-copernicus")
    );
    assert_eq!(r["distance"], 0.0, "title hit is closest: {r}");
    assert_eq!(r["relevance"], 100);

    // Body-only hit: distance 0.5, relevance 50.
    let body = get_ok(&server, &bearer, &format!("{path}?q=planetary")).await;
    let results = body["results"].as_array().expect("results");
    assert_eq!(results.len(), 1, "{body}");
    assert_eq!(results[0]["content_type"], "Assignment");
    assert_eq!(results[0]["distance"], 0.5);
    assert_eq!(results[0]["relevance"], 50);

    // All types surface.
    let body = get_ok(&server, &bearer, &format!("{path}?q={prefix}")).await;
    let types: Vec<&str> = body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["content_type"].as_str().unwrap())
        .collect();
    for t in ["WikiPage", "Assignment", "Announcement", "DiscussionTopic"] {
        assert!(types.contains(&t), "{types:?}");
    }

    // filter[] narrows object types.
    let body = get_ok(
        &server,
        &bearer,
        &format!("{path}?q={prefix}&filter[]=announcements"),
    )
    .await;
    let types: Vec<&str> = body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["content_type"].as_str().unwrap())
        .collect();
    assert_eq!(types, vec!["Announcement"], "{body}");
    let body = get_ok(
        &server,
        &bearer,
        &format!("{path}?q={prefix}&filter[]=pages&filter[]=assignments"),
    )
    .await;
    let types: Vec<&str> = body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["content_type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        vec!["WikiPage", "Assignment"],
        "distance tie -> id order: {body}"
    );

    // include[]=status adds published/due_date.
    let body = get_ok(
        &server,
        &bearer,
        &format!("{path}?q=copernicus&include[]=status"),
    )
    .await;
    let r = &body["results"][0];
    assert_eq!(r["published"], true, "{r}");

    // Student (reader) can search too.
    let body = get_ok(
        &server,
        &auth(student, "student"),
        &format!("{path}?q=copernicus"),
    )
    .await;
    assert_eq!(body["results"].as_array().unwrap().len(), 1, "{body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn smartsearch_index_status_and_log() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let bearer = auth(teacher, "teacher");
    let base = format!("/courses/{course}/smartsearch");

    // Unknown course → 404.
    let resp = server
        .get("/courses/999999999/smartsearch/index_status")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Feature off → 401 anonymous, 403 authenticated (same split as
    // #search; Canvas checks availability only on this action).
    let resp = server
        .get(&format!("{base}/index_status"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "authenticated: {}", resp.text());
    let resp = server.get(&format!("{base}/index_status")).await;
    assert_eq!(resp.status_code(), 401, "anonymous: {}", resp.text());

    enable_smart_search(&pool, course).await;

    // No index row yet: "indexing", 0 (Canvas check_course first touch).
    let body = get_ok(&server, &bearer, &format!("{base}/index_status")).await;
    assert_eq!(body, json!({"status": "indexing", "progress": 0}), "{body}");

    // A flipped row reports through (tests flip state directly in SQL, the
    // shared job-model convention).
    sqlx::query(
        "INSERT INTO smart_search_indexes (course_id, status, progress) \
         VALUES ($1, 'indexing', 42.5) \
         ON CONFLICT (course_id) DO UPDATE SET status = 'indexing', progress = 42.5",
    )
    .bind(course)
    .execute(&pool)
    .await
    .expect("flip index row");
    let body = get_ok(&server, &bearer, &format!("{base}/index_status")).await;
    assert_eq!(
        body,
        json!({"status": "indexing", "progress": 42.5}),
        "{body}"
    );

    sqlx::query(
        "UPDATE smart_search_indexes SET status = 'indexed', progress = 100 WHERE course_id = $1",
    )
    .bind(course)
    .execute(&pool)
    .await
    .expect("flip to indexed");
    let body = get_ok(&server, &bearer, &format!("{base}/index_status")).await;
    assert_eq!(
        body,
        json!({"status": "indexed", "progress": 100}),
        "{body}"
    );

    // log: 204 with arbitrary telemetry params, on the real course; unknown
    // course still 204 (Canvas require_context would 404 — but log is a
    // no-op whose guard chain spec §4 leaves to require_context; star-lms
    // pins the discard-first shape on the real course and 404 elsewhere).
    let resp = server
        .get(&format!(
            "{base}/log?a=1&c=2&course_id={course}&oid=3&ot=WikiPage&q=foo"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    assert_eq!(resp.text(), "");

    // log on an unknown course: 404 (require_context parity).
    let resp = server
        .get("/courses/999999999/smartsearch/log?a=1")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // log anonymous → 401 (require_user before the no-op body).
    let resp = server.get(&format!("{base}/log?a=1")).await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
