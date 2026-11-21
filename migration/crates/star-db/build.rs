//! Rebuild star-db whenever a sqlx migration changes.
//!
//! `sqlx::migrate!()` (src/lib.rs) embeds the migration directory at macro
//! expansion time, but cargo does not fingerprint that directory: adding or
//! editing a file under migrations/ alone does not dirty the crate, so a warm
//! target/ cache keeps serving a stale embedded MIGRATOR and new migrations
//! are never applied. Emitting `cargo:rerun-if-changed` here forces the
//! rebuild the macro needs.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
