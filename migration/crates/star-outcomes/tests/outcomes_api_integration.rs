//! Integration tests for the Canvas outcome imports + outcome results routes.
//!
//! Pinned behavior (from `canvas-src/app/controllers/outcome_imports_api_controller.rb`
//! and `canvas-src/app/controllers/outcome_results_controller.rb`):
//! - `POST /accounts/{id}/outcome_imports` stores an import, answers `200` with
//!   the Canvas `OutcomeImport` JSON and queues the import job;
//! - `GET /accounts/{id}/outcome_imports/{id}` answers `200` with the same JSON
//!   and `404` with `{"message": "Couldn't find OutcomeImport with 'id'=..."}` when
//!   the import does not exist in that context;
//! - `GET .../created_group_ids` answers `404 {"message": "Import is still being
//!   processed"}` until the import succeeded;
//! - `GET /courses/{id}/outcome_results` returns `{"outcome_results": [...]}` and
//!   refuses a student that did not name any `user_ids` with a `403`;
//! - `GET /courses/{id}/outcome_rollups` returns the rollups plus the JSON-API
//!   pagination `meta`, and rejects an unknown `aggregate` / `include` with a `400`;
//! - `GET /courses/{id}/outcome_mastery_distribution` buckets the students into
//!   the outcome's rubric ratings;
//! - `GET /courses/{id}/outcomes/{outcome_id}/contributing_scores` lists the
//!   alignments and the matching scores;
//! - `POST /courses/{id}/assign_outcome_order` stores the ordering and answers
//!   `204 No Content` (the Ruby action renders nothing);
//! - `POST /enqueue_outcome_rollup_calculation` answers `204` while the
//!   `outcomes_rollup_propagation` feature flag is off.
//!
//! Fleet rule: dedicated users, accounts and courses per test, every seeded row
//! is cleaned up. Skipped cleanly when DATABASE_URL is unset.
//!
//! The extra columns/tables the routes need are created by this test (the same
//! DDL the migration needs) so the suite runs before the migration lands.

use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// The module is included by path so the suite compiles before the coordinator
// wires `pub mod outcomes_api;` into `src/lib.rs`.
#[path = "../src/outcomes_api.rs"]
mod outcomes_api;

/// The DDL the outcome import / outcome result routes need on top of the
/// `star-db` migrations.
const OUTCOMES_DDL: [&str; 11] = [
    "ALTER TABLE learning_outcomes
        ADD COLUMN IF NOT EXISTS data JSONB,
        ADD COLUMN IF NOT EXISTS calculation_method TEXT,
        ADD COLUMN IF NOT EXISTS calculation_int INTEGER",
    "ALTER TABLE learning_outcome_results
        ALTER COLUMN score DROP NOT NULL,
        ADD COLUMN IF NOT EXISTS context_code TEXT,
        ADD COLUMN IF NOT EXISTS content_tag_id BIGINT,
        ADD COLUMN IF NOT EXISTS association_type TEXT,
        ADD COLUMN IF NOT EXISTS association_id BIGINT,
        ADD COLUMN IF NOT EXISTS hidden BOOLEAN NOT NULL DEFAULT FALSE,
        ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'active',
        ADD COLUMN IF NOT EXISTS possible DOUBLE PRECISION,
        ADD COLUMN IF NOT EXISTS percent DOUBLE PRECISION,
        ADD COLUMN IF NOT EXISTS mastery BOOLEAN,
        ADD COLUMN IF NOT EXISTS hide_points BOOLEAN NOT NULL DEFAULT FALSE,
        ADD COLUMN IF NOT EXISTS title TEXT,
        ADD COLUMN IF NOT EXISTS submitted_at TIMESTAMP,
        ADD COLUMN IF NOT EXISTS assessed_at TIMESTAMP",
    "ALTER TABLE outcome_groups ADD COLUMN IF NOT EXISTS outcome_import_id BIGINT",
    "ALTER TABLE courses ADD COLUMN IF NOT EXISTS latest_outcome_import_id BIGINT",
    "ALTER TABLE accounts ADD COLUMN IF NOT EXISTS latest_outcome_import_id BIGINT",
    "ALTER TABLE users ADD COLUMN IF NOT EXISTS uuid TEXT",
    "CREATE TABLE IF NOT EXISTS outcome_import_errors (
        id                BIGSERIAL PRIMARY KEY,
        outcome_import_id BIGINT NOT NULL REFERENCES outcome_imports(id) ON DELETE CASCADE,
        row               INTEGER,
        message           TEXT NOT NULL,
        failure           BOOLEAN NOT NULL DEFAULT FALSE,
        created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
    )",
    "CREATE INDEX IF NOT EXISTS idx_learning_outcome_results_context
        ON learning_outcome_results (context_code, user_id)",
    "CREATE INDEX IF NOT EXISTS idx_outcome_groups_import ON outcome_groups (outcome_import_id)",
    "CREATE INDEX IF NOT EXISTS idx_outcome_imports_context
        ON outcome_imports (context_type, context_id)",
    "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_uuid ON users (uuid) WHERE uuid IS NOT NULL",
];

/// A running test server plus the pool it uses.
struct TestApp {
    base_url: String,
    client: reqwest::Client,
    pool: PgPool,
}

impl TestApp {
    /// A bearer token for the user.
    fn token(&self, user_id: i64) -> String {
        star_auth::create_token(user_id, "student", "test-jwt-secret").expect("mint token")
    }
}

/// Build the server. Returns `None` when `DATABASE_URL` is not set.
async fn setup() -> Option<TestApp> {
    let db_url = std::env::var("DATABASE_URL").ok()?;
    if db_url.trim().is_empty() {
        println!("SKIP: DATABASE_URL is not set");
        return None;
    }

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    // Serialize the one-shot DDL across the parallel tests of this binary:
    // concurrent `CREATE INDEX IF NOT EXISTS` statements race on the catalog
    // check and can both try to insert the same pg_class row.
    static DDL_MUTEX: std::sync::LazyLock<tokio::sync::Mutex<()>> =
        std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));
    {
        // tokio Mutex: the guard may span the awaited DDL statements (the
        // std::sync guard held across await was clippy await_holding_lock).
        let _ddl_guard = DDL_MUTEX.lock().await;
        for statement in OUTCOMES_DDL {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("apply the outcome DDL");
        }
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let app: axum::Router = axum::Router::new()
        .nest("/api/v1", outcomes_api::router())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the test listener");
    let address = listener.local_addr().expect("read the listener address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    Some(TestApp {
        base_url: format!("http://{address}"),
        client: reqwest::Client::new(),
        pool,
    })
}

/// The ids of one seeded fixture.
struct Fixture {
    account_id: i64,
    course_id: i64,
    teacher_id: i64,
    student_id: i64,
    outcome_id: i64,
    group_id: i64,
    assignment_id: i64,
}

/// Seed an account, a course, a teacher, a student, one outcome with a rubric
/// criterion, a group, a link, an alignment and one outcome result.
async fn seed(pool: &PgPool, prefix: &str) -> Fixture {
    let root_account_id: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .expect("find the root account");

    let account_id: i64 = sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-account"))
    .bind(root_account_id)
    .fetch_one(pool)
    .await
    .expect("seed account");

    let course_id: i64 = sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-course"))
    .bind(format!("{prefix}-course"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course");

    let teacher_id = seed_user(pool, prefix, "teacher", "teacher").await;
    let student_id = seed_user(pool, prefix, "student", "student").await;
    seed_enrollment(pool, teacher_id, course_id, "teacher").await;
    seed_enrollment(pool, student_id, course_id, "student").await;
    seed_account_user(pool, account_id, teacher_id).await;

    let assignment_id: i64 = sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, $2, 10) RETURNING id",
    )
    .bind(course_id)
    .bind(format!("{prefix}-assignment"))
    .fetch_one(pool)
    .await
    .expect("seed assignment");

    let outcome_id: i64 = sqlx::query_scalar(
        "INSERT INTO learning_outcomes (title, description, workflow_state, data, calculation_method)
         VALUES ($1, $2, 'active', $3, 'highest') RETURNING id",
    )
    .bind(format!("{prefix}-outcome"))
    .bind("an outcome")
    .bind(json!({
        "rubric_criterion": {
            "points_possible": 10,
            "mastery_points": 8,
            "ratings": [
                { "description": "Exceeds", "points": 9, "color": "#127A1B" },
                { "description": "Mastery", "points": 8, "color": "#0B874B" },
                { "description": "Developing", "points": 0, "color": "#666666" }
            ]
        }
    }))
    .fetch_one(pool)
    .await
    .expect("seed outcome");

    let group_id: i64 = sqlx::query_scalar(
        "INSERT INTO outcome_groups (course_id, context_type, context_id, title, workflow_state)
         VALUES ($1, 'Course', $1, $2, 'active') RETURNING id",
    )
    .bind(course_id)
    .bind(format!("{prefix}-group"))
    .fetch_one(pool)
    .await
    .expect("seed outcome group");

    sqlx::query(
        "INSERT INTO outcome_links (outcome_id, outcome_group_id, context_type, context_id, workflow_state)
         VALUES ($1, $2, 'Course', $3, 'active')",
    )
    .bind(outcome_id)
    .bind(group_id)
    .bind(course_id)
    .execute(pool)
    .await
    .expect("seed outcome link");

    let alignment_id: i64 = sqlx::query_scalar(
        "INSERT INTO outcome_alignments (outcome_id, context_type, context_id, content_type, content_id, workflow_state)
         VALUES ($1, 'Course', $2, 'Assignment', $3, 'active') RETURNING id",
    )
    .bind(outcome_id)
    .bind(course_id)
    .bind(assignment_id)
    .fetch_one(pool)
    .await
    .expect("seed outcome alignment");

    sqlx::query(
        "INSERT INTO learning_outcome_results
            (outcome_id, user_id, score, context_code, content_tag_id, association_type,
             association_id, workflow_state, possible, percent, title, submitted_at)
         VALUES ($1, $2, 9, $3, $4, 'Assignment', $5, 'active', 10, 0.9, 'Quiz 1, Student', LOCALTIMESTAMP)",
    )
    .bind(outcome_id)
    .bind(student_id)
    .bind(format!("course_{course_id}"))
    .bind(alignment_id)
    .bind(assignment_id)
    .execute(pool)
    .await
    .expect("seed outcome result");

    Fixture {
        account_id,
        course_id,
        teacher_id,
        student_id,
        outcome_id,
        group_id,
        assignment_id,
    }
}

/// Remove every seeded row.
async fn cleanup(pool: &PgPool, fixture: &Fixture) {
    sqlx::query("DELETE FROM learning_outcome_results WHERE outcome_id = $1")
        .bind(fixture.outcome_id)
        .execute(pool)
        .await
        .expect("cleanup results");
    sqlx::query("DELETE FROM outcome_alignments WHERE outcome_id = $1")
        .bind(fixture.outcome_id)
        .execute(pool)
        .await
        .expect("cleanup alignments");
    sqlx::query("DELETE FROM outcome_links WHERE outcome_group_id = $1")
        .bind(fixture.group_id)
        .execute(pool)
        .await
        .expect("cleanup links");
    sqlx::query("DELETE FROM outcome_groups WHERE id = $1")
        .bind(fixture.group_id)
        .execute(pool)
        .await
        .expect("cleanup groups");
    sqlx::query("DELETE FROM learning_outcomes WHERE id = $1")
        .bind(fixture.outcome_id)
        .execute(pool)
        .await
        .expect("cleanup outcomes");
    sqlx::query("DELETE FROM assignments WHERE id = $1")
        .bind(fixture.assignment_id)
        .execute(pool)
        .await
        .expect("cleanup assignments");
    sqlx::query("DELETE FROM outcome_imports WHERE context_id = $1 AND context_type = 'Course'")
        .bind(fixture.course_id)
        .execute(pool)
        .await
        .expect("cleanup imports");
    sqlx::query("DELETE FROM outcome_imports WHERE context_id = $1 AND context_type = 'Account'")
        .bind(fixture.account_id)
        .execute(pool)
        .await
        .expect("cleanup account imports");
    sqlx::query("DELETE FROM jobs WHERE context_id IN ($1, $2)")
        .bind(fixture.course_id)
        .bind(fixture.account_id)
        .execute(pool)
        .await
        .expect("cleanup jobs");
    sqlx::query(
        "DELETE FROM attachments WHERE context_type = 'OutcomeImport' AND context_id IN ($1, $2)",
    )
    .bind(fixture.course_id)
    .bind(fixture.account_id)
    .execute(pool)
    .await
    .expect("cleanup attachments");
    sqlx::query("DELETE FROM enrollments WHERE course_id = $1")
        .bind(fixture.course_id)
        .execute(pool)
        .await
        .expect("cleanup enrollments");
    sqlx::query("DELETE FROM account_users WHERE account_id = $1")
        .bind(fixture.account_id)
        .execute(pool)
        .await
        .expect("cleanup account users");
    sqlx::query("DELETE FROM courses WHERE id = $1")
        .bind(fixture.course_id)
        .execute(pool)
        .await
        .expect("cleanup course");
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(fixture.account_id)
        .execute(pool)
        .await
        .expect("cleanup account");
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(vec![fixture.teacher_id, fixture.student_id])
        .execute(pool)
        .await
        .expect("cleanup users");
}

/// Seed a user.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role, uuid)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .bind(format!("{prefix}-{name}-uuid"))
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Seed an enrollment.
async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state)
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// Seed an account admin.
async fn seed_account_user(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account user");
}

/// A unique prefix for the seeded rows.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "oa-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
        n
    )
}

/// The CSV body of an outcome import.
const IMPORT_CSV: &str = "id,vendor_guid,object_type,object_id,description,rubric_alignment_id\n\
1,vendor-1,assignment,1,an outcome,\n";

// ---------------------------------------------------------------------------
// Outcome imports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcome_import_create_stores_the_import_and_answers_the_canvas_json() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .post(format!(
            "{}/api/v1/accounts/{}/outcome_imports?import_type=instructure_csv",
            app.base_url, fixture.account_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .header("Content-Type", "text/csv")
        .body(IMPORT_CSV.to_string())
        .send()
        .await
        .expect("create the import");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    assert!(body["id"].as_i64().is_some());
    assert_eq!(body["workflow_state"], "created");
    assert_eq!(body["progress"], 0);
    assert_eq!(body["data"]["import_type"], "instructure_csv");
    assert_eq!(body["processing_errors"], json!([]));
    assert_eq!(body["user"]["id"], fixture.teacher_id);

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_import_create_accepts_a_multipart_attachment() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let boundary = "----staroutcomesboundary";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"import_type\"\r\n\r\n\
         instructure_csv\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"attachment\"; filename=\"outcomes.csv\"\r\n\
         Content-Type: text/csv\r\n\r\n\
         {IMPORT_CSV}\r\n\
         --{boundary}--\r\n"
    );

    let response = app
        .client
        .post(format!(
            "{}/api/v1/courses/{}/outcome_imports",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .expect("create the import");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["workflow_state"], "created");
    assert_eq!(body["data"]["import_type"], "instructure_csv");

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_import_create_refuses_a_user_without_the_import_permission() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .post(format!(
            "{}/api/v1/accounts/{}/outcome_imports",
            app.base_url, fixture.account_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.student_id)),
        )
        .header("Content-Type", "text/csv")
        .body(IMPORT_CSV.to_string())
        .send()
        .await
        .expect("create the import");

    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["status"], "unauthorized");
    assert_eq!(
        body["errors"][0]["message"],
        "user not authorized to perform that action"
    );

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_import_show_returns_the_import_and_404s_for_an_unknown_id() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let created = app
        .client
        .post(format!(
            "{}/api/v1/accounts/{}/outcome_imports",
            app.base_url, fixture.account_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .header("Content-Type", "text/csv")
        .body(IMPORT_CSV.to_string())
        .send()
        .await
        .expect("create the import");
    let created: Value = created.json().await.expect("decode the body");
    let import_id = created["id"].as_i64().expect("the import id");

    let shown = app
        .client
        .get(format!(
            "{}/api/v1/accounts/{}/outcome_imports/{}",
            app.base_url, fixture.account_id, import_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("show the import");
    assert_eq!(shown.status(), 200);
    let shown: Value = shown.json().await.expect("decode the body");
    assert_eq!(shown["id"], import_id);
    assert_eq!(shown["workflow_state"], "created");

    let missing = app
        .client
        .get(format!(
            "{}/api/v1/accounts/{}/outcome_imports/999999",
            app.base_url, fixture.account_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("show the missing import");
    assert_eq!(missing.status(), 404);
    let missing: Value = missing.json().await.expect("decode the body");
    assert_eq!(
        missing["message"],
        "Couldn't find OutcomeImport with 'id'=999999"
    );

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_import_created_group_ids_404s_until_the_import_succeeded() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let created = app
        .client
        .post(format!(
            "{}/api/v1/courses/{}/outcome_imports",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .header("Content-Type", "text/csv")
        .body(IMPORT_CSV.to_string())
        .send()
        .await
        .expect("create the import");
    let created: Value = created.json().await.expect("decode the body");
    let import_id = created["id"].as_i64().expect("the import id");

    let pending = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_imports/{}/created_group_ids",
            app.base_url, fixture.course_id, import_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the created group ids");
    assert_eq!(pending.status(), 404);
    let pending: Value = pending.json().await.expect("decode the body");
    assert_eq!(pending["message"], "Import is still being processed");

    // Mark the import as succeeded and attach the group to it.
    sqlx::query("UPDATE outcome_imports SET workflow_state = 'succeeded' WHERE id = $1")
        .bind(import_id)
        .execute(&app.pool)
        .await
        .expect("finish the import");
    sqlx::query("UPDATE outcome_groups SET outcome_import_id = $1 WHERE id = $2")
        .bind(import_id)
        .bind(fixture.group_id)
        .execute(&app.pool)
        .await
        .expect("attach the group");

    let done = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_imports/{}/created_group_ids",
            app.base_url, fixture.course_id, import_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the created group ids");
    assert_eq!(done.status(), 200);
    let done: Value = done.json().await.expect("decode the body");
    assert_eq!(done, json!([fixture.group_id.to_string()]));

    cleanup(&app.pool, &fixture).await;
}

// ---------------------------------------------------------------------------
// Outcome results
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcome_results_returns_the_seeded_results() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_results",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the outcome results");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    let results = body["outcome_results"]
        .as_array()
        .expect("the results array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["score"], 9.0);
    assert_eq!(results[0]["hidden"], false);
    assert_eq!(results[0]["links"]["user"], fixture.student_id.to_string());
    assert_eq!(
        results[0]["links"]["learning_outcome"],
        fixture.outcome_id.to_string()
    );
    assert_eq!(
        results[0]["links"]["assignment"],
        format!("assignment_{}", fixture.assignment_id)
    );

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_results_refuses_a_student_that_did_not_name_the_users() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_results",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.student_id)),
        )
        .send()
        .await
        .expect("read the outcome results");

    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["status"], "forbidden");
    assert_eq!(
        body["message"],
        "users not specified and no access to all grades"
    );

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_results_rejects_an_outcome_that_is_not_in_the_course() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_results?outcome_ids[]=999999",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the outcome results");

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["status"], "bad_request");
    assert_eq!(
        body["message"],
        "can only include id's of outcomes in the outcome context"
    );

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_rollups_returns_the_rollups_and_the_pagination_meta() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_rollups",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the rollups");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    let rollups = body["rollups"].as_array().expect("the rollups array");
    assert_eq!(rollups.len(), 1);
    assert_eq!(rollups[0]["links"]["user"], fixture.student_id.to_string());
    assert_eq!(rollups[0]["links"]["status"], "active");
    let scores = rollups[0]["scores"].as_array().expect("the scores array");
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0]["score"], 9.0);
    assert_eq!(scores[0]["count"], 1);
    assert_eq!(
        scores[0]["links"]["outcome"],
        fixture.outcome_id.to_string()
    );
    assert_eq!(body["meta"]["pagination"]["per_page"], 10);
    assert_eq!(body["meta"]["pagination"]["count"], 1);

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_rollups_rejects_an_unknown_aggregate_or_include() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let aggregate = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_rollups?aggregate=bogus",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the rollups");
    assert_eq!(aggregate.status(), 400);
    let aggregate: Value = aggregate.json().await.expect("decode the body");
    assert_eq!(aggregate["message"], "invalid aggregate parameter value");

    let include = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_rollups?include[]=bogus",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the rollups");
    assert_eq!(include.status(), 400);
    let include: Value = include.json().await.expect("decode the body");
    assert_eq!(include["message"], "invalid include: bogus");

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_rollups_aggregates_the_course_when_asked() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_rollups?aggregate=course",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the rollups");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    let rollups = body["rollups"].as_array().expect("the rollups array");
    assert_eq!(rollups.len(), 1);
    assert_eq!(rollups[0]["links"]["course"], fixture.course_id.to_string());
    let scores = rollups[0]["scores"].as_array().expect("the scores array");
    assert_eq!(scores[0]["score"], 9.0);

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn mastery_distribution_buckets_the_students_into_the_ratings() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_mastery_distribution",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the mastery distribution");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    let key = fixture.outcome_id.to_string();
    let distribution = &body["outcome_distributions"][&key];
    assert_eq!(distribution["outcome_id"], key.as_str());
    assert_eq!(distribution["total_students"], 1);
    let ratings = distribution["ratings"]
        .as_array()
        .expect("the ratings array");
    assert_eq!(ratings.len(), 3);
    assert_eq!(ratings[0]["description"], "Exceeds");
    assert_eq!(ratings[0]["points"], 9.0);
    assert_eq!(ratings[0]["count"], 1);
    assert_eq!(
        ratings[0]["student_ids"],
        json!([fixture.student_id.to_string()])
    );
    let students = body["students"].as_array().expect("the students array");
    assert_eq!(students.len(), 1);
    assert_eq!(students[0]["id"], fixture.student_id.to_string());

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn contributing_scores_lists_the_alignments_and_the_scores() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcomes/{}/contributing_scores",
            app.base_url, fixture.course_id, fixture.outcome_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the contributing scores");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["outcome"]["id"], fixture.outcome_id.to_string());
    let alignments = body["alignments"].as_array().expect("the alignments array");
    assert_eq!(alignments.len(), 1);
    assert_eq!(alignments[0]["associated_asset_type"], "Assignment");
    assert_eq!(
        alignments[0]["associated_asset_id"],
        fixture.assignment_id.to_string()
    );
    let scores = body["scores"].as_array().expect("the scores array");
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0]["user_id"], fixture.student_id.to_string());
    assert_eq!(scores[0]["score"], 9.0);

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn contributing_scores_rejects_an_outcome_that_is_not_in_the_course() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcomes/999999/contributing_scores",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the contributing scores");

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["message"], "outcome not found in this context");

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn assign_outcome_order_stores_the_ordering_and_answers_no_content() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .post(format!(
            "{}/api/v1/courses/{}/assign_outcome_order",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .header("Content-Type", "application/json")
        .body(json!([{ "outcome_id": fixture.outcome_id, "position": 3 }]).to_string())
        .send()
        .await
        .expect("assign the outcome order");

    assert_eq!(response.status(), 204);

    let stored: (i64, i32) = sqlx::query_as(
        "SELECT learning_outcome_id, position FROM user_lmgb_outcome_orderings
         WHERE user_id = $1 AND course_id = $2 AND learning_outcome_id = $3",
    )
    .bind(fixture.teacher_id)
    .bind(fixture.course_id)
    .bind(fixture.outcome_id)
    .fetch_one(&app.pool)
    .await
    .expect("the stored ordering");
    assert_eq!(stored, (fixture.outcome_id, 3));

    sqlx::query("DELETE FROM user_lmgb_outcome_orderings WHERE course_id = $1")
        .bind(fixture.course_id)
        .execute(&app.pool)
        .await
        .expect("cleanup the ordering");

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn assign_outcome_order_rejects_an_outcome_of_another_course() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .post(format!(
            "{}/api/v1/courses/{}/assign_outcome_order",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .header("Content-Type", "application/json")
        .body(json!([{ "outcome_id": 999999, "position": 1 }]).to_string())
        .send()
        .await
        .expect("assign the outcome order");

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["message"], "Outcomes do not belong to Course");

    cleanup(&app.pool, &fixture).await;
}

/// The `outcomes_rollup_propagation` feature flag drives the whole endpoint,
/// so both halves live in one test to keep them from racing over the shared
/// flag row.
#[tokio::test]
async fn enqueue_outcome_rollup_calculation_honours_the_feature_flag() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let site_admin: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id LIMIT 1",
    )
    .fetch_one(&app.pool)
    .await
    .expect("the site admin account");

    // The flag is off by default: the endpoint answers `204 No Content`.
    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state)
         VALUES ('Account', $1, 'outcomes_rollup_propagation', 'Rollup propagation', 'disabled')
         ON CONFLICT (context_type, context_id, feature)
         DO UPDATE SET state = 'disabled'",
    )
    .bind(site_admin)
    .execute(&app.pool)
    .await
    .expect("disable the feature flag");

    let student_uuid = format!("{prefix}-student-uuid");
    let response = app
        .client
        .post(format!(
            "{}/api/v1/enqueue_outcome_rollup_calculation?course_id={}&student_uuid={}",
            app.base_url, fixture.course_id, student_uuid
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("enqueue the rollup calculation");

    assert_eq!(response.status(), 204);

    // With the flag on the endpoint validates its parameters.
    sqlx::query(
        "UPDATE feature_flags SET state = 'enabled'
         WHERE context_type = 'Account' AND context_id = $1
           AND feature = 'outcomes_rollup_propagation'",
    )
    .bind(site_admin)
    .execute(&app.pool)
    .await
    .expect("enable the feature flag");

    let response = app
        .client
        .post(format!(
            "{}/api/v1/enqueue_outcome_rollup_calculation",
            app.base_url
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("enqueue the rollup calculation");

    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["error"], "course id and student uuid are required");

    let missing = app
        .client
        .post(format!(
            "{}/api/v1/enqueue_outcome_rollup_calculation?course_id={}&student_uuid=nope",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("enqueue the rollup calculation");
    assert_eq!(missing.status(), 404);
    let missing: Value = missing.json().await.expect("decode the body");
    assert_eq!(missing["error"], "Invalid course or student");

    let student_uuid = format!("{prefix}-student-uuid");
    let enqueued = app
        .client
        .post(format!(
            "{}/api/v1/enqueue_outcome_rollup_calculation?course_id={}&student_uuid={}",
            app.base_url, fixture.course_id, student_uuid
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("enqueue the rollup calculation");
    assert_eq!(enqueued.status(), 200);
    let enqueued: Value = enqueued.json().await.expect("decode the body");
    assert_eq!(enqueued["type"], "student");
    assert_eq!(
        enqueued["message"],
        format!(
            "Rollup calculation enqueued for student {} in course {}",
            fixture.student_id, fixture.course_id
        )
    );

    sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Account' AND context_id = $1
         AND feature = 'outcomes_rollup_propagation'",
    )
    .bind(site_admin)
    .execute(&app.pool)
    .await
    .expect("cleanup the feature flag");

    cleanup(&app.pool, &fixture).await;
}
