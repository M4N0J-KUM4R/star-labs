//! Integration tests for the Phase 5 account-extras routes in star-accounts:
//!
//! - GET    /accounts/{id}/terms_of_service
//! - GET    /accounts/{id}/help_links
//! - GET    /accounts/{id}/permissions
//! - DELETE /accounts/{id}/sub_accounts/{subAccountId}
//! - PUT    /accounts/{accountId}/account_notifications/{id}
//! - GET    /accounts/{accountId}/account_notifications/{id}
//! - GET    /accounts/{accountId}/admins/self
//!
//! Conventions mirror `tests/account_notifications_admins.rs`: skipped cleanly
//! when DATABASE_URL is unset, unique per-run prefix, full teardown.
//!
//! Behavior pinned (Canvas parity):
//! - terms_of_service reads the ROOT account's policy
//!   (`account_terms_of_service`, migration 0089) and falls back to the Canvas
//!   default shape (id 0, terms_type "default") when no row exists.
//! - help_links returns the static Canvas default list with an empty
//!   custom_help_links array.
//! - permissions maps each requested permission name (or the default set) to
//!   the account-admin gate for the requesting user.
//! - Sub-account deletion refuses the root account (401), sub-accounts with
//!   courses (409) or sub-accounts (409) and deletes only empty leaf
//!   sub-accounts, echoing the former account JSON.
//! - Notification update merges partial payloads (Canvas update semantics) and
//!   replaces the role rows when the request carries a role list; show is
//!   scoped to the owning account (404 otherwise).
//! - admins/self lists the caller's own account_users rows and must not
//!   shadow the parameterized /admins/{userId} route (DELETE still works).

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "acc-p5-{}-{}-{}",
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
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(star_accounts_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn star_accounts_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_accounts::router())
        .with_state(state)
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role, sortable_name)
        VALUES ($1, $2, $3, $4, $1)
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

async fn seed_account(pool: &PgPool, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO accounts (name, parent_account_id, workflow_state)
        VALUES ($1, $2, 'active')
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code, account_id)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // courses restrict account deletion (no ON DELETE clause), so they go
    // first; account_notifications/account_users/account_terms_of_service
    // cascade from accounts, which cascade nothing that outlives the prefix.
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

/// POST a valid notification and return its id.
async fn create_notification(server: &TestServer, account_id: i64, token: &str, icon: &str) -> i64 {
    let resp = server
        .post(&format!("/accounts/{account_id}/account_notifications"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({
            "subject": "Original subject",
            "message": "Original message",
            "start_at": "2026-01-01T00:00:00Z",
            "end_at": "2026-02-01T00:00:00Z",
            "icon": icon,
            "account_notification_roles": ["StudentEnrollment"]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    body["id"].as_i64().expect("notification id")
}

// ---------------------------------------------------------------------------
// GET /accounts/{id}/terms_of_service
// ---------------------------------------------------------------------------

#[tokio::test]
async fn terms_of_service_shape_and_unknown_account() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let root = seed_account(&pool, &format!("{prefix}-Root"), None).await;
    let sub = seed_account(&pool, &format!("{prefix}-Sub"), Some(root)).await;
    let bare = seed_account(&pool, &format!("{prefix}-Bare"), None).await;
    let token = auth(admin, "admin");

    // Canvas reads the root account's policy.
    let tos_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO account_terms_of_service (account_id, terms_type, content)
        VALUES ($1, 'custom', 'Be kind to your classmates')
        RETURNING id
        "#,
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();

    let resp = server
        .get(&format!("/accounts/{root}/terms_of_service"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tos: Value = resp.json();
    assert_eq!(tos["id"].as_i64(), Some(tos_id));
    assert_eq!(tos["terms_type"], "custom");
    assert_eq!(tos["content"], "Be kind to your classmates");
    assert_eq!(tos["passive"], false);
    assert_eq!(tos["account_id"].as_i64(), Some(root));
    assert!(tos["self_registration_type"].is_null(), "body: {tos}");

    // A sub-account resolves to the root account's policy.
    let resp = server
        .get(&format!("/accounts/{sub}/terms_of_service"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let sub_tos: Value = resp.json();
    assert_eq!(sub_tos["id"].as_i64(), Some(tos_id));
    assert_eq!(sub_tos["account_id"].as_i64(), Some(root));

    // Without a stored policy the Canvas default shape is returned.
    let resp = server
        .get(&format!("/accounts/{bare}/terms_of_service"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let default: Value = resp.json();
    assert_eq!(default["id"].as_i64(), Some(0));
    assert_eq!(default["terms_type"], "default");
    assert!(default["content"].is_null(), "body: {default}");
    assert_eq!(default["passive"], false);

    // Non-admins are rejected; unknown accounts 404 for admins.
    let resp = server
        .get(&format!("/accounts/{root}/terms_of_service"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .get("/accounts/999999999/terms_of_service")
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{id}/help_links
// ---------------------------------------------------------------------------

#[tokio::test]
async fn help_links_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    let token = auth(admin, "admin");

    let resp = server
        .get(&format!("/accounts/{account}/help_links"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let links: Value = resp.json();
    assert_eq!(links["help_link_name"], "Help");
    assert_eq!(links["help_link_icon"], "help");
    assert_eq!(links["custom_help_links"], json!([]));
    let defaults = links["default_help_links"].as_array().expect("array");
    assert_eq!(defaults.len(), 3, "body: {links}");
    for link in defaults {
        assert!(link["id"].is_string(), "body: {links}");
        assert!(link["text"].is_string(), "body: {links}");
        assert!(link["subtext"].is_string(), "body: {links}");
        assert!(link["url"].is_string(), "body: {links}");
        assert_eq!(link["type"], "default");
        assert!(link["available_to"].as_array().is_some(), "body: {links}");
    }

    // Non-admins are rejected; unknown accounts 404 for admins.
    let resp = server
        .get(&format!("/accounts/{account}/help_links"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .get("/accounts/999999999/help_links")
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{id}/permissions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permissions_map_for_admin_and_non_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    let token = auth(admin, "admin");

    // The default map covers the granted rights and is all-true for admins.
    let resp = server
        .get(&format!("/accounts/{account}/permissions"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let admin_map: Value = resp.json();
    let defaults = [
        "manage_account_memberships",
        "manage_courses_admin",
        "create_courses",
        "manage_admin_roles",
        "manage_alerts",
        "manage_sis",
        "manage_settings",
        "manage_feature_flags",
        "view_statistics",
        "become_user",
    ];
    assert_eq!(
        admin_map.as_object().map(|m| m.len()),
        Some(defaults.len()),
        "body: {admin_map}"
    );
    for name in defaults {
        assert_eq!(admin_map[name], true, "body: {admin_map}");
    }

    // A plain teacher gets the same names mapped to false.
    let resp = server
        .get(&format!("/accounts/{account}/permissions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let teacher_map: Value = resp.json();
    assert_eq!(teacher_map.as_object().map(|m| m.len()), Some(10));
    for name in defaults {
        assert_eq!(teacher_map[name], false, "body: {teacher_map}");
    }

    // A requested subset is returned as-is (repeated permissions[] params).
    let resp = server
        .get(&format!("/accounts/{account}/permissions"))
        .add_query_params([
            ("permissions[]", "manage_sis"),
            ("permissions[]", "become_user"),
        ])
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let subset: Value = resp.json();
    assert_eq!(subset, json!({ "manage_sis": true, "become_user": true }));

    // The comma-separated form is accepted too.
    let resp = server
        .get(&format!("/accounts/{account}/permissions"))
        .add_query_params([("permissions", "manage_sis,become_user")])
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let csv: Value = resp.json();
    assert_eq!(csv, json!({ "manage_sis": false, "become_user": false }));

    // Unknown accounts 404.
    let resp = server
        .get("/accounts/999999999/permissions")
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /accounts/{id}/sub_accounts/{subAccountId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sub_account_delete_routes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let root = seed_account(&pool, &format!("{prefix}-Root"), None).await;
    let empty = seed_account(&pool, &format!("{prefix}-Empty"), Some(root)).await;
    let with_course = seed_account(&pool, &format!("{prefix}-Courses"), Some(root)).await;
    let with_subs = seed_account(&pool, &format!("{prefix}-Parent"), Some(root)).await;
    let leaf = seed_account(&pool, &format!("{prefix}-Leaf"), Some(with_subs)).await;
    let other_root = seed_account(&pool, &format!("{prefix}-Other"), None).await;
    let other_sub = seed_account(&pool, &format!("{prefix}-OtherSub"), Some(other_root)).await;
    let token = auth(admin, "admin");

    seed_course(&pool, &prefix, "hold", with_course).await;

    // Non-admins cannot delete.
    let resp = server
        .delete(&format!("/accounts/{root}/sub_accounts/{empty}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A root account is refused (Canvas unauthorized).
    let resp = server
        .delete(&format!("/accounts/{root}/sub_accounts/{root}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // Sub-accounts holding courses conflict.
    let resp = server
        .delete(&format!("/accounts/{root}/sub_accounts/{with_course}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());

    // Sub-accounts holding sub-accounts conflict.
    let resp = server
        .delete(&format!("/accounts/{root}/sub_accounts/{with_subs}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());

    // A sub-account outside the target account's subtree is 404.
    let resp = server
        .delete(&format!("/accounts/{root}/sub_accounts/{other_sub}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unknown sub-account is 404.
    let resp = server
        .delete(&format!("/accounts/{root}/sub_accounts/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // An empty leaf sub-account is deleted and echoed in account JSON shape.
    let resp = server
        .delete(&format!("/accounts/{root}/sub_accounts/{empty}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["id"].as_i64(), Some(empty));
    assert_eq!(deleted["name"], format!("{prefix}-Empty"));
    assert_eq!(deleted["parent_account_id"].as_i64(), Some(root));
    assert_eq!(deleted["root_account_id"].as_i64(), Some(root));
    assert_eq!(deleted["workflow_state"], "active");

    let (remaining,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts WHERE id = ANY($1)")
        .bind(vec![empty, with_course, with_subs, leaf, other_sub])
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 4, "only the empty leaf was deleted");

    // A deeper ancestor works too: deleting via the mid-level account removes
    // its own empty sub-account.
    let resp = server
        .delete(&format!("/accounts/{with_subs}/sub_accounts/{leaf}"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let (leaf_gone,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts WHERE id = $1")
        .bind(leaf)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(leaf_gone, 0);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT/GET /accounts/{accountId}/account_notifications/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_notification_update_and_show_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    let other = seed_account(&pool, &format!("{prefix}-Other"), None).await;
    let token = auth(admin, "admin");

    let notification = create_notification(&server, account, &token, "warning").await;

    // Full update replaces subject/message/window/icon and the role rows.
    let resp = server
        .put(&format!(
            "/accounts/{account}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({
            "subject": "Updated subject",
            "message": "Updated message",
            "start_at": "2026-01-02T00:00:00Z",
            "end_at": "2026-03-01T00:00:00Z",
            "icon": "error",
            "account_notification_roles": ["TeacherEnrollment"]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["id"].as_i64(), Some(notification));
    assert_eq!(updated["subject"], "Updated subject");
    assert_eq!(updated["message"], "Updated message");
    assert_eq!(updated["icon"], "error");
    assert_eq!(updated["start_at"], "2026-01-02T00:00:00Z");
    assert_eq!(updated["end_at"], "2026-03-01T00:00:00Z");
    assert_eq!(updated["roles"], json!(["TeacherEnrollment"]));
    assert_eq!(updated["role_ids"], json!([null]));
    // Update serializes author as nil (display_author false).
    assert!(updated["author"].is_null(), "body: {updated}");

    let (roles,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM account_notification_roles WHERE account_notification_id = $1",
    )
    .bind(notification)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(roles, 1, "role rows replaced");

    // Partial update keeps the untouched columns.
    let resp = server
        .put(&format!(
            "/accounts/{account}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "subject": "Patched subject" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let patched: Value = resp.json();
    assert_eq!(patched["subject"], "Patched subject");
    assert_eq!(patched["message"], "Updated message");
    assert_eq!(patched["icon"], "error");
    assert_eq!(patched["roles"], json!(["TeacherEnrollment"]));

    // The deprecated pub_icon alias still lands in icon.
    let resp = server
        .put(&format!(
            "/accounts/{account}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "pub_icon": "information" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let aliased: Value = resp.json();
    assert_eq!(aliased["icon"], "information");

    // An empty role list clears the roles.
    let resp = server
        .put(&format!(
            "/accounts/{account}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "account_notification_roles": [] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let cleared: Value = resp.json();
    assert_eq!(cleared["roles"], json!([]));
    assert_eq!(cleared["role_ids"], json!([]));

    // Canvas renders model validation errors as 400.
    let resp = server
        .put(&format!(
            "/accounts/{account}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({
            "start_at": "2026-05-01T00:00:00Z",
            "end_at": "2026-04-01T00:00:00Z"
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // GET show round-trips the persisted state.
    let resp = server
        .get(&format!(
            "/accounts/{account}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"].as_i64(), Some(notification));
    assert_eq!(shown["subject"], "Patched subject");
    assert_eq!(shown["message"], "Updated message");
    assert_eq!(shown["icon"], "information");
    assert_eq!(shown["roles"], json!([]));

    // Show/update are scoped: another account's path is 404, unknown ids too.
    let resp = server
        .get(&format!(
            "/accounts/{other}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .put(&format!(
            "/accounts/{other}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "subject": "stolen" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/accounts/{account}/account_notifications/999999999"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .put(&format!(
            "/accounts/{account}/account_notifications/999999999"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "subject": "ghost" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Non-admins cannot update.
    let resp = server
        .put(&format!(
            "/accounts/{account}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "subject": "nope" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{accountId}/admins/self
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admins_self_returns_own_roles_and_plain_route_still_works() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let target = seed_user(&pool, &prefix, "Target", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    let other = seed_account(&pool, &format!("{prefix}-Other"), None).await;
    let token = auth(admin, "admin");

    // Grant target as account admin (Canvas AdminsController#create).
    let resp = server
        .post(&format!("/accounts/{account}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "user_id": target }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    let grant_id = created["id"].as_i64().expect("admin grant id");

    // The grant holder sees exactly their own roles on the account.
    let resp = server
        .get(&format!("/accounts/{account}/admins/self"))
        .add_header(axum::http::header::AUTHORIZATION, auth(target, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let self_rows: Value = resp.json();
    let rows = self_rows.as_array().expect("bare array");
    assert_eq!(rows.len(), 1, "body: {self_rows}");
    assert_eq!(rows[0]["id"].as_i64(), Some(grant_id));
    assert_eq!(rows[0]["role"], "AccountAdmin");
    assert!(rows[0]["role_id"].is_null(), "body: {self_rows}");
    assert_eq!(rows[0]["workflow_state"], "active");
    assert_eq!(rows[0]["user"]["id"].as_i64(), Some(target));
    assert_eq!(rows[0]["user"]["name"], "Target");

    // A global admin without an account_users row gets an empty list.
    let resp = server
        .get(&format!("/accounts/{account}/admins/self"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!([]));

    // Account admins of one account cannot read another account's self list.
    let resp = server
        .get(&format!("/accounts/{other}/admins/self"))
        .add_header(axum::http::header::AUTHORIZATION, auth(target, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Plain users are rejected; unknown accounts 404.
    let resp = server
        .get(&format!("/accounts/{account}/admins/self"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .get("/accounts/999999999/admins/self")
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The literal `self` segment did not shadow the parameterized route:
    // DELETE /admins/{userId} still resolves and removes the grant.
    let resp = server
        .delete(&format!("/accounts/{account}/admins/{target}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let removed: Value = resp.json();
    assert_eq!(removed["user"]["id"].as_i64(), Some(target));
    assert_eq!(removed["workflow_state"], "deleted");

    let resp = server
        .get(&format!("/accounts/{account}/admins/self"))
        .add_header(axum::http::header::AUTHORIZATION, auth(target, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
