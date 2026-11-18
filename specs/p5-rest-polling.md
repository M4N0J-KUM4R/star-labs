# P5 REST Spec — Polling (polls, poll choices, poll sessions, poll submissions)

- **Issue**: #113 (epic `specs`), family: polling. Companion file: `p5-rest-misc-index.md` (shared conventions).
- **Baseline**: fresh parity run at `526de19` (see index file §2). All four polling controllers are at **0%**.
- **Target crate**: new `star-polling` crate (standalone domain, no existing crate is a natural host; mirrors the one-crate-per-domain layout).

## 1. Overview and fresh coverage

| Controller | Done | Total | Coverage |
|---|---:|---:|---:|
| `polling/polls` | 0 | 5 | 0.0% |
| `polling/poll_choices` | 0 | 5 | 0.0% |
| `polling/poll_sessions` | 0 | 9 | 0.0% |
| `polling/poll_submissions` | 0 | 2 | 0.0% |

Polling is the in-class "clicker" feature: a teacher creates a poll with choices, opens a session bound to a course (optionally a section), students submit one choice each, and results are (optionally) public. It is the **only fully synchronous family** in issue #113 — no jobs, no attachments. The manifest contains no `polling_responses` controller (verified, 0 matches); submissions are the only response entity. All 21 routes are `ui_referenced: false` (teacher-facing mobile/API surface), so they do not count toward the UI-referenced denominator.

## 2. Route inventory (21 routes, all todo)

| # | Verb | Canvas path | Action | Status | Notes |
|---|---|---|---|---|---|
| 1 | GET | `/api/v1/polls` | polls#index | todo | creator's polls, paginated |
| 2 | GET | `/api/v1/polls/:id` | polls#show | todo | |
| 3 | POST | `/api/v1/polls` | polls#create | todo | array-wrapped params (§6) |
| 4 | PUT | `/api/v1/polls/:id` | polls#update | todo | `is_correct`-blank quirk (§6) |
| 5 | DELETE | `/api/v1/polls/:id` | polls#destroy | todo | hard delete |
| 6 | GET | `/api/v1/polls/:poll_id/poll_choices` | poll_choices#index | todo | |
| 7 | GET | `/api/v1/polls/:poll_id/poll_choices/:id` | poll_choices#show | todo | |
| 8 | POST | `/api/v1/polls/:poll_id/poll_choices` | poll_choices#create | todo | |
| 9 | PUT | `/api/v1/polls/:poll_id/poll_choices/:id` | poll_choices#update | todo | |
| 10 | DELETE | `/api/v1/polls/:poll_id/poll_choices/:id` | poll_choices#destroy | todo | |
| 11 | GET | `/api/v1/polls/:poll_id/poll_sessions` | poll_sessions#index | todo | |
| 12 | GET | `/api/v1/polls/:poll_id/poll_sessions/:id` | poll_sessions#show | todo | |
| 13 | POST | `/api/v1/polls/:poll_id/poll_sessions` | poll_sessions#create | todo | binds course/section |
| 14 | PUT | `/api/v1/polls/:poll_id/poll_sessions/:id` | poll_sessions#update | todo | |
| 15 | DELETE | `/api/v1/polls/:poll_id/poll_sessions/:id` | poll_sessions#destroy | todo | |
| 16 | GET | `/api/v1/polls/:poll_id/poll_sessions/:id/open` | poll_sessions#open | todo | **GET** transitions state |
| 17 | GET | `/api/v1/polls/:poll_id/poll_sessions/:id/close` | poll_sessions#close | todo | **GET** transitions state |
| 18 | GET | `/api/v1/poll_sessions/opened` | poll_sessions#opened | todo | global, no poll prefix |
| 19 | GET | `/api/v1/poll_sessions/closed` | poll_sessions#closed | todo | global, no poll prefix |
| 20 | GET | `/api/v1/polls/:poll_id/poll_sessions/:poll_session_id/poll_submissions/:id` | poll_submissions#show | todo | |
| 21 | POST | `/api/v1/polls/:poll_id/poll_sessions/:poll_session_id/poll_submissions` | poll_submissions#create | todo | `poll_submissions[][poll_choice_id]` |

Guard chain (verified in controllers): `poll_choices` and `poll_sessions` run `before_action :require_poll` (sessions exempt on `opened`/`closed`); `poll_submissions` runs `require_poll` + `require_poll_session`. A missing poll/session in the path must 404 before any authorization.

## 3. Permission matrix (Canvas abilities)

| Route group | Canvas check (verified) | Who passes | Star enforcement |
|---|---|---|---|
| polls index/show/create | implicit scope: `@current_user.polls` (create) — polls are private to their creator | any authenticated user sees only their own polls | 401 anonymous; 404 when reading another user's poll (scoped query, not 403) |
| polls update/destroy, choices CUD, sessions CUD/open/close | poll/session ownership via `@current_user.polls` scope + `require_poll` | the poll's creator only | 404 for non-owner (record not in scope), mirroring Canvas's scoped lookups |
| poll_sessions create/update | `get_poll_session_params` course/section binding; session policy | poll creator; course must exist | 404 unknown course/section id |
| poll_submissions#show | `authorized_action(@poll_submission, :read)` | submitter or poll owner | 403 otherwise |
| poll_submissions#create | `authorized_action(@poll_submission, :submit)` | student with an active enrollment in the session's course (or section); session must be open | 403 closed session or non-enrolled; 400 on validation failure (Canvas renders `errors`, `bad_request`) |

Ability names on the models (`:read`, `:submit`, session `:open`/`:close`) are the Canvas policy verbs; star-lms should encode them as direct SQL predicates (owner check / enrollment check) rather than adding a generic ability engine — consistent with how existing crates enforce permissions.

## 4. Job/storage dependency model

**None.** No Delayed::Job usage, no attachments, no exports anywhere in `app/controllers/polling/`. Everything is synchronous CRUD. This family is the simplest in issue #113 and is a natural warm-up for the wave-3 polling issue.

## 5. Data model and migration needs

One migration — **"010x, coordinator-assigned"** via `orchestration/migration-reservations.json` — creating four tables (Canvas model names, additive only):

- `polls` — `id BIGSERIAL PK`, `user_id BIGINT NOT NULL REFERENCES users(id)`, `question TEXT NOT NULL`, `description TEXT`, `created_at/updated_at TIMESTAMPTZ NOT NULL`.
- `poll_choices` — `id`, `poll_id BIGINT NOT NULL REFERENCES polls(id) ON DELETE CASCADE`, `text TEXT NOT NULL`, `is_correct BOOLEAN NOT NULL DEFAULT false`, `position INTEGER`, `created_at/updated_at`.
- `poll_sessions` — `id`, `poll_id ... REFERENCES polls(id) ON DELETE CASCADE`, `course_id BIGINT REFERENCES courses(id)`, `course_section_id BIGINT REFERENCES course_sections(id)`, `has_public_results BOOLEAN NOT NULL DEFAULT false`, `workflow_state TEXT NOT NULL DEFAULT 'open' CHECK (workflow_state IN ('open','closed'))`, timestamps. (The `open`/`close` controller actions and the `opened`/`closed` scopes are verified; the exact Canvas column name for the state is an open question, §8 — a dedicated boolean/state column is the safe representation either way.)
- `poll_submissions` — `id`, `poll_id NOT NULL REFERENCES polls(id)`, `poll_session_id NOT NULL REFERENCES poll_sessions(id) ON DELETE CASCADE`, `poll_choice_id NOT NULL REFERENCES poll_choices(id)`, `user_id NOT NULL REFERENCES users(id)`, `created_at/updated_at`, `UNIQUE (poll_session_id, user_id)` (one submission per user per session — Canvas replaces on re-submit; confirm replace-vs-reject in §8).

## 6. Behavior contracts (params, shapes, quirks)

- **Array-wrapped params** (JSON-API style, verified in all three param methods): create/update wrap the entity in a one-element array. Polls: `params.require(:polls)[0].permit(:question, :description)`. Choices: `params.require(:poll_choices)[0].permit(:text, :is_correct, :position)`. Sessions: `params.require(:poll_sessions)[0].permit(:course_id, :course_section_id, :has_public_results)`. star-lms handlers must accept `{"polls": [{...}]}` and plain `{...}` (Canvas's `require` on an array is tolerant of a hash).
- **Response envelope**: every endpoint returns `{"polls": [...]}` / `{"poll_choices": [...]}` / `{"poll_sessions": [...]}` / `{"poll_submissions": [...]}` with a `meta: {pagination}` object from `Canvas::APIArraySerializer` — including single-record show/create (`Array.wrap`). Single-object JSON bodies (no root array) must be pinned as a deliberate deviation or wrapped to match.
- **Quirk (polls#update)**: blank `is_correct` in the incoming params is deleted before update (verified: `poll_params.delete(:is_correct) if ...blank?`) — i.e. you cannot blank out correctness on the polls endpoint; star-lms must reproduce "absent/blank means leave unchanged".
- **Quirk (open/close are GET)**: routes 16–17 are GET requests that mutate state. star-lms registers them as GET handlers regardless of REST taste; parity tooling matches on verb.
- **`opened`/`closed` are global** (`/api/v1/poll_sessions/opened`) and list the current user's sessions across all their polls, filtered by state.
- **poll_submissions#create**: body `poll_submissions[0][poll_choice_id]`; the choice must belong to the poll (404 otherwise via `@poll.poll_choices.find`); save failure → 400 with error JSON.
- Status codes: 401 anonymous, 404 out-of-scope/missing ids (scoped lookups), 403 failed ability, 400 validation, 200 success.

## 7. Test plan

Integration (unique-prefix data, full teardown, skip without `DATABASE_URL`):
1. Poll lifecycle: create (array-wrapped body) → show → update (verify `is_correct` blank-preserve quirk) → delete → 404 on show.
2. Choices: full CRUD under a poll; cascade delete when parent poll is destroyed; cross-poll choice id → 404.
3. Sessions: create with course + section; GET open/close transitions; `opened`/`closed` global lists return the session in the right bucket and nothing from other users.
4. Submissions: student enrolled in the bound course submits → 200; second submission by same user (verify replace vs 400); non-enrolled student → 403; session closed → 403; submitter shows own submission; other student → 403; poll owner reads any.
5. Envelope shape: every response has the plural root key and `meta.pagination` on collections.
6. Permission negatives: anonymous 401 on all 21; user B cannot see/modify user A's poll (404); student cannot delete a session (404/403 per matrix).

## 8. Open questions

1. Exact Canvas column for session state (`workflow_state` values `open`/`closed` vs an `is_open` boolean) — confirm against the polling schema at implementation; the API surface is identical either way.
2. Re-submission semantics: replace-in-place vs reject (`UNIQUE` then upsert?) — pin with a test once verified against Canvas behavior.
3. Whether `has_public_results` gates the students' ability to see aggregate results via an unimplemented results endpoint (no such route is in this Canvas checkout — out of scope).
