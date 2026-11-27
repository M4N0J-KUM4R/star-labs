---
tags: [decision, rust, axum]
---
# Decision: chain same-path registrations, never delete a Canvas method (2026-11-27)

**Decision.** When one Canvas path is registered more than once in a router,
chain the methods into a single `.route(path, get(a).put(b).delete(c))` call.
Delete a registration only when the SAME method is registered twice — and then
only after checking which implementation matches the Canvas controller.

**Why.** Canvas routes are dense, and two workers porting the same route
routinely split it into two registrations. A blanket "delete the duplicate"
drops a Canvas method silently — the port answers 404 where Canvas answers 200,
and nothing in the test suite notices.

Chaining is also correct where axum would have merged silently
(see [[axum-merge-semantics]]): one registration per path removes the
same-matchit-shape hazard, makes the method set explicit, and reads like the
Canvas route list.

**How the Canvas-faithful one is chosen.** The doc comment above each handler
cites the Canvas controller and action. Check it against
`canvas-src/config/routes.rb`, then confirm the response body matches the Canvas
serializer. `docs/knowledge/facts/route-map.md` gives the route → crate → Canvas
file mapping.
