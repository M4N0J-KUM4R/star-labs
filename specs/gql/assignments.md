# GraphQL: Assignment type + assignmentsConnection (issue #26)

> Area spec for the gql-foundation epic. The issue text references
> `specs/p5-gql-foundation.md`, which was never merged; this per-area file is
> the agreed workaround and carries the same contract weight.

## Scope

`Assignment` object type and a paginated `assignmentsConnection` on `Course`,
following the merged course/user/enrollments foundation slice
(`migration/crates/star-graphql`).

## Field table

| Canvas field (`assignment_type.rb`) | DB column (`assignments`) | Status |
|---|---|---|
| `id` | `id` | exposed |
| `courseId` | `course_id` | exposed |
| `name` | `title` | exposed (renamed) |
| `description` | `description` | exposed |
| `pointsPossible` | `points_possible` | exposed (NUMERIC → float8) |
| `dueAt` | `due_at` | exposed |
| `createdAt` | `created_at` | exposed |
| `updatedAt` | `updated_at` | exposed |
| `state` | — | **omitted**: no `workflow_state` column on `assignments` |
| `lockAt` / `unlockAt` | — | **omitted**: no columns |
| `submissionTypes`, `rubric`, `muted`, … | — | omitted: no columns; later slices |

## Connection contract

Relay-style, mirroring `enrollmentsConnection`: `edges { cursor node }`,
`pageInfo { hasNextPage endCursor }`, opaque hex cursors over the id,
`first` clamped to 1..=100 (default 10), `LIMIT first+1` ordered by id
ascending.

## Authorization

- The connection is reachable only through `course(id)`, which is already
  null for callers without an enrollment (any role) and non-admins — the
  connection inherits that gate.
- Inside the connection, **REST parity** applies: the star-assignments list
  handler returns every assignment of the course to anyone who can see the
  course, because the schema has no `workflow_state` on assignments. There is
  no published/unpublished split. **Documented deviation from Canvas**, to be
  revisited when a `workflow_state`/`published` column lands.

## Test plan

DB-backed integration tests (`tests/assignments_connection_integration.rs`),
all SKIP-clean without DATABASE_URL, unique run prefix, full teardown:

1. `assignments_connection_returns_canvas_field_names` — camelCase shape:
   `courseId`, `pointsPossible`, `dueAt`, `createdAt` populated; `description`
   null; `id` a string.
2. `student_sees_all_assignments_rest_parity` — enrolled student sees all
   assignments (REST parity, no published filter).
3. `outsider_gets_null_course_admin_sees_assignments` — unauthorized callers
   get a null course (negative); admin sees the connection without an
   enrollment.
4. `assignments_connection_paginates_across_two_pages` — `first`/`after`
   pagination, `hasNextPage`, `endCursor` equals the last edge's cursor, pages
   cover the set exactly once.

## Dependencies / follow-ups

- A future migration adding `workflow_state` (or `published`) to `assignments`
  unlocks the Canvas-published filter and the `state` field; the connection
  and tests then gain a student-published case.
- `Course.assignmentsConnection` is the pattern for the remaining
  gql-foundation slices (#27, #28, #48, #78–#82).
