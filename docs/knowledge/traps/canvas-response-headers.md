---
tags: [trap, parity, canvas]
---
# Trap: the port sets NONE of Canvas' response headers

**Status: FIXED.** The `star-canvas-response` crate now emits them. Before it,
every one of the 430 recorded cases reported a missing header; after it, only
`x-canvas-meta` still differs — and only in its `at=` and `dk=` fields.

**The rule (still true).** Canvas sets ~19 response headers on every API
response. Always present: `cache-control`, `referrer-policy`, `x-canvas-meta`,
`x-content-type-options`, `x-download-options`,
`x-permitted-cross-domain-policies`, `x-request-context-id`, `x-runtime`,
`x-session-id`, `x-xss-protection`. Usually present: `set-cookie`,
`x-rate-limit-remaining`, `x-request-cost`, `x-canvas-user-id`,
`content-security-policy`. Sometimes: `pragma`, `link` (Canvas' pagination
header — RFC 5988, NOT a `data`/`page`/`per_page`/`total` envelope).

**Where Canvas sets them.** `app/controllers/application_controller.rb`
(`set_security_headers`, `set_user_context_header`, `set_api_context`,
`set_cache_header`, `set_cors_headers`, `set_rate_limit_header`,
`set_request_context`) and `lib/request_context/generator.rb`.

**Two things the port needs to match them.**
- `STAR_CANVAS_DOMAIN` must be set to Canvas' configured domain
  (`config/domain.yml`), or the `content-security-policy` host differs and
  every case mismatches.
- `x-canvas-meta`'s `at=` and `dk=` are Canvas' access-token and
  developer-key global ids. The port authenticates with a bearer JWT, so it
  has no access-token identity and cannot fill them in. **Closing this needs
  the access-token auth model, not a header tweak.**

**The recorded cases are the spec.** `migration/rust/app-cases/http/*.toml`
records the exact headers Canvas returned, per case. Read them instead of
guessing the format.

**Related.** [[rust-errors-are-plain-text]], [[axum-json-extractor-415]],
[[canvas-access-token-auth-model]].
