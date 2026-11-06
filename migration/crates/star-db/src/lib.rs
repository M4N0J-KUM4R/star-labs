//! Database migrations for star-lms.
//! Embed the schema so the binary can run migrations at startup.

use sqlx::migrate::Migrator;

pub static MIGRATOR: Migrator = sqlx::migrate!();
