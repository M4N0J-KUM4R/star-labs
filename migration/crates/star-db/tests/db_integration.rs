//! Integration tests for star-db.
//!
//! Verifies that the embedded `MIGRATOR` (sqlx::migrate!) applies the whole
//! schema to a real PostgreSQL and that the tables the domain crates rely on
//! actually exist afterwards.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
//!   cargo test -p star-db
//! - Migrations are checksummed by sqlx (_sqlx_migrations), so re-running
//!   against an already-migrated shared test DB is a no-op — safe to run
//!   repeatedly and in parallel with the other crates' DB tests.

use sqlx::PgPool;
use star_db::MIGRATOR;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Connect and run migrations. Returns None when DATABASE_URL is unset so
/// callers can skip cleanly.
async fn db_pool() -> Option<PgPool> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    Some(pool)
}

/// True when the given table exists in the current schema of the connected DB.
async fn table_exists(pool: &PgPool, table: &str) -> bool {
    sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM information_schema.tables
        WHERE table_schema = current_schema() AND table_name = $1
        "#,
    )
    .bind(table)
    .fetch_one(pool)
    .await
    .expect("query information_schema")
        == 1
}

// ---------------------------------------------------------------------------
// MIGRATOR applies cleanly (idempotent on re-run)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migrator_runs_cleanly_and_is_idempotent() {
    let Some(pool) = db_pool().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // First (possibly no-op) run against the shared test DB.
    MIGRATOR
        .run(&pool)
        .await
        .expect("first MIGRATOR.run must succeed");

    // Re-running the same checksummed migrations must be a clean no-op.
    MIGRATOR
        .run(&pool)
        .await
        .expect("second MIGRATOR.run must be a no-op (checksummed)");

    // The sqlx bookkeeping table exists and is populated.
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("count applied migrations");
    assert!(
        applied > 0,
        "expected at least one row in _sqlx_migrations, got {applied}"
    );
}

// ---------------------------------------------------------------------------
// Core tables exist after migration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn core_tables_exist_after_migrations() {
    let Some(pool) = db_pool().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    MIGRATOR
        .run(&pool)
        .await
        .expect("run migrations before asserting schema");

    for table in ["users", "courses", "enrollments", "assignments"] {
        assert!(
            table_exists(&pool, table).await,
            "table '{table}' must exist after migrations"
        );
    }
}

// ---------------------------------------------------------------------------
// Later-migration tables exist too (content domain, 0002-0007)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn content_domain_tables_exist_after_migrations() {
    let Some(pool) = db_pool().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    MIGRATOR
        .run(&pool)
        .await
        .expect("run migrations before asserting schema");

    // 0002 content extras + 0003 modules + 0004 announcements + 0007 items.
    for table in ["pages", "modules", "module_items", "announcements"] {
        assert!(
            table_exists(&pool, table).await,
            "table '{table}' must exist after migrations"
        );
    }
}

// ---------------------------------------------------------------------------
// Late-migration tables exist too (0005-0027 tail of the migrations dir)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn late_migration_tables_exist_after_migrations() {
    let Some(pool) = db_pool().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    MIGRATOR
        .run(&pool)
        .await
        .expect("run migrations before asserting schema");

    // A spread across the tail of the migrations directory.
    for table in ["files", "quizzes", "discussion_topics", "grading_standards"] {
        assert!(
            table_exists(&pool, table).await,
            "table '{table}' must exist after migrations"
        );
    }
}

// ---------------------------------------------------------------------------
// Column parity spot-check on the core tables (0001 core schema)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn core_tables_have_expected_columns() {
    let Some(pool) = db_pool().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    MIGRATOR
        .run(&pool)
        .await
        .expect("run migrations before asserting schema");

    let expected: &[(&str, &[&str])] = &[
        (
            "users",
            &[
                "id",
                "name",
                "email",
                "password_hash",
                "role",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "courses",
            &["id", "name", "course_code", "created_at", "updated_at"],
        ),
        (
            "enrollments",
            &[
                "id",
                "user_id",
                "course_id",
                "role",
                "created_at",
                "updated_at",
            ],
        ),
        (
            "assignments",
            &[
                "id",
                "course_id",
                "title",
                "points_possible",
                "due_at",
                "created_at",
                "updated_at",
            ],
        ),
    ];

    for (table, columns) in expected {
        for column in *columns {
            let found: i64 = sqlx::query_scalar(
                r#"
                SELECT COUNT(*)
                FROM information_schema.columns
                WHERE table_schema = current_schema()
                  AND table_name = $1
                  AND column_name = $2
                "#,
            )
            .bind(table)
            .bind(column)
            .fetch_one(&pool)
            .await
            .expect("query information_schema.columns");
            assert!(
                found == 1,
                "column {table}.{column} must exist after migrations"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Schema supports basic writes (FK graph is real, not just table names)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn core_foreign_keys_accept_seeded_rows() {
    let Some(pool) = db_pool().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    MIGRATOR
        .run(&pool)
        .await
        .expect("run migrations before seeding");

    // Unique-per-run values so repeat runs never collide on UNIQUE keys.
    let uniq = format!(
        "dbit-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );

    let user_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role)
        VALUES ($1, $2, $3, 'student')
        RETURNING id
        "#,
    )
    .bind("Db Integration Student")
    .bind(format!("{uniq}@test.local"))
    .bind("not-a-real-hash")
    .fetch_one(&pool)
    .await
    .expect("seed user");

    let course_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind("Db Integration Course")
    .bind(&uniq)
    .fetch_one(&pool)
    .await
    .expect("seed course");

    let enrollment_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO enrollments (user_id, course_id, role)
        VALUES ($1, $2, 'student')
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(&pool)
    .await
    .expect("seed enrollment");

    let assignment_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, points_possible)
        VALUES ($1, $2, 10.50)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind("Db Integration Assignment")
    .fetch_one(&pool)
    .await
    .expect("seed assignment");

    assert!(user_id > 0 && course_id > 0 && enrollment_id > 0 && assignment_id > 0);

    // The FK edge really exists: deleting the course cascades to its rows.
    let deleted = sqlx::query("DELETE FROM courses WHERE id = $1")
        .bind(course_id)
        .execute(&pool)
        .await
        .expect("delete course");
    assert_eq!(deleted.rows_affected(), 1, "course row must be deleted");

    let orphaned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM enrollments WHERE course_id = $1")
        .bind(course_id)
        .fetch_one(&pool)
        .await
        .expect("count enrollments after cascade");
    assert_eq!(orphaned, 0, "enrollment must cascade on course delete");

    let orphaned: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assignments WHERE course_id = $1")
        .bind(course_id)
        .fetch_one(&pool)
        .await
        .expect("count assignments after cascade");
    assert_eq!(orphaned, 0, "assignment must cascade on course delete");

    // Teardown (best effort — safe to run against a shared test DB).
    let _ = sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(format!("{uniq}@test.local"))
        .execute(&pool)
        .await;
}
