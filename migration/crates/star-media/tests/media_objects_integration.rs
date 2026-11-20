//! Integration tests for star-media (`media_objects_api` parity).
//!
//! Pinned behavior:
//! - `POST /media_attachments` is metadata only, answers 201 with the
//!   `MediaObject` shape (`can_add_captions`, `media_sources: []`,
//!   `media_tracks: []`, `viewer_restrictions: {}`); a blank `media_id` is a
//!   422 validation error. Re-posting the same `media_id` upserts (200 vs 201
//!   is not distinguished by Canvas `first_or_initialize`).
//! - `GET /media_attachments` returns only the caller's objects, sorted by
//!   `COALESCE(user_entered_title, title)` ascending unless `sort`/`order` say
//!   otherwise; anonymous callers get 401.
//! - `PUT /media_attachments/{id}` requires the owner (or a global admin, 403
//!   otherwise), truncates to 255 chars, and answers 400 with
//!   `{"message":"The user_entered_title parameter must have a value"}` when
//!   the title is missing/blank (Canvas `update_media_object`).
//! - `GET /media_objects/{id}/media_tracks` is owner/admin only (403 for other
//!   users, 404 for an unknown media object). Default fields mirror
//!   `media_track_api_json`; `include[]=content` adds the stored SRT/VTT body.
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
        "media_{}_{}",
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

    let server = TestServer::new(star_media::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

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
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_track(
    pool: &PgPool,
    media_object_id: i64,
    user_id: i64,
    locale: &str,
    content: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO media_tracks (media_object_id, user_id, kind, locale, content)
        VALUES ($1, $2, 'subtitles', $3, $4)
        "#,
    )
    .bind(media_object_id)
    .bind(user_id)
    .bind(locale)
    .bind(content)
    .execute(pool)
    .await
    .expect("seed track");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn create_ok(server: &TestServer, bearer: &str, media_id: &str, title: &str) -> Value {
    let resp = server
        .post("/media_attachments")
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&json!({ "media_id": media_id, "media_type": "video", "title": title }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    resp.json()
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM media_tracks WHERE media_object_id IN
         (SELECT id FROM media_objects WHERE media_id LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM media_objects WHERE media_id LIKE $1")
        .bind(format!("{prefix}-%"))
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
async fn create_list_update_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let bearer = auth(owner, "student");

    // Create returns the Canvas MediaObject shape.
    let created = create_ok(
        &server,
        &bearer,
        &format!("{prefix}-m-1"),
        &format!("{prefix}-Recording"),
    )
    .await;
    assert_eq!(created["title"], format!("{prefix}-Recording"));
    assert_eq!(created["media_type"], "video");
    assert_eq!(created["can_add_captions"], true);
    assert_eq!(created["media_sources"], json!([]));
    assert_eq!(created["media_tracks"], json!([]));
    assert_eq!(created["viewer_restrictions"], json!({}));
    let id = created["id"].as_i64().unwrap();

    // A blank media_id is a validation error (422).
    let resp = server
        .post("/media_attachments")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "media_id": "  " }))
        .await;
    assert_eq!(resp.status_code(), 422);

    // Anonymous listing is rejected by the auth extractor (401).
    let resp = server.get("/media_attachments").await;
    assert_eq!(resp.status_code(), 401);

    // Listing shows the object; title falls back to `title` until overridden.
    let resp = server
        .get("/media_attachments")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], id);

    // PUT without a title is Canvas' 400 with a JSON message.
    let resp = server
        .put(&format!("/media_attachments/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "The user_entered_title parameter must have a value"
    );

    // PUT with a title updates the response title.
    let resp = server
        .put(&format!("/media_attachments/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "user_entered_title": "Edited title" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["user_entered_title"], "Edited title");
    assert_eq!(updated["title"], "Edited title");

    // Unknown ids are 404.
    let resp = server
        .put("/media_attachments/999999999")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "user_entered_title": "x" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_requires_owner_and_listing_is_scoped() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let created = create_ok(
        &server,
        &auth(owner, "student"),
        &format!("{prefix}-m-owned"),
        "Owned",
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    // Another user cannot update it.
    let resp = server
        .put(&format!("/media_attachments/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "user_entered_title": "hijack" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // They also do not see it in their own listing.
    let resp = server
        .get("/media_attachments")
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>(), json!([]));

    // Global admins pass the owner check.
    let resp = server
        .put(&format!("/media_attachments/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "user_entered_title": "admin edit" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["title"], "admin edit");

    // `order=desc` reverses the title sort.
    for (suffix, title) in [("m-a", "AAA"), ("m-z", "ZZZ")] {
        create_ok(
            &server,
            &auth(owner, "student"),
            &format!("{prefix}-{suffix}"),
            &format!("{prefix}-{title}"),
        )
        .await;
    }
    let resp = server
        .get("/media_attachments?sort=title&order=desc")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    let titles: Vec<String> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value["title"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(titles.len(), 3);
    let zzz = titles
        .iter()
        .position(|title| title.ends_with("-ZZZ"))
        .unwrap();
    let aaa = titles
        .iter()
        .position(|title| title.ends_with("-AAA"))
        .unwrap();
    assert!(zzz < aaa, "desc should put ZZZ before AAA: {titles:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn media_tracks_listing_and_include_content() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;

    let created = create_ok(
        &server,
        &auth(owner, "student"),
        &format!("{prefix}-m-tracks"),
        "Captioned",
    )
    .await;
    let id = created["id"].as_i64().unwrap();
    seed_track(
        &pool,
        id,
        owner,
        "es",
        "1\n00:00:00,000 --> 00:00:01,000\nHola",
    )
    .await;

    let url = format!("/media_objects/{id}/media_tracks");

    // Default fields mirror media_track_api_json (no content).
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tracks: Value = resp.json();
    assert_eq!(tracks.as_array().unwrap().len(), 1);
    assert_eq!(tracks[0]["locale"], "es");
    assert_eq!(tracks[0]["kind"], "subtitles");
    assert_eq!(tracks[0]["asr"], false);
    assert_eq!(tracks[0]["media_object_id"], id);
    assert!(tracks[0].get("content").is_none());

    // include[]=content adds the stored body.
    let resp = server
        .get(&format!("{url}?include%5B%5D=content"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tracks: Value = resp.json();
    assert_eq!(
        tracks[0]["content"],
        "1\n00:00:00,000 --> 00:00:01,000\nHola"
    );

    // Non-owners get 403; unknown media objects get 404.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    let resp = server
        .get("/media_objects/999999999/media_tracks")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_and_update_media_object() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let bearer_owner = auth(owner, "student");

    let created = create_ok(
        &server,
        &bearer_owner,
        &format!("{prefix}-m-show"),
        "Original Title",
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    // GET /media_objects/{id} returns the full MediaObjectJson.
    let resp = server
        .get(&format!("/media_objects/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"], id);
    assert_eq!(shown["title"], "Original Title");
    assert_eq!(shown["can_add_captions"], true);
    assert!(shown["media_tracks"].as_array().unwrap().is_empty());

    // PUT /media_objects/{id} updates user_entered_title.
    let resp = server
        .put(&format!("/media_objects/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .json(&json!({ "user_entered_title": "Updated Title" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["title"], "Updated Title");

    // PUT without title is 400 (Canvas behavior).
    let resp = server
        .put(&format!("/media_objects/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "The user_entered_title parameter must have a value"
    );

    // Non-owner gets 403.
    let resp = server
        .put(&format!("/media_objects/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "user_entered_title": "Hijack" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Admin can update.
    let resp = server
        .put(&format!("/media_objects/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "user_entered_title": "Admin Title" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["title"], "Admin Title");

    // Unknown id is 404.
    let resp = server
        .get("/media_objects/999999999")
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_and_delete_media_track() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let bearer_owner = auth(owner, "student");

    let created = create_ok(
        &server,
        &bearer_owner,
        &format!("{prefix}-m-track-crud"),
        "Track CRUD",
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    // POST /media_objects/{id}/media_tracks — create a subtitle track.
    let resp = server
        .post(&format!("/media_objects/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .json(
            &json!({ "locale": "fr", "kind": "subtitles", "content": "1\n00:00 --> 01\nBonjour" }),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let track_obj: Value = resp.json();
    let track_id = track_obj["media_tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["locale"] == "fr")
        .expect("fr track in response")["id"]
        .as_i64()
        .unwrap();

    // Invalid locale returns 422.
    let resp = server
        .post(&format!("/media_objects/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .json(&json!({ "locale": "invalid!locale", "kind": "subtitles" }))
        .await;
    assert_eq!(resp.status_code(), 422);

    // Invalid kind returns 422.
    let resp = server
        .post(&format!("/media_objects/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .json(&json!({ "locale": "de", "kind": "bogus" }))
        .await;
    assert_eq!(resp.status_code(), 422);

    // Non-owner gets 403.
    let resp = server
        .post(&format!("/media_objects/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "locale": "pt", "kind": "subtitles" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Admin can create track.
    let resp = server
        .post(&format!("/media_objects/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "locale": "pt", "kind": "captions" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // DELETE /media_objects/{id}/media_tracks/{track_id}.
    let resp = server
        .delete(&format!("/media_objects/{id}/media_tracks/{track_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let after_delete: Value = resp.json();
    let fr_track = after_delete["media_tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["locale"] == "fr");
    assert!(fr_track.is_none(), "fr track should be gone");

    // Non-owner gets 403 on delete.
    let resp = server
        .post(&format!("/media_objects/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "locale": "it", "kind": "subtitles" }))
        .await;
    let it_track_id = resp.json()["media_tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["locale"] == "it")
        .expect("it track")["id"]
        .as_i64()
        .unwrap();

    let resp = server
        .delete(&format!("/media_objects/{id}/media_tracks/{it_track_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Unknown track returns 404.
    let resp = server
        .delete(&format!("/media_objects/{id}/media_tracks/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_replace_tracks() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let bearer_owner = auth(owner, "student");

    let created = create_ok(
        &server,
        &bearer_owner,
        &format!("{prefix}-m-bulk"),
        "Bulk Replace",
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    // Seed two tracks.
    seed_track(&pool, id, owner, "en", "existing EN").await;
    seed_track(&pool, id, owner, "ja", "existing JA").await;

    // Bulk replace with two new locales — removes EN and JA, adds FR and DE.
    let resp = server
        .put(&format!("/media_objects/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .json(&json!([
            { "locale": "fr", "content": "nouveau", "kind": "subtitles" },
            { "locale": "de", "content": "neu", "kind": "subtitles" }
        ]))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tracks: Value = resp.json();
    let locales: Vec<&str> = tracks
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["locale"].as_str().unwrap())
        .collect();
    assert!(locales.contains(&"fr"), "should have fr: {locales:?}");
    assert!(locales.contains(&"de"), "should have de: {locales:?}");
    assert!(
        !locales.contains(&"en"),
        "en should be removed: {locales:?}"
    );
    assert!(
        !locales.contains(&"ja"),
        "ja should be removed: {locales:?}"
    );

    // Non-owner gets 403.
    let resp = server
        .put(&format!("/media_objects/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!([{ "locale": "zz", "content": "test" }]))
        .await;
    assert_eq!(resp.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn asr_creates_processing_track() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let bearer_owner = auth(owner, "student");

    let created = create_ok(
        &server,
        &bearer_owner,
        &format!("{prefix}-m-asr"),
        "ASR Test",
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    // POST /media_objects/{id}/asr — creates processing track.
    let resp = server
        .post(&format!("/media_objects/{id}/asr"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .json(&json!({ "locale": "en" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let after_asr: Value = resp.json();
    let asr_track = after_asr["media_tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["locale"] == "en" && t["asr"] == true);
    assert!(asr_track.is_some(), "should have ASR track: {}", after_asr);

    // Duplicate ASR request is idempotent (no new track).
    let resp = server
        .post(&format!("/media_objects/{id}/asr"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .json(&json!({ "locale": "en" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let after_dup: Value = resp.json();
    let en_tracks: Vec<_> = after_dup["media_tracks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["locale"] == "en")
        .collect();
    assert_eq!(en_tracks.len(), 1, "should still be one en track");

    // Invalid locale returns 422.
    let resp = server
        .post(&format!("/media_objects/{id}/asr"))
        .add_header(axum::http::header::AUTHORIZATION, bearer_owner.clone())
        .json(&json!({ "locale": "" }))
        .await;
    assert_eq!(resp.status_code(), 422);

    // Non-owner gets 403.
    let resp = server
        .post(&format!("/media_objects/{id}/asr"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "locale": "es" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    cleanup(&pool, &prefix).await;
}
