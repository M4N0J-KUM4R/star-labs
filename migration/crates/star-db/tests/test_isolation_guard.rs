//! Test-isolation guard (issue #75).
//!
//! Static source scan over every crate's integration tests, enforcing the
//! dedicated-account convention so parallel test binaries cannot contaminate
//! each other through the shared root account 1 (seeded by migration 0024;
//! `courses.account_id` DEFAULTs to 1 since migration 0030):
//!
//! 1. `[account-1 seed]` — no `INSERT INTO accounts ... VALUES (1, ...)`.
//!    Seed a dedicated account instead (`INSERT INTO accounts (name) ...`).
//! 2. `[accounts/1 path]` — no literal `/accounts/1` request paths. Use a
//!    dedicated seeded account, or a sentinel id (999999999) for
//!    unauthenticated 401/404 probes where the account is never looked up.
//!    Prefixed API paths (`/api/v1/accounts/1`, `/api/lti/accounts/1`) are
//!    out of scope for this rule.
//! 3. `[root-account flag]` — no file may both write `feature_flags` rows
//!    and grab the globally-first root account (`parent_account_id IS
//!    NULL`): flags must target a dedicated account, never account 1.
//!
//! The convention and the canonical helpers are documented under "Test
//! isolation" in `migration/CLAUDE.md`. This test needs no DATABASE_URL; it
//! fails on the source text alone.

use std::fs;
use std::path::{Path, PathBuf};

/// The guard scans its own sources for the banned patterns, so it must skip
/// itself (this file legitimately contains them inside the matchers).
fn is_guard_itself(path: &Path) -> bool {
    path.ends_with("star-db/tests/test_isolation_guard.rs")
}

fn workspace_crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates")
        .canonicalize()
        .expect("canonicalize crates dir")
}

/// Every `crates/*/tests/*.rs`, sorted for stable failure output.
fn test_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries = fs::read_dir(workspace_crates_dir()).expect("read crates dir");
    for crate_entry in entries.flatten() {
        let tests_dir = crate_entry.path().join("tests");
        let Ok(test_entries) = fs::read_dir(&tests_dir) else {
            continue;
        };
        for file_entry in test_entries.flatten() {
            let path = file_entry.path();
            if path.extension().is_some_and(|ext| ext == "rs") && !is_guard_itself(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Rule 1: `INSERT INTO accounts ... VALUES (1, ...)` — a literal seed of
/// the shared root account.
fn account_one_seed_offset(lower: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(found) = lower[search_from..].find("insert into accounts") {
        let start = search_from + found;
        let window_end = (start + 300).min(lower.len());
        let window = &lower[start..window_end];
        let Some(values_at) = window.find("values") else {
            search_from = start + 1;
            continue;
        };
        let after_values = window[values_at..]
            .trim_start_matches("values")
            .trim_start();
        if let Some(rest) = after_values.strip_prefix('(') {
            let rest = rest.trim_start();
            if rest.starts_with("1,") || rest.starts_with("1)") || rest == "1" {
                return Some(start);
            }
        }
        search_from = start + 1;
    }
    None
}

/// 1-based line number of a byte offset.
fn line_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].lines().count()
}

fn relative(path: &Path) -> String {
    let base = workspace_crates_dir();
    path.strip_prefix(base.parent().unwrap_or(path))
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[test]
fn no_account_one_seeds_or_paths_or_root_account_flags() {
    let mut violations = Vec::new();
    for path in test_files() {
        let Ok(src) = fs::read_to_string(&path) else {
            violations.push(format!("{}: [io] cannot read test source", relative(&path)));
            continue;
        };
        let lower = src.to_lowercase();

        // Rule 1: literal account-1 seeds.
        if let Some(offset) = account_one_seed_offset(&lower) {
            violations.push(format!(
                "{}:{}: [account-1 seed] INSERT INTO accounts with literal id 1 — \
                 seed a dedicated account instead (issue #75, see \"Test isolation\" \
                 in CLAUDE.md)",
                relative(&path),
                line_of(&src, offset)
            ));
        }

        // Rule 2: literal "/accounts/1" request paths (any verb). The quote
        // anchor keeps prefixed API paths (/api/v1/..., /api/lti/...) out.
        for (idx, line) in src.lines().enumerate() {
            if line.contains("\"/accounts/1") {
                violations.push(format!(
                    "{}:{}: [accounts/1 path] literal /accounts/1 path in {:?} — \
                     use a dedicated seeded account, or the 999999999 sentinel \
                     for unauthenticated 401/404 probes (issue #75)",
                    relative(&path),
                    idx + 1,
                    line.trim()
                ));
            }
        }

        // Rule 3: feature_flags writes in a file that also grabs the
        // globally-first root account.
        if lower.contains("insert into feature_flags")
            && lower.contains("parent_account_id is null")
        {
            violations.push(format!(
                "{}: [root-account flag] file writes feature_flags rows AND looks up \
                 the globally-first root account (parent_account_id IS NULL) — flags \
                 must target a dedicated account (issue #75)",
                relative(&path)
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "test-isolation guard: {} violation(s):\n  {}\n\
         Convention: dedicated per-test accounts, feature_flags cleanup, no \
         account-1 seeds — see \"Test isolation\" in migration/CLAUDE.md.",
        violations.len(),
        violations.join("\n  ")
    );
}
