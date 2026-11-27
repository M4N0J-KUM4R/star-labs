---
tags: [trap, permissions, canvas-semantics]
---
# Trap: the port's role model is stricter than Canvas' role-override defaults

**Status: KNOWN DIVERGENCE** in `crates/star-collaborations`.

Canvas decides a permission in two steps. The **role override** gives the
value (`config/initializers/permissions_registry.rb`), and the **policy**
decides whether the permission is reachable at all
(`app/models/*.rb`, the `set_policy` blocks). The registry's `true_for` is
the default for a role — and for two permissions it includes
`StudentEnrollment`:

```
read_roster:          true_for StudentEnrollment TaEnrollment DesignerEnrollment TeacherEnrollment AccountAdmin
create_collaborations: true_for StudentEnrollment TaEnrollment DesignerEnrollment TeacherEnrollment AccountAdmin
```

So in upstream Canvas an **enrolled student holds `read_roster`** and
**holds `create_collaborations`** on their course. The port's permission
model is enrollment-based and has no per-course role-override table, so it
models both as **staff-only**:

| the caller | Canvas | the port |
|---|---|---|
| student asking for `/courses/:id/potential_collaborators` | 200 (the roster they may see) | 403 |
| student who authored a collaboration, updating it | 200 (the author's branch needs `create_collaborations`) | 403 |

**Why the port is stricter.** `specs/p5-rest-collaborations.md` §8 pins the
403 for a student without `read_roster`, and the crate's tests follow the
spec. The divergence is deliberate, not an oversight — but it is a
divergence, and the front door does not paper over it: the port *owns* the
route, so it answers, and a student gets 403 where Canvas answers 200.

**What closing it needs.** A `role_overrides` table the permission layer
reads per (context, role, permission), seeded with the registry defaults.
That is the same table `crates/star-permissions` already models for the
account and course scopes, so the fix belongs in that crate rather than in
the collaborations crate.

**How the two decisions interact.** Canvas' Collaboration policy
(`app/models/collaboration.rb:70-97`) grants `:update` and `:delete` to
the **author** only when the context also grants `create_collaborations`:

```
can :update → the context grants manage_course_content_edit
              OR (the caller IS the author AND the context grants create_collaborations)
```

"Is the author" alone is not enough. A port that treats the author as
sufficient shows a student a delete button Canvas does not show.

Canvas' own spec (`spec/apis/v1/collaborations_api_spec.rb:192`) only
pins the *unenrolled* caller to 403 — it says nothing about an enrolled
student, which is where the two models differ.
