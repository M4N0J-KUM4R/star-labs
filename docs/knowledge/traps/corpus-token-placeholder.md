---
tags: [trap, harness, security]
---
# Trap: the recorded corpus must never contain a live token

**The rule.** `migration/rust/app-cases/http/*.toml` stores the request that was
recorded, headers included. The committed corpus therefore carries
`Authorization = "Bearer __REPLAY_TOKEN__"` — a placeholder, never a token.

**Why.** The recorder writes what it sent. If you record with a live Canvas
access token, all 430 case files contain that token, and it is about to be
committed.

**The replay.** `tools/parity/token_tool.py` substitutes a real token for the
placeholder, runs the replay, and puts the placeholder back:

```sh
python3 tools/parity/token_tool.py inject rust/app-cases/http --secret "$STAR_JWT_SECRET"
rust-anything app replay . --build "target/debug/star-http"
python3 tools/parity/token_tool.py restore rust/app-cases/http
```

**Mint the token with the server's own code.** `crates/star-http/src/bin/mint_token.rs`
calls `star_auth::create_token` — the same function the app uses. A hand-rolled
JWT outside the app is a needless way to get a 401 on every route.

**Check before committing:**
`grep -rl "Bearer eyJ" migration/rust/` must return nothing.
