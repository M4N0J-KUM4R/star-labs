# P5 REST Spec — Web conferences

- **Issue**: #113 (epic `specs`), family: conferences (`web_conferences`). Companion: `p5-rest-misc-index.md`.
- **Baseline**: fresh parity run at `526de19`; `conferences` at **0%** (0/7).
- **Target crate**: new `star-conferences` crate.

## 1. Overview and fresh coverage

| Controller | Done | Total | Coverage |
|---|---:|---:|---:|
| `conferences` | 0 | 7 | 0.0% |

Conferences wrap an external web-conferencing provider (BigBlueButton, LTI tools). The REST API surface is: list a course's/group's conferences (with preloaded recordings), create one (optionally inviting users/observers and creating a calendar event), list the current user's conferences across contexts, and the provider-callback `recording_ready` webhook. The join/close/settings actions are HTML-only. Guards (verified): `require_context` (except `for_user`), `require_config` (except `for_user`) — a configured conference provider is required —, `reject_student_view_student`, and `get_conference` before all per-record actions.

## 2. Route inventory (7 routes, all todo)

| # | Verb | Canvas path | Action | Status | Notes |
|---|---|---|---|---|---|
| 1 | GET | `/api/v1/courses/:course_id/conferences` | index | todo | recordings preloaded |
| 2 | GET | `/api/v1/groups/:group_id/conferences` | index | todo | group twin |
| 3 | POST | `/api/v1/courses/:course_id/conferences` | create | todo | invitees + calendar_event |
| 4 | POST | `/api/v1/groups/:group_id/conferences` | create | todo | group twin |
| 5 | GET | `/api/v1/conferences` | for_user | todo | current user's conferences, `state=live` filter |
| 6 | POST | `/api/v1/courses/:course_id/conferences/:conference_id/recording_ready` | recording_ready | todo | JWT webhook, **no auth user** |
| 7 | POST | `/api/v1/groups/:group_id/conferences/:conference_id/recording_ready` | recording_ready | todo | group twin |

## 3. Permission matrix (verified)

| Route(s) | Check | Who passes |
|---|---|---|
| index (1–2) | `:read` on context + conferences tab enabled + configured provider | course/group readers |
| create (3–4) | `:create` on `context.web_conferences.temp_record` | users who may create conferences in the context (teachers/moderators) |
| for_user (5) | authenticated user, no context ability | any logged-in user (their own rows only) |
| recording_ready (6–7) | **none** — `skip_before_action :load_user, :require_user`; auth is a JWT signed with the provider secret | the external provider |

Negatives: anonymous 401 (routes 1–5); unconfigured provider → `require_config` failure (redirect in HTML; for API pin a 403-style error, §8); non-reader 403; recording_ready with a bad JWT → 401, good JWT but wrong meeting → 422 (verified statuses, §6).

## 4. Job/storage dependency model

**Canvas**: creating a conference can enqueue attendee-sync work (`sync_attendees`), the join flow starts the provider session on demand, and BigBlueButton recordings are fetched from the provider (`preload_recordings`, `recording_ready!`). The **only async-adjacent API surface here is the recordings webhook**, which is a state flip, not a job.

**star-lms**: no provider exists in the workspace. Recommendation: implement the **data plane** fully (tables below, JSON CRUD semantics, participant invites, calendar-event linking via the existing `calendar_events` table) and treat the provider as a swappable adapter trait with a "not configured" default. `require_config` then fails for every context until a provider is configured — which matches Canvas's own behavior for an unconfigured site and keeps the permission/negative tests meaningful. `recording_ready` verifies a JWT (HS-signed with a per-provider secret, configurable in dev/test) and flips the row's recordings-ready flag + 202; recording **listing** returns `recordings: []` until then. No `star-jobs` rows are needed for the first cut; if attendee sync is desired later, it slots into the shared enqueue convention.

## 5. Data model and migration needs

One migration — **"010x, coordinator-assigned"** — creating two tables:

- `web_conferences` — `id`, `context_type TEXT NOT NULL` ('Course'/'Group'), `context_id BIGINT NOT NULL`, `user_id NOT NULL REFERENCES users(id)` (creator), `conference_type TEXT NOT NULL` (validated against configured types; 'LtiConference' requires a valid LTI tool — validated in Canvas), `conference_key TEXT` (provider meeting id), `title TEXT NOT NULL`, `description TEXT`, `duration INTEGER`, `start_at TIMESTAMPTZ`, `end_at TIMESTAMPTZ`, `sync_attendees BOOLEAN NOT NULL DEFAULT false`, `user_settings JSONB NOT NULL DEFAULT '{}'`, `lti_settings JSONB NOT NULL DEFAULT '{}'`, `settings JSONB NOT NULL DEFAULT '{}'` (includes `default_return_url`), `recordings JSONB` (provider recording metadata, filled by webhook/pull), `recordings_ready BOOLEAN NOT NULL DEFAULT false`, `workflow_state TEXT NOT NULL DEFAULT 'active'`, timestamps. Index `(context_type, context_id)`. (Canvas has no `state :` macros on this model; `active` is a type-scope — a simple state column with 'active'/'closed'/'finished' values suffices; pin exact set at implementation, §8.)
- `web_conference_participants` — `id`, `web_conference_id NOT NULL REFERENCES web_conferences(id) ON DELETE CASCADE`, `user_id NOT NULL REFERENCES users(id)`, `participation_type TEXT NOT NULL CHECK (participation_type IN ('invitee','attendee','observer'))` (verified values: invitees and attendees via scopes; observers referenced in create params), timestamps; unique `(web_conference_id, user_id, participation_type)`.

Reuses existing `users`, `groups`, `courses`, `calendar_events` tables; no changes to them.

## 6. Behavior contracts

- **index (1–2)**: users with granular manage-course-content rights see all `active` conferences; everyone else sees the ones they participate in (verified branch on `RoleOverride::GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS`). Response items carry `recordings` (preloaded for BBB in Canvas; `[]` in star-lms until the webhook fires) and `bbb_recording_enabled`-style flags only when a provider is configured. Paginated.
- **create (3–4)**: params `web_conference` permitting `start_at, end_at, sync_attendees, title, duration, description, conference_type, user_settings (free-form), lti_settings (free-form)` (verified permit list); a nested `web_conference[calendar_event]` hash is extracted and creates a linked calendar event (existing `calendar_events` table); invitees via `user[]` ids or `user[all]`; `observers[remove]` prunes observer participants. Unknown `conference_type` → validation error (400). Response: the conference JSON with `permissions`.
- **for_user (5)**: the current user's conferences across all contexts (participant or creator); `state=live` filters to live ones. No context permission.
- **recording_ready (6–7)**: body carries `signed_parameters` — a JWT signed with the provider secret (`config[:secret_dec]`). Verified outcomes: valid JWT whose `meeting_id` equals the conference's `conference_key` → `recording_ready!` then **202 Accepted with `[]`**; valid JWT, wrong meeting → **422** with `signed_id_invalid_json`; invalid JWT → **401** with `invalid_jwt_token_json`. No user session is involved.
- 404s: conference id outside the context (scoped find); 401 anonymous on 1–5; 403 failed abilities.

## 7. Test plan

1. CRUD plane: create conference in course + group with invitees, `user[all]`, nested calendar_event (assert a `calendar_events` row is linked); index shows it to managers and to invitees but not to a non-participating student (the granular-permission branch).
2. for_user: creator + invitee see it; unrelated user does not; `state=live` filter on an active row.
3. recording_ready: forge the HS256 JWT with the test secret — happy path 202 `[]` and `recordings_ready=true`; wrong `meeting_id` → 422 pinned body; garbage token → 401; no auth header needed.
4. Provider-unconfigured negatives: index/create return the pinned `require_config` error until the test configures a fake provider.
5. Permission negatives: anonymous 401 ×5 (routes 1–5); student create → 403; cross-context conference id → 404; tab-disabled context.

## 8. Open questions

1. Exact `workflow_state` value set for web_conferences (no `state :` macros found; `active` is scope-based) — pin from the schema at implementation.
2. `require_config` failure rendering for API requests (HTML redirects) — recommend 403 with an error body, pinned.
3. Which provider to fake first (a stub adapter returning deterministic join URLs/recordings) — recommend a `ConferenceProvider` trait with a test double and no real BBB/Zoom client in wave 3.
4. The non-API `join` flow (user-facing URL negotiation with signature) — out of REST scope here, needed for UI parity later.
