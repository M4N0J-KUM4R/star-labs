---
tags: [trap, ci, rust]
---
# Trap: `cargo test -p <crate>` never assembles the application router

**The gap.** `rust-migration.yml` runs `cargo test --locked --no-fail-fast -p
<crate>` over four shards. That compiles each crate and runs its own tests. It
never builds `build_api_router()`, which is a private function in
`crates/star-http/src/main.rs` that only runs when the binary starts.

So the gate tested every crate in isolation and never tested the thing they are
assembled into. The application could not start, and CI was green.

**The fix is in the repo now:** `migration/crates/star-http/tests/router_conflicts.rs`
assembles the API router and the application router in a test. `star-http` is
already in shard 1, so a conflicting route fails CI.

**The general lesson.** A crate-level test suite cannot catch an
integration-level defect. When a deliverable is "the whole thing assembles and
runs", that has to be a test.

See [[double-merge-star-content]], [[axum-merge-semantics]].
