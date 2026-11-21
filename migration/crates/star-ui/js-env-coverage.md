# js_env coverage report (issue #112)

`window.ENV` is a single typed struct (`star-ui/src/env.rs::JsEnv`) rendered once
in `<head>` of `layouts/base.html` before any other script. The governance test
`env_keys_match_checked_in_contract` (pure unit test, no DB) serializes the
struct and diffs its key set against the checked-in `ENV_KEYS` list — a key
cannot appear or disappear without editing both, and this table.

To add a key: add the field to `JsEnv`, the name to `ENV_KEYS`, a row here.

## Phase 1 — required by the shell and the first journey (18 keys)

| Key | Type | Source |
|---|---|---|
| `current_user_id` | integer | `users.id` via session (`SessionUser`) |
| `current_user_roles` | array of strings | `users.role` (single role today) |
| `current_user_is_student` | boolean | derived: `role == "student"` |
| `DOMAIN_ROOT_ACCOUNT_ID` | integer | root account (accounts.id = 1, seeded) |
| `RAILS_ENVIRONMENT` | string | `STAR_ENV` env var, default `"development"` |
| `ASSET_HOST` | string or null | null (same-origin) |
| `DEEP_LINKING_POST_MESSAGE_ORIGIN` | string | request origin (constant phase 1) |
| `active_context_tab` | string or null | shell route handler arg (`"home"` on course home) |
| `context_asset_string` | string | `"{context}_{id}"` e.g. `course_42` |
| `PREFERENCES.dashboard_view` | `"cards"` \| `"activity"` | user_preferences JSONB, default `"cards"` |
| `PREFERENCES.custom_colors` | object<string,string> | user_preferences JSONB, default `{}` |
| `PREFERENCES.hide_dashcard_color_overlays` | boolean | user_preferences JSONB, default false |
| `SETTINGS.collapse_global_nav` | boolean | user_preferences JSONB, default false |
| `use_high_contrast` | boolean | user_preferences JSONB, default false |
| `help_link_name` | string | constant `"Help"` |
| `k12` | boolean | constant false |
| `files_domain` | string | constant `""` (no files host in phase 1) |
| `notices` | array of strings | flash messages on redirect |

## Phase 2 — frozen shape, constant/null until subsystems land

| Key | Value today | Future source |
|---|---|---|
| `current_user_global_id` | equals `current_user_id` | global ids (no sharding) |
| `current_user_uuid` | null | `users.uuid` (column needed) |
| `current_user_usage_metrics_id` | null | none (no telemetry, likely permanent) |
| `current_user_types` | `[]` | account-users role names |
| `current_user_disabled_inbox` | false | user preference |
| `current_user_visited_tabs` | `[]` | user preference |
| `discussions_reporting` | false | feature flag |
| `group_information` | null | groups API |
| `DOMAIN_ROOT_ACCOUNT_UUID` | null | accounts uuid |
| `captcha_site_key` | null | captcha provider (none) |
| `confetti_branding_enabled` | false | site-admin flag |
| `active_brand_config` | null | branding subsystem |
| `active_brand_config_json_url` | null | branding subsystem |
| `url_to_what_gets_loaded_inside_the_tinymce_editor_css` | `[]` | RCE phase |
| `url_for_high_contrast_tinymce_editor_css` | `[]` | RCE phase |
| `LTI_LAUNCH_FRAME_ALLOWANCES` | `[]` | star-lti launch phase |
| `comment_library_suggestions_enabled` | false | comment library |
| `FEATURES` | `{}` | feature-flag registry |

Permanently omitted (spec §3): SENTRY, PENDO, usage-metrics keys — no telemetry
in star-lms.
