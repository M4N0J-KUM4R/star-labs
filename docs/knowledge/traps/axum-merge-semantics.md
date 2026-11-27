---
tags: [trap, rust, axum]
---
# Trap: axum 0.8 silently MERGES same-path routes — it only panics on two specific cases

**Symptom.** `star-http` panics at startup with
`Overlapping method route. Handler for `GET /x` already exists`, or matchit's
`Insertion failed due to conflict with previously registered route`.

**The rule (axum 0.8.9, verified with a probe crate — not inferred).**

| situation | behaviour |
|---|---|
| same path, **different** methods | **merges silently** — no panic |
| same path, **same** method | panics (`MethodRouter::merge_for_path`) |
| different literal paths, same **matchit shape** (e.g. `/{id}/bind` vs `/{registration_id}/bind`) | panics in matchit |

**Why it matters.** "The same path registered twice" is NOT by itself a bug, and a
duplicate-path count is NOT a defect count. The shape rule is the nasty one:
`/accounts/{account_id}/lti_registrations/{id}/bind` and
`/accounts/{account_id}/lti_registrations/{registration_id}/bind` are the same URL
space, so axum refuses the second even though the parameter names look different.

**Fix.** Chain the methods into one registration —
`.route(path, get(a).put(b).delete(c))`. The chain starts with a **bare** call and
continues with a leading dot: `get(a).put(b)`, never `.get(a).put(b)`.

**Guard.** `migration/crates/star-http/tests/router_conflicts.rs` assembles the API
router and the application router in a test. A conflicting route fails CI instead
of the server. See [[ci-never-assembled-the-router]].

**Related.** [[double-merge-star-content]], [[chaining-not-deleting]].
