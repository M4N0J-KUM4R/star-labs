---
tags: [trap, rust, parity]
---
# Trap: `serde_json::Map` sorts its keys alphabetically

**Symptom.** The port answers `{"errors":[{"message":"…"}],"status":"unauthorized"}`
where Canvas answers `{"status":"unauthorized","errors":[{"message":"…"}]}`.

The bodies are the same fields; the **order** is wrong. The replay compares
the JSON text, so the order is a mismatch.

**Cause.** `serde_json::Map` is a `BTreeMap` by default — its `Serialize`
impl emits the keys in sorted order. Anything built as a
`serde_json::Value` therefore comes out alphabetical, whatever order you
wrote it in.

**Fix.** Do not build the body as a `Value`. Make the renderer generic over
`serde::Serialize` and pass a `#[derive(Serialize)]` struct — the derive
emits the fields in **declaration** order:

```rust
pub fn canvas_json<T: Serialize>(status: StatusCode, body: &T) -> Response { … }
```

**Where it bit.** `crates/star-rubrics/src/rubrics_api.rs` built its 403 body
by hand; `crates/star-outcomes/src/outcomes_api.rs` did the same for three
error kinds. Both now go through the shared renderer.

**Related.** [[rust-errors-are-plain-text]].
