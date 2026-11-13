# Spec P5: REST Platform Controllers (conversations, calendar, channels, favorites, misc)

- **Issue:** #46 (epic `specs`)
- **Branch / worktree:** `p5/specs-46` at `origin/main` `3dc34f6`
- **Status:** SPECIFICATION ONLY — no Rust code, no migrations, no cargo. Additive migration numbers 0108+ are proposals; agents never self-assign numbers (protocol: issue #20, coordinator reserves via `orchestration/migration-reservations.json`, current `max_merged` = `0107`, `next_available` = `0108`).
- **Canvas source read:** `canvas-lms` at `app/controllers/{conversations,calendar_events_api,appointment_groups,blackout_dates,learning_object_dates,communication_channels,favorites,profile,release_notes,tabs,admins,sub_accounts}_controller.rb` plus `config/routes.rb` API v1 blocks (conversation scope lines 2033–2049, communication_channels 2051–2064, calendar_events_api 2089–2110, appointment_groups 2108–2117, favorites 2287–2295, release_notes 2801–2810, blackout_dates 2965–2979, tabs 1361–1366, learning_object_dates 1857–1876, sub_accounts 1959–1962, admins 1995–2000, profile 2026–2027, conversations find_recipients 2028–2029).

## 1. Overview

This spec covers twelve Canvas REST platform controllers that sit outside the curriculum (assignments/quizzes/modules) and infra (auth, files, GraphQL) tracks: conversations, calendar events (incl. timetables and reservations), appointment groups, blackout dates, learning-object date details (the "Assign To" API), communication channels (incl. account bounce reports), favorites (courses and groups), profile avatars, release notes, tabs, admins, and sub-accounts.

Fresh coverage, regenerated at `main 3dc34f6` with
`python3 tools/parity/parity.py report --manifest parity/canvas_manifest.json --star-src . --out /tmp/parity-fresh`:

| Metric | Value |
|---|---|
| Workspace REST routes implemented | **471 / 1403 (33.6%)** |
| REST routes referenced from `ui/` | **258 / 503 (51.3%)** |
| Scope of this spec (12 controllers) | **61 / 108 (56.5%)** — **47 missing** |

Per-controller (implemented / total, from `/tmp/parity-fresh/report.md`):

| Controller | Impl/Total | Coverage | Missing |
|---|---:|---:|---:|
| `conversations` | 9/16 | 56.2% | 7 |
| `calendar_events_api` | 10/15 | 66.7% | 5 |
| `appointment_groups` | 6/8 | 75.0% | 2 |
| `blackout_dates` | 6/13 | 46.2% | 7 |
| `learning_object_dates` | 8/15 | 53.3% | 7 |
| `communication_channels` | 4/12 | 33.3% | 8 |
| `favorites` | 4/8 | 50.0% | 4 |
| `profile` | 1/2 | 50.0% | 1 |
| `release_notes` | 6/8 | 75.0% | 2 |
| `tabs` | 1/5 | 20.0% | 4 |
| `admins` | 4/4 | 100% | 0 |
| `sub_accounts` | 2/2 | 100% | 0 |

Existing star-labs implementations live in: `star-conversations` (conversations), `star-calendar` (calendar events, appointment groups; blackout-date flag on calendar events), `star-learning-dates` (date details), `star-notifications` (communication channels, bounce report), `star-dashboard` (favorites courses, release notes), `star-users` (profile), `star-courses` (course tabs), `star-accounts` (admins, sub_accounts). DB: favorites + release notes in `star-db` migration `0097`, communication channels `0045` + `0093`, blackout dates `0091`, course settings (incl. tab_configuration JSON) `0055`, learning object dates `0090`, calendar events deep `0095`.

Completion of the 47 missing routes would move the scope to 108/108 (100%) and the workspace to 518/1403 (36.9%), with 17 of the 47 being `ui_referenced` (batch_update, next_appointment, .csv bounce report, tabs×3, release publish/unpublish, convert_tag_overrides×4, groups favorites list/add/remove, find_recipients, timetable set).

## 2. Route inventory

Status key: DONE = route registered in the workspace and matched by the parity report; MISS = to be implemented. `ui` = referenced from Canvas `ui/` sources (prioritized).

### 2.1 conversations (star-conversations)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/conversations` | index | DONE `ui` | scopes `unread|starred|sent|archived|inbox`, `filter[]`, `filter_mode`, `include_all_conversation_ids` |
| POST | `/api/v1/conversations` | create | DONE `ui` | reuse of private conversation, async mode returns 202 + `X-Conversation-Batch-Id` |
| GET | `/api/v1/conversations/batches` | batches | DONE | |
| GET | `/api/v1/conversations/unread_count` | unread_count | DONE `ui` | value is a **string** |
| GET | `/api/v1/conversations/:id` | show | DONE | |
| DELETE | `/api/v1/conversations/:id` | destroy | DONE | |
| POST | `/api/v1/conversations/:id/add_message` | add_message | DONE | |
| POST | `/api/v1/conversations/:id/add_recipients` | add_recipients | DONE | |
| POST | `/api/v1/conversations/:id/remove_messages` | remove_messages | DONE | |
| PUT | `/api/v1/conversations/:id` | update | **MISS** | star/subscribe/archive; see §5.1 |
| PUT | `/api/v1/conversations` | batch_update | **MISS** `ui` | 500-conversation cap, returns Progress; see §5.2 |
| POST | `/api/v1/conversations/mark_all_as_read` | mark_all_as_read | **MISS** | see §5.3 |
| GET | `/api/v1/conversations/deleted` | deleted_index | **MISS** | site-admin; see §5.4 |
| PUT | `/api/v1/conversations/restore` | restore_message | **MISS** | site-admin; see §5.5 |
| DELETE | `/api/v1/conversations/:id/delete_for_all` | delete_for_all | **MISS** | site-admin `manage_students`; see §5.6 |
| GET | `/api/v1/conversations/find_recipients` | find_recipients | **MISS** `ui` | deprecated alias; see §5.7 |

### 2.2 calendar_events_api (star-calendar)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/calendar_events` | index | DONE `ui` | |
| GET | `/api/v1/users/:user_id/calendar_events` | user_index | DONE `ui` | observer scoping (§3.2) |
| POST | `/api/v1/calendar_events` | create | DONE `ui` | |
| GET | `/api/v1/calendar_events/:id` | show | DONE `ui` | |
| PUT | `/api/v1/calendar_events/:id` | update | DONE `ui` | |
| DELETE | `/api/v1/calendar_events/:id` | destroy | DONE `ui` | |
| POST | `/api/v1/calendar_events/:id/reservations` | reserve | DONE | participant defaults to current user |
| POST | `/api/v1/calendar_events/:id/reservations/:participant_id` | reserve | **MISS** | explicit participant; see §5.8 |
| GET | `/api/v1/calendar_events/:id/participants` | participants | **MISS** | paginated; see §5.9 |
| GET | `/api/v1/calendar_events/visible_contexts` | visible_contexts | DONE `ui` | |
| POST | `/api/v1/calendar_events/save_selected_contexts` | save_selected_contexts | DONE `ui` | |
| POST | `/api/v1/calendar_events/save_enabled_account_calendars` | save_enabled_account_calendars | DONE `ui` | |
| GET | `/api/v1/courses/:course_id/calendar_events/timetable` | get_course_timetable | **MISS** | see §5.10 |
| POST | `/api/v1/courses/:course_id/calendar_events/timetable` | set_course_timetable | **MISS** | see §5.11 |
| POST | `/api/v1/courses/:course_id/calendar_events/timetable_events` | set_course_timetable_events | **MISS** | see §5.12 |

### 2.3 appointment_groups (star-calendar)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/appointment_groups` | index | DONE `ui` | scope reservable/manageable |
| POST | `/api/v1/appointment_groups` | create | DONE | |
| GET | `/api/v1/appointment_groups/:id` | show | DONE `ui` | |
| PUT | `/api/v1/appointment_groups/:id` | update | DONE | |
| DELETE | `/api/v1/appointment_groups/:id` | destroy | DONE `ui` | cancel_reason |
| GET | `/api/v1/appointment_groups/:id/users` | users | DONE | |
| GET | `/api/v1/appointment_groups/:id/groups` | groups | **MISS** | see §5.13 |
| GET | `/api/v1/appointment_groups/next_appointment` | next_appointment | **MISS** `ui` | see §5.14 |

### 2.4 blackout_dates (star-calendar)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/courses/:course_id/blackout_dates` | index | DONE `ui` | |
| POST | `/api/v1/courses/:course_id/blackout_dates` | create | DONE `ui` | |
| PUT | `/api/v1/courses/:course_id/blackout_dates` | bulk_update | DONE `ui` | |
| DELETE | `/api/v1/courses/:course_id/blackout_dates/:id` | destroy | DONE | |
| GET | `/api/v1/accounts/:account_id/blackout_dates` | index | DONE | |
| POST | `/api/v1/accounts/:account_id/blackout_dates` | create | DONE | |
| GET | `/api/v1/courses/:course_id/blackout_dates/:id` | show | **MISS** | see §5.15 |
| GET | `/api/v1/accounts/:account_id/blackout_dates/:id` | show | **MISS** | see §5.15 |
| GET | `/api/v1/courses/:course_id/blackout_dates/new` | new | **MISS** | see §5.16 |
| GET | `/api/v1/accounts/:account_id/blackout_dates/new` | new | **MISS** | see §5.16 |
| PUT | `/api/v1/courses/:course_id/blackout_dates/:id` | update | **MISS** | see §5.17 |
| PUT | `/api/v1/accounts/:account_id/blackout_dates/:id` | update | **MISS** | see §5.17 |
| DELETE | `/api/v1/accounts/:account_id/blackout_dates/:id` | destroy | **MISS** | see §5.18 |

### 2.5 learning_object_dates (star-learning-dates)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/courses/:course_id/assignments/:assignment_id/date_details` | show | DONE `ui` | |
| PUT | `/api/v1/courses/:course_id/assignments/:assignment_id/date_details` | update | DONE | |
| GET | `/api/v1/courses/:course_id/quizzes/:quiz_id/date_details` | show | DONE `ui` | |
| PUT | `/api/v1/courses/:course_id/quizzes/:quiz_id/date_details` | update | DONE | |
| GET | `/api/v1/courses/:course_id/discussion_topics/:discussion_topic_id/date_details` | show | DONE `ui` | |
| PUT | `/api/v1/courses/:course_id/discussion_topics/:discussion_topic_id/date_details` | update | DONE | |
| GET | `/api/v1/courses/:course_id/pages/:url_or_id/date_details` | show | DONE `ui` | |
| PUT | `/api/v1/courses/:course_id/pages/:url_or_id/date_details` | update | DONE `ui` | |
| GET | `/api/v1/courses/:course_id/files/:attachment_id/date_details` | show | **MISS** | behind `differentiated_files` site-admin flag; see §5.19 |
| PUT | `/api/v1/courses/:course_id/files/:attachment_id/date_details` | update | **MISS** | see §5.20 |
| GET | `/api/v1/courses/:course_id/modules/:context_module_id/date_details` | show | **MISS** | no update/convert counterpart in Canvas; see §5.21 |
| PUT | `/api/v1/courses/:course_id/assignments/:assignment_id/date_details/convert_tag_overrides` | convert_tag_overrides_to_adhoc_overrides | **MISS** `ui` | see §5.22 |
| PUT | `/api/v1/courses/:course_id/quizzes/:quiz_id/date_details/convert_tag_overrides` | convert | **MISS** `ui` | see §5.22 |
| PUT | `/api/v1/courses/:course_id/discussion_topics/:discussion_topic_id/date_details/convert_tag_overrides` | convert | **MISS** `ui` | see §5.22 |
| PUT | `/api/v1/courses/:course_id/pages/:url_or_id/date_details/convert_tag_overrides` | convert | **MISS** `ui` | see §5.22 |

### 2.6 communication_channels (star-notifications)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/users/:user_id/communication_channels` | index | DONE | |
| POST | `/api/v1/users/:user_id/communication_channels` | create | DONE | |
| DELETE | `/api/v1/users/:user_id/communication_channels/:id` | destroy | DONE | |
| GET | `/api/v1/accounts/:account_id/bounced_communication_channels` | bouncing_channel_report | DONE `ui` | JSON report |
| POST | `/api/v1/users/:user_id/communication_channels/:id` | reset_bounce_count | **MISS** | see §5.23 |
| DELETE | `/api/v1/users/:user_id/communication_channels/:type/:address` | destroy | **MISS** | greedy `address` constraint `[^/?]+`; see §5.24 |
| DELETE | `/api/v1/users/self/communication_channels/push` | delete_push_token | **MISS** | body `push_token`; see §5.25 |
| GET | `/api/v1/accounts/:account_id/bounced_communication_channels.csv` | bouncing_channel_report | **MISS** `ui` | CSV; see §5.26 |
| GET | `/api/v1/accounts/:account_id/unconfirmed_communication_channels` | unconfirmed_channel_report | **MISS** | see §5.27 |
| GET | `/api/v1/accounts/:account_id/unconfirmed_communication_channels.csv` | unconfirmed_channel_report | **MISS** | CSV; see §5.27 |
| POST | `/api/v1/accounts/:account_id/bounced_communication_channels/reset` | bulk_reset_bounce_counts | **MISS** | see §5.28 |
| POST | `/api/v1/accounts/:account_id/unconfirmed_communication_channels/confirm` | bulk_confirm | **MISS** | see §5.29 |

### 2.7 favorites (star-dashboard)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/users/self/favorites/courses` | list_favorite_courses | DONE `ui` | empty favorites → enrolled courses |
| POST | `/api/v1/users/self/favorites/courses/:id` | add_favorite_course | DONE `ui` | |
| DELETE | `/api/v1/users/self/favorites/courses/:id` | remove_favorite_course | DONE `ui` | idempotent 200 `{}` |
| DELETE | `/api/v1/users/self/favorites/courses` | reset_course_favorites | DONE | |
| GET | `/api/v1/users/self/favorites/groups` | list_favorite_groups | **MISS** | see §5.30 |
| POST | `/api/v1/users/self/favorites/groups/:id` | add_favorite_groups | **MISS** | see §5.31 |
| DELETE | `/api/v1/users/self/favorites/groups/:id` | remove_favorite_groups | **MISS** | see §5.32 |
| DELETE | `/api/v1/users/self/favorites/groups` | reset_groups_favorites | **MISS** | see §5.33 |

### 2.8 profile (star-users)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/users/:user_id/profile` | settings | DONE `ui` | |
| GET | `/api/v1/users/:user_id/avatars` | profile_pics | **MISS** | see §5.34 |

### 2.9 release_notes (star-dashboard)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/release_notes` | index | DONE `ui` | |
| POST | `/api/v1/release_notes` | create | DONE | |
| GET | `/api/v1/release_notes/latest` | latest | DONE `ui` | |
| GET | `/api/v1/release_notes/unread_count` | unread_count | DONE | |
| PUT | `/api/v1/release_notes/:id` | update | DONE | |
| DELETE | `/api/v1/release_notes/:id` | destroy | DONE `ui` | |
| PUT | `/api/v1/release_notes/:id/published` | publish | **MISS** `ui` | see §5.35 |
| DELETE | `/api/v1/release_notes/:id/published` | unpublish | **MISS** `ui` | see §5.36 |

### 2.10 tabs (star-courses)

| Method | Canvas path | Action | Status | Notes |
|---|---|---|---|---|
| GET | `/api/v1/courses/:course_id/tabs` | index | DONE `ui` | |
| GET | `/api/v1/accounts/:account_id/tabs` | index | **MISS** | see §5.37 |
| GET | `/api/v1/groups/:group_id/tabs` | index | **MISS** | see §5.37 |
| GET | `/api/v1/users/:user_id/tabs` | index | **MISS** | see §5.37 |
| PUT | `/api/v1/courses/:course_id/tabs/:tab_id` | update | **MISS** | 400 error contracts; see §5.38 |

### 2.11 admins (star-accounts) — complete

| Method | Canvas path | Action | Status |
|---|---|---|---|
| GET | `/api/v1/accounts/:account_id/admins` | index | DONE |
| POST | `/api/v1/accounts/:account_id/admins` | create | DONE |
| DELETE | `/api/v1/accounts/:account_id/admins/:user_id` | destroy | DONE |
| GET | `/api/v1/accounts/:account_id/admins/self` | self_roles | DONE |

### 2.12 sub_accounts (star-accounts) — complete

| Method | Canvas path | Action | Status |
|---|---|---|---|
| POST | `/api/v1/accounts/:account_id/sub_accounts` | create | DONE `ui` |
| DELETE | `/api/v1/accounts/:account_id/sub_accounts/:id` | destroy | DONE `ui` |

No work remains on admins or sub_accounts; they are listed for completeness of the permission matrix and regression tests (§3.11, §3.12, §6).

## 3. Permission matrices

Canvas permission verbs below are `grants_right?` / `authorized_action` checks read directly from each controller. star-labs equivalents should go through the existing permission layer in `star-permissions` with the same semantics; where a check is "self only", enforce `user_id == current_user` plus the listed account right.

### 3.1 conversations — participant semantics

| Route | Guard | Deny behavior |
|---|---|---|
| index / create / show / update / add_message / add_recipients / remove_messages / destroy / batches / unread_count / mark_all_as_read / batch_update | logged-in user; every conversation is accessed through **ConversationParticipant rows for the current user** (`get_conversation` uses `@current_user.all_conversations`, `message_count > 0` unless allow-deleted) | 404 `RecordNotFound` when not a participant |
| create with `context_code` | if Course: recipients must be messageable in that course (`send_messages_all` check for `course_X_students`/`course_X_observers` suffixed recipients); completed course requires `read_as_admin` | 400 `[{attribute, message}]` |
| add_message on locked/restricted | `process_response` raises `RepliesLockedForUser` | 401 `render_unauthorized_action` |
| delete_for_all | `authorized_action(Account.site_admin, user, :manage_students)` | 401 |
| deleted_index / restore_message | `@current_user.roles(Account.site_admin).include?("admin")` | 401 `render_unauthorized_action` |
| find_recipients | logged-in (delegates to search; any user can search their own messageable users) | 401 |

Student-view students are rejected on all conversations actions (`reject_student_view_student`).

### 3.2 calendar_events_api — context/observer scoping

| Route | Guard | Deny behavior |
|---|---|---|
| index | context codes each resolve to a context the user can read (`get_options`); appointment-group codes get a free ride; limit 10 contexts (+account setting `calendar_contexts_limit`) | 401 for inaccessible code |
| user_index | `require_user_or_observer`: observee `read` right (parent/admin) OR an active observer enrollment linked to the observee; result set is then filtered to observed course ids | 401 |
| show / update / destroy | `authorized_action(@event, user, :read/:update/…)` on the event's context | 401 |
| create | context from `calendar_event[context_code]`; event `create` right | 404 if context missing; 401 |
| reserve | `authorized_action(@event, user, :reserve)` **plus** `check_for_past_signup` (past end_at and no `manage` on the appointment group → 403 "Cannot create or change reservation for past appointment") | 401 / 403 |
| reserve with `participant_id` | only when the user has `manage` on the appointment group may they reserve for another participant; otherwise participant must be the caller's own candidate | 400 `invalid participant` |
| participants | `authorized_action(@event, user, :read_child_events)` | 401 |
| timetable GET/POST, timetable_events | `authorized_action(@context(Course), user, :manage_calendar)` | 401 |
| save_selected_contexts / save_enabled_account_calendars | logged-in self | 401 |

### 3.3 appointment_groups

| Route | Guard |
|---|---|
| index | `reservable_by` (default) or `manageable_by` (scope=manageable) filtering by user rights |
| show / users / groups | `authorized_action(@group, user, :read)`; `users` returns `[]` unless participant_type == "User"; `groups` returns `[]` unless "Group" |
| create / update / destroy | `:manage` / `:update` / `:delete` on the group (destroy also takes `cancel_reason`) |
| next_appointment | logged-in; only groups `reservable_by` the current user are considered |

### 3.4 blackout_dates — granular course-content rights

Canvas uses `RoleOverride::GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS` (the read family) and the granular add/edit/delete verbs:

| Route | Right on context (Course or Account) |
|---|---|
| index / show / new | `manage_course_content` granular **read** family (GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS) |
| create | `manage_course_content_add` |
| update / bulk_update | `manage_course_content_edit` |
| destroy | `manage_course_content_delete` |

### 3.5 learning_object_dates

All actions run `check_authorized_action` **before** the action body: `asset.grants_right?(user, :manage_assign_to)`; if the asset is an Attachment and site-admin feature `differentiated_files` is off → 400 `{"error": "This API does not support files."}` instead of 401. `update` additionally returns 401 when editing restricted due dates/availability dates on child content (checkpoints) in a blueprint-associated course (`editing_restricted?`).

### 3.6 communication_channels

| Route | Guard |
|---|---|
| index | `authorized_action(@user, current, :read)` |
| create | self (`user_can_edit_comm_channels?`) or account `manage`/`manage_user_details`; push type requires access token + SNS-configured developer key |
| destroy (by id or type/address) | `manage_user_details` on target user, OR self with `user_can_edit_comm_channels?` and (not imported or account `edit_institution_email`) |
| reset_bounce_count | channel `grants_right?(user, :reset_bounce_count)` (self or site admin); also honors masquerading `@real_current_user` |
| delete_push_token | self (implicit; requires the push channel to exist) |
| bounced/unconfirmed reports (JSON and CSV) | account `view_bounced_emails`; `account_id` may be `self` for the domain root account |
| bulk_reset_bounce_counts / bulk_confirm | `authorized_action(Account.site_admin, user, :read_messages)` |

### 3.7 favorites

All routes are strictly **self** (`users/self`); no admin override exists. Adding requires the course/group to resolve via `api_find` (404 otherwise, including SIS-id lookup). `remove_favorite_*` returns 200 `{}` when no favorite exists (deliberate, to keep browser state in sync).

### 3.8 profile

`profile_pics`: `authorized_action(@user, current_user, :update_avatar)` — self, or admin with the avatar right on the user's context. Note the route is `/users/:user_id/avatars` and the action resolves the user via `api_find` when `api_request?`.

### 3.9 release_notes

`latest` and `unread_count`: any requester, including anonymous (anonymous users are treated as role `user` and as having seen everything). Everything else (`index`, `create`, `update`, `destroy`, `publish`, `unpublish`) requires site admin with `manage_release_notes` (`require_site_admin_with_permission`).

### 3.10 tabs

| Route | Guard |
|---|---|
| index (course/group/account/user) | `@context.grants_right?(user, :read)`; **failure is 404** (RecordNotFound), not 401 |
| update (course tabs only) | `@context.is_a?(Course)` && `authorized_action(course, user, :manage_course_content_edit)`; non-course context → `return` (empty 200 in Rails, prefer 404 in star-labs, see §7) |

### 3.11 admins (regression reference)

index: `manage_account_memberships`; create: `authorized_action(admin, :create)` on the account; destroy: `:destroy`; self_roles: account `:read` only (no admin-listing right needed).

### 3.12 sub_accounts (regression reference)

create: `manage_account_settings` on the parent account (SIS id additionally needs `manage_sis`, else 401); destroy: account management right, with business-rule 409s for non-empty sub-accounts and 401 for root account.

## 4. Data model & migration needs

All additive, 0108+ proposals, contingent on verifying existing columns at implementation time (per protocol the coordinator reserves the number; nothing below may renumber or drop):

| # | Need | Proposal | Verification required |
|---|---|---|---|
| M1 | `favorites` rows with `context_type = 'Group'` | none — migration `0097` already defines `context_type TEXT` with default `'Course'` and a unique key on `(user_id, context_type, context_id)`; verify no CHECK constraint restricts to Course | `0097_favorites_release_notes.sql` |
| M2 | tabs for account/group/user contexts | none — tabs are computed; per-course `tab_configuration` already stored as JSON on course settings (`0055`); account-level tab customization is not in Canvas's update API (course-only), so read-only listings need no DDL | `0055_course_settings.sql` |
| M3 | timetable data | `courses.timetable_data JSONB` (Canvas keeps `Course#timetable_data` as a serialized hash keyed by section id / `"all"`) | confirm no existing column on `courses` |
| M4 | conversations batch update | reuse existing progress/job infrastructure (`star-jobs`); no DDL expected | check whether `conversation_batches`/progress tables exist; prior conversations migrations |
| M5 | `conversations` update (star/subscribe/workflow_state) | none expected — participant columns should exist for the DONE show/index routes | confirm `conversation_participants` has `subscribed`, `starred`, `workflow_state` |
| M6 | comm-channel `unconfirmed` bulk report + `reset_bounce_count` | none — `0045` has `workflow_state unconfirmed|active|retired`, `0093` added bounce columns; the report is a query | `0045`, `0093` |
| M7 | release note publish/unpublish | none — `published BOOLEAN` already in `0097` | `0097` |
| M8 | profile avatars | avatar tokens are derived; if persisted avatars are wanted, `user_attachments`/profile pics may already be covered by files migrations — verify before proposing 0108+ | files track migrations |
| M9 | learning-object dates for modules/files | module date details read from context modules' existing date columns; files date details reuse `attachments` unlock/lock columns added for differentiated files — verify; overrides themselves already exist (`0090`) | `0090_learning_object_dates.sql`, attachments migrations |
| M10 | appointment-group groups listing / next_appointment | none — group memberships already modeled in the groups track | groups migrations |

Net: **at most one new migration (M3 timetable_data, plus M8/M9 only if verification fails)**; everything else is served by existing tables. Implementers must verify the listed migrations before reserving any number.

## 5. Behavior contracts for missing routes

Conventions: all bodies are JSON unless noted; "Link pagination" means Canvas `Api.paginate` emits `Link` headers with `current`, `next`, `prev` (and `first`/`last` where applicable) plus a `X-Total-Count`-style behavior of `Page-Items` count only when the caller passed `include[]=` options that require it — star-labs should mirror its existing Link-header helper. Canvas renders validation errors as `[{ "attribute": ..., "message": ... }]` with HTTP 400 for conversations, and plain `{"errors": [...]}` / model errors elsewhere; each contract below states the exact shape.

### 5.1 PUT /api/v1/conversations/:id — update

- Params: `conversation[workflow_state]` (`read|unread|archived`), `conversation[subscribed]` (group conversations only), `conversation[starred]` (boolean); optional `scope`, `filter[]`, `filter_mode` only affect the computed `visible` flag.
- Behavior: only the current user's ConversationParticipant row is touched. `subscribed` on a private (non-group) conversation has no effect. Allowed fields are exactly `workflow_state, subscribed, starred` (API_ALLOWED_FIELDS); unknown fields are ignored (strong params).
- Success: 200 with full conversation JSON (same shape as show minus messages), including `visible` computed against scope/filter.
- Failure: 400 with the participant errors object when validation fails (e.g. blank workflow_state is not possible via API since only enumerated values accepted — treat invalid enum as 400 errors hash); 404 when the user is not a participant or `message_count = 0`.

### 5.2 PUT /api/v1/conversations — batch_update

- Params: `conversation_ids[]` (required, array, ≤ 500), `event` (`mark_as_read|mark_as_unread|star|unstar|archive|destroy`).
- Behavior: validates first, then enqueues an async batch over the current user's participants. Runs as a Progress job.
- Success: 200 with a **Progress** object (`progress_json`: id, context_id, context_type "ConversationBatch" semantics, user_id, workflow_state `queued`, tags, completion, url).
- Failure (all 400, plain `{"message": ...}`): "conversation_ids not specified" (param missing or not an array), "conversation batch size limit (500) exceeded", "event not specified", "invalid event".

### 5.3 POST /api/v1/conversations/mark_all_as_read

- Params: none.
- Behavior: marks every conversation of the current user read (and resets the unread counter cache).
- Success: 200 `{}`.
- Failure: 401 when not logged in. No body validation errors.

### 5.4 GET /api/v1/conversations/deleted — deleted_index

- Params: `user_id` (filter to one user's deleted messages), `conversation_id` (optional; when present the query runs inside that conversation's shard), plus standard pagination.
- Behavior: site-admin only. Lists soft-deleted ConversationMessageParticipant rows (`workflow_state = 'deleted'`) via `ConversationMessageParticipant.query_deleted`, paginated, serialized with `deleted_conversation_json`.
- Success: 200 array + Link pagination.
- Failure: 401 unless the caller holds the `admin` role on `Account.site_admin`.

### 5.5 PUT /api/v1/conversations/restore — restore_message

- Params: `message_id` (required), `user_id` (required), `conversation_id` (required).
- Behavior: site-admin only. Flips the matching ConversationMessageParticipant rows back to `active`/NULL deleted_at, then recomputes the participant's `message_count` and `last_message_at`.
- Success: 200 array of `conversation_message_json` for the restored messages.
- Failure: 401 non-site-admin; 400 `[{attribute: "message_id"|"user_id"|"conversation_id", message: "required"}]` for each missing param.

### 5.6 DELETE /api/v1/conversations/:id/delete_for_all

- Params: `id` (conversation id, not participant id).
- Behavior: site admin with `manage_students`. Hard-deletes the conversation for **every** participant (`Conversation#delete_for_all`), unlike the soft per-user destroy.
- Success: 200 `{}`.
- Failure: 401 (unauthorized or missing `manage_students`); 404 when the conversation does not exist.

### 5.7 GET /api/v1/conversations/find_recipients — find_recipients

- Params: `search` (string), `context` (course/group string), `exclude_active_conversation_ids[]`, `permissions[]` (`send_messages_all` etc.), pagination.
- Behavior: **deprecated alias** of `search/recipients` (SearchController#recipients). Two routes exist with the same path; the conversations-controller one is a no-op method kept for YARD — the Search controller serves the request. star-labs should register the path in star-conversations and internally delegate to the same handler as `GET /api/v1/search/recipients` (user-index of messageable users with avatars).
- Success: 200 array of recipient hashes `{id, name, full_name, avatar_url, ...}` + Link pagination.
- Failure: 401 when not logged in.

### 5.8 POST /api/v1/calendar_events/:id/reservations/:participant_id — reserve (explicit participant)

- Params: `participant_id` is the path segment (user id or group id depending on the appointment group's participant_type); optional `comments`, `cancel_existing`.
- Behavior: identical to the DONE `/reservations` route except the participant is resolved from the path. Reserving **for someone else** requires `manage` on the appointment group and the participant must be in `possible_participants`; otherwise 400 `[{attribute: "reservation", type: "calendar_event", message: "invalid participant", reservations: [...]}]`. The error body embeds the participant's current reservations.
- Success: 200 with the new reservation CalendarEvent JSON.
- Failure: 401 (`:reserve` denied), 403 (past slot, non-manager), 400 (ReservationError, body above; also when `participant_id` names the caller's own candidate it is accepted, when it names a different user without manage it is `invalid participant`).

### 5.9 GET /api/v1/calendar_events/:id/participants

- Params: none beyond pagination.
- Behavior: parent appointment time-slot event only (`read_child_events` right). If the event is not part of an appointment group → `[]`. Otherwise paginates the users (or groups) that hold child events of this slot, ordered by id, and renders each with `user_display_json` (id, name, sortable_name, display_name, avatar_url — not the full user object).
- Success: 200 array + Link pagination.
- Failure: 401 without `read_child_events`.

### 5.10 GET /api/v1/courses/:course_id/calendar_events/timetable

- Params: none.
- Behavior: `manage_calendar` on the course. Returns the raw timetable hash last saved (or `{}`).
- Success: 200 with the timetable JSON object (keyed by section id or `"all"`, each an array of `{weekdays, start_time, end_time, location_name?}`).
- Failure: 401.

### 5.11 POST /api/v1/courses/:course_id/calendar_events/timetable — set_course_timetable

- Params: `timetables[<course_section_id>|all][]` arrays of `{weekdays: "Mon,Wed,Fri", start_time: "11:00 am", end_time: "11:50 am", location_name?}`.
- Behavior: `manage_calendar`. Validates every timetable through the timetable event builder; on validation error stops before saving. Stores the validated data on the course (`timetable_data`), generates/updates section-level `CalendarEvent`s with `for_timetable` semantics, and soft-deletes timetable events of sections absent from the payload. Event creation is delayed (async) — the response only acknowledges.
- Success: 200 `{"status": "ok"}`.
- Failure: 400 `{"errors": [...]}` (builder validation, e.g. bad weekday or end before start); 401; 404 unknown section id.

### 5.12 POST /api/v1/courses/:course_id/calendar_events/timetable_events

- Params: `course_section_id` (optional), `events[]` of `{start_at, end_at, location_name?, code?, title?}` (`code` is the stable identifier for later updates; title defaults to the course name).
- Behavior: `manage_calendar`. Takes a complete list of timetable events (create-or-update by `code`), async creation.
- Success: 200 `{"status": "ok"}`.
- Failure: 400 `{"errors": [...]}`; 401; 404 unknown section.

### 5.13 GET /api/v1/appointment_groups/:id/groups

- Params: `registration_status` (`all|registered`, default `all`), `context_code` (optional filter).
- Behavior: `read` on the group. Returns `[]` when participant_type is "User" (i.e. the group-signup route used against a user-signup group). Paginates possible (or registered) student groups; each rendered with the full Groups-API `group_json`.
- Success: 200 array + Link pagination.
- Failure: 401.

### 5.14 GET /api/v1/appointment_groups/next_appointment

- Params: `appointment_group_ids[]` (optional list to search; comma-separated string also accepted).
- Behavior: among the caller's currently reservable appointment groups, finds the nearest future slot that the caller can still sign up for (not already signed up, not full). **Always returns a 0- or 1-element array.**
- Success: 200 `[CalendarEvent]` (one element) or `[]`.
- Failure: 401 when not logged in. Never 404 for "no appointment".

### 5.15 GET /api/v1/{courses/:course_id,accounts/:account_id}/blackout_dates/:id — show

- Params: `id`.
- Behavior: granular manage-course-content **read** family on the context. Loads the blackout date scoped to the context (`context.blackout_dates.find`), so an id belonging to another context 404s.
- Success: 200 BlackoutDate JSON: `{id, context_id, context_type, start_date, end_date, event_title}` (dates as ISO dates; Canvas renders the raw model attributes — note the JSON key is `event_title` even though star-labs migration `0091` names the column `title`, so map it).
- Failure: 401; 404 (wrong context or unknown id).

### 5.16 GET /api/v1/{courses/:course_id,accounts/:account_id}/blackout_dates/new — new

- Params: none.
- Behavior: read-family right; returns an **unsaved, empty** BlackoutDate (`{id: null, context_id: <ctx>, context_type: "Course"|"Account", start_date: null, end_date: null, event_title: null}`). Exists for form initialization.
- Success: 200 with the blank object.
- Failure: 401.

### 5.17 PUT /api/v1/{courses/:course_id,accounts/:account_id}/blackout_dates/:id — update

- Params: `blackout_date[start_date]`, `blackout_date[end_date]`, `blackout_date[event_title]` (body may also be flat, but Canvas strong-params nests under `blackout_date`).
- Behavior: `manage_course_content_edit`. Updates the context-scoped row.
- Success: 200 BlackoutDate JSON.
- Failure: 422 with `errors.full_messages` array (Canvas uses `:unprocessable_content`; note the create action uses `{success: false, errors: [...]}` while update returns a bare array — mirror both exactly); 401; 404.

### 5.18 DELETE /api/v1/accounts/:account_id/blackout_dates/:id — destroy

- Params: `id`.
- Behavior: `manage_course_content_delete` on the account context. Deletes the row.
- Success: **204 No Content** (head), unlike the course-context destroy already implemented — verify parity there too.
- Failure: 401; 404.

### 5.19 GET /api/v1/courses/:course_id/files/:attachment_id/date_details — show

- Behavior: same response envelope as the DONE assignment/quiz/page variants (learning-object dates JSON + `overrides` array), but for attachments, and only when site-admin feature `differentiated_files` is enabled — otherwise **400** `{"error": "This API does not support files."}` (not 401, not 404). `manage_assign_to` on the asset.
- Success: 200 `{**learning_object_dates_json, **blueprint_date_locks_json, overrides: [...]}` with Link pagination on overrides.
- Failure: 400 (feature off), 401, 404.

### 5.20 PUT /api/v1/courses/:course_id/files/:attachment_id/date_details — update

- Params: same as other date-details updates: `due_at` (not applicable for files — ignore), `unlock_at`, `lock_at`, `only_visible_to_overrides`, `assignment_overrides[]` (Attachment goes through `update_ungraded_object`).
- Behavior: same feature gate as §5.19 (400 when off). 204 on success.
- Failure: 400 feature off / validation; 401; 404.

### 5.21 GET /api/v1/courses/:course_id/modules/:context_module_id/date_details — show

- Behavior: read-only date details for a context module (Canvas defines only the GET for modules; there is no module update/convert route — do not invent them). Same envelope; `manage_assign_to` on the module.
- Success: 200 date-details JSON + paginated overrides.
- Failure: 401; 404.

### 5.22 PUT /api/v1/courses/:course_id/{assignments/:assignment_id,quizzes/:quiz_id,discussion_topics/:discussion_topic_id,pages/:url_or_id}/date_details/convert_tag_overrides

- Params: none.
- Behavior: `manage_assign_to` (same before_action). Converts the object's differentiation-tag (section) overrides into adhoc (student) overrides via `OverrideConverterService` — for graded discussions the conversion runs on the underlying assignment. On validation errors returns 400 `{"errors": ...}`; on success **204 No Content** with empty body (the Canvas action name is `convert_tag_overrides_to_adhoc_overrides` for assignments but the route segment is `convert_tag_overrides` for all four — keep route segments exactly as listed in §2.5).
- Failure: 400 `{"errors": ...}`; 401; 404.

### 5.23 POST /api/v1/users/:user_id/communication_channels/:id — reset_bounce_count

- Params: none (id in path).
- Behavior: resolves the user (`api_find`), finds the channel among unretired ones, then requires channel `reset_bounce_count` right (self or site admin; masquerading real user also honored). Resets bounce counters (`reset_bounce_count!`).
- Success: 200 communication-channel JSON (with bounce fields for those entitled to see them).
- Failure: 401; 404 (user, channel, or retired channel).

### 5.24 DELETE /api/v1/users/:user_id/communication_channels/:type/:address — destroy by type+address

- Params: `type` (email/sms/…), `address` = the channel path. Route constraint on address is **greedy** `[^/?]+` — a slash-containing address still matches (only `?` and `/`+query break it). Route order matters: `/push` (delete_push_token) and `/:id` must not shadow this; in axum register the literal `push` route first and this before the plain `{id}` route, or better: since `{id}` and `{type}/{address}` differ in segment count, they cannot collide — but `users/self/communication_channels/push` (§5.25) must beat `{user_id}/communication_channels/{type}/{address}` when user_id is the literal `self`.
- Behavior: identical to destroy-by-id (§3.6 rights) after resolving the channel by `(user, path_type, path)`; unknown pair → 404.
- Success: 200 channel JSON.
- Failure: 400 (destroy validation errors), 401, 404.

### 5.25 DELETE /api/v1/users/self/communication_channels/push — delete_push_token

- Params: `push_token` (in body/query — the curl example uses `-d 'push_token=...'`).
- Behavior: self only (literal `self`). Finds the user's push channel (404 if none), then destroys the notification endpoints whose token matches case-insensitively; touches the user.
- Success: 200 `{"success": true}`.
- Failure: 400 with endpoint errors; 404 when no push channel exists.

### 5.26 GET /api/v1/accounts/:account_id/bounced_communication_channels.csv — CSV report

- Params: same filters as the JSON report: `after`, `before`, `pattern`, `with_invalid_paths`, `path_type`, `order`.
- Behavior: account `view_bounced_emails` (account_id `self` allowed). Streams `text/csv` of the bounced-channel report — same rows as the JSON route, CSV columns for user id/name, address, bounce count, last bounce at, summaries. The `.csv` suffix is part of the **path** (Rails `defaults: {format: :csv}`), so star-labs must register a distinct route ending in `.csv`.
- Success: 200 `text/csv` body.
- Failure: 401.

### 5.27 GET /api/v1/accounts/:account_id/unconfirmed_communication_channels[.csv] — unconfirmed_channel_report

- Params: `after`, `before`, `pattern`, `with_invalid_paths`, `path_type`, `order`.
- Behavior: `view_bounced_emails`; report of channels stuck in `unconfirmed` state for the account's users. JSON variant returns the JSON report; `.csv` variant returns CSV (§5.26 rules).
- Success: 200 JSON array / CSV.
- Failure: 401.

### 5.28 POST /api/v1/accounts/:account_id/bounced_communication_channels/reset — bulk_reset_bounce_counts

- Params: `after`, `before`, `pattern`, `with_invalid_paths`, `path_type`, `order` (selection filters for the affected channels).
- Behavior: **site-admin** `read_messages` (not just the account right — this is the mutating twin of the report). Performs the bulk reset and returns its result.
- Success: 200 with the action's result payload (count/rows as produced by the bulk action).
- Failure: 401.

### 5.29 POST /api/v1/accounts/:account_id/unconfirmed_communication_channels/confirm — bulk_confirm

- Params: same selection filters as §5.28.
- Behavior: site-admin `read_messages`; bulk-confirms the selected unconfirmed channels.
- Success: 200 result payload.
- Failure: 401.

### 5.30 GET /api/v1/users/self/favorites/groups — list_favorite_groups

- Params: none.
- Behavior: self only. Returns the **active groups the user is a member of that are favorited**; when none are favorited, returns all active memberships (same empty-favorites-returns-all fallback as courses). Each item is full Groups-API JSON.
- Success: 200 array (not paginated in Canvas — the docs say "paginated" but the implementation maps without Api.paginate; keep unpaginated for parity and note it).
- Failure: 401.

### 5.31 POST /api/v1/users/self/favorites/groups/:id — add_favorite_groups

- Params: `id` (group id or SIS id).
- Behavior: `api_find(Group, id)` (404 unknown), `create_or_find_by` favorite (idempotent), touches the user (menu cache).
- Success: 200 Favorite JSON `{context_id, context_type: "Group"}`.
- Failure: 401, 404.

### 5.32 DELETE /api/v1/users/self/favorites/groups/:id — remove_favorite_groups

- Params: `id`.
- Behavior: resolves the group (404 unknown), deletes the favorite if present; **200 `{}` when absent** (deliberate idempotency, keep it).
- Success: 200 Favorite JSON of the removed row, or `{}`.
- Failure: 401, 404.

### 5.33 DELETE /api/v1/users/self/favorites/groups — reset_groups_favorites

- Params: none.
- Behavior: deletes all Group favorites of the user.
- Success: 200 `{"status": "ok"}`.
- Failure: 401.

### 5.34 GET /api/v1/users/:user_id/avatars — profile_pics

- Params: `user_id` (`api_find`; the UI calls it with `self`).
- Behavior: `update_avatar` right on the user (self or entitled admin). Returns the avatar-choice list: a `gravatar` entry (`{type: "gravatar", url, token, display_name: "gravatar pic"}`), zero or more `attachment` entries (`{type, url, token, display_name, id, content-type, filename, size}`), and always a trailing `{type: "no_pic", url, token, display_name: "no pic"}`. `token` is the opaque value to submit back to the profile update.
- Success: 200 array (not Link-paginated).
- Failure: 401.

### 5.35 PUT /api/v1/release_notes/:id/published — publish

- Params: `id` only.
- Behavior: site-admin `manage_release_notes`. Sets `published = true` and saves. **Renders nothing** — Canvas's action body is just the save, so the default render produces an empty 200 body. star-labs should return 200 with empty body (or 204 — pick 200-empty to match Rails default; see open question §7).
- Failure: 401; 404 (unknown id), 422 on save failure.

### 5.36 DELETE /api/v1/release_notes/:id/published — unpublish

- Params: `id` only.
- Behavior: identical to §5.35 but `published = false`.
- Success/failure: same as §5.35.

### 5.37 GET /api/v1/{accounts/:account_id,groups/:group_id,users/:user_id}/tabs — index

- Params: `include[]` = `course_subject_tabs` (only meaningful for courses; harmless elsewhere).
- Behavior: `read` right on the context, **deny with 404** (not 401). Returns the context's navigation tabs: `{id, html_url, label, position (1-based), type internal|external, visibility public|members|admins|none, hidden? (only when true)}`. Account tabs cover the account navigation; user tabs cover the profile navigation; group tabs the group navigation. Non-Course contexts return the fixed navigation sets (admin-configurable tools included, position/hidden not updatable via API for these contexts).
- Success: 200 array + Link pagination (Canvas paginates these listings).
- Failure: 404.

### 5.38 PUT /api/v1/courses/:course_id/tabs/:tab_id — update

- Params: `tab_id` (the tab's string id, e.g. `context_external_tool_4`), `position` (1-based int), `hidden` (boolean). Only `hidden=true` or explicit `hidden=false`/absence semantics per Canvas: `hidden` set truthy → hidden true; key present falsy → unhide.
- Behavior: **Course contexts only** (`manage_course_content_edit`). Home and Settings tabs are not manageable → 400. Position must satisfy `1 < position < tab_count + 1` → else 400. Persists to the course's `tab_configuration` (JSON on course settings, migration `0055`).
- Success: 200 tab JSON (with new position/hidden).
- Failure: 400 `{"error": "<css_class> is not manageable"}`; 400 `{"error": "That tab location is invalid"}`; 401; 404 (course or, arguably, unknown tab — Canvas raises on `.with_indifferent_access` of nil, i.e. 500; star-labs should 404 the unknown tab, note in §7).

## 6. Per-controller test plans

Every plan below includes the permission negatives from §3 (401/403/404 as specified). Test crates follow the existing per-crate `#[cfg(test)]` integration layout. Shared fixtures: users (site admin, account admin, teacher, student, observer, student-view student), courses (active/completed), groups + group categories, appointment groups (User and Group participant types), communication channels (email/sms/push, confirmed/unconfirmed/bounced), favorites, release notes, tabs-enabled course.

### 6.1 conversations (star-conversations)

1. `update`: set workflow_state read/unread/archived; starred toggle; subscribed on group conversation; verify response shape incl. `visible`; participant row only (other participant unaffected).
2. Negatives: non-participant → 404; `message_count = 0` (deleted all messages) → 404; invalid `filter_mode` → 400.
3. `batch_update`: happy path returns Progress (queued); `conversation_ids` missing / non-array → 400 each message text exactly; 501 ids → 400 "conversation batch size limit (500) exceeded"; invalid event → 400 "invalid event"; verify the Progress completes and applies mark_as_read/star/destroy to only the caller's participants.
4. `mark_all_as_read`: unread counter goes to zero, `unread_count` endpoint then returns `"0"` (string).
5. `deleted_index`: site admin sees deleted rows for a user; non-admin (teacher) → 401; with `conversation_id` filter.
6. `restore_message`: restores message, recomputes `message_count`/`last_message_at`; missing params → 400 with `required`; non-admin → 401.
7. `delete_for_all`: removes for every participant (check a second participant's index); non-site-admin or site admin without `manage_students` → 401; unknown id → 404.
8. `find_recipients`: returns messageable users; identical output to `search/recipients` for the same query; unauthenticated → 401.
9. Regression: `unread_count` remains a **string**; existing routes still pass.

### 6.2 calendar_events_api (star-calendar)

1. `reserve` with `participant_id`: manager reserves for student → 200; student reserving for another → 400 `invalid participant` with embedded `reservations`; self id → 200; past slot → 403 message "Cannot create or change reservation for past appointment".
2. `participants`: paginated user list of a slot; non-appointment event → `[]`; without `read_child_events` → 401; Link header present.
3. `get_course_timetable`: returns stored hash; `{}` when never set; non-manager → 401.
4. `set_course_timetable`: valid Mon/Wed timetable → `{"status":"ok"}` and events generated (async — poll or run inline in test); invalid weekday / end<start → 400 `{"errors": [...]}`; unknown section → 404; sections omitted from payload have their timetable events soft-deleted.
5. `set_course_timetable_events`: create-or-update by `code`; validation errors → 400; missing section → 404.
6. Negatives for observer scoping on `user_index` remain covered (existing) — add one case where observer has no linked enrollment → 401.

### 6.3 appointment_groups (star-calendar)

1. `groups`: Group-type group lists student groups (registered/all); User-type group returns `[]`; `read` required; Link pagination.
2. `next_appointment`: returns 1-element array of the earliest eligible slot; `[]` when none (fully booked, already signed up, or all past); `appointment_group_ids[]` filter; non-reservable group excluded; unauthenticated → 401.

### 6.4 blackout_dates (star-calendar)

1. `show` in course and account contexts: correct row; id from another context → 404; read-family right required.
2. `new`: blank object with context fields; 401 negative.
3. `update`: happy path; validation failure → 422 with errors array; wrong context → 404.
4. account `destroy`: 204 empty body; 401/404 negatives. Also assert the existing course `destroy` still returns 204 (parity of both).
5. Granular rights matrix: teacher with only `manage_course_content_add` can create but not update/delete (401 each); read-only observer cannot even `show`.

### 6.5 learning_object_dates (star-learning-dates)

1. files `show`/`update`: with site-admin `differentiated_files` on → normal behavior; off → 400 `{"error": "This API does not support files."}`; overrides pagination.
2. modules `show`: date details + overrides; 404 unknown module; 401 without `manage_assign_to`.
3. `convert_tag_overrides` (all four asset types): tag (section) overrides replaced by adhoc (student) overrides covering the same students; 204 empty body; service errors → 400 `{"errors": ...}`; graded discussion converts its assignment; 401/404 negatives.

### 6.6 communication_channels (star-notifications)

1. `reset_bounce_count`: self resets own channel; site admin resets any; teacher on other user → 401; retired channel → 404; response carries bounce fields when entitled.
2. destroy by `type/:address`: email address containing dots and `%2B`-encoded `+`; greedy-path regression: an address containing an encoded slash still routes; unknown pair → 404; non-entitled admin → 401.
3. `delete_push_token`: destroys matching endpoint case-insensitively; `{success: true}`; no push channel → 404; wrong `push_token` destroys nothing but still `200` (verify Canvas behavior: `endpoints&.destroy_all` on empty set → `{"success": true}`).
4. Bounced/unconfirmed reports: JSON + `.csv` for both; `view_bounced_emails` on the account; `account_id=self` accepted; filters `after`/`before`/`pattern`/`path_type`/`order` narrow rows; CSV content-type `text/csv`; teacher → 401.
5. `bulk_reset_bounce_counts` / `bulk_confirm`: site-admin `read_messages` only (account `view_bounced_emails` alone → 401); selected channels change state; response payload non-empty.

### 6.7 favorites (star-dashboard)

1. `list_favorite_groups`: favorited groups only; empty favorites → all active memberships; non-member group never listed; response uses group JSON.
2. `add_favorite_groups`: idempotent (second POST returns same Favorite); SIS id accepted; unknown id → 404; user touched.
3. `remove_favorite_groups`: returns removed Favorite; absent → 200 `{}`; unknown group → 404.
4. `reset_groups_favorites`: deletes only Group favorites (course favorites untouched); `{"status":"ok"}`.
5. All four: another user's token → 401 (self only); regression: course favorites untouched.

### 6.8 profile (star-users)

1. `profile_pics`: includes gravatar entry, uploaded attachments, trailing `no_pic`; `update_avatar` right; another user without right → 401; array not paginated.

### 6.9 release_notes (star-dashboard)

1. `publish`: note becomes visible to `latest` for its target roles; response is 200 with empty body.
2. `unpublish`: note disappears from `latest` and `unread_count` drops.
3. Negatives: non-site-admin (even account admin) → 401; unknown id → 404; anonymous `latest`/`unread_count` keep working (regression).

### 6.10 tabs (star-courses)

1. account / group / user `index`: shape and pagination; no-read user → **404**; `include[]=course_subject_tabs` accepted.
2. `update` (course): position change persists to `tab_configuration`; `hidden=true` / explicit false; Home and Settings → 400 `<css_class> is not manageable`; position ≤1 or ≥ count+1 → 400 `That tab location is invalid`; teacher without `manage_course_content_edit` → 401; group/account context PUT → 404 (not 401).
3. Regression: course tabs GET unaffected.

### 6.11 admins / sub_accounts (star-accounts) — regression only

1. Re-run existing suites; add one negative each: `admins` index without `manage_account_memberships` → 401; `sub_accounts` destroy with courses → 409 "You can't delete a sub-account that has courses in it." (message parity).

## 7. Open questions (10)

1. **`find_recipients` duplication:** Canvas registers the same path on both `search#recipients` and `conversations#find_recipients`. Should star-labs register the alias in star-conversations (delegating to the search handler) or leave it to the search-controller spec (issue for search/recipient coverage)? Proposed: register the alias here, delegate.
2. **`batch_update` Progress semantics:** Canvas returns a Progress tied to `ConversationParticipant.batch_update`. Does the star-jobs progress implementation already model `context_type = ConversationBatch`-style progress, or should the spec mandate a generic progress context? Needs verification of star-jobs progress tables before M4.
3. **`deleted_index` / `restore_message` admin gate:** Canvas checks `roles(Account.site_admin).include?("admin")` (any site-admin role), while `delete_for_all` checks the granular `manage_students`. Mirror exactly, or unify on `manage_students`? Proposed: mirror Canvas.
4. **`publish`/`unpublish` empty responses:** Canvas renders nothing (empty 200). Keep 200-empty, or 204? Proposed: 200 with empty body for Rails parity; note that the DONE destroy action returns `{"status":"ok"}`, so an alternative is matching that. Needs a product call.
5. **Tabs `update` on non-course contexts:** Rails `return` yields an empty 200. 404 is proposed in §5.38; confirm against real Canvas traffic (the UI only ever PUTs course tabs).
6. **Tabs `update` unknown tab_id:** Canvas raises (500) on `nil.with_indifferent_access`. 404 proposed — confirm acceptable divergence (500-for-500 parity is not desirable).
7. **`list_favorite_groups` pagination:** docs say paginated, implementation is not. Confirm no UI consumer relies on Link headers before freezing unpaginated parity.
8. **Timetable async creation:** Canvas creates events via delayed job after responding. Should star-labs create synchronously (simpler, test-friendlier) or via star-jobs? Proposed: synchronous creation, documented divergence (no observable API difference except eventual consistency).
9. **Greedy `:type/:address` route vs axum:** the Rails constraint `[^/?]+` means encoded slashes inside the address match. Confirm the star-http router passes the raw (percent-encoded) segment through so `/users/1/communication_channels/email/a%2Fb` resolves `address = "a/b"`. If the router decodes first, ordering or a custom matcher is needed.
10. **Account/user/group tabs content source:** course tabs derive from `tab_configuration` + tool registrations. For account/group/user contexts, is there an existing star-labs navigation model, or does this spec need to fix the exact default tab lists (id/label/position) in the behavior contract? Needs a follow-up inventory of star-courses/star-accounts navigation code.
