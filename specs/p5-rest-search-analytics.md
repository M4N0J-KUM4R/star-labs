# P5 REST Spec — Search (recipients, all_courses, rubrics), smart search, and the analytics finding

- **Issue**: #113 (epic `specs`), families: search + smart_search + **analytics (investigated)**. Companion: `p5-rest-misc-index.md` (shared job-model convention §3).
- **Baseline**: fresh parity run at `526de19`; `search` 0/4, `smart_search` 0/3, analytics 0 routes.
- **Target crate**: new `star-search` crate (recipients/all_courses/rubrics + smartsearch routes); analytics requires no crate (see §1).

## 1. The analytics finding (important)

The issue lists "analytics" as a family, but **this Canvas checkout contains no analytics REST controller and the parity manifest contains zero analytics routes** (verified: no controller matching `analytics` except `analytics_hub`, whose single route is a non-API HTML page `GET /analytics_hub`, also absent from the manifest; `analytics_api` does not exist). Where analytics lives in modern Canvas:

1. **GraphQL**: `app/graphql/types/student_summary_analytics_type.rb` and `app/graphql/loaders/course_student_analytics_loader.rb` (verified to exist) — the New Analytics data plane is GraphQL, and GraphQL parity is 0/390 files workspace-wide, tracked separately from this issue.
2. **LTI**: `app/models/lti/analytics_service.rb` — analytics can be delegated to an external LTI tool.
3. **`analytics_hub`** (`GET /analytics_hub`, gated by `:view_analytics_hub`, verified) — an HTML shell page, UI-parity scope.

**Recommendation**: analytics is **out of REST scope for issue #113's spec** — there is nothing to spec beyond this finding. The wave-3 "analytics" issue, if it exists, should either be re-scoped to the GraphQL loaders or closed as "no REST surface". Smart search (§4) and `study_assist` (§5) are the AI/search family that actually has REST routes. This resolves the issue's mention of "analytics MR jobs": the historical analytics 1 REST API (mr job backed) is **not present** in this Canvas version.

## 2. Fresh coverage

| Controller | Done | Total | Coverage | UI-referenced |
|---|---:|---:|---:|---|
| `search` | 0 | 4 | 0.0% | no |
| `smart_search` | 0 | 3 | 0.0% | **all 3 yes** |

## 3. Search routes (4, all todo)

| # | Verb | Canvas path | Action | Status | Notes |
|---|---|---|---|---|---|
| 1 | GET | `/api/v1/search/recipients` | recipients | todo | the main recipient picker |
| 2 | GET | `/api/v1/conversations/find_recipients` | recipients (alias) | todo | legacy alias, same action |
| 3 | GET | `/api/v1/search/all_courses` | all_courses | todo | **public**, course catalog |
| 4 | GET | `/api/v1/search/rubrics` | rubrics | todo | course-context rubric matcher |

### 3.1 Permission matrix

| Route | Check (verified) |
|---|---|
| recipients (1–2) | authenticated user (`get_context` exempt); visibility is computed per-match by the AddressBook (shared courses/groups/enrollments), `messageable_only` adds the `send_messages` permission check per candidate |
| all_courses (3) | `skip_before_action :require_user` — **anonymous allowed**; but requires the root account's `enable_course_catalog?` setting, else `render_unauthorized_action` (401) |
| rubrics (4) | course context, `get_context` (course reader) |

### 3.2 Behavior contracts

- **recipients**: params (all verified) `search` (whitespace-separated terms, AND semantics), `context` (e.g. `course_3`/`group_4`), `exclude[]` (ids or `course_3`/`group_4` prefixed), `type` (`user`|`context`), `user_id` (exact lookup, at most one result), `from_conversation_id`, `permissions[]` (e.g. `send_messages`; the response carries per-context `permissions` granted to the caller), `messageable_only`, `per_page` (0/negative → nil/unset). Response: mixed user/context entries. star-lms implementation: compute recipient visibility from shared `enrollments`/`group_memberships` (both tables exist); no new tables.
- **all_courses**: only `indexed: true`, `workflow_state='available'` courses of the root account, ordered by `created_at`; `search` (name LIKE), `public_only`, `open_enrollment_only` filters; JSON variant returns the course array (`as_json`); HTML variant paginates 12/page (API path uses standard per_page). 401 body when the catalog setting is off. Anonymous callers allowed.
- **rubrics**: `Rubric publicly_reusable` plus course rubrics matching `q`, then a title downcase-match filter (verified double filter); response is a **bare array** of rubrics — no envelope.

## 4. Smart search (3 routes, all todo — all UI-referenced)

| # | Verb | Canvas path | Action | Status |
|---|---|---|---|---|
| 1 | GET | `/api/v1/courses/:course_id/smartsearch` | search | todo |
| 2 | GET | `/api/v1/courses/:course_id/smartsearch/index_status` | index_status | todo |
| 3 | GET | `/api/v1/courses/:course_id/smartsearch/log` | log | todo |

- **Permissions** (verified): `require_context` + `SmartSearch.smart_search_available?(context)` (feature-gated; failure → `render_unauthorized_action`/401) + search tab enabled for `search`/`show`; `index_status` checks availability but not the tab.
- **search**: `q` param **required** — missing → `400 {"error": "missing 'q' param"}` (verified). `filter[]` narrows object types; `include[]` controls payload. Results carry `title`, `body`, `html_url`, `distance` (relevance distance; the API doc example shows numeric strings). When the course index is incomplete the response adds `status: "index_incomplete"` and `indexing_progress` (verified). Results are filtered per-user by `show_in_search_for_user?` before returning.
- **index_status**: `{"status": "indexed"|"indexing", "progress": <number>}` (verified) — the polling endpoint the UI hits while indexing.
- **log**: a telemetry no-op that Canvas explicitly documents as "TODO: do something more" — accepts `a, c, course_id, oid, ot, q` and logs; star-lms should accept and discard, returning 200 with an empty body (pin it).

### 4.1 Job/storage model and star-lms approach

Canvas tracks per-course vector-index readiness behind `SmartSearch.check_course(course) → (ready, progress)`; the actual indexing is provider-side (the IgniteAI feature). **star-lms recommendation**: a `smart_search_indexes` table (`course_id PK/unique, status TEXT CHECK ('indexed','indexing'), progress REAL, updated_at`) driven through the shared jobs convention (`tag = "SmartSearch#index_course"`) — or, for a first cut with no AI backend, a trivial implementation that marks courses `indexed, progress 100` on first touch and returns keyword-search results computed with SQL `ILIKE` over pages/assignments/announcements/discussion topics, keeping the exact response shape (`distance` approximated, documented as a deviation). Either way the **API contract is what this spec pins**; the retrieval engine is swappable.

## 5. Study assist (adjacent, flagged for a future issue)

`POST /api/v1/courses/:course_id/study_assist` (`study_assist#create`, UI-referenced) sits in the same AI family: guards `require_context`, `require_study_assist_enabled`, `require_student_access`, `require_cedar_enabled` (all verified). It is **not** in issue #113's list of families, so it is documented here only to claim it for a follow-up issue rather than leave it orphaned.

## 6. Data model and migration needs

One migration — **"010x, coordinator-assigned"** — for smart search only (search routes 1–4 need no new tables):

- `smart_search_indexes` — `course_id BIGINT PRIMARY KEY REFERENCES courses(id) ON DELETE CASCADE`, `status TEXT NOT NULL DEFAULT 'indexing' CHECK (status IN ('indexed','indexing'))`, `progress REAL NOT NULL DEFAULT 0`, `updated_at TIMESTAMPTZ NOT NULL`. Optionally `smart_search_logs` if the `log` route should persist rather than discard (recommend discard; no table).

## 7. Test plan

1. recipients: seed two shared courses + a group; assert AND-term matching, `context` scoping, `exclude[]` (plain and prefixed ids), `user_id` exact single result, `permissions[]=send_messages` reflected per context, `messageable_only` filtering; anonymous → 401.
2. all_courses: catalog on → anonymous 200 with available+indexed courses, `search`/`public_only`/`open_enrollment_only` filters; catalog off → 401 body (pinned).
3. rubrics: publicly-reusable + course rubrics match `q`; non-matching excluded; bare-array response; non-course-reader → 403.
4. smartsearch: missing `q` → 400 exact body; `index_status` before/after indexing (SQL-advanced); results carry title/body/html_url/distance; per-user visibility filter excludes hidden items; feature off → 401; `log` returns 200 empty for arbitrary params.
5. Permission negatives: anonymous 401 (recipients, smartsearch family); student in another course gets no cross-course recipients; smart search tab disabled → error.

## 8. Open questions

1. Recipient response envelope (mixed user/prereg entries) — pin against Canvas's `search_result` serializer; it differs subtly from `user_json`.
2. `distance` semantics (Levenshtein-style vs provider score) — star-lms approximation must be documented per-response or per-spec deviation note.
3. Whether the wave-3 search issue includes the conversations `find_recipients` alias only or also needs conversation-specific `from_conversation_id` context logic — recommend alias-only first.
4. Confirm with the coordinator whether a separate "analytics" wave-3 issue should be re-scoped/closed per §1.
