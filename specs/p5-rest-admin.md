# Spec: REST admin remainder (LTI/IMS, master courses, roles, usage rights)

- **Issue:** #23 (epic:specs)
- **Branch:** `p5/specs-23` (documentation only — no Rust code, no migrations, no cargo)
- **Sources verified:** `canvas-lms` checkout at `/home/z/my-project/repos/canvas-lms` (`config/routes.rb`, `app/controllers/lti/**`, `app/controllers/master_courses/master_templates_controller.rb`, `app/controllers/role_overrides_controller.rb`, `app/controllers/usage_rights_controller.rb`) and the star-labs workspace at `migration/` (fresh parity run, see §1).
- **Migration numbers:** reserved by the coordinator per the issue #20 protocol; ledger `orchestration/migration-reservations.json` (max merged `0107`, next available `0108`). Additive only — no renumbering, no DROP/TRUNCATE.

## 1. Overview

This spec covers the remaining REST admin surface in four families:

| Canvas controller | Routes | Done | Missing | Owning crate today |
|---|---|---|---|---|
| `lti/registrations` | 20 | 4 | 16 | `star-lti` |
| `lti/deployments` | 5 | 2 | 3 | `star-lti` |
| `lti/context_controls` | 6 | 0 | 6 | `star-lti` (new surface) |
| `lti/ims/tool_setting` | 12 | 0 | 12 | `star-lti` (new surface) |
| `lti/ims/dynamic_registration` | 11 | 0 | 11 | `star-lti` (new surface) |
| `master_courses/master_templates` | 13 | **13 (complete)** | 0 | `star-courses` |
| `role_overrides` (roles API) | 8 | 5 | 3 | `star-permissions` |
| `usage_rights` | 9 | 5 | 4 | `star-files` |
| **Total in scope** | **84** | **29** | **55** | |

Fresh parity numbers (regenerated with
`python3 migration/tools/parity/parity.py report --manifest migration/parity/canvas_manifest.json --star-src migration --out /tmp/parity-fresh`;
the committed `migration/parity/coverage.json` is stale):

- Workspace REST: **471 / 1403 (33.6%)**; UI-referenced REST: 258/503 (51.3%); star route registrations 624 (157 extras).
- Scope families: `lti/registrations` 4/20 (20.0%), `lti/deployments` 2/5 (40.0%), `lti/context_controls` 0/6, `lti/ims/tool_setting` 0/12, `lti/ims/dynamic_registration` 0/11, `master_courses/master_templates` 13/13 (100%), `role_overrides` 5/8 (62.5%), `usage_rights` 5/9 (55.6%).
- Route counts in scope: **29 done / 0 partial / 55 missing of 84**.

Notable:

- **master_courses is fully covered** by `star-courses` (PR #18, commit `621ef9f` + migration `d946f85`/0107). Do **not** redo it; it appears here only as a verification anchor and for the §4 isolation notes.
- The roles API lives in `star-permissions` (not a dedicated crate); the role-overrides *value* routes (`GET/PUT /accounts|courses/{id}/role_overrides`) already exist as flat aliases.
- `usage_rights` group/user **PUT**s plus group/user `content_licenses` are the only real gap in `star-files`.
- `lti/ims/*` routes are mounted under **`/api/lti`** (not `/api/v1`), which is a new mount prefix for `star-http`.

## 2. Route inventory

Status legend: **done** = matched by the parity tool (routes registered by PR #18 or earlier waves count as done); **missing** = not registered. Target crate = where the handler should live.

### 2.1 `lti/registrations` (Canvas `config/routes.rb` lines 2165–2188)

| Method | Canvas path | Status | Target crate | Notes |
|---|---|---|---|---|
| GET | `/api/v1/accounts/:account_id/lti_registrations` | done | star-lti | `list`; returns `{ total, data }`, default per_page 15 |
| POST | `/api/v1/accounts/:account_id/lti_registrations` | done | star-lti | `create`; 201 Created |
| GET | `/api/v1/accounts/:account_id/lti_registrations/:id` | done | star-lti | `show` |
| DELETE | `/api/v1/accounts/:account_id/lti_registrations/:id` | done | star-lti | `destroy` |
| PUT | `/api/v1/accounts/:account_id/lti_registrations/:id` | missing | star-lti | `update`; blocked on 0109 (update requests) semantics |
| POST | `/api/v1/accounts/:account_id/lti_registrations/:id/bind` | missing | star-lti | `bind`; needs binding state (0109) |
| DELETE | `/api/v1/accounts/:account_id/lti_registrations/:id/bind` | missing | star-lti | `unbind` |
| GET | `/api/v1/accounts/:account_id/lti_registrations/:id/history` | missing | star-lti | `history`; needs 0110 overlays history |
| GET | `/api/v1/accounts/:account_id/lti_registrations/:id/overlay_history` | missing | star-lti | `overlay_history`; needs 0110 |
| PUT | `/api/v1/accounts/:account_id/lti_registrations/:id/reset` | missing | star-lti | `reset` to template/previous overlay |
| GET | `/api/v1/accounts/:account_id/lti_registrations/:id/latest_update_request` | missing | star-lti | needs 0109 table |
| GET | `/api/v1/accounts/:account_id/lti_registrations/:id/update_requests/:update_request_id` | missing | star-lti | needs 0109 |
| PUT | `/api/v1/accounts/:account_id/lti_registrations/:id/update_requests/:update_request_id/apply` | missing | star-lti | needs 0109 |
| GET | `/api/v1/accounts/:account_id/lti_registration_by_client_id/:client_id` | missing | star-lti | `show_by_client_id`; needs developer-key linkage (0112) |
| GET | `/api/v1/accounts/:account_id/lti_registrations/by_utid/:utid` | missing | star-lti | `show_by_utid` |
| GET | `/api/v1/accounts/:account_id/lti_registrations/check_domain_duplicates` | missing | star-lti | pure read; no migration needed |
| POST | `/api/v1/accounts/:account_id/lti_registrations/configuration/validate` | missing | star-lti | validates JSON config/URL; returns `{ configuration }` |
| GET | `/api/v1/accounts/:account_id/lti_registrations/install_status/:client_id` | missing | star-lti | needs 0112 DR columns |
| POST | `/api/v1/accounts/:account_id/lti_registrations/:id/install_from_template` | missing | star-lti | template install |
| GET | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/deployments/:deployment_id/context_search` | missing | star-lti | `context_search` lives in this controller in Canvas |

### 2.2 `lti/deployments` (routes.rb 2190–2196)

| Method | Canvas path | Status | Target crate | Notes |
|---|---|---|---|---|
| GET | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/deployments` | done | star-lti | `list` |
| POST | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/deployments` | done | star-lti | `create` (`for_subaccount_id` / `for_course_id` / `available`) |
| GET | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/deployments/:id` | missing | star-lti | `show` |
| DELETE | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/deployments/:id` | missing | star-lti | `destroy` |
| GET | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/deployments/:id/controls` | missing | star-lti | `list_controls`; joins context controls (0108) |

### 2.3 `lti/context_controls` (routes.rb 2198–2205)

| Method | Canvas path | Status | Target crate | Notes |
|---|---|---|---|---|
| GET | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/controls` | missing | star-lti | `index` |
| POST | `/api/v1/accounts/:current_account_id/lti_registrations/:registration_id/controls` | missing | star-lti | `create` — note the `:current_account_id` param name (Canvas avoids a conflict) |
| POST | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/controls/bulk` | missing | star-lti | `create_many` |
| GET | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/controls/:id` | missing | star-lti | `show` |
| PUT | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/controls/:id` | missing | star-lti | `update` |
| DELETE | `/api/v1/accounts/:account_id/lti_registrations/:registration_id/controls/:id` | missing | star-lti | `delete` |

### 2.4 `lti/ims/tool_setting` (routes.rb 3161–3170; **mounted under `/api/lti`**, LTI 2.x)

| Method | Canvas path | Status | Target crate | Notes |
|---|---|---|---|---|
| GET | `/api/lti/tool_settings/:tool_setting_id` | missing | star-lti | direct settings access |
| PUT | `/api/lti/tool_settings/:tool_setting_id` | missing | star-lti | |
| GET | `/api/lti/tool_proxy/:tool_proxy_guid/tool_setting` | missing | star-lti | ToolProxy-level |
| PUT | `/api/lti/tool_proxy/:tool_proxy_guid/tool_setting` | missing | star-lti | |
| GET | `/api/lti/tool_proxy/:tool_proxy_guid/accounts/:account_id/tool_setting` | missing | star-lti | ToolProxyBinding-level |
| PUT | `/api/lti/tool_proxy/:tool_proxy_guid/accounts/:account_id/tool_setting` | missing | star-lti | |
| GET | `/api/lti/tool_proxy/:tool_proxy_guid/courses/:course_id/tool_setting` | missing | star-lti | binding-level (course) |
| PUT | `/api/lti/tool_proxy/:tool_proxy_guid/courses/:course_id/tool_setting` | missing | star-lti | |
| GET | `/api/lti/tool_proxy/:tool_proxy_guid/courses/:course_id/resource_link_id/:resource_link_id/tool_setting` | missing | star-lti | LtiLink-level |
| PUT | `/api/lti/tool_proxy/:tool_proxy_guid/courses/:course_id/resource_link_id/:resource_link_id/tool_setting` | missing | star-lti | |
| GET | `/api/lti/tool_proxy/:tool_proxy_guid/accounts/:account_id/resource_link_id/:resource_link_id/tool_setting` | missing | star-lti | LtiLink-level (account) |
| PUT | `/api/lti/tool_proxy/:tool_proxy_guid/accounts/:account_id/resource_link_id/:resource_link_id/tool_setting` | missing | star-lti | |

### 2.5 `lti/ims/dynamic_registration` (routes.rb 3240–3251; **mounted under `/api/lti`**)

| Method | Canvas path | Status | Target crate | Notes |
|---|---|---|---|---|
| GET | `/api/lti/accounts/:account_id/registration_token` | missing | star-lti | needs 0112 `registration_tokens` |
| GET | `/api/lti/accounts/:account_id/registrations/uuid/:registration_uuid` | missing | star-lti | `ims_registration_by_uuid`; needs 0112 uuid column |
| GET | `/api/lti/accounts/:account_id/lti_registrations/uuid/:registration_uuid` | missing | star-lti | `lti_registration_by_uuid` |
| GET | `/api/lti/accounts/:account_id/lti_registration_update_request/uuid/:registration_uuid` | missing | star-lti | needs 0109 |
| GET | `/api/lti/accounts/:account_id/registrations/:registration_id` | missing | star-lti | `show` (IMS shape) |
| PUT | `/api/lti/accounts/:account_id/registrations/:registration_id/overlay` | missing | star-lti | needs 0110 |
| GET | `/api/lti/accounts/:account_id/dr_iframe` | missing | star-lti | iframe bootstrap page |
| GET | `/api/lti/accounts/:account_id/registrations/:registration_id/view` | missing | star-lti | `registration_view` |
| POST | `/api/lti/registrations` | missing | star-lti | `create` — **no user session** (token-authenticated) |
| PUT | `/api/lti/registrations/:registration_id` | missing | star-lti | `update` — no user session |
| GET | `/api/lti/registrations/:registration_id` | missing | star-lti | `show_configuration` — no user session |

### 2.6 `master_courses/master_templates` (routes.rb 2846–2861) — **COMPLETE, do not redo**

All 13 routes done in `star-courses` (PR #18): `show`, `associated_courses`, `update_associations`, `unsynced_changes`, `migrations` index/create/show/details, `restrict_item`, `blueprint_subscriptions` index/imports index/show/details. Listed here only as the verification anchor; the owning handler block is `migration/crates/star-courses/src/lib.rs` (router block around lines 3232–3280, shared authz helper `blueprint_*` at line 2330).

### 2.7 `role_overrides` — roles API (routes.rb 1964–1973)

| Method | Canvas path | Status | Target crate | Notes |
|---|---|---|---|---|
| GET | `/api/v1/accounts/:account_id/roles` | done | star-permissions | `api_index` |
| POST | `/api/v1/accounts/:account_id/roles` | done | star-permissions | `add_role` |
| GET | `/api/v1/accounts/:account_id/roles/:id` | done | star-permissions | `show` |
| PUT | `/api/v1/accounts/:account_id/roles/:id` | done | star-permissions | `update` |
| DELETE | `/api/v1/accounts/:account_id/roles/:id` | done | star-permissions | `remove_role` (soft delete) |
| POST | `/api/v1/accounts/:account_id/roles/:id/activate` | missing | star-permissions | `activate_role` |
| GET | `/api/v1/accounts/:account_id/roles/permissions` | missing | star-permissions | `manageable_permissions` |
| GET | `/api/v1/accounts/:account_id/permissions/:permission` | missing | star-permissions | `check_account_permission` (also serves the course variant `/courses/:course_id/permissions` style checks) |

### 2.8 `usage_rights` (routes.rb 2737–2743, expanded per context)

| Method | Canvas path | Status | Target crate | Notes |
|---|---|---|---|---|
| PUT | `/api/v1/courses/:course_id/usage_rights` | done | star-files | `set_usage_rights` |
| DELETE | `/api/v1/courses/:course_id/usage_rights` | done | star-files | `remove_usage_rights` |
| GET | `/api/v1/courses/:course_id/content_licenses` | done | star-files | `licenses` |
| PUT | `/api/v1/groups/:group_id/usage_rights` | missing | star-files | same handler shape as course PUT |
| DELETE | `/api/v1/groups/:group_id/usage_rights` | done | star-files | |
| GET | `/api/v1/groups/:group_id/content_licenses` | missing | star-files | |
| PUT | `/api/v1/users/:user_id/usage_rights` | missing | star-files | |
| DELETE | `/api/v1/users/:user_id/usage_rights` | done | star-files | |
| GET | `/api/v1/users/:user_id/content_licenses` | missing | star-files | |

## 3. Permission matrix

Canvas ability checks per controller (verified in source):

- `lti/registrations_controller.rb:2305` — `require_context_with_permission(@context, :manage_lti_registrations)`; `:2310` additionally requires the permission **on the registration's own account** for `reset/update/destroy`; `:1119` `restrict_sub_account_to_read_only` for every write except `index list show show_by_client_id context_search overlay_history history check_domain_duplicates`.
- `lti/deployments_controller.rb:88-91` — `require_account_context`, `require_root_account`, feature flag, then `require_manage_lti_registrations`.
- `lti/context_controls_controller.rb:204-208` — same chain as deployments (`require_account_context` except `create`, which uses the *current* account).
- `lti/ims/tool_setting_controller.rb:41-43` — **no user session at all**: `skip_before_action :load_user, :require_user`; `authenticate_api_call` (LTI 2 OAuth2 service token, scope `ToolProxySettings`/`ToolProxyBindingSettings`/`LtiLinkSettings`).
- `lti/ims/dynamic_registration_controller.rb:29-30` — `create/update/show_configuration` skip `require_user` (token-authenticated); everything else `require_account`; `registration_token` + `update_registration_overlay` additionally require **site-admin modify** permission.
- `master_courses/master_templates_controller.rb:243-247` — `require_course_level_manage_rights` for everything; `update_associations` additionally `require_account_level_manage_rights`.
- `role_overrides_controller.rb:173-182` — `require_context` + `require_role`; writes gated by `RoleOverride.manageable_roles`/`manage_role_overrides` style checks (`add_role`, `update`, `remove_role`, `activate_role` need `manage_role_overrides` on the context).
- `usage_rights_controller.rb:112/135/147` — `authorized_action(@context, @current_user, :manage_files_edit)` for set, `:manage_files_delete` for remove, `:read` (context read) for licenses.

| Action family | Root account admin | Sub-account admin | Teacher (course) | Student | Anonymous / service token |
|---|---|---|---|---|---|
| lti_registrations list/show/history/overlay_history/context_search/check_domain_duplicates | yes | read-only yes | no (403) | no (403) | no (401) |
| lti_registrations create/update/destroy/reset/bind/unbind/install_from_template/validate | yes | only if registration's account is in their sub-tree, else 400/403 | no | no | no |
| deployments + context_controls CRUD | yes (root only) | no (403 — `require_root_account`) | no | no | no |
| tool_setting GET/PUT | n/a | n/a | n/a | n/a | LTI 2 service token only (no user) |
| dynamic_registration registration_token / overlay PUT | site-admin modify only | no | no | no | n/a |
| dynamic_registration create/update/show_configuration | token-authenticated (no session); account-scoped | — | — | — | yes (DR token) |
| blueprint templates read + migrations | yes | yes (within tree) | yes if `manage_courses`-level rights (see note) | no | no |
| blueprint update_associations / restrict_item | yes | yes within tree | requires account-level manage rights | no | no |
| roles list/show/permissions list | yes | yes (within tree) | no (403) | no | no |
| roles create/update/delete/activate | yes | yes (within tree, `manage_role_overrides`) | no | no | no |
| check_account_permission | yes | yes (within tree) | course-context variant: enrolled teacher | no | no |
| usage_rights set (PUT) | yes | yes | yes (`manage_files_edit`) | no (403) | no |
| usage_rights remove (DELETE) | yes | yes | yes (`manage_files_delete`) | no | no |
| content_licenses (GET) | yes | yes | yes (context `read`) | yes (context read) | no |

Note on blueprint reads: `require_course_level_manage_rights` in Canvas maps to course `manage_content`/blueprint rights; the existing `star-courses` implementation (shared helper at `lib.rs:2330`) already encodes this — keep its behavior and mirror it in tests rather than re-deriving.

## 4. Tenant / isolation semantics

- **Account scoping.** All `lti/*` registration surfaces are rooted at `accounts/:account_id`. Canvas resolves the **root account** (`require_root_account`) for deployments/controls: a sub-account id in the path resolves to its root for permission purposes, but writes from sub-accounts are rejected (see `restrict_sub_account_to_read_only`). Roles API: a role is visible in an account only if the role's account is the context or an ancestor (Canvas `RoleOverridesController#api_index` includes base roles + custom roles of the account and ancestors); star-permissions must scope `WHERE role.account_id IN (ancestors of :account_id ∪ :account_id)` using the existing BFS account tree helper.
- **Cross-account 404 vs 403 (Canvas behavior, verified).**
  - `lti/registrations#show` for an id in a *different* account: Canvas looks up within the account chain and returns **404 with body `{"errors": "LTI registration not found"}`** (`show_by_client_id` path, lines 1466/1473).
  - `lti/registrations#destroy` when the registration exists but belongs to another account: **400 `{"errors": "registration does not belong to account"}`** (line 1710), because the record is found globally first.
  - `lti/deployments#create` with a context outside the current root account: **404 `{"error": "Context must belong to the current root account"}`** (line ~120 of deployments_controller).
  - Permission failures (wrong role, sub-account attempting writes) are **403** via `authorized_action`/`require_context_with_permission`.
  - `usage_rights`: file ids that belong to another context are simply not matched by `@context.files` scoping — the handler returns **400 "Must supply..." only when the parameter list is empty**; foreign ids produce a no-op success listing them under `file_ids`. Preserve this quirk.
- **Course isolation for master_courses:** all reads/writes scope by `courses/:course_id`; a template id from another course yields 404 (`get_template` → `ActiveRecord::RecordNotFound`), not 403.
- **Tool settings isolation:** settings are keyed by `(tool_proxy_guid, context_type, context_id, resource_link_id)`; a wrong `tool_proxy_guid` is **404 `{"status":"not_found","errors":[{"message":"not_found"}]}`** (rescue block at tool_setting_controller.rb:27-34).

## 5. Data model & migration needs

Coordinator reserves numbers at dispatch (ledger `orchestration/migration-reservations.json`); the proposed allocation below assumes the next available block starting at **0108** (additive only, `CREATE TABLE IF NOT EXISTS` / `ADD COLUMN IF NOT EXISTS`):

| Number | Name (proposal) | Contents |
|---|---|---|
| 0108 | `lti_context_controls` | `CREATE TABLE lti_context_controls (id BIGSERIAL PK, deployment_id BIGINT NOT NULL REFERENCES lti_deployments(id) ON DELETE CASCADE, registration_id BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE, context_type TEXT NOT NULL ('Account'\|'Course'), context_id BIGINT NOT NULL, state TEXT NOT NULL ('off'\|'on'\|'default'), workflow_state TEXT NOT NULL DEFAULT 'active', created_at/updated_at TIMESTAMPTZ DEFAULT now())` + unique index `(deployment_id, context_type, context_id)` + index on `registration_id`. Backs §2.3 and `deployments/:id/controls`. |
| 0109 | `lti_registration_update_requests` + binding state | `CREATE TABLE lti_registration_update_requests (id BIGSERIAL PK, registration_id BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE, uuid TEXT NOT NULL UNIQUE, status TEXT NOT NULL ('pending'\|'applied'\|'dismissed'), update_payload JSONB NOT NULL DEFAULT '{}', created_by_id BIGINT REFERENCES users(id), created_at/updated_at)`; plus `ALTER TABLE lti_registration_account_bindings ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'active'` (bind/unbind state machine: `on`\|`off`\|`deleted`) if 0078 lacks it. Backs update_requests/*, latest_update_request, unbind, and the DR uuid lookup. |
| 0110 | `lti_registration_overlays` (history) | `CREATE TABLE lti_registration_overlays (id BIGSERIAL PK, registration_id BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE, version INT NOT NULL, overlay JSONB NOT NULL DEFAULT '{}', created_by_id BIGINT REFERENCES users(id), created_at TIMESTAMPTZ DEFAULT now())` + unique `(registration_id, version)`. Backs `history`, `overlay_history`, `reset`, and DR `PUT .../overlay`. |
| 0111 | `tool_settings` (LTI 2) | `CREATE TABLE lti_tool_settings (id BIGSERIAL PK, tool_proxy_id TEXT NOT NULL, context_type TEXT NULL, context_id BIGINT NULL, resource_link_id TEXT NULL, custom JSONB NOT NULL DEFAULT '{}', created_at/updated_at, UNIQUE (tool_proxy_id, context_type, context_id, resource_link_id))` — nullable tuple mirrors Canvas (ToolProxy row has all NULLs; binding has context only; LtiLink has all three). Backs §2.4. |
| 0112 | DR columns + registration_tokens | `ALTER TABLE lti_registrations ADD COLUMN IF NOT EXISTS registration_uuid TEXT UNIQUE, ADD COLUMN IF NOT EXISTS vendor_domain TEXT, ADD COLUMN IF NOT EXISTS client_id BIGINT, ADD COLUMN IF NOT EXISTS installed_at TIMESTAMPTZ, ADD COLUMN IF NOT EXISTS source_lti_registration_id BIGINT REFERENCES lti_registrations(id)`; `CREATE TABLE lti_registration_tokens (token TEXT PRIMARY KEY, account_id BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE, expires_at TIMESTAMPTZ NOT NULL, used_at TIMESTAMPTZ, created_at TIMESTAMPTZ DEFAULT now())`. Backs by_client_id/by_utid/install_status and the whole §2.5 family. |

**No new migrations** for roles (table `roles` exists since 0099, `role_overrides` since 0031 with 0066 indexes) or usage_rights (`usage_rights` table + `files.usage_rights_id` since 0084).

## 6. Behavior contracts for missing routes

Unless stated, request/response bodies follow the Canvas JSON builders named in each item; error shape is the workspace standard `{"errors": "..."}`.

### 6.1 `lti/registrations` (16 missing)

- **PUT `.../lti_registrations/:id` (`update`)** — accepts the same registration params as create (`name`, `admin_nickname`, `description`, `icon_url`, `overlay`-level fields); Canvas `validate_registration_params` + `restrict_dynamic_registration_updates` (DR-created registrations reject most field updates). 200 with `lti_registration_json(registration, ...)`.
- **POST/DELETE `.../:id/bind` / `unbind`** — body `workflow_state: "on"|"off"`; site-admin context → 400 `{"error_code":"invalid_context","errors":"site admin local bindings are not allowed"}`; invalid state → 400 `invalid_workflow_state`; `unbind` of a registration whose account equals the path account → 400 `invalid_registration` "use destroy to delete local registrations"; no binding → **404** "No binding found for this registration in this account." Success returns `lti_registration_account_binding_json` (fields: `account_id`, `workflow_state`, `created_at`, `updated_at`).
- **GET `.../:id/history`** — array of overlay/registration versions (`registration_uuid`, `overlay`, `updated_at`, `updated_by`), newest first.
- **GET `.../:id/overlay_history`** — overlay-only versions.
- **PUT `.../:id/reset`** — clears the local overlay (or reinstalls the template); 200 with registration JSON.
- **GET `.../:id/latest_update_request`** — latest `lti_registration_update_request` row or `204`/null body when none.
- **GET `.../:id/update_requests/:update_request_id`** and **PUT `.../:update_request_id/apply`** — show one request; `apply` merges `update_payload` into the registration, marks status `applied`, returns registration JSON.
- **GET `.../lti_registration_by_client_id/:client_id`** — 404 `{"errors":"LTI registration not found"}` when absent (verified lines 1466–1476).
- **GET `.../lti_registrations/by_utid/:utid`** — same 404 contract ("Not found" then "LTI registration not found").
- **GET `.../lti_registrations/check_domain_duplicates`** — query `domain=`, returns `{ "duplicates": [...] }`.
- **POST `.../lti_registrations/configuration/validate`** — accepts `lti_configuration` (JSON) or `url` (fetch + parse); exactly one required; errors are arrays of strings with 400; success 200 `{ "configuration": {...} }` (verified lines 1244–1292).
- **GET `.../lti_registrations/install_status/:client_id`** — 404 when unknown; returns registration JSON with install status fields.
- **POST `.../:id/install_from_template`** — clones the template registration into the path account; 201 with the new registration JSON.
- **GET `.../:registration_id/deployments/:deployment_id/context_search`** — `q=` search of deployable courses/accounts, paginated `{ total, data }`-style.

### 6.2 `lti/deployments` (3 missing)

- **GET/DELETE `.../deployments/:id`** — `show` returns `lti_deployment_json` (`id`, `lti_registration_id`, `context_type`, `context_id`, `workflow_state`, `available`…); `destroy` soft-deletes and returns the JSON. Missing id → 404.
- **GET `.../deployments/:id/controls`** — list `lti_context_controls` rows for the deployment (see 6.3 shapes).

### 6.3 `lti/context_controls` (6 missing)

- **GET `.../controls`** — list; **POST `.../controls`** (path param `:current_account_id`) — body `{ context_type, context_id, state }`; **POST `.../controls/bulk`** — body `{ context_controls: [{...}] }` (validated by `validate_bulk_params`); **GET/PUT/DELETE `.../controls/:id`**. Control JSON: `{ id, lti_deployment_id, lti_registration_id, context_type, context_id, state, workflow_state, created_at, updated_at }`. All require root account + `manage_lti_registrations`.

### 6.4 `lti/ims/tool_setting` (12 missing) — **different mount (`/api/lti`) and auth**

- Auth: LTI 2 service token (`authenticate_api_call`), **no user session**. GET accepts `bubble=` (`all`|`distinct`); content negotiation on the `Accept` header selects between `application/vnd.ims.lti.v2.toolsettings+json` (`{"@graph":[{custom,type,id}]}`) and `...simple+json` (flat custom map). PUT parses the raw body JSON, stores `custom`, returns **200 empty body** (`head :ok`). Wrong/missing setting → 404 `{"status":"not_found","errors":[{"message":"not_found"}]}` (verified rescue block lines 27–34 and `tool_setting_json` lines 100–140).

### 6.5 `lti/ims/dynamic_registration` (11 missing)

- **GET `.../registration_token`** — site-admin modify; returns `{ registration_token }` (single-use, TTL).
- **GET `.../registrations/uuid/:uuid`** / `.../lti_registrations/uuid/:uuid` / `.../lti_registration_update_request/uuid/:uuid` — uuid lookups for the DR iframe flow; 404 when absent.
- **GET `.../registrations/:registration_id`** (`show`) and **GET `.../registrations/:registration_id/view`** — IMS-shaped registration/config view (`render_registration(registration, developer_key, deployment)`).
- **PUT `.../registrations/:registration_id/overlay`** — sets the overlay JSONB; site-admin modify.
- **GET `.../dr_iframe`** — minimal HTML bootstrap page (returns HTML, not JSON).
- **POST `/api/lti/registrations`** / **PUT `/api/lti/registrations/:registration_id`** / **GET `/api/lti/registrations/:registration_id`** — token-authenticated (no session): create upserts (Canvas dedupes via `registration_attrs_match?`), update applies changes, `show_configuration` returns the tool configuration JSON.

### 6.6 `role_overrides` roles API (3 missing)

- **POST `.../roles/:id/activate`** — flips `workflow_state` back to `active`; returns `role_json` (same builder as the other role routes: `id, label, base_role_type, workflow_state, permissions{...}`).
- **GET `.../roles/permissions`** — `manageable_permissions`: grouped permission list for the account (array of `{ permission, label, ... }`; Canvas iterates `RoleOverride::KNOWN_PERMISSIONS` grouped by context).
- **GET `.../permissions/:permission`** — `check_account_permission`: returns `{ permission, applicable_roles: [...], account_workflow_state }`-shaped body; unknown permission → 404.

### 6.7 `usage_rights` (4 missing)

- **PUT/GET groups & users variants** — identical contracts to the done course variants: PUT body `{ file_ids[], folder_ids[], publish, usage_rights: { use_justification, legal_copyright, license } }`; 400 `{"message":"Must supply 'file_ids' and/or 'folder_ids' parameter"}` when neither list present; 400 `{"message":"No 'usage_rights' object supplied"}` when the object is missing; response `{ usage_rights: {...}, file_ids: [...] }`. `content_licenses` returns the fixed `UsageRights.licenses` list `{ id, name, url }` (see `lib/content_licenses.rb`; star-files already has the list constant).

## 7. Test plan (per controller)

All tests are DB-backed integration tests in the owning crate's `tests/` following the workspace conventions (unique per-run prefix, full teardown, `SKIP: DATABASE_URL not set` when unset). Every controller group must include: happy path, permission negatives, tenant isolation, and missing-id 404s.

- **star-lti registrations (new routes):** (1) bind/unbind state transitions + the two 400 errors and the 404 no-binding case; (2) history/overlay_history ordering and version contents; (3) reset clears overlay; (4) update-request apply merges payload; (5) validate with inline JSON vs URL vs both/neither (4 cases); (6) check_domain_duplicates; (7) by_client_id/by_utid 404 body shape; (8) permission negatives — teacher token, student token, sub-account admin write attempt (403), cross-account destroy (400); (9) list pagination `{ total, data }`.
- **star-lti deployments + context_controls:** CRUD against seeded registration/deployment; `controls/bulk` validation errors; `for_subaccount_id` + `for_course_id` together → 422; context outside root account → 404; sub-account admin → 403; control JSON shape.
- **star-lti tool_setting (LTI 2):** the 6 GET/PUT path shapes; bubble=all/distinct graph semantics (simple vs full content types); PUT returns empty 200; wrong guid → the exact 404 body `{"status":"not_found", ...}`; **no Authorization user header needed** — service-token auth path.
- **star-lti dynamic_registration:** token issue/expiry; uuid lookups; overlay PUT; create dedupe (`registration_attrs_match?` analog); `dr_iframe` returns HTML.
- **star-courses master_courses (regression only, already complete):** existing blueprint suites must stay green; add no new cases beyond a smoke re-pin if handlers move.
- **star-permissions roles:** activate on deleted role (200 + workflow_state), manageable_permissions grouping, check_account_permission known/unknown permission, sub-account sees ancestor roles only, teacher 403.
- **star-files usage_rights:** group/user PUT + licenses mirror the course cases (both 400 messages verbatim), publish flag behavior, foreign-context file ids quirk (no-op success).

## 8. Open questions

1. **Feature flags.** Canvas gates `reset/context_search/overlay_history` behind `lti_registrations_next` and `history` behind an lti_registrations_history flag, plus `lti_deactivate_registrations` for unbind. Does star-lms implement feature flags for these (`feature_flags` are partially implemented), or do we ship them ungated? Proposal: ship ungated and note the deviation in tests.
2. **IMS auth.** `/api/lti` service-token authentication (OAuth2 client-credentials against developer keys) is a prerequisite for tool_setting + dynamic_registration. Is that in scope for the star-lti implementer, or does it need a shared star-auth primitive first?
3. **Site-admin semantics.** star-lms has no "site admin" account type; `registration_token`/`update_registration_overlay` require it in Canvas. Proposal: treat root-account admin with a `site_admin` role row as equivalent, and document the mapping.
4. **DR networking.** `configuration/validate?url=...` fetches remote JSON in Canvas; the workspace has no outbound HTTP in handlers so far. Confirm whether an outbound fetch is acceptable or whether only inline `lti_configuration` is supported in phase 5.
5. **Master courses scope freeze.** Confirm master_courses stays frozen at 13/13 (this spec treats it as done); if associations/queue internals are re-touched by another epic, re-run the parity tool before merging.
