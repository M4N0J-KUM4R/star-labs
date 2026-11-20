//! The `window.ENV` contract (spec §3) — one typed struct, rendered once.
//!
//! Canvas injects `window.ENV` in `layouts/_head.html.erb` from a hash that
//! grew to 124 files / 490 call sites. star-lms re-introduces it as a single
//! typed struct: every key is a named field, so adding a key is a compile
//! change that the governance test below forces to be deliberate (issue
//! #112).
//!
//! Phase-1 keys carry journey data; phase-2 keys are frozen in shape now and
//! serialize as constants/null until their subsystems exist. SENTRY/PENDO and
//! usage-metrics keys are permanently omitted (no telemetry in star-lms).

use serde::Serialize;
use serde_json::{json, Map, Value};

/// The checked-in key list. The governance test (issue #112) diffs the
/// serialized `JsEnv` key set against this list, so a new ENV key cannot land
/// accidentally: it must be added here (and to `js-env-coverage.md`) too.
pub const ENV_KEYS: &[&str] = &[
    // Phase 1 — required by the shell and the first journey (18 keys).
    "current_user_id",
    "current_user_roles",
    "current_user_is_student",
    "DOMAIN_ROOT_ACCOUNT_ID",
    "RAILS_ENVIRONMENT",
    "ASSET_HOST",
    "DEEP_LINKING_POST_MESSAGE_ORIGIN",
    "active_context_tab",
    "context_asset_string",
    "PREFERENCES",
    "SETTINGS",
    "use_high_contrast",
    "help_link_name",
    "k12",
    "files_domain",
    "notices",
    // Phase 2 — frozen shape, constant/null until the subsystems land.
    "current_user_global_id",
    "current_user_uuid",
    "current_user_usage_metrics_id",
    "current_user_types",
    "current_user_disabled_inbox",
    "current_user_visited_tabs",
    "discussions_reporting",
    "group_information",
    "DOMAIN_ROOT_ACCOUNT_UUID",
    "captcha_site_key",
    "confetti_branding_enabled",
    "active_brand_config",
    "active_brand_config_json_url",
    "url_to_what_gets_loaded_inside_the_tinymce_editor_css",
    "url_for_high_contrast_tinymce_editor_css",
    "LTI_LAUNCH_FRAME_ALLOWANCES",
    "comment_library_suggestions_enabled",
    "FEATURES",
];

/// User-level preference slice feeding `ENV.PREFERENCES` (phase 1).
///
/// Backed by the `user_preferences` JSONB document (migration 0123). Phase 1
/// serves the documented Canvas defaults when the keys are unset: the spec
/// (§7.1) requires no new migration for the shell.
#[derive(Debug, Clone, Serialize)]
pub struct EnvPreferences {
    /// `"cards"` or `"activity"`; the card view is the only phase-1 view.
    pub dashboard_view: String,
    /// course_id -> hex color, kept as a raw JSON object.
    pub custom_colors: Value,
    pub hide_dashcard_color_overlays: bool,
}

impl Default for EnvPreferences {
    fn default() -> Self {
        Self {
            dashboard_view: "cards".to_string(),
            custom_colors: Value::Object(Map::new()),
            hide_dashcard_color_overlays: false,
        }
    }
}

/// Account/user settings slice feeding `ENV.SETTINGS` (phase 1).
#[derive(Debug, Clone, Default, Serialize)]
pub struct EnvSettings {
    pub collapse_global_nav: bool,
}

/// The typed `window.ENV` payload. Field order matches the §3 tables; the
/// derive is `Serialize` only — nothing may read ENV back from a request.
#[derive(Debug, Clone, Serialize)]
pub struct JsEnv {
    // ---- Phase 1 -----------------------------------------------------------
    /// `users.id` of the logged-in user. `None` (key absent) when logged out
    /// — acceptance criterion 1 requires `current_user_id` to be missing on
    /// the login page, not merely zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_user_id: Option<i64>,
    /// Roles from `users.role` (single role today; array for Canvas parity).
    pub current_user_roles: Vec<String>,
    /// Derived: `role == "student"`.
    pub current_user_is_student: bool,
    /// Root account id (`accounts.id`, seeded as 1).
    #[serde(rename = "DOMAIN_ROOT_ACCOUNT_ID")]
    pub domain_root_account_id: i64,
    /// Deployment environment; `STAR_ENV` with a `"development"` default.
    #[serde(rename = "RAILS_ENVIRONMENT")]
    pub rails_environment: String,
    /// Static asset host; null = same-origin.
    #[serde(rename = "ASSET_HOST")]
    pub asset_host: Option<String>,
    /// Request origin, used for LTI postMessage validation.
    #[serde(rename = "DEEP_LINKING_POST_MESSAGE_ORIGIN")]
    pub deep_linking_post_message_origin: String,
    /// Active section tab id on course pages ("home" on course home).
    pub active_context_tab: Option<String>,
    /// `"{context}_{id}"`, e.g. `course_42`.
    pub context_asset_string: String,
    /// User preferences (dashboard view, custom colors, overlays).
    #[serde(rename = "PREFERENCES")]
    pub preferences: EnvPreferences,
    /// Account/user settings slice.
    #[serde(rename = "SETTINGS")]
    pub settings: EnvSettings,
    /// High-contrast UI preference.
    pub use_high_contrast: bool,
    /// Help link label (constant; the help subsystem is phase 2+).
    pub help_link_name: String,
    /// K-12 mode account setting (false in star-lms).
    pub k12: bool,
    /// Files domain (no separate files host in phase 1).
    pub files_domain: String,
    /// Flash messages surfaced on redirect.
    pub notices: Vec<String>,

    // ---- Phase 2 — frozen shape, constants/null ----------------------------
    #[serde(
        rename = "current_user_global_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub current_user_global_id: Option<i64>,
    #[serde(rename = "current_user_uuid")]
    pub current_user_uuid: Option<String>,
    #[serde(rename = "current_user_usage_metrics_id")]
    pub current_user_usage_metrics_id: Option<String>,
    #[serde(rename = "current_user_types")]
    pub current_user_types: Vec<String>,
    #[serde(rename = "current_user_disabled_inbox")]
    pub current_user_disabled_inbox: bool,
    #[serde(rename = "current_user_visited_tabs")]
    pub current_user_visited_tabs: Vec<String>,
    #[serde(rename = "discussions_reporting")]
    pub discussions_reporting: bool,
    #[serde(rename = "group_information")]
    pub group_information: Option<Value>,
    #[serde(rename = "DOMAIN_ROOT_ACCOUNT_UUID")]
    pub domain_root_account_uuid: Option<String>,
    #[serde(rename = "captcha_site_key")]
    pub captcha_site_key: Option<String>,
    #[serde(rename = "confetti_branding_enabled")]
    pub confetti_branding_enabled: bool,
    #[serde(rename = "active_brand_config")]
    pub active_brand_config: Option<Value>,
    #[serde(rename = "active_brand_config_json_url")]
    pub active_brand_config_json_url: Option<String>,
    #[serde(rename = "url_to_what_gets_loaded_inside_the_tinymce_editor_css")]
    pub url_to_what_gets_loaded_inside_the_tinymce_editor_css: Vec<String>,
    #[serde(rename = "url_for_high_contrast_tinymce_editor_css")]
    pub url_for_high_contrast_tinymce_editor_css: Vec<String>,
    #[serde(rename = "LTI_LAUNCH_FRAME_ALLOWANCES")]
    pub lti_launch_frame_allowances: Vec<String>,
    #[serde(rename = "comment_library_suggestions_enabled")]
    pub comment_library_suggestions_enabled: bool,
    #[serde(rename = "FEATURES")]
    pub features: Value,
}

impl JsEnv {
    /// Build the ENV payload for a logged-in shell page.
    #[allow(clippy::too_many_arguments)]
    pub fn for_user(
        user_id: i64,
        role: &str,
        preferences: EnvPreferences,
        use_high_contrast: bool,
        origin: &str,
        active_context_tab: Option<&str>,
        context_asset_string: &str,
        notices: Vec<String>,
    ) -> Self {
        Self {
            current_user_id: Some(user_id),
            current_user_roles: vec![role.to_string()],
            current_user_is_student: role == "student",
            domain_root_account_id: 1,
            rails_environment: std::env::var("STAR_ENV")
                .unwrap_or_else(|_| "development".to_string()),
            asset_host: None,
            deep_linking_post_message_origin: origin.to_string(),
            active_context_tab: active_context_tab.map(str::to_string),
            context_asset_string: context_asset_string.to_string(),
            preferences,
            settings: EnvSettings::default(),
            use_high_contrast,
            help_link_name: "Help".to_string(),
            k12: false,
            files_domain: String::new(),
            notices,
            current_user_global_id: Some(user_id),
            current_user_uuid: None,
            current_user_usage_metrics_id: None,
            current_user_types: Vec::new(),
            current_user_disabled_inbox: false,
            current_user_visited_tabs: Vec::new(),
            discussions_reporting: false,
            group_information: None,
            domain_root_account_uuid: None,
            captcha_site_key: None,
            confetti_branding_enabled: false,
            active_brand_config: None,
            active_brand_config_json_url: None,
            url_to_what_gets_loaded_inside_the_tinymce_editor_css: Vec::new(),
            url_for_high_contrast_tinymce_editor_css: Vec::new(),
            lti_launch_frame_allowances: Vec::new(),
            comment_library_suggestions_enabled: false,
            features: json!({}),
        }
    }

    /// The anonymous ENV (login page): no user keys leak, `current_user_id`
    /// is absent per acceptance criterion 1.
    pub fn anonymous(origin: &str, notices: Vec<String>) -> Self {
        Self {
            current_user_id: None,
            current_user_roles: Vec::new(),
            current_user_is_student: false,
            domain_root_account_id: 1,
            rails_environment: std::env::var("STAR_ENV")
                .unwrap_or_else(|_| "development".to_string()),
            asset_host: None,
            deep_linking_post_message_origin: origin.to_string(),
            active_context_tab: None,
            context_asset_string: String::new(),
            preferences: EnvPreferences::default(),
            settings: EnvSettings::default(),
            use_high_contrast: false,
            help_link_name: "Help".to_string(),
            k12: false,
            files_domain: String::new(),
            notices,
            current_user_global_id: None,
            current_user_uuid: None,
            current_user_usage_metrics_id: None,
            current_user_types: Vec::new(),
            current_user_disabled_inbox: false,
            current_user_visited_tabs: Vec::new(),
            discussions_reporting: false,
            group_information: None,
            domain_root_account_uuid: None,
            captcha_site_key: None,
            confetti_branding_enabled: false,
            active_brand_config: None,
            active_brand_config_json_url: None,
            url_to_what_gets_loaded_inside_the_tinymce_editor_css: Vec::new(),
            url_for_high_contrast_tinymce_editor_css: Vec::new(),
            lti_launch_frame_allowances: Vec::new(),
            comment_library_suggestions_enabled: false,
            features: json!({}),
        }
    }

    /// Serialized JSON for the `<script>window.ENV = ...</script>` block.
    ///
    /// `</` sequences are escaped to `<\/` — valid JSON (browsers and
    /// `JSON.parse` accept it) and it prevents a `</script>` string value
    /// from terminating the inline script element early.
    pub fn to_json_string(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| "{}".to_string())
            .replace("</", "<\\/")
    }
}

/// Extract the `window.ENV = {...};` object from a rendered shell page.
///
/// Test/verification helper (used by the integration suite): finds the ENV
/// statement in the head script block and parses it as JSON.
pub fn parse_env_from_html(html: &str) -> Option<Value> {
    let marker = "window.ENV = ";
    let start = html.find(marker)? + marker.len();
    let rest = &html[start..];
    let end = rest.find(';')?;
    serde_json::from_str(rest[..end].trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Governance test (spec §7.5, issue #112): the serialized ENV key set
    /// must exactly equal the checked-in [`ENV_KEYS`] list. Pure unit test —
    /// no DB, no HTTP — so it runs in every shard that compiles the crate.
    ///
    /// To add a key: add the field to `JsEnv`, the name to `ENV_KEYS`, and a
    /// row to `js-env-coverage.md` (key -> phase -> source). That is the
    /// whole procedure; this test is what makes it impossible to skip.
    #[test]
    fn env_keys_match_checked_in_contract() {
        let env = JsEnv::anonymous("http://localhost", Vec::new());
        let rendered = serde_json::to_value(&env).expect("serialize JsEnv");
        let object = rendered.as_object().expect("JsEnv serializes to an object");

        let mut rendered_keys: Vec<&str> = object.keys().map(String::as_str).collect();
        rendered_keys.sort_unstable();
        let mut expected: Vec<&str> = ENV_KEYS.to_vec();
        expected.sort_unstable();

        assert_eq!(
            rendered_keys, expected,
            "window.ENV key set drifted from the checked-in contract (spec §3, issue #112)"
        );
    }

    /// The phase-1 subset — the 18 keys the shell and first journey rely on,
    /// with their specified types.
    #[test]
    fn phase_one_keys_have_the_contract_types() {
        let env = JsEnv::for_user(
            42,
            "student",
            EnvPreferences::default(),
            false,
            "http://localhost",
            Some("home"),
            "course_7",
            vec!["hi".to_string()],
        );
        let v = serde_json::to_value(&env).expect("serialize");

        assert!(v["current_user_id"].is_i64());
        assert!(v["current_user_roles"].is_array());
        assert!(v["current_user_is_student"].is_boolean());
        assert!(v["DOMAIN_ROOT_ACCOUNT_ID"].is_i64());
        assert!(v["RAILS_ENVIRONMENT"].is_string());
        assert!(v["ASSET_HOST"].is_null());
        assert!(v["DEEP_LINKING_POST_MESSAGE_ORIGIN"].is_string());
        assert!(v["active_context_tab"].is_string());
        assert!(v["context_asset_string"].is_string());
        assert!(v["PREFERENCES"].is_object());
        assert!(v["PREFERENCES"]["dashboard_view"].is_string());
        assert!(v["PREFERENCES"]["custom_colors"].is_object());
        assert!(v["PREFERENCES"]["hide_dashcard_color_overlays"].is_boolean());
        assert!(v["SETTINGS"]["collapse_global_nav"].is_boolean());
        assert!(v["use_high_contrast"].is_boolean());
        assert!(v["help_link_name"].is_string());
        assert!(v["k12"].is_boolean());
        assert!(v["files_domain"].is_string());
        assert!(v["notices"].is_array());
    }

    #[test]
    fn phase_two_keys_are_frozen_constants() {
        let env = JsEnv::anonymous("http://localhost", Vec::new());
        let v = serde_json::to_value(&env).expect("serialize");

        assert_eq!(v["current_user_usage_metrics_id"], Value::Null);
        assert_eq!(v["captcha_site_key"], Value::Null);
        assert_eq!(v["active_brand_config"], Value::Null);
        assert_eq!(v["active_brand_config_json_url"], Value::Null);
        assert_eq!(v["group_information"], Value::Null);
        assert_eq!(v["discussions_reporting"], json!(false));
        assert_eq!(v["confetti_branding_enabled"], json!(false));
        assert_eq!(
            v["url_to_what_gets_loaded_inside_the_tinymce_editor_css"],
            json!([])
        );
        assert_eq!(v["LTI_LAUNCH_FRAME_ALLOWANCES"], json!([]));
        assert_eq!(v["comment_library_suggestions_enabled"], json!(false));
        assert_eq!(v["FEATURES"], json!({}));
        assert!(v["current_user_types"].is_array());
        assert!(v["current_user_visited_tabs"].is_array());
    }

    #[test]
    fn json_string_escapes_script_close() {
        let mut env = JsEnv::anonymous("http://localhost", Vec::new());
        env.notices = vec!["</script><script>alert(1)</script>".to_string()];
        let json = env.to_json_string();
        assert!(!json.contains("</script>"));
        // Still valid JSON after the escape.
        let parsed: Value = serde_json::from_str(&json).expect("escaped JSON parses");
        assert_eq!(
            parsed["notices"][0],
            json!("</script><script>alert(1)</script>")
        );
    }

    #[test]
    fn anonymous_env_has_no_user_keys() {
        let env = JsEnv::anonymous("http://localhost", Vec::new());
        let v = serde_json::to_value(&env).expect("serialize");
        assert!(v.get("current_user_id").is_none());
        assert!(v.get("current_user_global_id").is_none());
    }

    #[test]
    fn parse_env_from_html_round_trips() {
        let env = JsEnv::for_user(
            7,
            "teacher",
            EnvPreferences::default(),
            false,
            "https://lms.example",
            None,
            "",
            Vec::new(),
        );
        let html = format!(
            "<script>\nwindow.ENV = {};\n</script>",
            env.to_json_string()
        );
        let parsed = parse_env_from_html(&html).expect("parse");
        assert_eq!(parsed["current_user_id"], json!(7));
        assert_eq!(parsed["current_user_roles"], json!(["teacher"]));
    }

    #[test]
    fn dashboard_view_defaults_to_cards() {
        assert_eq!(EnvPreferences::default().dashboard_view, "cards");
        assert_eq!(
            EnvPreferences::default().custom_colors,
            serde_json::json!({})
        );
    }
}
