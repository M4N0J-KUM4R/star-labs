---
tags: [trap, auth, parity]
---
# Trap: a route that ends the caller's sessions must not end the replay's own token

**Symptom.** `DELETE /users/{id}/sessions` answered 200 in the port and 403 in
Canvas, and then **every later case answered 401** (30 recorded cases).

**Cause.** The port executed the session termination. The replay's own bearer
token was one of the sessions it killed, so the rest of the run had no
caller.

**Canvas's rule.** `UsersController#terminate_sessions` runs
`authorized_action(user, me, :terminate_sessions)` — which requires
`manage_user_logins` on a **root account** the target belongs to
(`app/models/user.rb:1529`, `permissions_registry.rb:500`). A user managing
their *own* settings is not enough, and Canvas answers **403**.

**The rule.** A route that invalidates credentials must not run for the
caller it would invalidate unless Canvas allows it, and the port must not
delete the caller's own token as a side effect of answering.

**Related.** [[canvas-permission-order]], [[canvas-access-token-auth-model]].
