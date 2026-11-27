---
tags: [trap, parity, permissions]
---
# Trap: Canvas' permission ORDER decides 401 vs 403 vs 404

**The rule.** The first check that denies wins:

1. not authenticated → **401**
2. authenticated but not permitted → **403**
3. permitted, but the resource does not exist → **404**
4. permitted and present → **200**

Two rules make this subtle:

**Permission before existence.** Canvas answers 403 for a request it may not
make, *even when the resource does not exist*. A port that looks the resource
up first answers 404 where Canvas answers 403 — 16 recorded cases were exactly
this.

**Permissions are contextual, not a column.** A permission is granted by a
*role in a context*: a course enrollment, an account membership, or a role
override. `users.role` is not Canvas' model. The port's per-handler string
checks (`require_permission("manage_courses")`) cannot express this.

**Concluded enrollments.** Canvas *concludes* an enrollment
(`workflow_state='completed'`); the port used to hard-`DELETE` the row. A
concluded enrollment keeps course `:read` but loses the course-admin rights —
**except** for permissions carrying `applies_to_concluded`
(`permissions_registry.rb:1775`, `:1963`; `course.rb:2200`). Getting this
wrong turned every later course-scoped read into a 403 (≈45 cases).

**Sources.** `app/models/permissions.rb`, `app/models/enrollment.rb:1438`
(`active_or_pending_by_date`), `:1454` (`not_inactive_by_date`),
`app/models/course.rb:2200`, `config/initializers/permissions_registry.rb`.

**Rule of thumb.** Read the Canvas controller's `before_action` chain and
copy the order. The first `before_action` that denies is the status.

**Related.** [[axum-json-extractor-415]], [[replay-fixture-rules]].
