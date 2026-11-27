---
tags: [trap, auth, parity]
---
# Trap: Canvas resolves the caller from an `access_tokens` row, not from a token's contents

**Status: FIXED.** `crates/star-auth/src/access_token.rs` resolves the bearer
token the way Canvas does: the token is hashed (HMAC-SHA1 keyed with the app
secret) and looked up in `access_tokens`; the row must be `active`, not
revoked, not expired, carry a user, and have a usable developer key.

**Why it mattered.** Canvas always knows the access token's id and the
developer key's id, so `x-canvas-meta` carries `at=` and `dk=`:

```
at=10000000000007;dk=10000000000001;o=accounts;n=index;...
```

A port that authenticates with a self-signed JWT carries no row id, so those
two fields could never be filled — 429 of the 430 recorded cases failed on
nothing else.

**The model.** Canvas' `AccessToken#authenticate`
(`app/models/access_token.rb:151-168`) looks the row up by
`crypted_token`; `AccessToken.hashed_token` (`:176-180`) is the HMAC-SHA1 of
the token keyed with the app secret; a row that fails any of
`active` / not revoked / not expired / carries a user / usable developer key
is not a caller (`lib/authentication_methods.rb:153-157`).

**The port's own session token still works** as a fallback, because
`/auth/login` hands one out and the committed integration tests use it. It is
not Canvas' model, but removing it is a separate change.

**The replay needs the row.** The corpus was recorded with access token id
**7** and developer key id **1**; the replay database must have them, or every
case answers 401:

```sh
python3 tools/parity/token_tool.py seed --secret "$STAR_JWT_SECRET" --user-id 1 --token-id 7
```

**Related.** [[canvas-meta-volatile-fields]], [[canvas-response-headers]].
