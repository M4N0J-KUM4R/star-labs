---
tags: [trap, cutover, axum, proxy]
---
# Trap: the front door must forward the path the request came in with

**Status: FIXED** in `crates/star-proxy` (`OriginalUri`).

axum's `Router::nest("/api/v1", …)` **strips the prefix** before the
nested router runs — and the nested router's fallback is inside it. So a
handler that reads `parts.uri.path()` sees `/courses/1`, not
`/api/v1/courses/1`.

The port's router nests its API router under `/api/v1`, because Canvas
serves its API there. The delegation therefore has to use the URI the
request came in with, or Canvas is asked for a path it does not own and
answers 404 for a route it serves perfectly well.

**The fix.** `axum::extract::OriginalUri` carries the URI as it entered the
router, before any nesting rewrote it:

```rust
let original = parts
    .extensions
    .get::<OriginalUri>()
    .map(|original| original.0.clone())
    .unwrap_or_else(|| parts.uri.clone());
let path = original.path().to_string();
```

`OriginalUri` is inserted by axum's `PathRouter` on the first router that
sees the request, and only when it is not already in the extensions — so the
outer router's copy survives the nesting. The feature is on by default in
axum 0.8 (`original-uri`).

**How it showed up.** The CI front-door gate
(`tools/parity/front_door_check.py`) started a mock upstream, asked the port
for a path it does not own, and asserted that the upstream received
`/api/v1/front_door_probe/does-not-exist`. The upstream received
`/front_door_probe/does-not-exist` — the prefix was gone. The gate was
right and the proxy was wrong: a cutover that delegated `/courses/1` to
Canvas would have broken every unowned route in production.

**The same trap has a second half.** The gate read the upstream's marker
header out of `dict(response.headers)`, a plain dict, so the lookup was
case-sensitive — and HTTP header names are not. hyper lowercases the names
it forwards, so `X-Upstream-Marker` arrives as `x-upstream-marker` and the
case-sensitive lookup missed it. Read headers out of the
`email.message.Message` the HTTP client hands back; it is case-insensitive.
