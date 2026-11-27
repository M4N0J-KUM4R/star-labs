---
tags: [trap, rust, router]
---
# Trap: `star_content::router()` already merges star_pages, star_modules and star_announcements

**Symptom.** The application panics at startup on a route that belongs to
`star-pages` (`/courses/{courseId}/front_page`,
`/courses/{courseId}/pages/{url}/revisions/latest`), while every crate's own
router builds fine in isolation.

**Cause.** `crates/star-content/src/lib.rs` merges three sibling crates:

```rust
.merge(star_pages::router())
.merge(star_announcements::router())
.merge(star_modules::router())
```

`crates/star-http/src/main.rs` merged those three **again**, directly. Every route
in those crates was registered twice, so axum panicked on the first duplicate.

**How it stayed hidden.** The three crates were empty scaffolds when the direct
merges were added ("never mounted, so their endpoints answered the 404 fallback").
They later grew real routes, and the double registration became fatal. CI never
caught it because `cargo test -p <crate>` never assembles the application router.

**Fix.** Do NOT merge `star_pages`, `star_modules` or `star_announcements` in
`build_api_router()` — `star_content::router()` brings all three in.

**How to find a conflict like this.** Use the probe — it merges the routers in
order and names the crate whose merge fails:

```sh
cd migration && cargo run --bin route_conflict_probe
```

See [[axum-merge-semantics]] and [[ci-never-assembled-the-router]].
