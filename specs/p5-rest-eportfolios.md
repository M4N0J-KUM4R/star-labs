# P5 REST Spec — ePortfolios

- **Issue**: #113 (epic `specs`), family: eportfolios. Companion: `p5-rest-misc-index.md`.
- **Baseline**: fresh parity run at `526de19`; `eportfolios_api` at **0%** (0/7).
- **Target crate**: new `star-eportfolios` crate.

## 1. Overview and fresh coverage

| Controller | Done | Total | Coverage |
|---|---:|---:|---:|
| `eportfolios_api` | 0 | 7 | 0.0% |

ePortfolios are user-owned collections of category-organized pages with spam moderation on top. The API is small and synchronous: list a user's portfolios, show one, list its pages, soft-delete, restore, and the two spam-moderation actions. Guards (verified): `reject_student_view_student` and `verified_user_check` on every action — the acting user must have a **verified communication channel** (star-lms equivalent: a confirmed `communication_channels` row, which the schema already models via migration 0093).

## 2. Route inventory (7 routes, all todo)

| # | Verb | Canvas path | Action | Status | Notes |
|---|---|---|---|---|---|
| 1 | GET | `/api/v1/users/:user_id/eportfolios` | index | todo | own or `:moderate_user_content`; `include[]=deleted` |
| 2 | GET | `/api/v1/eportfolios/:id` | show | todo | `:read` on the portfolio |
| 3 | GET | `/api/v1/eportfolios/:eportfolio_id/pages` | pages | todo | category-grouped page list |
| 4 | DELETE | `/api/v1/eportfolios/:id` | delete | todo | soft delete |
| 5 | PUT | `/api/v1/eportfolios/:eportfolio_id/restore` | restore | todo | un-delete |
| 6 | PUT | `/api/v1/eportfolios/:eportfolio_id/moderate` | moderate | todo | `spam_status` param |
| 7 | PUT | `/api/v1/users/:user_id/eportfolios` | moderate_all | todo | spam-moderate a user's portfolios |

(The create/update surface for portfolios and pages is HTML-only in Canvas — not in the API manifest.)

## 3. Permission matrix (verified)

| Route | Check | Who passes |
|---|---|---|
| index (1) | `user == current_user` **or** `:moderate_user_content` on the target user | the owner; account moderators/spam admins |
| show (2) | `:read` on the portfolio record | owner; moderators; public portfolios where applicable (portfolio policy) |
| pages (3) | follows show's read path on the portfolio | same as show |
| delete (4) | owner path (scoped find + policy; verified `Eportfolio.find` + authorized read/policy family) | the owner |
| restore (5) | owner path | the owner |
| moderate (6) | `:moderate` on the portfolio | moderators |
| moderate_all (7) | `:moderate_user_content` on the target user | account moderators |

Negatives: anonymous 401 ×7; unverified acting user (no confirmed channel) → the verified-user failure (403, pinned); other student reading someone's portfolio → 403/404 per policy; `spam_status` outside the allowed set → 400 (see §6).

## 4. Job/storage dependency model

**None.** No jobs, no attachments (page content is HTML stored on rows; portfolio snapshots in Canvas use `simply_versioned` — out of API scope). Fully synchronous family.

## 5. Data model and migration needs

One migration — **"010x, coordinator-assigned"** — creating three tables:

- `eportfolios` — `id`, `user_id NOT NULL REFERENCES users(id)`, `name TEXT NOT NULL`, `uuid TEXT`, `workflow_state TEXT NOT NULL DEFAULT 'active' CHECK (workflow_state IN ('active','deleted'))` (verified states), `spam_status TEXT CHECK (spam_status IN ('marked_as_spam','marked_as_safe','flagged_as_possible_spam'))` (values verified from the controller docs/model constants), `public BOOLEAN NOT NULL DEFAULT false`, timestamps. Index `(user_id)`.
- `eportfolio_categories` — `id`, `eportfolio_id NOT NULL REFERENCES eportfolios(id) ON DELETE CASCADE`, `name TEXT NOT NULL`, `position INTEGER`, timestamps.
- `eportfolio_entries` — `id`, `eportfolio_id NOT NULL REFERENCES eportfolios(id) ON DELETE CASCADE`, `eportfolio_category_id REFERENCES eportfolio_categories(id)`, `name TEXT NOT NULL`, `content TEXT`, `slug TEXT`, `position INTEGER`, timestamps.

No changes to existing tables.

## 6. Behavior contracts

- **index (1)**: owner sees own portfolios (active only); `include[]=deleted` adds soft-deleted ones (verified); a moderator sees the target user's portfolios including flagged ones. Envelope `{"eportfolios": [...]}` with pagination.
- **show (2)**: `Eportfolio.find` (global find — a portfolio id is globally addressable), then `:read`; response includes `workflow_state` and `spam_status` (the controller's own API doc exposes `spam_status` with the three values above).
- **pages (3)**: returns the portfolio's entries organized by category; the Canvas doc shape is a list of page objects with their category association — pin the exact envelope at implementation (likely `{"pages": [...]}` with `eportfolio_category` fields; a deliberate-deviation note is acceptable if pinned).
- **delete (4)**: sets `workflow_state='deleted'` (verified model method) and returns the portfolio JSON.
- **restore (5)**: sets `workflow_state='active'`; restoring a non-deleted portfolio is idempotent-success.
- **moderate (6)**: `spam_status` must be one of `marked_as_spam` / `marked_as_safe` (verified permit set); anything else → the controller's invalid-status branch (400-family; pin the exact status — Canvas renders nothing after the guard, so choose 400 with an error body and pin it).
- **moderate_all (7)**: applies `spam_status` to all of the target user's portfolios (including deleted/flagged ones); used by the moderation UI.
- `verified_user_check`: acting user without a confirmed communication channel → 403-style failure on every action (pin status; Canvas's check precedes authorization).

## 7. Test plan

1. Lifecycle: seed portfolio + categories + entries; index (own), show, pages (grouped); delete → workflow_state deleted; index excludes it; `include[]=deleted` re-includes; restore → active again.
2. Moderation: `moderate` with `marked_as_spam` → spam_status set; invalid value → pinned 400; `moderate_all` flips every portfolio of the user; moderator reads another user's index; non-moderator → 403.
3. Verified-user guard: acting user with no confirmed channel → pinned failure on all 7 routes.
4. Permission negatives: anonymous 401 ×7; student B reading student A's private portfolio → 403/404 per policy; `moderate_user_content` holder succeeds on index/moderate_all.

## 8. Open questions

1. Exact pages envelope and whether categories are embedded or sidecar — pin against Canvas's serializer.
2. Whether show exposes public portfolios to anonymous callers in this Canvas checkout (public flag exists; no anonymous API route in the manifest) — recommend requiring auth and noting the deviation.
3. ePortfolio **create/update** (and page create/update) are HTML-only; if the wave-3+ UI work needs them, they are new route additions beyond parity — out of scope here.
