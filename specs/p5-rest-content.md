# Spec: REST content — pages, modules, files/folders, migrations/exports

- **Issue:** #25 (epic: `specs`)
- **Branch:** `p5/specs-25` (from `origin/main` @ `3dc34f6`)
- **Scope:** REST parity for `wiki_pages_api`, `context_modules_api`, `context_module_items_api`, `folders`, `files`, `content_migrations`, `content_exports_api`, plus adjacent `migration_issues`
- **Status:** SPEC — documentation only; no code, no migrations, no push

---

## 1. Overview and fresh coverage

Numbers below were freshly re-measured for this spec (parity tool re-run at `origin/main` @ `3dc34f6`):

```
python3 migration/tools/parity/parity.py report \
  --manifest migration/parity/canvas_manifest.json \
  --star-src migration --out /tmp/parity-fresh
```

Workspace totals: **REST 471/1403 (33.6%)**, UI-referenced **258/503 (51.3%)**, 624 star route registrations (157 extras).

In-scope controllers (from `/tmp/parity-fresh/report.md`):

| Controller | Done | Total | Coverage | Target crate |
|---|---:|---:|---:|---|
| `wiki_pages_api` | 20 | 29 | 69.0% | `star-pages` |
| `context_modules_api` | 5 | 8 | 62.5% | `star-modules` |
| `context_module_items_api` | 8 | 12 | 66.7% | `star-modules` |
| `folders` | 14 | 34 | 41.2% | `star-files` |
| `files` | 12 | 26 | 46.2% | `star-files` |
| `content_migrations` | 6 | 25 | 24.0% | `star-content` |
| `content_exports_api` | 3 | 14 | 21.4% | `star-content` |
| `migration_issues` (adjacent) | 3 | 16 | 18.8% | `star-content` |
| **In-scope total (7 controllers)** | **68** | **148** | **45.9%** | |
| **With `migration_issues`** | **71** | **164** | **43.3%** | |

Missing routes in scope: **80** across the 7 controllers (**93** including `migration_issues`).

Priorities within the gap (UI-referenced missing routes first):
1. Module progression: `modules` batch_update PUT + duplicate, `module_item_sequence`, `select_mastery_path` (all UI-referenced).
2. Folder path resolution (`by_path` x2 for courses is UI-referenced via files browser) and user-facing files listing (`users/:user_id/files` index, quota).
3. Upload finalization: `create_success` GET/POST/OPTIONS — required to close the three-step upload contract (Section 3).
4. Context fan-out for content migrations/exports (accounts/users/groups variants) — mechanical once the course variant exists.
5. Feature-gated niceties last: accessibility scan/queue_scan, `page_title_availability`, icon-maker endpoints, InstFS-internal endpoints.

---

## 2. Route inventory per controller

Status legend: `done` = registered in a star crate and matched by the parity report; `missing` = Canvas route with no star match. `UI` = route referenced from Canvas `ui/`. Canvas source: `config/routes.rb` + the listed controllers.

### 2.1 `wiki_pages_api` — 20/29 done (9 missing) → `star-pages`

| Method | Canvas path | Status | Target | Notes |
|---|---|---|---|---|
| GET | `/courses/:course_id/front_page` | done | star-pages | |
| GET | `/groups/:group_id/front_page` | done | star-pages | |
| GET | `/courses/:course_id/pages` | done UI | star-pages | index |
| GET | `/groups/:group_id/pages` | done | star-pages | index |
| GET | `/courses/:course_id/pages/:url_or_id` | done UI | star-pages | |
| GET | `/groups/:group_id/pages/:url_or_id` | done | star-pages | |
| GET | `/courses/:course_id/pages/:url_or_id/revisions` | done | star-pages | |
| GET | `/courses/:course_id/pages/:url_or_id/revisions/latest` | done | star-pages | |
| GET | `/courses/:course_id/pages/:url_or_id/revisions/:revision_id` | done | star-pages | |
| POST | `/courses/:course_id/pages/:url_or_id/revisions/:revision_id` | done | star-pages | revert |
| GET | `/groups/:group_id/pages/:url_or_id/revisions` | **missing** | star-pages | groups-side revisions |
| GET | `/groups/:group_id/pages/:url_or_id/revisions/latest` | **missing** | star-pages | |
| GET | `/groups/:group_id/pages/:url_or_id/revisions/:revision_id` | **missing** | star-pages | |
| POST | `/groups/:group_id/pages/:url_or_id/revisions/:revision_id` | **missing** | star-pages | groups revert |
| POST | `/courses/:course_id/pages` | done UI | star-pages | |
| POST | `/groups/:group_id/pages` | done | star-pages | |
| POST | `/courses/:course_id/pages/:url_or_id/duplicate` | done UI | star-pages | |
| POST | `/courses/:course_id/pages_ai/alt_text` | done UI | star-pages | Cedar/AI alt text |
| POST | `/groups/:group_id/pages_ai/alt_text` | **missing** | star-pages | groups variant |
| PUT | `/courses/:course_id/front_page` | done | star-pages | |
| PUT | `/groups/:group_id/front_page` | done | star-pages | |
| PUT | `/courses/:course_id/pages/:url_or_id` | done UI | star-pages | |
| PUT | `/groups/:group_id/pages/:url_or_id` | done | star-pages | |
| DELETE | `/courses/:course_id/pages/:url_or_id` | done UI | star-pages | |
| DELETE | `/groups/:group_id/pages/:url_or_id` | done | star-pages | |
| GET | `/courses/:course_id/page_title_availability` | **missing** | star-pages | gated on `permanent_page_links` site-admin feature |
| GET | `/groups/:group_id/page_title_availability` | **missing** | star-pages | same gate |
| POST | `/courses/:course_id/pages/:url_or_id/accessibility/scan` | **missing** | star-pages | sync scan; `a11y_checker_enabled?` gate |
| POST | `/courses/:course_id/pages/:url_or_id/accessibility/queue_scan` | **missing** | star-pages | async scan; same gate |

Note: the prior agent's findings mentioned a groups-side page *duplicate* route; `config/routes.rb` registers `pages/:url_or_id/duplicate` for **courses only** (line 2303), and the manifest contains no groups duplicate route. The manifest-derived inventory above is authoritative.

### 2.2 `context_modules_api` — 5/8 done (3 missing) → `star-modules`

| Method | Canvas path | Status | Target | Notes |
|---|---|---|---|---|
| GET | `/courses/:course_id/modules` | done UI | star-modules | index |
| GET | `/courses/:course_id/modules/:id` | done UI | star-modules | |
| POST | `/courses/:course_id/modules` | done UI | star-modules | |
| PUT | `/courses/:course_id/modules` | **missing** UI | star-modules | batch_update (publish/unpublish/delete) |
| PUT | `/courses/:course_id/modules/:id` | done UI | star-modules | |
| PUT | `/courses/:course_id/modules/:id/relock` | **missing** | star-modules | relock progressions |
| POST | `/courses/:course_id/modules/:module_id/duplicate` | **missing** UI | star-modules | |
| DELETE | `/courses/:course_id/modules/:id` | done UI | star-modules | |

### 2.3 `context_module_items_api` — 8/12 done (4 missing) → `star-modules`

| Method | Canvas path | Status | Target | Notes |
|---|---|---|---|---|
| GET | `/courses/:course_id/modules/:module_id/items` | done UI | star-modules | |
| GET | `/courses/:course_id/modules/:module_id/items/:id` | done UI | star-modules | |
| POST | `/courses/:course_id/modules/:module_id/items` | done UI | star-modules | |
| PUT | `/courses/:course_id/modules/:module_id/items/:id` | done UI | star-modules | |
| DELETE | `/courses/:course_id/modules/:module_id/items/:id` | done UI | star-modules | |
| PUT | `/courses/:course_id/modules/:module_id/items/:id/done` | done | star-modules | mark done |
| DELETE | `/courses/:course_id/modules/:module_id/items/:id/done` | done | star-modules | mark not done |
| POST | `/courses/:course_id/modules/items/:id/duplicate` | done UI | star-modules | note: path omits `:module_id` |
| GET | `/courses/:course_id/module_item_redirect/:id` | **missing** | star-modules | 302 for ExternalUrl items |
| GET | `/courses/:course_id/module_item_sequence` | **missing** UI | star-modules | progression sequence lookup |
| POST | `/courses/:course_id/modules/:module_id/items/:id/mark_read` | **missing** | star-modules | fulfills must-view |
| POST | `/courses/:course_id/modules/:module_id/items/:id/select_mastery_path` | **missing** UI | star-modules | conditional release (cyoe) |

### 2.4 `folders` — 14/34 done (20 missing) → `star-files`

| Method | Canvas path | Status | Target | Notes |
|---|---|---|---|---|
| GET | `/courses/:course_id/folders` | done UI | star-files | list_all_folders; `sort_by` name (default) or position |
| GET | `/courses/:course_id/folders/:id` | **missing** | star-files | show; `:id` may be the literal `root` for the context root folder |
| GET | `/courses/:course_id/folders/:id/duplicates` | done UI | star-files | duplicates; Canvas gates on site-admin `files_a11y_rewrite` + `files_a11y_folder_duplicates` |
| GET | `/courses/:course_id/folders/by_path` | **missing** UI | star-files | resolve_path with empty path → root folder alone |
| GET | `/courses/:course_id/folders/by_path/*full_path` | **missing** UI | star-files | resolve_path; glob segments walked from the context root |
| GET | `/courses/:course_id/folders/icon_maker` | **missing** | star-files | icon_maker_folder; creates on demand |
| GET | `/courses/:course_id/folders/media` | **missing** | star-files | media_folder; creates on demand |
| GET | `/courses/:course_id/folders_and_files` | done UI | star-files | list_all_folders_and_files |
| GET | `/folders/:id` | done UI | star-files | show (context-free form; the done one) |
| GET | `/folders/:id/all` | done UI | star-files | list_folders_and_files |
| GET | `/folders/:id/folders` | done UI | star-files | api_index (subfolders) |
| GET | `/groups/:group_id/folders` | **missing** | star-files | list_all_folders |
| GET | `/groups/:group_id/folders/:id` | **missing** | star-files | show |
| GET | `/groups/:group_id/folders/:id/duplicates` | **missing** UI | star-files | duplicates |
| GET | `/groups/:group_id/folders/by_path` | **missing** | star-files | resolve_path |
| GET | `/groups/:group_id/folders/by_path/*full_path` | **missing** | star-files | resolve_path |
| GET | `/groups/:group_id/folders/media` | **missing** | star-files | media_folder |
| GET | `/groups/:group_id/folders_and_files` | done UI | star-files | list_all_folders_and_files |
| GET | `/users/:user_id/folders` | **missing** | star-files | list_all_folders |
| GET | `/users/:user_id/folders/:id` | **missing** | star-files | show |
| GET | `/users/:user_id/folders/:id/duplicates` | **missing** | star-files | duplicates |
| GET | `/users/:user_id/folders/by_path` | **missing** | star-files | resolve_path |
| GET | `/users/:user_id/folders/by_path/*full_path` | **missing** | star-files | resolve_path |
| GET | `/users/:user_id/folders_and_files` | done UI | star-files | list_all_folders_and_files |
| POST | `/accounts/:account_id/folders` | done UI | star-files | create |
| POST | `/courses/:course_id/folders` | done UI | star-files | create |
| POST | `/folders/:dest_folder_id/copy_file` | **missing** | star-files | copy_file (Section 6.4) |
| POST | `/folders/:dest_folder_id/copy_folder` | **missing** | star-files | copy_folder (Section 6.4) |
| POST | `/folders/:folder_id/files` | done UI | star-files | create_file — upload preflight (Section 3) |
| POST | `/folders/:folder_id/folders` | done UI | star-files | create subfolder |
| POST | `/groups/:group_id/folders` | **missing** | star-files | create |
| POST | `/users/:user_id/folders` | **missing** | star-files | create |
| PUT | `/folders/:id` | done UI | star-files | update |
| DELETE | `/folders/:id` | done UI | star-files | api_destroy |

Note: the context-scoped `GET /courses/:course_id/folders/:id` (and its group/user twins) are missing even though the context-free `GET /folders/:id` is done — the scoped forms additionally accept `root` as `:id`.

### 2.5 `files` — 12/26 done (14 missing) → `star-files`

| Method | Canvas path | Status | Target | Notes |
|---|---|---|---|---|
| GET | `/courses/:course_id/files` | done UI | star-files | api_index |
| GET | `/courses/:course_id/files/:id` | done | star-files | api_show |
| GET | `/courses/:course_id/files/file_ref/:migration_id` | done | star-files | file_ref |
| GET | `/courses/:course_id/files/quota` | done UI | star-files | api_quota |
| GET | `/files/:id` | done UI | star-files | api_show |
| GET | `/files/:id/create_success` | **missing** | star-files | api_create_success; registered twice in `config/routes.rb` (top-level + api scope) — both rows counted in the 26 |
| GET | `/files/:id/create_success` | **missing** | star-files | duplicate registration of the same action |
| GET | `/files/:id/icon_metadata` | **missing** | star-files | icon-maker SVG `<metadata>` parse |
| GET | `/files/:id/public_url` | done | star-files | public_url |
| GET | `/folders/:id/files` | done UI | star-files | api_index (files controller, folder-scoped) |
| GET | `/groups/:group_id/files` | done UI | star-groups | index; served by `star-groups`, not `star-files` |
| GET | `/groups/:group_id/files/:id` | done | star-files | api_show |
| GET | `/groups/:group_id/files/quota` | **missing** | star-files | api_quota |
| GET | `/users/:user_id/files` | **missing** UI | star-files | api_index — user files listing (files browser) |
| GET | `/users/:user_id/files/:id` | done UI | star-files | api_show |
| GET | `/users/:user_id/files/quota` | **missing** UI | star-files | api_quota — admin-visible user quota |
| POST | `/files/:id` | **missing** UI | star-files | undocumented GET-alias of api_show for legacy S3 clients |
| POST | `/files/:id/create_success` | **missing** | star-files | api_create_success; registered twice — both rows counted |
| POST | `/files/:id/create_success` | **missing** | star-files | duplicate registration of the same action |
| POST | `/files/:id/reset_verifier` | **missing** | star-files | rotates uuid; Canvas deprecation NOTICE 2026-04-08, EFFECTIVE 2026-07-07 |
| POST | `/files/capture` | **missing** | star-files | api_capture — InstFS-internal, JWT-authed |
| POST | `/files/update_word_count` | **missing** | star-files | DocViewer-internal, `attachment_jwt` |
| POST | `/rce_linked_file_instfs_ids` | **missing** | star-files | InstFS-internal |
| PUT | `/files/:id` | done UI | star-files | api_update |
| DELETE | `/files/:id` | done UI | star-files | destroy |
| OPTIONS | `/files/:id/create_success` | **missing** | star-files | api_create_success_cors — bare 200 for CORS preflight |

### 2.6 `content_migrations` — 6/25 done (19 missing) → `star-content`

All six course-variant routes are done; the gaps are `asset_id_mapping` and the account/group/user fan-out.

| Method | Canvas path | Status | Target | Notes |
|---|---|---|---|---|
| GET | `/courses/:course_id/content_migrations` | done UI | star-content | index |
| GET | `/courses/:course_id/content_migrations/:id` | done UI | star-content | show; `include[]` |
| GET | `/courses/:course_id/content_migrations/:id/asset_id_mapping` | **missing** | star-content | source→destination id map (Section 6.6) |
| GET | `/courses/:course_id/content_migrations/:id/selective_data` | done UI | star-content | content_list — selective-import tree |
| GET | `/courses/:course_id/content_migrations/migrators` | done UI | star-content | available_migrators |
| POST | `/courses/:course_id/content_migrations` | done UI | star-content | create |
| PUT | `/courses/:course_id/content_migrations/:id` | done UI | star-content | update |
| GET | `/accounts/:account_id/content_migrations` | **missing** | star-content | index |
| GET | `/accounts/:account_id/content_migrations/:id` | **missing** | star-content | show |
| GET | `/accounts/:account_id/content_migrations/:id/selective_data` | **missing** | star-content | content_list |
| GET | `/accounts/:account_id/content_migrations/migrators` | **missing** | star-content | available_migrators |
| POST | `/accounts/:account_id/content_migrations` | **missing** | star-content | create |
| PUT | `/accounts/:account_id/content_migrations/:id` | **missing** | star-content | update |
| GET | `/groups/:group_id/content_migrations` | **missing** | star-content | index |
| GET | `/groups/:group_id/content_migrations/:id` | **missing** | star-content | show |
| GET | `/groups/:group_id/content_migrations/:id/selective_data` | **missing** | star-content | content_list |
| GET | `/groups/:group_id/content_migrations/migrators` | **missing** | star-content | available_migrators |
| POST | `/groups/:group_id/content_migrations` | **missing** | star-content | create |
| PUT | `/groups/:group_id/content_migrations/:id` | **missing** | star-content | update |
| GET | `/users/:user_id/content_migrations` | **missing** | star-content | index |
| GET | `/users/:user_id/content_migrations/:id` | **missing** | star-content | show |
| GET | `/users/:user_id/content_migrations/:id/selective_data` | **missing** | star-content | content_list |
| GET | `/users/:user_id/content_migrations/migrators` | **missing** | star-content | available_migrators |
| POST | `/users/:user_id/content_migrations` | **missing** | star-content | create |
| PUT | `/users/:user_id/content_migrations/:id` | **missing** | star-content | update |

### 2.7 `content_exports_api` — 3/14 done (11 missing) → `star-content`

| Method | Canvas path | Status | Target | Notes |
|---|---|---|---|---|
| GET | `/courses/:course_id/content_exports` | done UI | star-content | index; visible-to-user, not-for-copy |
| GET | `/courses/:course_id/content_exports/:id` | done | star-content | show |
| GET | `/courses/:course_id/content_list` | **missing** | star-content | content_list — `read_as_admin` |
| POST | `/courses/:course_id/content_exports` | done UI | star-content | create |
| PUT | `/courses/:course_id/content_exports/:id` | **missing** | star-content | update — New Quizzes export state only |
| PUT | `/courses/:course_id/content_exports/:id/fail` | **missing** | star-content | fail — site-admin only |
| GET | `/groups/:group_id/content_exports` | **missing** | star-content | index |
| GET | `/groups/:group_id/content_exports/:id` | **missing** | star-content | show |
| POST | `/groups/:group_id/content_exports` | **missing** | star-content | create (zip only for groups) |
| PUT | `/groups/:group_id/content_exports/:id/fail` | **missing** | star-content | fail — site-admin only |
| GET | `/users/:user_id/content_exports` | **missing** | star-content | index |
| GET | `/users/:user_id/content_exports/:id` | **missing** | star-content | show |
| POST | `/users/:user_id/content_exports` | **missing** | star-content | create (zip only for users) |
| PUT | `/users/:user_id/content_exports/:id/fail` | **missing** | star-content | fail — site-admin only |

### 2.8 `migration_issues` (adjacent) — 3/16 done (13 missing) → `star-content`

| Method | Canvas path | Status | Target | Notes |
|---|---|---|---|---|
| GET | `/courses/:course_id/content_migrations/:content_migration_id/migration_issues` | done UI | star-content | index |
| GET | `/courses/:course_id/content_migrations/:content_migration_id/migration_issues/:id` | **missing** | star-content | show |
| POST | `/courses/:course_id/content_migrations/:content_migration_id/migration_issues` | done UI | star-content | create |
| PUT | `/courses/:course_id/content_migrations/:content_migration_id/migration_issues/:id` | done | star-content | update — resolve/reactivate |
| GET | `/accounts/:account_id/content_migrations/:content_migration_id/migration_issues` | **missing** | star-content | index |
| GET | `/accounts/:account_id/content_migrations/:content_migration_id/migration_issues/:id` | **missing** | star-content | show |
| POST | `/accounts/:account_id/content_migrations/:content_migration_id/migration_issues` | **missing** | star-content | create |
| PUT | `/accounts/:account_id/content_migrations/:content_migration_id/migration_issues/:id` | **missing** | star-content | update |
| GET | `/groups/:group_id/content_migrations/:content_migration_id/migration_issues` | **missing** | star-content | index |
| GET | `/groups/:group_id/content_migrations/:content_migration_id/migration_issues/:id` | **missing** | star-content | show |
| POST | `/groups/:group_id/content_migrations/:content_migration_id/migration_issues` | **missing** | star-content | create |
| PUT | `/groups/:group_id/content_migrations/:content_migration_id/migration_issues/:id` | **missing** | star-content | update |
| GET | `/users/:user_id/content_migrations/:content_migration_id/migration_issues` | **missing** | star-content | index |
| GET | `/users/:user_id/content_migrations/:content_migration_id/migration_issues/:id` | **missing** | star-content | show |
| POST | `/users/:user_id/content_migrations/:content_migration_id/migration_issues` | **missing** | star-content | create |
| PUT | `/users/:user_id/content_migrations/:content_migration_id/migration_issues/:id` | **missing** | star-content | update |

## 3. Storage, upload and download boundary

Canvas hides whether an Attachment's bytes live on local disk, S3, or InstFS behind the `Attachment` model; controllers and the JSON API only ever talk to the attachment. star-lms must keep the same boundary:

- **Require a storage trait.** An async `Storage` trait (put/get/delete/stat keyed by the file uuid) with a **local-filesystem adapter first** and an **object-store adapter later** (dependency MIG-04, Section 8). Route handlers and JSON serialization never name a backend.
- **Three-step upload semantics** (Canvas contract to replicate):
  1. **Preflight** — `POST /courses/:course_id/files`, `POST /folders/:folder_id/files`, or the `pre_attachment` sub-hash on content-migration create, with `name` and `size`. Quota is checked here. The response creates the attachment in the pre-upload state (Canvas `file_state: "deleted"`) and returns `upload_url` + `upload_params` (plus a `Location` header).
  2. **Byte transfer** — the client POSTs the bytes as multipart to the `upload_url` with the returned params. In star-lms the upload URL points at a workspace-owned **proxied** endpoint that streams to the storage adapter; no presigned object-store URLs in v1.
  3. **Finalization** — `GET`/`POST /files/:id/create_success?uuid=...`. Requires the uuid to match the pre-created attachment and the file to still be in the pre-upload state; flips it to available, applies `on_duplicate` (`rename`/`overwrite`; **409 conflict** when a same-name file exists in the destination folder and no `on_duplicate` was given), runs the context upload-success callback, and returns the attachment JSON (`preview_url`/`enhanced_preview_url` and `usage_rights` includes; verifier omitted in-app). `OPTIONS` on the same path is a bare 200 for CORS preflight, and `POST /files/:id` stays as the undocumented GET-alias for legacy clients.
- **Download** — `GET /files/:id/download?download_frd=1&verifier=<files.uuid>`: the verifier is the attachment uuid (Canvas has no separate verifier column). `POST /files/:id/reset_verifier` rotates the uuid so old links stop working. Canvas has posted a deprecation **NOTICE 2026-04-08, EFFECTIVE 2026-07-07** for uuid-based verification: implement `reset_verifier` for parity and revisit before the effective date (Section 8).
- **Folder tree** — one `parent_folder_id` tree per context with `by_path` resolution (walk name segments from the root; hidden/locked folders only included when the viewer may see hidden files) and the full path cached in `folders.full_name`. Each context gets **root** (`course files`), **unfiled**, and **media** folders created on demand (plus an icon-maker folder for icon uploads).

---

## 4. Permission matrix per controller

Abilities below are the Canvas `authorized_action`/`grants_right` checks verified in the controllers. `GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS` = any of `manage_course_content_add` / `manage_course_content_edit` / `manage_course_content_delete`.

| Controller / route family | Ability | Notes |
|---|---|---|
| **wiki_pages_api** — index, show, front_page | context wiki `read` | plus pages tab enabled |
| wiki_pages_api — create, duplicate | page `create`; duplicate: context `GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS` | |
| wiki_pages_api — update | page `update` or `update_content` | `update_content` allows body-only edits where full `update` is denied |
| wiki_pages_api — delete | page `delete` | plus editing-restricted check |
| wiki_pages_api — revisions list/show | page `read_revisions` | |
| wiki_pages_api — revert | page `read_revisions` **and** `update` | |
| wiki_pages_api — accessibility scan/queue_scan | context `GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS` + `a11y_checker_enabled?` | |
| wiki_pages_api — page_title_availability | site-admin `permanent_page_links` feature | feature-gated |
| **context_modules_api** — index, show | context `read` | |
| context_modules_api — create | context_modules temp-record `create` (maps to `manage_course_content_add`) | |
| context_modules_api — duplicate | context `manage_course_content_add` | |
| context_modules_api — batch_update | context `manage_course_content_edit` | |
| context_modules_api — update, relock | module `update` | |
| context_modules_api — destroy | module `delete` | |
| **context_module_items_api** — index, show | context `read` | |
| context_module_items_api — create, update, destroy, duplicate | parent module `update` | |
| context_module_items_api — redirect | context `read`; unpublished items also need module `update` | |
| context_module_items_api — done/not-done, item_sequence, mark_read | progression checks: `read_grades` (via `student_id` enrollments) or `participate_as_student` (self) or `read_as_admin` | `find_student` logic |
| context_module_items_api — select_mastery_path | context `read` + (student is self or `manage_assignments_edit`) + cyoe enabled | |
| **folders** — list_all_folders, by_path, media_folder | context `read_files`; by_path also accepts any granular file permission | |
| folders — show | folder `read_contents` (context `read_files` for the `root` alias) | |
| folders — create | folder `create` | |
| folders — update | folder `update` | |
| folders — api_destroy | folder `delete` | |
| folders — copy_file | dest folder `manage_contents` + source file `download` + new attachment `create` | |
| folders — copy_folder | dest folder `manage_contents` + source context granular file permissions + new folder `create` | |
| folders — icon_maker_folder | context granular file permissions | |
| folders — duplicates | folder `read_contents` (plus site-admin feature gates) | |
| **files** — api_index, quota | context `read_files`; user-context quota also visible with granular file permissions | |
| files — create (preflight) | attachment `create` | |
| files — api_show, public_url, download | attachment `read` / `download` | |
| files — api_update | attachment `update` | |
| files — destroy | attachment `delete` | |
| files — create_success | attachment lineage from the preflight (matching uuid + pre-upload state) | Section 3 |
| files — reset_verifier | `manage_files_edit` or `manage_files_delete` **and** root account `become_user` (user context: masquerade) | |
| **content_migrations** — index, show, selective_data, migrators, asset_id_mapping | context `GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS` | any-of |
| content_migrations — create | `manage_course_content_add` | |
| content_migrations — update | `manage_course_content_edit` | |
| **content_exports_api** — index, show, create | context `read`; create also validates export_type per context | |
| content_exports_api — content_list | `read_as_admin` | |
| content_exports_api — update | (New Quizzes export state; internal caller contract) | |
| content_exports_api — fail | `Account.site_admin` `read` | site-admin only |
| **migration_issues** — index, show | context `GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS` | |
| migration_issues — create | `GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS` | |
| migration_issues — update | `manage_course_content_edit` | |

## 5. Data model and migration needs

**Numbering protocol:** max merged migration is `0107`; the next number is coordinator-assigned via `orchestration/migration-reservations.json` (currently `next_available: 0108`). Agents never self-assign. All changes below are **additive columns only** — no renumbering, no DROP/TRUNCATE. All needed tables already exist (`files`, `folders`, `pages`, `page_revisions`, `modules`, `module_items`, `module_item_completions`, `content_migrations`, `content_exports`, `migration_issues`, `usage_rights`), so the needs are new columns on existing tables.

Correction to earlier notes (spot-checked against `migration/crates/star-db/migrations/`): `files.uuid` and `files.visibility_level` **already exist** — added by `0061_folder_tree.sql` (along with `workflow_state`, `hidden`, `locked`, `lock_at`/`unlock_at`, `modified_at`, `category`, `position`). Canvas has no separate verifier column (the URL verifier *is* the uuid). The real additive gaps:

| Table | Existing (highlights) | Additive needs |
|---|---|---|
| `files` | id, course_id, uploader_id, filename, display_name, content_type, size, folder_id, workflow_state, hidden, locked, lock_at/unlock_at, modified_at, category, visibility_level, position, uuid, usage_rights_id | `context_type` + `context_id` (today course-only — blocks the group/user/account file fan-out); upload-lifecycle states on `workflow_state` (`deleted` pre-upload → `available`, for the Section 3 contract; today's default is `processed`); `replacement_attachment_id` (replacement chain in api_show); `thumbnail_url`; `media_entry_id`; `word_count` (update_word_count); `instfs_uuid` (nullable, reserved) |
| `folders` | full Canvas-parity set incl. context polymorphism, tree, locking, `for_submissions` | none required — `by_path` resolves over `parent_folder_id` + `full_name`; root/unfiled/media/icon-maker folders are rows created on demand |
| `pages` | url, workflow_state, editing_roles, front_page, duplicated_from_page_id (+ `page_revisions`) | `context_type` + `context_id` for group wikis — today `pages.course_id` is course-only, so the done-marked group page routes in star-pages decline group contexts; the missing groups revisions routes need the same polymorphism |
| `modules` | name, position, workflow_state | `unlock_at`, `require_sequential_progress`, `publish_final_grade`, `prerequisite_module_ids` (BIGINT[] or join table) — relock reuses `module_item_completions` (delete + recompute; Canvas uses a separate progression table) |
| `module_items` | item_type/item_id, position, title, indent, workflow_state | **completion-requirement columns**: `completion_requirement` JSONB (`{"type":"must_view\|must_contribute\|must_submit\|must_mark_done\|min_score","min_score":n}`); `new_tab`; `external_url`; `page_url` |
| `module_item_completions` | module_id, item_id, user_id, state | none required (optionally `requirement_completed_at`) |
| `content_migrations` | course_id, user_id, migration_type, workflow_state (full Canvas state set), migration_settings, source_course_id, attachment_name/size | `context_type` + `context_id` (account/group/user fan-out — today course-only); `initiated_source`; `asset_id_mapping` JSONB (Section 6.6); `use_global_identifiers` BOOLEAN; `child_subscription_id` BIGINT NULL (index filter) |
| `content_exports` | course_id, user_id, export_type, workflow_state, progress | `context_type` + `context_id`; `selected_content` JSONB (the `select` param); `attachment_id` (export artifact, once MIG-04 lands); `new_quizzes_export_state`; `new_quizzes_export_url` |
| `migration_issues` | full set incl. `0086` extras (issue_link, fixable_by_web_upload, fixable_by_admin) | none required |

---

## 6. Behavior contracts for missing routes

Contracts below are transcribed from the Canvas controllers in `canvas-lms/app/controllers/` (files cited). Status codes and exact error messages matter for client compatibility.

### 6.1 `wiki_pages_api` (star-pages)

- **Groups revisions** (`GET …/revisions`, `…/revisions/latest`, `…/revisions/:revision_id`, `POST …/revisions/:revision_id`): identical contract to the done courses variant — list returns `[{revision_id, created_at, …}]` paginated; `latest`/`:revision_id` return the page-shaped revision body; revert requires `read_revisions` + `update`, bumps a new revision and returns the reverted page. Blocked only by the group-context modeling (Section 5).
- **`page_title_availability`** (`GET`): gated on the site-admin `permanent_page_links` feature; returns availability of a page title as a link slug. Ship behind the same feature flag; 403/404 when off.
- **Accessibility scan / queue_scan** (`POST …/accessibility/scan`, `…/accessibility/queue_scan`): both require context granular manage-content permissions **and** `a11y_checker_enabled?` (else unauthorized). `scan` runs synchronously and returns the issue list; `queue_scan` enqueues and returns a Progress-style accepted response.
- **Groups `pages_ai/alt_text`** (`POST`): same Cedar/AI contract as the courses variant against the group wiki.

### 6.2 `context_modules_api` (star-modules)

- **`PUT /courses/:course_id/modules` (batch_update)**: params `event` (`publish` | `unpublish` | `delete`), `module_ids[]`, optional `async`, `skip_content_tags`. 400 `{"message":"need to specify event"}` / `{"message":"invalid event"}` / `{"message":"must specify module_ids[]"}`; 404 `{"message":"no modules found"}`. Sync response `{"completed":[ids],"progress":null}`; with `async=true` a Progress is created (tag `context_module_batch_update`) and returned in `progress` while the job runs. `publish` failures report per-item reasons (e.g. `file_in_hidden_folder`, `usage_rights_required`).
- **`POST /courses/:course_id/modules/:module_id/duplicate`**: 400 `{"error":"unable to find module to duplicate"}` / `{"error":"cannot duplicate this module"}` (`can_be_duplicated?`). The copy is inserted at `old.position + 1`; response is the new module JSON with `content_tags` included plus an `ENV_UPDATE` array carrying `{id, content_id, content_details}` for attachment tags.
- **`PUT /courses/:course_id/modules/:id/relock`**: resets progressions to the default locked state and recalculates against current requirements; returns the module JSON. In star-lms: delete the module's `module_item_completions` and recompute.

### 6.3 `context_module_items_api` (star-modules)

- **`GET /courses/:course_id/module_item_redirect/:id`**: finds the not-deleted tag; unpublished tag or module additionally requires module `update`. Only `ExternalUrl` items redirect (302 to `tag.url`, recording a read action); anything else is 400 `{"message":"incorrect module item type"}`.
- **`GET /courses/:course_id/module_item_sequence`**: params `asset_type` (mapped API→Canvas name) and `asset_id`; 400 `{"message":"invalid asset_type"}` / `{"message":"missing asset_id"}`. Returns the module/item sequence around the asset (`modules[]` with `items[]` and prev/next linkage).
- **`POST …/items/:id/mark_read`**: fulfills the `must_view` requirement without the html redirect; 403 `{"message":"The module item is locked."}` for locked items; cannot complete locked/unpublished items; success is `{"message":"OK"}`.
- **`POST …/items/:id/select_mastery_path`**: 400 `{"message":"mastery paths not enabled"}` unless cyoe is enabled; 400 `{"message":"assignment_set_id required"}`; 400 `{"message":"requested item is not an assignment"}`. Selection is processed via the conditional-release handler (delayed); response `{"meta":{"primaryCollection":"assignments"},"items":[…],"assignments":[…]}`.

### 6.4 `folders` (star-files)

- **`by_path`** (`GET …/folders/by_path` and `…/by_path/*full_path`): returns the folder chain from the context root to the target (root alone for an empty path); 404 when any segment misses. Hidden/locked folders appear only for viewers who can see hidden files. Authorization is `read_files` or any granular file permission.
- **Context-scoped `show`** (`GET /courses/:course_id/folders/:id` etc.): same folder JSON as the done `/folders/:id`; `:id` may be `root`.
- **`media_folder`** (`GET …/folders/media`): `read_files`; returns (creating if absent) the context's media upload folder — owned by the **current user** when they lack granular file permissions, else by the context.
- **`icon_maker_folder`** (`GET …/folders/icon_maker`): granular file permissions; returns/creates the icon-maker folder.
- **`copy_file`** (`POST /folders/:dest_folder_id/copy_file`): param `source_file_id` (400 `{"message":"source_file_id must be provided"}`), optional `on_duplicate` (`rename`/`overwrite`, else 400), optional `name`. When the destination already holds a same-display-name file and no `on_duplicate` is given: **409** `{"message":"file already exists; set on_duplicate to 'rename' or 'overwrite'"}`; races default to rename. Cross-institution copies: 400 `{"message":"cannot copy across institutions"}`. Returns the new attachment JSON.
- **`copy_folder`** (`POST /folders/:dest_folder_id/copy_folder`): param `source_folder_id` (400 if missing); 400 `{"message":"source folder may not contain destination folder"}` for same-context cycles; folder is renamed at the destination on name collision (deep copy, `everything: true`). Returns the new folder JSON.
- **`duplicates`** (groups/users variants): same contract as the done courses route — `{duplicates:[…]}` of same-name sibling folders under the same parent; site-admin feature-gated in Canvas.
- **Groups/users `list_all_folders`, `show`, `create`**: mechanical fan-out of the course contracts over the context polymorphism.

### 6.5 `files` (star-files)

- **`create_success`** (`GET`/`POST`/`OPTIONS /files/:id/create_success`): Section 3 step 3. Looks the attachment up by **id + matching uuid**; 400 unless it is still in the pre-upload (`deleted`) state; quota re-checked (over-quota → 400); then flips to available, applies `on_duplicate` (409 on unspecified collision), and returns the attachment JSON with `preview_url`/`enhanced_preview_url`, `usage_rights`, and doc-preview fields merged. OPTIONS is a bare 200 (CORS). Canvas registers the GET and POST each twice in `config/routes.rb`; star-lms registers each once.
- **`POST /files/:id`**: undocumented alias of GET api_show (legacy S3 clients POSTing to the `Location` header); same response body.
- **`POST /files/:id/reset_verifier`**: rotates the uuid (invalidating old verifier links) and returns the file JSON. Permissions per Section 4. Canvas deprecation NOTICE 2026-04-08 / EFFECTIVE 2026-07-07.
- **`GET /users/:user_id/files` and `…/files/quota`**: api_index over the user's files (hidden files only for admins/self) and `{quota, quota_used}` — the user-context quota is additionally visible to admins holding granular file permissions.
- **`GET /groups/:group_id/files/quota`**: `{quota, quota_used}` for the group context.
- **`GET /files/:id/icon_metadata`**: 404 for deleted attachments (after following the replacement chain); 400 `{"errors":[{"message":"The requested attachment does not support viewing metadata."}]}` unless the attachment category is icon-maker; streams the SVG and extracts `<metadata>` — `{"name": display_name, …}` on success, 204 when absent.
- **Internal endpoints** — `POST /files/capture` (InstFS service JWT; 404 when InstFS is disabled, 403 on bad token, 400 on missing params), `POST /files/update_word_count` (attachment_jwt + word_count; 400 on missing/invalid; 204 on success), `POST /rce_linked_file_instfs_ids` (400 "No valid file URLs given"; 422 above 100 URLs). Recommend serving 404/decline until storage parity (Section 8).

### 6.6 `content_migrations` (star-content)

- **`asset_id_mapping`** (`GET /courses/:course_id/content_migrations/:id/asset_id_mapping`): 400 `{"message":"Migration is incomplete"}` unless `imported?`; 400 `{"message":"Migration is not course copy or blueprint"}` unless `source_course_id` is set. Response maps API asset types to `{source_id: destination_id}` strings: `{"assignments":{"13":"740"},"modules":{…}}` for `announcements`, `assignments`, `discussion_topics`, `files`, `module_items`, `modules`, `pages`, `quizzes`.
- **Account/group/user fan-out (index, show, selective_data, migrators, create, update)**: identical contracts to the done course routes once `content_migrations` carries context polymorphism (Section 5). `create` validates the migrator plugin per context (400 `{"message":"Invalid migration_type"}` / `{"message":"Unsupported migration_type for context"}`); file-upload migrators require `pre_attachment[name]` or `settings[file_url]` or `settings[content_export_id]` (400 otherwise). `update` cannot change `migration_type`; new `pre_attachment` values restart a failed upload; selective copy params (`copy[all_x]` / `copy[x][id_…]`) drive the import. `index` filters out child subscriptions and orders by id desc; `migrators` returns `[{type, requires_file_upload, name, required_settings}]`.

### 6.7 `content_exports_api` (star-content)

- **`PUT /courses/:course_id/content_exports/:id`**: only touches common-cartridge exports created for New Quizzes; params `new_quizzes_export_state` and `new_quizzes_export_url` — a `completed` state re-triggers the export, a `failed` state fails it with "New Quizzes failed to export"; returns the export JSON with `new_quizzes_export_settings`. 400 with errors on invalid updates.
- **`PUT …/content_exports/:id/fail`** (all three contexts): site-admin only; marks the export failed with "manually marked failed by a site administrator" and returns the export JSON.
- **`GET /courses/:course_id/content_list`**: `read_as_admin`; enumerates exportable content as a selective tree; 400 `{"message":"unsupported migration type"}` for bad `type`.
- **Groups/users fan-out (index, show, create)**: `create` accepts only `export_type=zip` outside Course contexts (Course adds `qti`, `common_cartridge`, `quizzes2`; anything else 400 `{"message":"invalid export_type"}`); `skip_notifications` and the `select` hash (per-type id lists) are honored; index lists exports visible to the user, excluding for-copy exports, newest first.

### 6.8 `migration_issues` (star-content)

- **`GET …/migration_issues/:id` (show)**: same MigrationIssue JSON as index, scoped to the content migration.
- **Account/group/user fan-out (index, show, create, update)**: index is paginated by `created_at`; `update` takes `workflow_state` = `active` | `resolved` — anything else is **403** `{"message":"Must send a valid workflow state"}`; save failures return errors with 400.

## 7. Test plan per controller

Every controller gets integration tests against a real PostgreSQL pool (the workspace pattern: seed, call, assert JSON) plus **permission negatives** (anonymous 401, student 403 on manage routes, cross-course 404). Feature-gated and internal routes get explicit gate tests.

### 7.1 `wiki_pages_api` (star-pages)

- Integration: groups revisions list/latest/show/revert round-trip (revision numbering, revert creates a new revision); page_title_availability on/off the `permanent_page_links` feature; accessibility scan returns issues, queue_scan returns accepted/Progress.
- Negatives: student cannot revert (needs `read_revisions` + `update`); a11y scan 403 with `a11y_checker_enabled?` off; page_title_availability 404 with the feature off; anonymous 401 on all.

### 7.2 `context_modules_api` (star-modules)

- Integration: batch_update publish/unpublish/delete (sync `{completed, progress:null}`); async=true returns a Progress and the job completes; duplicate inserts at position+1 and returns content_tags + ENV_UPDATE; relock clears and recomputes completions.
- Negatives: batch_update 400 for missing/invalid event, missing module_ids, 404 no modules; student 403 on duplicate/batch_update/relock; anonymous 401.

### 7.3 `context_module_items_api` (star-modules)

- Integration: module_item_redirect 302 for ExternalUrl (read action recorded); item_sequence returns prev/current/next around the asset; mark_read completes must_view (403 when locked); select_mastery_path happy path with cyoe enabled.
- Negatives: redirect 400 for non-ExternalUrl items; item_sequence 400 invalid asset_type/missing asset_id; mark_read 403 locked; select_mastery_path 400 mastery-paths-disabled / missing assignment_set_id / non-assignment item; student cannot call redirect for unpublished items without update right.

### 7.4 `folders` (star-files)

- Integration: by_path resolves nested paths (root alone for empty, 404 on miss, hidden folders only for permitted viewers); context-scoped show incl. `root`; media_folder creates on demand (user-owned fallback without granular perms); icon_maker_folder; copy_file rename/overwrite/409-matrix; copy_folder deep copy with rename and cycle 400; groups/users fan-out.
- Negatives: student 403 on copy_file/copy_folder/create/update/destroy; read_files required for by_path/media_folder; duplicates requires folder context match; cross-context copy 400s.

### 7.5 `files` (star-files)

- Integration: full three-step upload (preflight 201 with upload_url/params → byte POST → create_success returns attachment JSON); uuid mismatch 400; double finalize 400 (already available); on_duplicate rename/overwrite/409; quota exceeded 400; reset_verifier rotates uuid and old links stop working; user/group files index + quota shapes; POST /files/:id GET-alias; OPTIONS create_success 200.
- Negatives: anonymous 401; student 403 on api_update/destroy/reset_verifier; reset_verifier without become_user 401; icon_metadata 404 deleted / 400 non-icon / 204 no metadata; internal endpoints (capture, update_word_count, rce_linked_file_instfs_ids) return 404/decline until storage parity.

### 7.6 `content_migrations` (star-content)

- Integration: asset_id_mapping for an imported course-copy migration (`{"assignments":{"13":"740"}}` shape) with 400s for incomplete/non-copy migrations; account/group/user index/show/create/update round-trip; migrators list shape; selective_data tree with property/sub_items_url.
- Negatives: create 400 invalid/unsupported migration_type, 400 missing pre_attachment/file_url/content_export_id; student 403 on all (granular manage-content); update 400 on type change attempt; context mismatch 404.

### 7.7 `content_exports_api` (star-content)

- Integration: create validates export_type per context (zip-only for groups/users); select-hash and skip_notifications persisted; update drives the New Quizzes state machine (completed re-export, failed marks failed); fail marks failed with the site-admin message; content_list returns the tree; groups/users fan-out.
- Negatives: create 400 invalid export_type; fail 401/403 for non-site-admins; content_list 403 without read_as_admin; student 403 where `read` is absent (e.g. concluded enrollments).

### 7.8 `migration_issues` (star-content)

- Integration: show within the migration; update resolve→active round-trip; account/group/user fan-out.
- Negatives: update 403 invalid workflow_state; 400 on save errors; student 403; issue not in the migration 404.

---

## 8. Dependencies and open questions

### Dependencies

- **MIG-04 (storage backend)** — the `Storage` trait + local-filesystem adapter. Blocks real byte handling for the three-step upload (Section 3), `content_exports.attachment_id` artifacts, and migration source packages. The `create_success` / `POST /files/:id` routes can land metadata-first against the trait with the local adapter.
- **Progress/jobs infrastructure** — `batch_update` with `async=true`, `queue_scan`, and migration/export workers all return or require a Progress resource. Reuse the existing jobs API work; define the Progress JSON link fields once.
- **Conditional release (cyoe) model** — `select_mastery_path` needs assignment-set selection. If mastery paths are not yet modeled, ship the route with the Canvas 400 gates and a faithful "mastery paths not enabled" default until the model exists.
- **Accessibility checker** — scan/queue_scan need an a11y engine behind `a11y_checker_enabled?`; keep both routes feature-gated regardless.
- **Context polymorphism migrations** (Section 5) — the account/group/user fan-out for files, migrations, exports, and issues, plus groups wiki pages, all wait on additive `context_type`/`context_id` columns.

### Open questions

1. **Verifier deprecation** — Canvas NOTICE 2026-04-08, EFFECTIVE 2026-07-07 for uuid-based file verification. Do we keep `reset_verifier` past the effective date, and does the download verifier get replaced by signed access tokens (Canvas `access_verifier`)? Decide before 2026-07-07.
2. **InstFS-internal endpoints** (`files/capture`, `rce_linked_file_instfs_ids`, and InstFS-linked `update_word_count` semantics) — no InstFS analog is planned; propose serving 404/decline permanently and documenting the divergence, revisit if an object-store adapter needs a capture-style callback.
3. **Groups wiki storage** — context-polymorphic `pages` vs a separate `group_pages` table; the polymorphic column matches Canvas and the rest of this spec assumes it.
4. **Relock fidelity** — Canvas tracks `ContextModuleProgression` separately; star-lms approximates with `module_item_completions` (delete + recompute). Confirm no UI depends on progression-level state beyond completion.
5. **Selective-data fidelity** — how deep the `selective_data` tree must go for course copy (Canvas enumerates per-item sub_items); v1 can cap depth and keep `sub_items_url` lazy.
6. **`asset_id_mapping` retention** — Canvas keeps mappings across migrations with the same course pair; decide whether star-lms persists the map per migration only (simplest) or accumulates per course pair.
