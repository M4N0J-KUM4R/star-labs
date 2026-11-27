---
tags: [trap, parity, rust, axum]
---
# Trap: axum's `Json<T>` answers 415 before the handler's permission check runs

**Status: FIXED for the core crates.** `ApiJson<T>` in
`crates/star-common/src/lib.rs` accepts a body-less, form-encoded or JSON
request and parses it the way Canvas parses `params[:xyz]`, so the handler runs
and produces Canvas' status.

**What went wrong.** `Json<T>` as an extractor rejects a request with no
`Content-Type: application/json` with **415 Unsupported Media Type** — and it
does so *before* the handler body runs, so the permission check that would have
produced Canvas' 403 never happens. 45 recorded cases showed `403→415`.

**The rule.** When Canvas accepts a request without a JSON content type, the
port must too. Take `ApiJson<T>`, not `Json<T>`, for those handlers. The
recorded corpus tells you which: a case whose Canvas status is 400/403/404
while the port answered 415.

**Source.** Canvas parses `params[:xyz]` and answers its own validation error
(`lib/api.rb`, `lib/request_error.rb:28-33`); a content-type mismatch is not
a 415 there.

**Related.** [[rust-errors-are-plain-text]], [[canvas-permission-order]].
