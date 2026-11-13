# P5 REST Spec — Collaborations

- **Issue**: #113 (epic `specs`), family: collaborations. Companion: `p5-rest-misc-index.md`.
- **Baseline**: fresh parity run at `526de19`; `collaborations` at **0%** (0/7).
- **Target crate**: new `star-collaborations` crate. (The `collaborative_atoms`/feature-flag angle raised in the issue does **not** exist in this manifest — 0 matches for `collaborative_atom`; the only collaboration surface is the routes below.)

## 1. Overview and fresh coverage

| Controller | Done | Total | Coverage |
|---|---:|---:|---:|
| `collaborations` | 0 | 7 | 0.0% |

Collaborations link a course/group to an external document (historically Google Docs/LTI tools). Importantly, the **API surface is read + delete only**: create and update exist in Canvas solely as HTML-form actions (they take `contentItems` LTI deep-linking payloads) and are not in the API manifest. The API's job is to list collaborations with their collaborators, list potential collaborators, list members, and delete. Guard chain (verified): `require_context` (except `members`), `require_collaboration_and_context` (members), `require_collaborations_configured` (redirects with a flash error when no collaboration type is configured — for API calls star-lms should map this to a 403-style error response and pin the choice), and `reject_student_view_student`.

## 2. Route inventory (7 routes, all todo)

| # | Verb | Canvas path | Action | Status | Notes |
|---|---|---|---|---|---|
| 1 | GET | `/api/v1/courses/:course_id/collaborations` | api_index | todo | envelope `collaborations` |
| 2 | GET | `/api/v1/groups/:group_id/collaborations` | api_index | todo | group twin |
| 3 | GET | `/api/v1/collaborations/:id/members` | members | todo | paginated collaborators |
| 4 | GET | `/api/v1/courses/:course_id/potential_collaborators` | potential_collaborators | todo | `@returns [User]` |
| 5 | GET | `/api/v1/groups/:group_id/potential_collaborators` | potential_collaborators | todo | group twin |
| 6 | DELETE | `/api/v1/courses/:course_id/collaborations/:id` | destroy | todo | `delete_doc` param |
| 7 | DELETE | `/api/v1/groups/:group_id/collaborations/:id` | destroy | todo | group twin |

Non-API companions (HTML `index`/`create`/`update`/`lti_index`, plus `Collaboration` polymorphic context) are out of scope for REST parity but inform the data model.

## 3. Permission matrix (verified)

| Route(s) | Ability | Who passes |
|---|---|---|
| api_index (1–2) | `:read` on context, **and** the collaborations tab enabled | course/group readers when the tab is enabled; `render_tab_disabled` otherwise |
| members (3) | `:read` on the collaboration record | anyone who can read the collaboration (context readers via record policy) |
| potential_collaborators (4–5) | `:read_roster` on context | course: enrolled users per `potential_collaborators_for(current_user)`; group: group members plus the parent course's admins |
| destroy (6–7) | record lookup is scoped `@context.collaborations.find` + destroy policy | effectively context managers; cross-context id → 404 |

Negatives: anonymous 401 ×7; tab disabled → tab-disabled error response; non-member 403; collaboration id from another context → 404.

## 4. Job/storage dependency model

**None for the API surface.** Listing, membership, and destroy are synchronous. (Canvas's create path can spawn `Collaboration::Association` callbacks and LTI launches, but those are not API routes.) No attachments: `url` is an external link stored on the row. This is the second-simplest family after polling.

## 5. Data model and migration needs

One migration — **"010x, coordinator-assigned"** — creating:

- `collaborations` — `id`, `context_type TEXT NOT NULL` ('Course'/'Group'; star-lms also accepts 'course'/'group' lowercase, matching its existing polymorphic columns), `context_id BIGINT NOT NULL`, `user_id NOT NULL REFERENCES users(id)` (creator), `title TEXT NOT NULL`, `description TEXT`, `url TEXT`, `collaboration_type TEXT NOT NULL` (e.g. 'ExternalToolCollaboration'), `workflow_state TEXT NOT NULL DEFAULT 'active' CHECK (workflow_state IN ('active','deleted'))` (verified states), `lti_id`/`lti_settings JSONB` for LTI-backed ones, timestamps. Index `(context_type, context_id)` with the state filter used by the `active` scope.
- `collaborators` — `id`, `collaboration_id NOT NULL REFERENCES collaborations(id) ON DELETE CASCADE`, `context_type` ('User'/'Group'), `context_id BIGINT`, timestamps; unique on `(collaboration_id, context_type, context_id)`. Verified semantics: `Collaborator` rows point at both users and groups (the members endpoint preloads `:group, :user`).

No changes to existing tables.

## 6. Behavior contracts

- **api_index (1–2)**: paginated list of `active` collaborations for the context; each item serialized with `collaborator_ids` in the method set (verified `as_json(methods: [:collaborator_ids], ...)` on the HTML create path — same serializer family). Tab disabled → Canvas `render_tab_disabled`; star-lms should return the workspace's tab-disabled error and pin it with a test.
- **members (3)**: `include[]=collaborator_lti_id` adds LTI ids (UserPastLtiId preload); paginated; user-collaborators serialize as users, group-collaborators as groups — the response mixes shapes and star-lms must document/pin the exact envelope (a flat array; Canvas renders `collaborator_json` per row).
- **potential_collaborators (4–5)**: `@returns [User]`, ordered by `sortable_name`, paginated. For groups: members **plus the admins of the parent course** (verified comment). For courses: all enrolled users the caller may see (teacher sees all; student sees per-roster-visibility rules via `potential_collaborators_for`).
- **destroy (6–7)**: `delete_doc=true` also deletes the external document (best-effort; for star-lms with no external provider, accept the param and no-op it); the collaboration soft-deletes to `deleted` and disappears from api_index. Response: Canvas's HTML path redirects — the API twin should return the workspace's standard delete confirmation; pin the choice (recommend 200 with the row JSON, consistent with account_reports#destroy).
- 404 for ids outside the context (scoped find), 401 anonymous, 403 failed ability.

## 7. Test plan

1. Seed course + group collaborations with user and group collaborators; api_index returns active ones with `collaborator_ids`; soft-deleted excluded.
2. members: pagination; mixed user/group rows; `include[]=collaborator_lti_id` (absent LTI id → null field).
3. potential_collaborators: course variant returns enrolled users sorted by name; group variant returns members + parent-course admins (assert the admin appears — the quirk).
4. destroy: soft-delete; `delete_doc` accepted; subsequent api_index excludes; id from other course → 404.
5. Permission negatives: anonymous 401 ×7; student without `read_roster` on a restricted roster 403 (route 4/5); tab-disabled context returns the pinned error; cross-context collaboration id 404 (route 3 via `require_collaboration_and_context`).

## 8. Open questions

1. Exact member-serialization envelope for group collaborators (group JSON vs user JSON of members) — pin against Canvas's `collaborator_json` at implementation.
2. `require_collaborations_configured` behavior for API requests (Canvas redirects an HTML request; an API call should not 302) — recommend 403 `{"errors": ["Collaborations have not been enabled for this Canvas site"]}` and pin it.
3. Whether any star-lms account will ever configure a collaboration type — if not, the tab-enabled guard makes most of this family return errors in practice; tests must configure the feature/tab explicitly.
