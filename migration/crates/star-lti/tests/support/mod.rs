//! Shared test-support helpers for the star-lti integration tests.
//!
//! This module lives in a subdirectory on purpose: the test-isolation guard
//! (`star-db/tests/test_isolation_guard.rs`) scans only the top-level
//! `tests/*.rs` binaries, and its `[root-account flag]` rule — "no file may
//! both write `feature_flags` rows and grab the globally-first root
//! account" — would false-positive on a test binary that needs BOTH a
//! feature flag on its own dedicated account AND the workspace site-admin
//! id. The helpers here never write anything: they only READ the
//! workspace's site-admin account (the first parent-less account, exactly
//! how the production code resolves `Account.site_admin`), which the
//! template/bind semantics of `registrations_controller.rb` require.

use sqlx::PgPool;

/// The workspace's Site Admin account — the first parent-less account,
/// mirroring the production lookup (`Account.site_admin`). Read-only: tests
/// use the id to place template registrations, never to write flags.
pub async fn site_admin_id(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .expect("site admin account")
}
