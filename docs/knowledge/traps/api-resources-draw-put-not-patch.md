---
tags: [trap, routes, canvas-semantics]
---
# Trap: Canvas' API `resources` draws PUT, never PATCH

**Status: KNOWN DIVERGENCE** — the port registers 9 API routes Canvas does
not serve.

Canvas' API route set has its own `resources` helper
(`lib/api_route_set.rb:92-107`), and it is not Rails':

```ruby
put("#{path}/:#{resource_name.singularize}_id", opts.merge(action: :update)) if maybe_action[:update]
delete("#{path}/:#{resource_name.singularize}_id", opts.merge(action: :destroy)) if maybe_action[:destroy]
```

Rails' `resources` draws **PATCH and PUT** for `update`; Canvas' draws only
**PUT**. The API set has a `patch` method (`lib/api_route_set.rb:88`), but
no route in the API block uses it — the recorded API surface
(`migration/parity/canvas-api-surface.json`) lists 106 PATCH routes and
**every one of them is `api: false`**, a web route.

So `PATCH /api/v1/group_categories/1` is not a route in Canvas. The
recording says so: the corpus holds two PATCH cases and both answered
**404 with `content-type: text/html` and an empty body** — Rails' routing
error, not a controller's answer:

```
PATCH /api/v1/group_categories/1   404  text/html
PATCH /api/v1/groups/1              404  text/html
```

The port registers PATCH on nine API routes — `/group_categories/{id}`,
`/groups/{id}`, `/groups/{group_id}/memberships/{id}`,
`/groups/{group_id}/users/{id}`,
`/courses/{courseId}/assignment_groups/{id}`, the grading-period and
gradebook routes, and the two pacing routes. Each answers where Canvas
answers a routing 404.

**What removing them buys.** In a cutover the front door delegates the
routes the port does not own, so an unowned PATCH would be asked of
Canvas and Canvas would answer its 404. With the route registered, the
port answers it itself. The two shapes differ (the port's 404 is JSON,
Rails' routing error is HTML with an empty body), so the recorded cases
would still not match — but the port would no longer answer a route
Canvas does not serve.

**Why it is not done yet.** Eighteen test calls across
`star-groups`, `star-gradebook`, `star-grading-standards` and
`star-modules` use PATCH; they have to move to PUT or assert the routing
404, and the port's tests run in CI only (the project's rule), so each
attempt costs a CI cycle. The two PATCH routes under `star-ui` are the
HTML form routes — Canvas' **web** routes do have PATCH, and those are
correct.

**The check.** `migration/parity/canvas-api-surface.json` lists the API
routes with an `api` flag; a route the port registers under `/api/v1`
whose verb is PATCH is a divergence, because the API set cannot draw one.
