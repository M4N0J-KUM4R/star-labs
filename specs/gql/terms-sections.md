# GraphQL: terms and sections queries (issue #27)

> Area spec for the gql-foundation epic. The issue text references
> `specs/p5-gql-foundation.md`, which was never merged; this per-area file is
> the agreed workaround and carries the same contract weight (same approach
> as `specs/gql/assignments.md`).

## Scope

`enrollmentTerm(id)` and `courseSection(id)` root queries with their
Canvas-shaped object types, on top of the merged course/user/enrollments
foundation slice (`migration/crates/star-graphql`). DB tables come from
migrations `0034_terms_sections.sql` and `0047_enrollment_sections.sql`:
the term table is named `terms` (not `enrollment_terms`), and
`course_sections` requires `course_id` + `name` with nullable `start_at` /
`end_at`. No new migration was needed.

## Field tables

### EnrollmentTerm (Canvas `app/graphql/types/term_type.rb`)

| Canvas field | DB column (`terms`) | Status |
|---|---|---|
| `id` | `id` | exposed |
| `name` | `name` | exposed |
| `startAt` | `start_at` | exposed (nullable) |
| `endAt` | `end_at` | exposed (nullable) |
| `state` | `workflow_state` | exposed (renamed, like `Course.state`) |
| `createdAt` | `created_at` | exposed |
| `updatedAt` | `updated_at` | exposed |
| `sisTermId` / `sisId` | — | **omitted**: no SIS columns on `terms` |
| `coursesConnection` | — | **omitted**: `courses` has no term/enrollment_term_id column; future slice |

### CourseSection (Canvas `app/graphql/types/section_type.rb`)

| Canvas field | DB column (`course_sections`) | Status |
|---|---|---|
| `id` | `id` | exposed |
| `name` | `name` | exposed |
| `courseId` | `course_id` | exposed |
| `startAt` | `start_at` | exposed (nullable) |
| `endAt` | `end_at` | exposed (nullable) |
| `state` | `workflow_state` | exposed (renamed, like `Course.state`) |
| `createdAt` | `created_at` | exposed |
| `updatedAt` | `updated_at` | exposed |
| `sisId` / `sisSectionId` | — | **omitted**: no SIS columns |
| `userCount`, `students`, `allStudents` | — | **omitted**: would need join/aggregation; later slice |
| `gradesPosted`, `gradesPresent` | — | **omitted**: no columns |

Both types are single-object reads; neither has a connection in this slice,
so no cursor pagination is involved.

## Authorization

- Unauthenticated requests are rejected with HTTP 401 before GraphQL
  execution (shared `AuthUser` extractor in the router) — unchanged from the
  foundation slice.
- `enrollmentTerm(id: ID!): EnrollmentTerm` — any authenticated caller may
  read the term shape; unknown ids resolve to null. **Documented deviation
  from REST**: `GET /api/v1/terms/{id}` is account-admin-only, but Canvas's
  GraphQL API serves terms to authenticated users; GraphQL parity was chosen
  (per issue #27's study notes). No `workflow_state != 'deleted'` filter is
  applied yet, matching the REST show handler's behavior of returning the
  row regardless of state.
- `courseSection(id: ID!): CourseSection` — visible to users enrolled in the
  section's course (any role), teachers of the course, and admins; everyone
  else gets null, mirroring `course(id)`'s null semantics.

## Test plan

DB-backed integration tests (`tests/terms_sections_integration.rs`), all
SKIP-clean without DATABASE_URL, unique run prefix (`gql-ts-`), full teardown
(users + enrollments, courses cascading sections, accounts cascading terms):

1. `term_query_returns_seeded_fields` — camelCase shape (`startAt` populated,
   `endAt` null, `state`, `createdAt`, `updatedAt`) plus unknown-id → null.
2. `section_query_visible_to_enrolled_teacher_and_admin` — enrolled student,
   course teacher, and admin each resolve the section (`courseId` string,
   `state`, timestamps); unknown-id → null.
3. `section_query_null_for_outsider` — a non-enrolled student gets null, not
   an error.
4. `term_query_readable_by_plain_student` — a student with no enrollments
   and no admin role reads the term (any-authenticated semantics).

## Dependencies / follow-ups

- SIS columns on `terms` / `course_sections` would unlock `sisTermId` /
  `sisId`; a term column on `courses` would unlock `coursesConnection` and a
  `Course.term` field.
- `userCount` / `students` / `allStudents` / `gradesPosted` / `gradesPresent`
  on sections need join/aggregation work and belong to a later slice
  (gql-foundation follow-ups).
- Module-local copies of the `user_enrolled_in_course` / `user_teaches_course`
  helpers keep this slice's `lib.rs` diff minimal across the parallel
  gql-foundation branches; a later consolidation may hoist them into a
  shared visibility module.
