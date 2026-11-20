//! Database migrations for star-lms.
//! Embed the schema so the binary can run migrations at startup.
//!
//! `sqlx::migrate!()` snapshots the migrations/ directory at compile time.
//! build.rs emits `cargo:rerun-if-changed=migrations` so adding or editing a
//! migration recompiles this crate and the macro re-embeds the new list; the
//! 0214 moderator column initially missed CI because a warm cargo cache kept
//! the pre-0214 MIGRATOR.

use sqlx::migrate::Migrator;

pub static MIGRATOR: Migrator = sqlx::migrate!();
