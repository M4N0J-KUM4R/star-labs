---
tags: [trap, rust, harness]
---
# Trap: the binary is `star-http`, not `star-api`

**Trap.** `star-api` is a library crate (the shared `AppState`, the error type).
The only binary is `star-http`.

```sh
cargo build --locked --release --bin star-http
```

**Harness details that matter.**
- The port comes from `STAR_PORT`, with `PORT` accepted as a fallback — the
  replay harness starts the binary on the port it picked, and that is `PORT`.
- `DATABASE_URL` and `STAR_JWT_SECRET` are required; there is no fallback.
- The binary runs `star_db::MIGRATOR` on start, so the schema comes from the
  migrations — no separate schema step.

**Minting a token.** `crates/star-http/src/bin/mint_token.rs` uses the app's own
signing code:

```sh
STAR_JWT_SECRET=... cargo run --bin mint_token -- 1 AccountAdmin
```

See [[corpus-token-placeholder]].
