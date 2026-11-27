---
tags: [trap, parity, rust]
---
# Trap: Canvas' error body shape depends on the kind of failure

**Status: FIXED.** The shared renderer in `crates/star-common/src/lib.rs` now
produces every Canvas shape. This note keeps the shapes, because the next
route that hand-rolls an error body will get it wrong again.

**Canvas uses a different body per failure kind:**

| failure | Canvas body |
|---|---|
| 401 unauthenticated | `{"status":"unauthenticated","errors":[{"message":"user authorization required"}]}` |
| 403 unauthorized | `{"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}` |
| 404 not found | `{"errors":[{"message":"The specified resource does not exist."}]}` |
| **400 parameter validation** | **`[{"attribute":"recipients","message":"blank"}]`** — a bare ARRAY |
| 500 | `{"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}]}` |

Other shapes Canvas also uses: `{"message":"…"}`, `{"error":"…"}`,
`{"error":{"message":"…"}}`, and `{"status":"bad_request","message":"…"}`
(Canvas' `reject!`).

**Sources.** `lib/authentication_methods.rb:414-428` (401/403),
`application_controller.rb:2296` (404), `:2322` (500),
`conversations_controller.rb:1149-1156` (the bare-array 400),
`lib/request_error.rb:28-33` (`reject!`).

**The rule.** Every crate builds its errors through the shared renderer —
never a hand-rolled `json!`. The recorded corpus is the contract: the body is
compared byte for byte, so the shape and the message text both matter.

**Watch the key order** — see [[serde-json-sorts-keys]].

**Related.** [[canvas-response-headers]], [[axum-json-extractor-415]].
