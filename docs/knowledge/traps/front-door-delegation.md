---
tags: [trap, cutover, axum, proxy]
---
# Trap: the port's own router is what the front door delegates on

**Status: FIXED.** `crates/star-proxy` is the front door. It answers two
cases and nothing else:

| the request | who answers it |
|---|---|
| matches a route the port registers | the port |
| matches no route at all | Canvas (axum's fallback) |
| matches a path the port knows but not that method | Canvas (axum's method-not-allowed fallback) |

**What went wrong if you only use the fallback.** axum answers a known path
with an unknown method with **405 Method Not Allowed**. Canvas answers the
same request with **404** — Rails' routing error, not a method error. So a
port that owns `GET /api/v1/courses/:id` and not `DELETE` would answer 405
where Canvas answers 404. `Router::method_not_allowed_fallback` is what
closes that gap; set it to the same handler as `Router::fallback`.

**Delegation is off unless `STAR_CANVAS_ORIGIN` is set.** With no origin the
fallback is the *same empty 404* axum's own fallback returns — no body, no
content type. That is deliberate: the parity replay and CI must see only the
port's own answers, or the recorded corpus stops measuring the port. A
front door that changes the measured behaviour is a front door nobody can
trust. Turn it on with one environment variable, so the rollback is one
variable and not a redeploy.

**Forwarding rules that are easy to get wrong.**
- Do **not** follow redirects. Canvas' 302 and its `Location` header belong
  to the client; a proxy that follows them answers with the page behind the
  redirect and the client never learns it was redirected.
- `Set-Cookie` repeats. Append it, do not insert it — one `Set-Cookie` per
  `insert` loses every cookie but the last.
- Drop the hop-by-hop headers (`Connection`, `Transfer-Encoding`, `Keep-Alive`,
  `Upgrade`, the `Proxy-*` family, `TE`, `Trailer`) and `Host`: they describe
  one connection, not the message. Drop `Content-Length` on the way back and
  let the body carry it.
- Drop `Accept-Encoding` on the way out. The client asked for a compressed
  answer; the proxy answers with the bytes it has.

**Related.** [[decisions/cutover-front-door]], [[canvas-response-headers]],
[[star-http-is-the-only-binary]].
