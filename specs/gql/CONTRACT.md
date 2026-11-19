# GraphQL transport contract

Issue: #137 (epic: specs) · Status: **Decided** · Applies to: every GraphQL spec
(`specs/p5-gql-foundation.md`, `specs/p5-gql-mutations.md`, all `specs/gql/*.md`)
and every `star-graphql` slice, current and future.

> This page is the **single normative definition** of the star-lms GraphQL
> transport contract (issue #137). Other GraphQL specs may summarize or point
> into it, but must not redefine it; on any conflict this page wins. The
> decisions below resolve `specs/p5-gql-mutations.md` §7 Q1–Q3 (ID encoding,
> score type, mutation names).

Ground truth: Canvas LMS `master`
(`app/controllers/graphql_controller.rb`, `config/routes.rb`,
`lib/authentication_methods.rb`, `app/graphql/graphql_helpers.rb`,
`app/graphql/mutations/update_submission_grade.rb`) and the merged
`migration/crates/star-graphql` on `main` (`src/lib.rs` doc header and
handlers; `migration/crates/star-http/src/main.rs` mount).

## 1. Authentication — HTTP 401 before execution

**Rule (decided):** every request to the GraphQL endpoint without a valid
bearer token is rejected with **HTTP 401 and a non-GraphQL body, before any
GraphQL execution**. Authentication happens in the shared `star-auth`
`AuthUser` extractor in the router, not in the schema; the extractor is the
same one the REST crates use (invalid token → 401 before the handler runs).
The merged test
`star-graphql/tests/graphql_integration.rs::unauthenticated_request_is_rejected_with_401`
pins this for POST and GET, asserting the body is *not* a GraphQL error shape.

There is **no anonymous access** in star-lms. Canvas has exactly one narrow
exception, documented here for the record and **not ported**:

- Canvas `app/controllers/graphql_controller.rb` line 24:
  `skip_before_action :require_user, unless: :require_auth?` — the Rails auth
  filter always runs unless `require_auth?` returns false.
- `require_auth?` (lines ~105–111) returns false **only** when the action is
  `execute` **and** a persisted query exists for `params[:operationName]`
  **and** that persisted query has `anonymous_access_allowed` set.
- A persisted query exists only when the root-account feature flag
  `graphql_persisted_queries` is enabled and the operation name is registered
  (`persisted_query`, lines ~117–119); an actual anonymous call additionally
  requires an in-app (mobile) client (`anonymous_call?`, lines ~113–115).
- When the filter does fire and no user is present, Canvas halts with 401:
  `require_user` → `redirect_to_login`
  (`lib/authentication_methods.rb` lines ~348–356, ~399–411): API/JSON
  requests get `render_json_unauthorized` (code `unauthorized`, "user
  authorization required"), other formats get
  `render plain: "Unauthenticated", status: :unauthorized`.

star-lms has no persisted-query infrastructure and no account feature-flag
equivalent, so the exception cannot occur: a plain unauthenticated POST halts
in the filter with 401 and never reaches the schema. If persisted queries are
ever ported, this section must be revised before any anonymous execution is
allowed.

## 2. Mount — POST `/api/graphql` only

**Rule (decided):** the GraphQL API mounts at **`POST /api/graphql`** —
top-level, **outside** `/api/v1`, matching Canvas `config/routes.rb` line 36:

```ruby
post "/api/graphql", to: "graphql#execute"
get "graphiql", to: "graphql#graphiql"
```

- Canvas's `graphiql` route (line 37) is a browser IDE page
  (`GraphQLController#graphiql` renders a GraphiQL view), **not** part of the
  API surface; star-lms does not mount it.
- Wiring in star-lms (`migration/crates/star-http/src/main.rs` ~lines 104–105):

  ```rust
  // Canvas serves its GraphQL API at /api/graphql (outside /api/v1).
  .nest("/api/graphql", star_graphql::router())
  ```

- **Documented deviation (to be removed by a follow-up slice):** the merged
  `star_graphql::router()` currently routes `/` with
  `get(graphql_handler).post(graphql_handler)`, i.e. it also answers
  `GET /api/graphql?query=…`. This predates this contract (the merged 401
  test exercises GET as a side effect). The contract target is **POST only**;
  dropping the GET route — and its test leg — is an implementation follow-up,
  not a spec change.

## 3. Status codes and error channels

Three error channels, never mixed (Canvas renders `render json: result` —
`graphql_controller.rb` line ~53 — so every request that *reaches execution*
is HTTP 200 and carries errors in the body; the 401 in §1 happens *before*
execution, so the errors-in-body convention can never describe
authentication):

| Channel | Transport | When | star-lms implementation |
| --- | --- | --- | --- |
| Unauthenticated | **HTTP 401**, non-GraphQL body, **before execution** | missing/invalid bearer token | `AuthUser` extractor in the router (§1) |
| Execution error | HTTP 200, top-level `errors[]`, `data.<field>` null | authz failure, record not found, malformed cursor/id, DB failure | `async_graphql::Error::new(msg)`; `db_err(e)` → `"database error"` — never leak SQL text |
| Validation error | HTTP 200, payload `errors: [{attribute, message}]` | input/validation failures inside mutations | payload field on every mutation payload type (`specs/p5-gql-mutations.md` §3) |

Execution timeout (Canvas wraps `schema.execute` in
`Timeout.timeout(1.minute)`, `graphql_controller.rb` lines ~100–102): the cap
is portable and adopted — star-lms caps execution at **60 seconds**
(`tokio::time::timeout` around schema execution). On expiry the request is an
**HTTP 500 error, never a partial-data 200** (Canvas raises `Timeout::Error`,
which Rails renders as 500). Not yet enforced in the merged handler; applies
to the foundation follow-up slice that touches the handler.

## 4. Cursor scheme — opaque hex-over-id (decided)

One connection convention everywhere, mirroring the merged
`enrollmentsConnection` family (`star-graphql/src/lib.rs`):

- Shape: relay-style `edges { cursor node }` + `pageInfo { hasNextPage
  endCursor }`.
- Cursor encoding: **opaque, fixed-width 16-char zero-padded lowercase hex of
  the row id** (`encode_cursor` → `format!("{id:016x}")`). Clients treat
  cursors as opaque tokens; `decode_cursor` (`i64::from_str_radix(…, 16)`) is
  the only reader.
- Malformed cursor → execution error `"invalid cursor"`.
- Arguments: `first: Option<i32>` clamped to **1..=100** (default **10**),
  `after: Option<String>` as an exclusive lower bound (`id > after`).
- Query shape: `LIMIT first+1` ordered by **id ascending**; the extra row
  drives `hasNextPage`; `endCursor` is the last returned edge's cursor.
- `last`/`before` are **not offered** — star-lms implements the `first`/`after`
  subset of Canvas's relay connections (documented; revisit only if a client
  needs reverse paging).

## 5. ID encoding — plain decimal i64, no base64 relay layer (decided)

**Rule:** star-lms GraphQL IDs are **plain stringified i64** (`"42"`), in and
out — `ID(row.id.to_string())` on output, `i64::from_str` on input.

Canvas's default GraphQL IDs are base64-encoded relay global IDs, and Canvas
inputs accept both forms: `GraphQLHelpers.parse_relay_or_legacy_id`
(`app/graphql/graphql_helpers.rb` lines ~45–58) passes through legacy numeric
strings (`/\A\d+\Z/`) and otherwise base64-decodes + type-checks the relay id,
raising `InvalidIDError "expected an id for {type}"` (surfaced as an
execution error) on failure.

Parity choice (decided, resolves `p5-gql-mutations.md` §7 Q1): star-lms speaks
**only the legacy numeric form** — no base64 relay encoding or decoding
anywhere. Consequences:

- Malformed (non-decimal) ID input is a resolver-level execution error, not a
  not-found: the merged queries use `"invalid course id"` / `"invalid user
  id"` / `"invalid term id"` / `"invalid section id"`.
- Unknown-but-numeric ids are **not** transport errors: queries resolve null
  (`course(id)`, `user(id)`, …); mutations use their per-mutation Canvas
  not-found strings (`"not found"`, `"invalid course: {id}"`,
  `"assignment not found: {id}"`, … per `p5-gql-mutations.md` §2).

## 6. Scalar and name decisions (resolve §7 Q2/Q3 of the mutations spec)

- **Score input is `Int!`, score output is `Float`.** Canvas types the grade
  input as Int (`argument :score, Int, required: true`,
  `app/graphql/mutations/update_submission_grade.rb` line ~24) while the
  star-lms `Submission.score` read field is `Float` (`Option<f64>`, NUMERIC
  cast `::float8` in SQL). Both sides stay as they are: `Int!` on the
  `UpdateSubmissionsGrade` input for byte-parity with Canvas, `Float` on
  reads. Decimal grades go through REST or a later mutation.
- **Mutation names are Canvas's `graphql_name`, verbatim, quirks included.**
  `UpdateSubmissionsGrade` stays plural (the Ruby class is
  `UpdateSubmissionGrade`; `graphql_name "UpdateSubmissionsGrade"`,
  `update_submission_grade.rb` line ~22). Never rename to "fix" a Canvas
  quirk — clients depend on the exact operation names.

## 7. Conventions every GraphQL spec inherits

- Field names are camelCase (`courseId`, `pointsPossible`, `createdAt`),
  matching Canvas `app/graphql/types/*_type.rb`.
- IDs serialize as GraphQL `ID` strings (§5); timestamps serialize ISO8601
  (`chrono::DateTime<Utc>`); `workflow_state` renames to `state` where Canvas
  does.
- Permission-gated fields resolve to **null, never errors** (Canvas GraphQL
  semantics), except where a mutation's Canvas contract dictates a payload or
  execution error.
- The schema is built per request around the pool and the `AuthUser`
  (`Schema::build(Query, …).data(GraphQLContext { pool, auth_user })`); the
  JWT role claim is never trusted for authorization — the role is re-read
  from the database by the extractor, matching the REST crates.
- DB-backed tests: SKIP cleanly without `DATABASE_URL`, unique per-run
  prefix, full teardown of every seeded row (protocol: pinned issue #20).
