# GraphQL: Submission and DiscussionTopic types + connections (issue #48)

> Area spec for the gql-foundation epic. The issue text references
> `specs/p5-gql-foundation.md`, which was never merged; this per-area file is
> the agreed workaround and carries the same contract weight (same approach as
> `specs/gql/assignments.md` and `specs/gql/terms-sections.md`).

## Scope

`Submission` and `DiscussionTopic` object types with paginated connections,
matching Canvas field names:

- `Assignment.submissionsConnection` (Canvas `assignment_type.rb` mounts
  `submissionsConnection` via `SubmissionSearch`);
- `Course.discussionsConnection` (Canvas `course_type.rb` implements
  `Interfaces::DiscussionsConnectionInterface`, whose field is
  `discussionsConnection` — note: **not** `discussionTopicsConnection`).

`DiscussionEntry` is **out of scope** for this slice (the issue covers the
Submission and DiscussionTopic types only); entries need their own visibility
work and ship later. No root queries are added — both types stay reachable
through `course(id)` like the rest of the foundation.

## Field tables

### Submission (Canvas `app/graphql/types/submission_type.rb` + `interfaces/submission_interface.rb`)

| Canvas field | DB column (`submissions`, 0001 + 0060) | Status |
|---|---|---|
| `id` | `id` | exposed |
| `assignmentId` | `assignment_id` | exposed |
| `userId` | `user_id` | exposed |
| `attempt` | `attempt` | exposed (column `NOT NULL DEFAULT 1`; Canvas coalesces nil→0) |
| `body` | `body` | exposed |
| `score` | `score` | exposed (NUMERIC → float8) |
| `state` | `workflow_state` | exposed (renamed; Canvas `SubmissionStateType`) |
| `submissionType` | `submission_type` | exposed |
| `submittedAt` | `submitted_at` | exposed |
| `gradedAt` | `graded_at` | exposed |
| `excused` | `excused` | exposed |
| `latePolicyStatus` | `late_policy_status` | exposed |
| `deductedPoints` | `points_deducted` | exposed (renamed; NUMERIC → float8) |
| `grade`, `enteredScore`, `enteredGrade` | — | **omitted**: no columns; the REST handlers treat `score` as the grade |
| `posted`, `postedAt` | — | **omitted**: no columns (no grade posting in star-lms yet) |
| `url`, `externalToolUrl`, `previewUrl` | — | **omitted**: no `url` column on `submissions` |
| `readState` | — | omitted: needs a `submission_reads` join; later slice |
| `secondsLate`, `late`, `missing`, `submissionStatus`, `status` | — | omitted: Canvas-computed; no columns |
| `user`, `assignment` object refs | — | omitted: later slice (the id fields are exposed) |
| `commentsConnection`, `rubricAssessmentsConnection`, `submissionHistoriesConnection`, `enrollmentsConnection`, `attachments`, … | — | omitted: later slices |

Note: `submissions.grader_id` exists but Canvas's `Submission` GraphQL type has
no `graderId` field, so it is not exposed (parity is with the Canvas surface,
not the table).

### DiscussionTopic (Canvas `app/graphql/types/discussion_type.rb`)

| Canvas field | DB column (`discussion_topics`, 0017 + 0105) | Status |
|---|---|---|
| `id` | `id` | exposed |
| `contextId` | `course_id` | exposed (renamed; star-lms topics are course-scoped only) |
| `contextType` | — (constant) | exposed as the literal `"Course"` — the table has no Group/Account contexts |
| `title` | `title` | exposed |
| `message` | `message` | exposed |
| `published` | `workflow_state` | exposed (derived: `workflow_state = 'active'`; Canvas `null: false` Boolean) |
| `locked` | `locked` | exposed |
| `discussionType` | `discussion_type` | exposed (Canvas enum values `side_comment|threaded|flat|not_threaded` match the CHECK constraint) |
| `createdAt`, `updatedAt` | `created_at`, `updated_at` | exposed (TimestampInterface) |
| `isAnnouncement` | — | **omitted**: no announcement/type column (star-lms keeps announcements in their own table) |
| `author`, `editor`, `anonymousAuthor` | — | **omitted**: no `user_id` column on `discussion_topics` |
| `anonymousState`, `isAnonymousAuthor` | — | **omitted**: no anonymous columns |
| `delayedPostAt`, `lockAt`, `editedAt`, `lastReplyAt`, `postedAt`, `todoDate`, `position`, `expanded`, … | — | omitted: no columns |
| `pinned` (column exists, 0105) | — | not exposed: Canvas's `DiscussionType` has no `pinned` field — REST-only in star-lms, as `specs/p5-gql-mutations.md` §2.3 also notes |
| `discussionEntriesConnection`, `entryCounts`, `subscribed`, `permissions`, `author` gates, … | — | omitted: later slices |
| (type name) | — | Canvas's `graphql_name` is `"Discussion"`; star-lms exposes `"DiscussionTopic"` per the issue's terminology. Field shapes are unaffected — type names never appear in queries |

## Connection contract

Relay-style, mirroring `enrollmentsConnection` / `assignmentsConnection`:
`edges { cursor node }`, `pageInfo { hasNextPage endCursor }`, opaque hex
cursors over the id, `first` clamped to 1..=100 (default 10), `LIMIT first+1`
ordered by id ascending.

- `Assignment.submissionsConnection(first, after)` — Canvas's `filter` /
  `orderBy` args (SubmissionSearch) are omitted for now; the default ordering
  is by id ascending.
- `Course.discussionsConnection(first, after)` — Canvas's `filter` arg
  (userId/searchTerm/isAnnouncement) is omitted. Canvas orders regular
  discussions by `id: asc`, which matches this slice's ordering.

## Authorization

- Both connections are reachable only through `course(id)`, which is null for
  callers without an enrollment (any role) and non-admins — the connections
  inherit that gate, and unauthenticated requests stay HTTP 401 pre-execution
  (router `AuthUser` extractor, unchanged).
- `Assignment.submissionsConnection` mirrors the `enrollmentsConnection`
  roster rule: admins and course teachers see every submission of the
  assignment; anyone else (enrolled students, TAs) sees only their own. A
  student who has not submitted gets empty edges, not null — star-lms has no
  "unsubmitted" placeholder rows, where Canvas's `SubmissionSearch` would
  synthesize them.
- No `workflow_state` filter is applied to submissions: Canvas's default
  states (`submitted|pending_review|graded`) exist to exclude unsubmitted
  placeholders, which star-lms does not have — REST parity with the
  star-assignments list handlers.
- `Course.discussionsConnection` returns `workflow_state <> 'deleted'` to
  everyone who can see the course. This is identical to Canvas's
  `DiscussionTopic.active` scope *and* to the star-discussions REST list
  handler. Canvas additionally applies `ScopedToUser` section/availability
  scoping per viewer — that is a documented deviation here (needs section
  visibility and availability columns), as is the absence of a
  published/unpublished split for students.

## Test plan

DB-backed integration tests (`tests/submissions_discussions_integration.rs`),
all SKIP-clean without DATABASE_URL, unique run prefix (`gql-sd-`), full
teardown (users + enrollments; courses cascade assignments → submissions and
discussion_topics):

1. `submissions_connection_returns_canvas_field_names` — camelCase shape:
   `assignmentId`, `userId`, `attempt`, `submissionType`, `state`,
   `submittedAt`/`gradedAt`/`createdAt`/`updatedAt` ISO strings, `score` float,
   `excused` false, `latePolicyStatus`/`deductedPoints` null, `id` a string.
2. `student_sees_own_submissions_teacher_sees_all` — student isolation
   (exactly own submission), teacher sees all; a student without a submission
   gets empty edges.
3. `outsider_gets_null_course_admin_sees_submissions` — non-enrolled caller
   gets a null course (negative); admin sees submissions without an enrollment.
4. `submissions_connection_paginates_across_two_pages` — `first`/`after`
   pagination, `hasNextPage`, `endCursor` equals the last edge's cursor, pages
   cover the set exactly once.
5. `discussions_connection_returns_canvas_field_names` — `contextId`,
   `contextType` = "Course", `title`, `message`, `published`, `locked`,
   `discussionType`, timestamps, string id.
6. `discussions_connection_scopes_deleted_keeps_unpublished` — deleted topics
   are excluded; unpublished topics are included with `published: false`
   (Canvas `active` scope = non-deleted; REST parity).
7. `outsider_gets_null_course_admin_sees_discussions` — negative + admin
   without enrollment.
8. `discussions_connection_paginates_across_two_pages` — same pagination
   contract on the topics connection.

## Dependencies / follow-ups

- `submissions.url` column → `url`-typed submissions (`online_url`,
  `basic_lti_launch`); grade/entered-score and posted-at columns →
  `grade`/`enteredScore`/`posted` fields.
- `discussion_topics.user_id` → `author`; anonymous columns → `anonymousState`;
  an announcement/type column → `isAnnouncement`.
- `Course.submissionsConnection` (Canvas `course_type.rb` has one),
  `submission(id)` / `discussionTopic(id)` root queries, and
  `Discussion.discussionEntriesConnection` + `DiscussionEntry` are later
  gql-foundation slices.
- Canvas's per-viewer section/availability discussion scoping needs section
  visibility work; revisit together with the published/unpublished split.
