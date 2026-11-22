//! Integration tests for star-eportfolios (issue #91, spec
//! `specs/p5-rest-eportfolios.md` §7).
//!
//! Pinned behavior (verified against canvas-lms master,
//! EportfoliosApiController + Eportfolio model + Api::V1::Eportfolio):
//! - index: own portfolios (active only — include[]=deleted NEVER adds
//!   deleted rows for the owner), moderator index of another user includes
//!   flagged/spam portfolios; `include[]=deleted` lifts the active filter for
//!   a moderator viewing ANOTHER user only. Bare array + meta.pagination,
//!   ordered by updated_at.
//! - show: owner of an active portfolio (spam included), public portfolios
//!   (not spam) to any verified caller, moderators on anything, deleted
//!   portfolios hidden from their author.
//! - pages: FLAT position-ordered entry list (not category-grouped), entry
//!   JSON id/eportfolio_id/position/name/content/created_at/updated_at.
//! - delete: soft delete (workflow_state 'deleted' + deleted_at set),
//!   renders the portfolio JSON; author delete of a deleted portfolio 404s
//!   (no :delete grant), moderator delete is an idempotent state rewrite.
//! - restore: moderator only (the author never holds :restore); back to
//!   active with deleted_at null; restoring a non-deleted portfolio is an
//!   idempotent success.
//! - moderate / moderate_all: moderators only; spam_status must be
//!   marked_as_safe|marked_as_spam (anything else — including blank — is the
//!   upstream 400 error body, AFTER the ability check so 403 wins for
//!   non-moderators); moderate_all rewrites every ACTIVE portfolio of the
//!   user (deleted excluded) and renders the JSON string "success".
//! - verified_user_check: an acting user without a confirmed (active)
//!   communication channel gets 401 {"status": "unverified", ...} on every
//!   action; anonymous callers get the plain 401.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix plus
//! full teardown keeps shared-DB runs safe.

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
        "eport_{}_{}",
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
        TestServer::new(star_eportfolios::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

/// Seed a user; `verified` adds a confirmed (active) communication channel —
/// the verified_user_check prerequisite.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str, verified: bool) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user");
    if verified {
        sqlx::query(
            "INSERT INTO communication_channels (user_id, path, path_type, position, workflow_state) \
             VALUES ($1, $2, 'email', 0, 'active')",
        )
        .bind(id)
        .bind(format!("{prefix}-{name}@test.local"))
        .execute(pool)
        .await
        .expect("seed channel");
    }
    id
}

/// Seed a portfolio. `public` defaults to false, workflow_state to 'active'.
#[allow(clippy::too_many_arguments)]
async fn seed_portfolio(
    pool: &PgPool,
    user_id: i64,
    name: &str,
    public: bool,
    workflow_state: &str,
    spam_status: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO eportfolios \
         (user_id, name, public, workflow_state, spam_status, uuid, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now(), now()) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(public)
    .bind(workflow_state)
    .bind(spam_status)
    .bind(format!("uuid-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed portfolio")
}

/// Seed a category (entries need one: eportfolio_category_id is NOT NULL in
/// the snapshot schema).
async fn seed_category(pool: &PgPool, eportfolio_id: i64, name: &str, position: i32) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO eportfolio_categories \
         (eportfolio_id, name, position, slug, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, now(), now()) RETURNING id",
    )
    .bind(eportfolio_id)
    .bind(name)
    .bind(position)
    .bind(format!("cat-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed category")
}

/// Seed an entry (a "page") in a category.
async fn seed_entry(
    pool: &PgPool,
    eportfolio_id: i64,
    category_id: i64,
    name: &str,
    position: i32,
    content: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO eportfolio_entries \
         (eportfolio_id, eportfolio_category_id, name, position, content, slug, \
          created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now(), now()) RETURNING id",
    )
    .bind(eportfolio_id)
    .bind(category_id)
    .bind(name)
    .bind(position)
    .bind(content)
    .bind(format!("entry-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed entry")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // eportfolios have no FK cascade to users in the snapshot, so delete in
    // dependency order: entries, categories, portfolios, channels, users.
    let _ = sqlx::query(
        "DELETE FROM eportfolio_entries WHERE eportfolio_id IN \
         (SELECT id FROM eportfolios WHERE user_id IN \
         (SELECT id FROM users WHERE email LIKE $1))",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM eportfolio_categories WHERE eportfolio_id IN \
         (SELECT id FROM eportfolios WHERE user_id IN \
         (SELECT id FROM users WHERE email LIKE $1))",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM eportfolios WHERE user_id IN \
         (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM communication_channels WHERE user_id IN \
         (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn index_show_and_pages_shapes() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student", true).await;
    let bearer = auth(owner, "student");
    let eport = seed_portfolio(&pool, owner, "My Academic Journey", true, "active", None).await;
    let cat = seed_category(&pool, eport, "Home", 0).await;
    let entry2 = seed_entry(&pool, eport, cat, "Second page", 2, "Later work").await;
    let entry1 = seed_entry(&pool, eport, cat, "First page", 1, "A long time ago...").await;

    // Index: bare array (upstream render json: portfolios.map) + meta.
    let resp = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "one portfolio: {listed}");
    assert!(listed["eportfolios"].is_null(), "no plural root: {listed}");
    assert_eq!(listed["meta"]["pagination"]["total"], 1);
    let portfolio = &items[0];
    assert_eq!(portfolio["id"], eport);
    assert_eq!(portfolio["user_id"], owner);
    assert_eq!(portfolio["name"], "My Academic Journey");
    assert_eq!(portfolio["public"], true);
    assert_eq!(portfolio["workflow_state"], "active");
    assert_eq!(portfolio["deleted_at"], Value::Null);
    assert_eq!(portfolio["spam_status"], Value::Null);
    assert!(portfolio["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(portfolio["updated_at"].as_str().unwrap().ends_with('Z'));
    assert!(portfolio.get("uuid").is_none(), "uuid is not serialized");

    // Show: single object, same field set.
    let resp = server
        .get(&format!("/eportfolios/{eport}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"], eport);
    assert_eq!(shown["name"], "My Academic Journey");
    assert_eq!(shown["workflow_state"], "active");
    assert_eq!(shown["public"], true);

    // Pages: FLAT position-ordered list of entries with ENTRY_ATTRIBUTES.
    let resp = server
        .get(&format!("/eportfolios/{eport}/pages"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let pages: Value = resp.json();
    let entries = pages["items"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "both pages listed: {pages}");
    assert_eq!(entries[0]["id"], entry1, "position order: {pages}");
    assert_eq!(entries[1]["id"], entry2);
    assert_eq!(entries[0]["eportfolio_id"], eport);
    assert_eq!(entries[0]["name"], "First page");
    assert_eq!(entries[0]["content"], "A long time ago...");
    assert_eq!(entries[0]["position"], 1);
    assert!(entries[0]["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(entries[0]["updated_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(pages["meta"]["pagination"]["total"], 2);
    // No category fields on the page JSON (ENTRY_ATTRIBUTES only).
    assert!(entries[0].get("eportfolio_category_id").is_none());

    // Unknown ids are 404s (global find; unknown user likewise).
    for path in [
        "/eportfolios/999999999",
        "/eportfolios/999999999/pages",
        "/users/999999999/eportfolios",
    ] {
        let resp = server
            .get(path)
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 404, "GET {path}");
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn soft_delete_restore_lifecycle() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student", true).await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin", true).await;
    let owner_bearer = auth(owner, "student");
    let admin_bearer = auth(admin, "admin");
    let eport = seed_portfolio(&pool, owner, "Journey", false, "active", None).await;
    let other = seed_portfolio(&pool, owner, "Second", false, "active", None).await;

    // Owner soft-deletes: 200 with the portfolio JSON in its deleted state.
    let resp = server
        .delete(&format!("/eportfolios/{eport}"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["id"], eport);
    assert_eq!(deleted["workflow_state"], "deleted");
    assert!(deleted["deleted_at"].as_str().is_some(), "deleted_at set");

    // Soft delete only: the row is still there.
    let (rows,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM eportfolios WHERE id = $1")
        .bind(eport)
        .fetch_one(&pool)
        .await
        .expect("count rows");
    assert_eq!(rows, 1, "soft delete keeps the row");

    // Index excludes it; include[]=deleted does NOT bring it back for the
    // owner (upstream: own scope is always .active).
    let resp = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .await;
    let listed: Value = resp.json();
    let ids: Vec<i64> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![other], "deleted excluded from own index");
    let resp = server
        .get(&format!("/users/{owner}/eportfolios?include[]=deleted"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .await;
    let listed: Value = resp.json();
    assert_eq!(
        listed["items"].as_array().unwrap().len(),
        1,
        "include[]=deleted never adds deleted rows for the owner: {listed}"
    );

    // Show: deleted portfolios are unreadable by their author (403).
    let resp = server
        .get(&format!("/eportfolios/{eport}"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Owner cannot re-delete a deleted portfolio (no :delete grant) — 403.
    let resp = server
        .delete(&format!("/eportfolios/{eport}"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Restore is moderator-only: the author is 403, the admin 200.
    let resp = server
        .put(&format!("/eportfolios/{eport}/restore"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .put(&format!("/eportfolios/{eport}/restore"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let restored: Value = resp.json();
    assert_eq!(restored["workflow_state"], "active");
    assert_eq!(restored["deleted_at"], Value::Null);

    // Restored: back in the own index, readable again by the author.
    let resp = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .await;
    let listed: Value = resp.json();
    assert_eq!(listed["items"].as_array().unwrap().len(), 2);
    let resp = server
        .get(&format!("/eportfolios/{eport}"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);

    // Restoring a non-deleted portfolio is an idempotent success.
    let resp = server
        .put(&format!("/eportfolios/{eport}/restore"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "active");

    // Unknown portfolio id on delete/restore: 404 (guard before ability).
    for (method, path) in [
        ("delete", "/eportfolios/999999999".to_string()),
        ("put", "/eportfolios/999999999/restore".to_string()),
    ] {
        let resp = if method == "delete" {
            server
                .delete(&path)
                .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
                .await
        } else {
            server
                .put(&path)
                .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
                .await
        };
        assert_eq!(resp.status_code(), 404, "{method} {path}");
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn moderation_and_moderate_all() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student", true).await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin", true).await;
    let other_admin = seed_user(&pool, &prefix, "Admin2", "admin", true).await;
    let owner_bearer = auth(owner, "student");
    let admin_bearer = auth(admin, "admin");
    let eport = seed_portfolio(&pool, owner, "Suspect", true, "active", None).await;
    let keepline = seed_portfolio(
        &pool,
        owner,
        "Keeper",
        false,
        "active",
        Some("marked_as_safe"),
    )
    .await;

    // Moderate: admin marks spam — 200 with the updated portfolio JSON.
    let resp = server
        .put(&format!("/eportfolios/{eport}/moderate"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let moderated: Value = resp.json();
    assert_eq!(moderated["id"], eport);
    assert_eq!(moderated["spam_status"], "marked_as_spam");

    // A spam-marked public portfolio is no longer publicly readable, but the
    // author still reads it ("whether this eportfolio is spam or not, the
    // author can see it").
    let resp = server
        .get(&format!("/eportfolios/{eport}"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "author reads own spam portfolio");

    // marked_as_safe flips it back.
    let resp = server
        .put(&format!("/eportfolios/{eport}/moderate"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .json(&json!({ "spam_status": "marked_as_safe" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["spam_status"], "marked_as_safe");

    // Non-moderators are 403 — even with an invalid spam_status (the ability
    // check precedes validation upstream).
    for bad in ["marked_as_spam", "bogus", ""] {
        let resp = server
            .put(&format!("/eportfolios/{eport}/moderate"))
            .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
            .json(&json!({ "spam_status": bad }))
            .await;
        assert_eq!(resp.status_code(), 403, "spam_status={bad:?}");
    }

    // Invalid spam_status values → the upstream 400 error body.
    for bad in ["bogus", "flagged_as_possible_spam", ""] {
        let resp = server
            .put(&format!("/eportfolios/{eport}/moderate"))
            .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
            .json(&json!({ "spam_status": bad }))
            .await;
        assert_eq!(
            resp.status_code(),
            400,
            "spam_status={bad:?}: {}",
            resp.text()
        );
        let body: Value = resp.json();
        assert_eq!(
            body["error"], "spam_status must be one of [\"marked_as_safe\", \"marked_as_spam\"]",
            "body: {body}"
        );
    }

    // A missing body / blank param is the same 400.
    let resp = server
        .put(&format!("/eportfolios/{eport}/moderate"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Unknown portfolio: 404 before ability/validation.
    let resp = server
        .put("/eportfolios/999999999/moderate")
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // moderate_all: admin rewrites every ACTIVE portfolio of the user.
    // Upstream `render json: :success` (eportfolios_api_controller.rb
    // moderate_all) — the Symbol :success serializes to the JSON string,
    // so the raw body is "success" WITH the quotes.
    let resp = server
        .put(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.text(), "\"success\"");
    for id in [eport, keepline] {
        let (status,): (Option<String>,) =
            sqlx::query_as("SELECT spam_status FROM eportfolios WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("read spam_status");
        assert_eq!(status.as_deref(), Some("marked_as_spam"), "portfolio {id}");
    }

    // moderate_all only touches ACTIVE portfolios: soft-delete one, flip the
    // rest to safe, the deleted row keeps its spam_status.
    sqlx::query(
        "UPDATE eportfolios SET workflow_state = 'deleted', deleted_at = now() WHERE id = $1",
    )
    .bind(keepline)
    .execute(&pool)
    .await
    .expect("soft delete");
    let resp = server
        .put(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, other_admin)
        .json(&json!({ "spam_status": "marked_as_safe" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let (status,): (Option<String>,) =
        sqlx::query_as("SELECT spam_status FROM eportfolios WHERE id = $1")
            .bind(keepline)
            .fetch_one(&pool)
            .await
            .expect("read spam_status");
    assert_eq!(
        status.as_deref(),
        Some("marked_as_spam"),
        "deleted portfolios are excluded from moderate_all"
    );

    // moderate_all permission + validation negatives (403 before 400 for a
    // non-moderator; unknown user 404).
    let resp = server
        .put(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, owner_bearer.clone())
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .put(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .json(&json!({ "spam_status": "bogus" }))
        .await;
    assert_eq!(resp.status_code(), 400);
    let resp = server
        .put("/users/999999999/eportfolios")
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // A moderator CAN soft-delete and restore another user's portfolio
    // (:delete/:restore grants) and reads the moderation view of the index
    // (spam portfolios included).
    let resp = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed["items"][0]["spam_status"], "marked_as_safe");

    let resp = server
        .delete(&format!("/eportfolios/{eport}"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "admin soft-deletes: {}",
        resp.text()
    );
    // And the moderator sees deleted rows with include[]=deleted.
    let resp = server
        .get(&format!("/users/{owner}/eportfolios?include[]=deleted"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    let listed: Value = resp.json();
    let states: Vec<&str> = listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["workflow_state"].as_str().unwrap())
        .collect();
    assert_eq!(states, vec!["deleted", "deleted"], "both rows: {listed}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn read_permissions_matrix() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student", true).await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student", true).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher", true).await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin", true).await;
    let stranger_bearer = auth(stranger, "student");
    let teacher_bearer = auth(teacher, "teacher");
    let admin_bearer = auth(admin, "admin");

    let private_p = seed_portfolio(&pool, owner, "Private", false, "active", None).await;
    let public_p = seed_portfolio(&pool, owner, "Public", true, "active", None).await;
    let public_spam = seed_portfolio(
        &pool,
        owner,
        "PublicSpam",
        true,
        "active",
        Some("marked_as_spam"),
    )
    .await;

    // Another student cannot list the owner's portfolios (403).
    let resp = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, stranger_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Another student on a private portfolio: 403 (known id, failed :read).
    let resp = server
        .get(&format!("/eportfolios/{private_p}"))
        .add_header(axum::http::header::AUTHORIZATION, stranger_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    // ... and on its pages (same :read path).
    let resp = server
        .get(&format!("/eportfolios/{private_p}/pages"))
        .add_header(axum::http::header::AUTHORIZATION, stranger_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A public portfolio IS readable by any verified caller (stranger,
    // teacher), but a spam-marked public one is not.
    for bearer in [&stranger_bearer, &teacher_bearer] {
        let resp = server
            .get(&format!("/eportfolios/{public_p}"))
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 200, "public portfolio readable");
        let resp = server
            .get(&format!("/eportfolios/{public_p}/pages"))
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 200, "public pages readable");
    }
    let resp = server
        .get(&format!("/eportfolios/{public_spam}"))
        .add_header(axum::http::header::AUTHORIZATION, stranger_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "spam public portfolio not public");

    // Admins read everything, including spam and deleted portfolios, and can
    // list another user's portfolios (the :moderate_user_content view).
    for id in [private_p, public_p, public_spam] {
        let resp = server
            .get(&format!("/eportfolios/{id}"))
            .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 200, "admin reads portfolio {id}");
    }
    let resp = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    assert_eq!(listed["items"].as_array().unwrap().len(), 3, "{listed}");

    // Only the owner (or admin) can delete: the stranger is 403.
    let resp = server
        .delete(&format!("/eportfolios/{private_p}"))
        .add_header(axum::http::header::AUTHORIZATION, stranger_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn unverified_user_is_401_on_every_action() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // No confirmed communication channel → verified_user_check fails on
    // every action with the unverified 401 body.
    let owner = seed_user(&pool, &prefix, "Owner", "student", false).await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin", false).await;
    let bearer = auth(owner, "student");
    let admin_bearer = auth(admin, "admin");
    let eport = seed_portfolio(&pool, owner, "Unverified view", true, "active", None).await;

    let gets = [
        format!("/users/{owner}/eportfolios"),
        format!("/eportfolios/{eport}"),
        format!("/eportfolios/{eport}/pages"),
    ];
    for path in gets {
        let resp = server
            .get(&path)
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 401, "GET {path}: {}", resp.text());
        let body: Value = resp.json();
        assert_eq!(body["status"], "unverified", "body: {body}");
        assert!(body["errors"].as_array().is_some());
    }

    let resp = server
        .delete(&format!("/eportfolios/{eport}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    for path in [
        format!("/eportfolios/{eport}/restore"),
        format!("/eportfolios/{eport}/moderate"),
    ] {
        let resp = server
            .put(&path)
            .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
            .json(&json!({ "spam_status": "marked_as_spam" }))
            .await;
        assert_eq!(resp.status_code(), 401, "PUT {path}: {}", resp.text());
        assert_eq!(resp.json::<Value>()["status"], "unverified");
    }
    let resp = server
        .put(&format!("/users/{owner}/eportfolios"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // Confirming the channel flips every route back to its normal answer.
    sqlx::query(
        "INSERT INTO communication_channels (user_id, path, path_type, position, workflow_state) \
         VALUES ($1, $2, 'email', 0, 'active')",
    )
    .bind(owner)
    .bind(format!("{prefix}-Owner@test.local"))
    .execute(&pool)
    .await
    .expect("confirm channel");
    let resp = server
        .get(&format!("/eportfolios/{eport}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "verified now: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn anonymous_requests_are_401_on_every_route() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // All 7 routes (spec §2): anonymous callers are rejected by the auth
    // extractor before any handler logic runs.
    let gets = [
        "/users/1/eportfolios",
        "/eportfolios/1",
        "/eportfolios/1/pages",
    ];
    for path in gets {
        let resp = server.get(path).await;
        assert_eq!(resp.status_code(), 401, "GET {path}");
    }
    let resp = server.delete("/eportfolios/1").await;
    assert_eq!(resp.status_code(), 401, "DELETE /eportfolios/1");
    for path in ["/eportfolios/1/restore", "/eportfolios/1/moderate"] {
        let resp = server.put(path).await;
        assert_eq!(resp.status_code(), 401, "PUT {path}");
    }
    let resp = server.put("/users/1/eportfolios").await;
    assert_eq!(resp.status_code(), 401, "PUT /users/1/eportfolios");

    // Sanity: the seeded prefix is unused here, but the harness still owns
    // its rows (none created).
    assert!(prefix.starts_with("eport_"));
    let _ = pool;
    cleanup(&pool, &prefix).await;
}
