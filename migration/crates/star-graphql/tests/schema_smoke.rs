//! Schema-build smoke test: constructs the full GraphQL schema (Query +
//! Mutation roots) without touching the database. This is the exact code
//! path that panicked in CI (`registry::create_type` duplicate-name
//! check), so a local green run proves the duplicate-type fix without
//! needing a Postgres.
//!
//! The schema is built lazily in the router (`Schema::build(...).finish()`),
//! and `.finish()` runs async-graphql's registry pass — duplicates abort
//! there. No DATABASE_URL required: the schema shape is DB-independent.

use star_common::AppState;

#[tokio::test]
async fn schema_builds_without_duplicate_type_names() {
    let state = AppState {
        db: sqlx::postgres::PgPool::connect_lazy("postgres://invalid:invalid@localhost:1/invalid")
            .expect("lazy connect never dials"),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let app: axum::Router = axum::Router::new()
        .nest("/api/graphql", star_graphql::router())
        .with_state(state);
    // Dropping proves construction (and Schema::build inside the router)
    // completed — a duplicate type name would have panicked inside router().
    drop(app);
}
