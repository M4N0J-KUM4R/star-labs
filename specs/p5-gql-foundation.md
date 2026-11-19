# Spec p5: GraphQL foundation — read queries over the existing schema

Issue: #137 (epic: specs) · Status: **Merged into `specs/` (corrected)** ·
Parity target: canvas-lms `app/graphql/types/`, `app/controllers/graphql_controller.rb`,
`config/routes.rb` · Implementation target: `migration/crates/star-graphql/`.

> Inherits transport rules from `specs/gql/CONTRACT.md`.

> History: this file was originally drafted during wave-0 dispatch but never
> merged, while the dispatched gql-foundation issues (#26–#28, #48, #78–#82)
> cited it as if it existed. Issue #137 merges it into `specs/` **corrected**:
> the old draft's auth rule (§2.3/§2.4/§2.7 of that draft — answer missing
> credentials with a GraphQL errors payload instead of the transport 401) is
> replaced by the 401 contract, the old `/graphql` + `/graphiql` mount (§5.1)
> is replaced by `POST /api/graphql`, the old draft's numeric slice labels are
> replaced by `GQL-S1..GQL-S6`, and the stale "Zero GraphQL crates" inventory
> (§1.1) is replaced by the real merged state. The per-area files
> `specs/gql/*.md` were the interim workaround and remain the authoritative
> detail pages for their domains.

## 1. Context and current state

### 1.1 Crate inventory (corrected)

**`star-graphql` is merged on `main`** — the old "Zero GraphQL crates"
statement is stale. Current state (all verified on `main`):

- `migration/crates/star-graphql/` — async-graphql 7 crate; `src/` modules:
  `lib.rs` (context, cursors, `Course`/`User`/`Enrollment` types, `Query`
  root, router + handler, permission helpers), `assignment.rs`,
  `course_user_fields.rs`, `discussion_topic.rs`, `quiz.rs`, `submission.rs`,
  `terms_sections.rs`.
- Root queries: `course(id)`, `user(id)`, `enrollmentTerm(id)`,
  `courseSection(id)`.
- Connections (relay-style, cursor-paginated per `specs/gql/CONTRACT.md` §4):
  `Course.enrollmentsConnection`, `Course.assignmentsConnection`,
  `Course.quizzesConnection`, `Course.usersConnection`,
  `Course.discussionsConnection`, `Assignment.submissionsConnection`,
  `User.enrollmentsConnection`.
- Mutation root: `EmptyMutation`; subscription root: `EmptySubscription` —
  mutations are specced by `specs/p5-gql-mutations.md` (batches A–E) and are
  not part of the foundation.
- Tests: `tests/graphql_integration.rs` (foundation + the 401 transport
  tests), `assignments_connection_integration.rs`,
  `terms_sections_integration.rs`, `course_user_parity_integration.rs`,
  `submissions_discussions_integration.rs`, `quiz_integration.rs` — all
  SKIP-clean without `DATABASE_URL`, unique run prefix, full teardown.
- Mount: `star-http/src/main.rs` nests the crate's `router()` at
  `/api/graphql` (§5.1 below).

REST parity scoreboard context lives in `migration/parity/`; GraphQL is
tracked by operation/type coverage separately from the REST route count.

## 2. Transport contract (normative text: `specs/gql/CONTRACT.md`)

The transport contract is **defined once, in `specs/gql/CONTRACT.md`**; the
subsections below are pointers, not a re-definition. On any conflict,
CONTRACT.md wins.

### 2.1 Mount

`POST /api/graphql`, top-level, outside `/api/v1` — see CONTRACT.md §2
(Canvas `config/routes.rb:36`). GraphiQL is a Canvas browser IDE, not part of
the API, and is not mounted (CONTRACT.md §2, deviation note included).

### 2.2 Execution shell

Per-request schema around a `GraphQLContext { pool, auth_user }`; the
`AuthUser` is resolved by the shared extractor before the schema runs; DB
failures map to `async_graphql::Error("database error")` and never leak SQL
(CONTRACT.md §3, §7).

### 2.3 Authentication — HTTP 401 pre-execution (corrected)

Every request without a valid bearer token is rejected with **HTTP 401,
non-GraphQL body, before execution** — Canvas halts in the Rails
`require_user` filter (`graphql_controller.rb:24`, `require_auth?` at
~105–111; `lib/authentication_methods.rb` `require_user` → 401). The old
draft answered missing credentials with a GraphQL errors payload; that rule
was wrong and is withdrawn — the merged code and `graphql_integration.rs`
implement the 401 rule. Canvas's single narrow exception — anonymous
execution of a registered, explicitly anonymous persisted query behind the
root-account `graphql_persisted_queries` flag — is documented in CONTRACT.md
§1 and not ported.

### 2.4 Error channels (corrected)

401 transport (pre-execution) vs HTTP 200 + top-level `errors[]` (execution)
vs HTTP 200 + payload `errors: [{attribute, message}]` (validation) vs HTTP
500 (60-second execution timeout). The old draft applied the
errors-in-body convention to *all* requests, missing-credentials ones
included — wrong: the convention describes requests that **reach execution**
(`graphql_controller.rb` `render json: result`), and authentication happens
strictly before that. Full table: CONTRACT.md §3.

### 2.5 Cursor pagination

Opaque 16-char hex cursors over the row id, `first` clamped 1..=100
(default 10), `after` exclusive, `LIMIT first+1` ordered by id ascending,
`edges { cursor node }` + `pageInfo { hasNextPage endCursor }` — CONTRACT.md
§4.

### 2.6 ID encoding

Plain decimal stringified i64 in and out; Canvas's base64 relay layer is not
ported; malformed ids are execution errors, unknown ids are null / Canvas
not-found strings — CONTRACT.md §5.

### 2.7 Status-code policy (corrected)

The complete policy, restated from CONTRACT.md §1/§3 as the one-line version
every slice tests against:

- missing or invalid credentials → **401** with a non-GraphQL body (never a
  GraphQL error shape);
- executed request → **200**, errors in the body (top-level `errors[]` or
  payload `errors`);
- execution exceeding 60 s → **500**, never a partial-data 200.

The old draft's §2.7 auth rule is superseded by exactly this list.

## 3. Type conventions (inherited by every foundation slice)

- Field names camelCase, mirroring Canvas `app/graphql/types/*_type.rb`;
  `workflow_state` renames to `state` where Canvas does.
- Permission-gated fields resolve **null, never errors** (`usersConnection`
  for non-roster viewers, `User.enrollmentsConnection` for non-self/admin).
- Visibility gates mirror the star-lms REST handlers of the matching domain
  (the merged `user_enrolled_in_course` / `user_teaches_course` helpers are
  the same checks as star-enrollments); documented deviations from Canvas
  are listed per domain in `specs/gql/*.md`.
- Every exposed field needs a backing column; omissions are recorded as
  "omitted: no column" rows in the per-area spec field tables — never invent
  a field Canvas does not have.

## 4. Testing conventions

- DB-backed integration tests in `star-graphql/tests/`, one file per area,
  following `graphql_integration.rs`: `setup()` returns `None` without
  `DATABASE_URL` → `println!("SKIP: DATABASE_URL not set")`; unique per-run
  prefix; full teardown of every seeded row; dedicated accounts when feature
  flags are involved (never account 1).
- Every slice's negatives: 401 unauthenticated (transport, body not a GraphQL
  error shape), null-gating for unauthorized viewers, unknown-id → null,
  pagination cover-exactly-once when a connection is added.

## 5. Wiring and mount

### 5.1 Route table (corrected)

| Route | Canvas | star-lms | Notes |
| --- | --- | --- | --- |
| `POST /api/graphql` | `config/routes.rb:36` `post "/api/graphql", to: "graphql#execute"` | `star-http/src/main.rs` ~104–105 `.nest("/api/graphql", star_graphql::router())` | The API mount. |
| `GET /graphiql` | `config/routes.rb:37` `get "graphiql", to: "graphql#graphiql"` | **not mounted** | Browser IDE page (`graphql_controller.rb#graphiql`), not API surface. |

The old draft's §5.1 quoted a `/graphql` + `/graphiql` mount and a `main.rs`
shape that does not match the merged code; both are corrected above to the
real wiring. The merged `star_graphql::router()` currently also answers GET
(`get(graphql_handler).post(graphql_handler)`) — a documented deviation from
the POST-only contract; see CONTRACT.md §2 for the follow-up note.

## 6. Foundation slices (labels corrected to GQL-S1..GQL-S6)

The six original foundation slices, with their current status:

| Label | Slice | Issue | Status | Detail spec |
| --- | --- | --- | --- | --- |
| GQL-S1 | Execution shell: router, context, 401 transport, `course(id)`/`user(id)` + `enrollmentsConnection` | — | **merged** (PR #14) | this file + CONTRACT.md |
| GQL-S2 | `Assignment` + `assignmentsConnection` | #26 | **merged** (PR #115 closed; content re-landed via wave-1 batch PR #123) | `specs/gql/assignments.md` |
| GQL-S3 | `enrollmentTerm(id)` / `courseSection(id)` root queries | #27 | **merged** (PR #116 closed; content re-landed via wave-1 batch PR #123) | `specs/gql/terms-sections.md` |
| GQL-S4 | Course/User Canvas field parity (`courseCode`, `syllabusBody`, `account`, `permissions`, `usersConnection`, `sortableName`, `avatarUrl`) | #28 | **merged** (PR #125) | `course_user_fields.rs` doc header + `course_user_parity_integration.rs` |
| GQL-S5 | `Submission` + `DiscussionTopic` types and connections | #48 | **merged** (PR #126) | `specs/gql/submissions-discussions.md` |
| GQL-S6 | `Quiz`, `QuizSubmission`, `QuizQuestion` types + connections | #78 | **merged** (PR #128) | `quiz.rs` + `quiz_integration.rs` |

Later gql-foundation dispatches beyond the original six — #79 (modules),
#80 (files/folders/pages), #81 (outcomes/groups), #82
(conversations/calendar/account) — are follow-up foundation slices: they
inherit this file's conventions and `specs/gql/CONTRACT.md`, and get their
own `specs/gql/*.md` area pages when they land.

## 7. Superseded artifacts

- The stale `p5/gql-foundation*` branches (p5/gql-foundation,
  p5/gql-foundation-26/-27/-28/-48/-78/-81/-82) are **superseded by this
  file + `specs/gql/CONTRACT.md`**; they are listed in PR #137's body and are
  deleted only by the coordinator.
- The per-area workaround banners in `specs/gql/*.md` ("references
  `specs/p5-gql-foundation.md`, which was never merged") describe the
  pre-#137 state; this file now exists and is the foundation page they
  complement.
