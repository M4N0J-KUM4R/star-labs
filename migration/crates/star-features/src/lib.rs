//! star-features — Canvas feature flags (`feature_flags_controller`).
//!
//! Canvas routes (config/routes.rb, `scope(controller: :feature_flags)`, one
//! block per context — course/account/user):
//! - GET    /{context}s/{id}/features                 — list features (index)
//! - GET    /{context}s/{id}/features/enabled         — enabled feature names
//! - GET    /{context}s/{id}/features/flags/{feature} — show resolved flag
//! - PUT    /{context}s/{id}/features/flags/{feature} — set flag (update)
//! - DELETE /{context}s/{id}/features/flags/{feature} — remove direct flag
//! plus:
//! - POST /accounts/{accountId}/features/early_access_program — accept the
//!   Early Access Program terms (root accounts only)
//! - GET /features/environment — the `ENV.FEATURES` hash
//!
//! Flag states are Canvas's (`FeatureFlag`): "off" and "on" (any context) and
//! "allowed"/"allowed_on" (accounts only). Lookups resolve inherited flags by
//! walking the account chain from the root account down to the context
//! (Canvas `FeatureFlags#lookup_feature_flag`): the most specific flag that
//! cannot be overridden ("on"/"off") wins; inheritable "allowed"/"allowed_on"
//! flags let more specific contexts override them. `root_opt_in` features
//! stay hidden below the root account until the root account opts in, and
//! read as a virtual "off" flag on the root account itself.
//!
//! Known simplifications of this single-tenant port:
//! - No Site Admin account: site-admin-gated branches (hidden features, the
//!   `hidden` JSON key) are omitted, and `GET /features/environment`
//!   evaluates both the site-admin and root-account feature lists on the
//!   root account (account 1).
//! - No pagination: the catalog is far smaller than Canvas's API page size.
//! - `ApiError` renders plain text, so error bodies carry the Canvas status
//!   codes but not its JSON message envelopes.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Feature catalog (Canvas `config/feature_flags/*.yml` via lib/feature.rb)
// ---------------------------------------------------------------------------

/// A feature definition. `None` attributes serialize as JSON null, matching
/// Canvas features whose yml entry leaves the attribute unset.
struct FeatureDef {
    feature: &'static str,
    display_name: &'static str,
    description: &'static str,
    /// "Course" | "Account" | "RootAccount" | "User" ("SiteAdmin" never
    /// applies — this port has no Site Admin account).
    applies_to: &'static str,
    /// Default (global) state of the feature.
    state: &'static str,
    root_opt_in: Option<bool>,
    beta: Option<bool>,
    early_access_program: Option<bool>,
    release_notes_url: Option<&'static str>,
    autoexpand: Option<bool>,
    /// Canvas `type` attribute: "setting" | "feature_option".
    feature_type: &'static str,
}

/// Feature catalog. `fancy_wickets`, `automatic_essay_grading` and
/// `differentiation_tags` are the synthetic examples pinned by earlier
/// revisions of this crate (the first two are the Canvas API documentation
/// examples); the rest are real Canvas `config/feature_flags/00_standard.yml`
/// entries chosen to exercise every `applies_to` and the `root_opt_in`
/// inheritance rules.
const FEATURES: &[FeatureDef] = &[
    FeatureDef {
        feature: "fancy_wickets",
        display_name: "Fancy Wickets",
        description: "An example feature from the Canvas API documentation.",
        applies_to: "Account",
        state: "allowed",
        root_opt_in: None,
        beta: None,
        early_access_program: None,
        release_notes_url: None,
        autoexpand: None,
        feature_type: "setting",
    },
    FeatureDef {
        feature: "automatic_essay_grading",
        display_name: "Automatic Essay Grading",
        description: "Popup text describing the feature goes here",
        applies_to: "Course",
        state: "allowed",
        root_opt_in: None,
        beta: None,
        early_access_program: None,
        release_notes_url: None,
        autoexpand: None,
        feature_type: "setting",
    },
    FeatureDef {
        feature: "differentiation_tags",
        display_name: "Differentiation Tags",
        description: "Assign tags to students for differentiated instruction.",
        applies_to: "Course",
        state: "allowed",
        root_opt_in: None,
        beta: None,
        early_access_program: None,
        release_notes_url: None,
        autoexpand: None,
        feature_type: "setting",
    },
    FeatureDef {
        feature: "epub_export",
        display_name: "ePub Exporting",
        description: "This enables users to generate and download course ePub.",
        applies_to: "Course",
        state: "allowed",
        root_opt_in: Some(true),
        beta: None,
        early_access_program: None,
        release_notes_url: None,
        autoexpand: None,
        feature_type: "setting",
    },
    FeatureDef {
        feature: "anonymous_marking",
        display_name: "Anonymous Grading",
        description: "Enable anonymous grading of assignments.",
        applies_to: "Course",
        state: "allowed",
        root_opt_in: Some(true),
        beta: None,
        early_access_program: None,
        release_notes_url: None,
        autoexpand: None,
        feature_type: "setting",
    },
    FeatureDef {
        feature: "new_user_tutorial",
        display_name: "New User Tutorial",
        description: "Provide tutorial information for instructors new to Canvas.",
        applies_to: "RootAccount",
        state: "allowed",
        root_opt_in: Some(true),
        beta: None,
        early_access_program: None,
        release_notes_url: None,
        autoexpand: None,
        feature_type: "setting",
    },
    FeatureDef {
        feature: "high_contrast",
        display_name: "High Contrast UI",
        description: "High Contrast enhances the color contrast of the UI (text, buttons, etc.), making those items more distinct and easier to identify.",
        applies_to: "User",
        state: "allowed",
        root_opt_in: None,
        beta: None,
        early_access_program: None,
        release_notes_url: None,
        autoexpand: None,
        feature_type: "setting",
    },
    FeatureDef {
        feature: "underline_all_links",
        display_name: "Underline Links",
        description: "Underline Links displays hyperlinks in navigation menus, the Dashboard, and page sidebars as underlined text. This feature does not apply to user-generated content links in the Rich Content Editor, which always underlines links for all users.",
        applies_to: "User",
        state: "allowed",
        root_opt_in: None,
        beta: None,
        early_access_program: None,
        release_notes_url: None,
        autoexpand: None,
        feature_type: "setting",
    },
];

fn definition(feature: &str) -> Option<&'static FeatureDef> {
    FEATURES.iter().find(|def| def.feature == feature)
}

impl FeatureDef {
    /// Canvas forces `root_opt_in` on for RootAccount and Early Access
    /// features (lib/feature.rb `Feature#initialize`).
    fn effective_root_opt_in(&self) -> bool {
        self.root_opt_in == Some(true)
            || self.applies_to == "RootAccount"
            || self.early_access_program == Some(true)
    }
    /// Definition default states that let lower contexts override
    /// ("allowed"/"allowed_on"; Canvas STATE_DEFAULT_OFF / STATE_DEFAULT_ON).
    fn can_override(&self) -> bool {
        self.state == "allowed" || self.state == "allowed_on"
    }
    fn hidden(&self) -> bool {
        self.state == "hidden"
    }
}

// ---------------------------------------------------------------------------
// Flag contexts
// ---------------------------------------------------------------------------

/// One node of a flag-lookup chain.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextNode {
    context_type: &'static str, // "Account" | "Course" | "User"
    context_id: i64,
}

/// A loaded flag context: the requested account/course/user plus the account
/// chain its flags resolve against (`chain` excludes the context itself).
#[derive(Debug, Clone)]
enum FlagContext {
    Account {
        id: i64,
        /// Ancestor accounts, root account first, immediate parent last.
        chain: Vec<ContextNode>,
        /// True when this account has no parent (Canvas `root_account?`).
        root: bool,
    },
    Course {
        id: i64,
        /// The course's account chain, root account first, the course's own
        /// account last.
        chain: Vec<ContextNode>,
    },
    User {
        id: i64,
    },
}

impl FlagContext {
    fn id(&self) -> i64 {
        match self {
            FlagContext::Account { id, .. }
            | FlagContext::Course { id, .. }
            | FlagContext::User { id } => *id,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            FlagContext::Account { .. } => "Account",
            FlagContext::Course { .. } => "Course",
            FlagContext::User { .. } => "User",
        }
    }

    fn self_node(&self) -> ContextNode {
        ContextNode {
            context_type: self.type_name(),
            context_id: self.id(),
        }
    }

    fn chain(&self) -> &[ContextNode] {
        match self {
            FlagContext::Account { chain, .. } | FlagContext::Course { chain, .. } => chain,
            FlagContext::User { .. } => &[],
        }
    }

    /// Contexts to search for a flag, most general first, this context last
    /// (Canvas `FeatureFlags#feature_flag_account_ids` + self).
    fn walk_contexts(&self, inherited_only: bool) -> Vec<ContextNode> {
        let mut nodes: Vec<ContextNode> = self.chain().to_vec();
        if !inherited_only {
            nodes.push(self.self_node());
        }
        nodes
    }

    fn is_root_account(&self) -> bool {
        matches!(self, FlagContext::Account { root: true, .. })
    }

    /// The root account of this context, if any (Canvas `#root_account`;
    /// users have none, and a root account is its own root).
    fn root_account_id(&self) -> Option<i64> {
        match self {
            FlagContext::Account { .. } => Some(
                self.chain()
                    .first()
                    .map(|node| node.context_id)
                    .unwrap_or_else(|| self.id()),
            ),
            FlagContext::Course { .. } => self.chain().first().map(|node| node.context_id),
            FlagContext::User { .. } => None,
        }
    }

    /// Account ids whose `account_users` grants apply to this context: its
    /// own account plus every ancestor.
    fn account_ids(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .chain()
            .iter()
            .filter(|node| node.context_type == "Account")
            .map(|node| node.context_id)
            .collect();
        if let FlagContext::Account { id, .. } = self {
            ids.push(*id);
        }
        ids
    }
}

/// Account chain for `account_id` (root account first, `account_id` last).
/// Cycle-safe via UNION de-duplication with a depth guard.
async fn account_chain(pool: &PgPool, account_id: i64) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE chain AS (
            SELECT id, parent_account_id, 0 AS depth
            FROM accounts
            WHERE id = $1
          UNION
            SELECT a.id, a.parent_account_id, chain.depth + 1
            FROM accounts a
            JOIN chain ON a.id = chain.parent_account_id
            WHERE chain.depth < 64
        )
        SELECT id FROM chain ORDER BY depth DESC
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Load an account context; missing or deleted accounts are 404 (Canvas
/// `get_context` uses `Account.active`).
async fn load_account_context(pool: &PgPool, account_id: i64) -> Result<FlagContext, ApiError> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT parent_account_id FROM accounts WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    let Some((parent_id,)) = row else {
        return Err(ApiError::NotFound);
    };
    let chain: Vec<ContextNode> = account_chain(pool, account_id)
        .await?
        .into_iter()
        .filter(|id| *id != account_id)
        .map(|id| ContextNode {
            context_type: "Account",
            context_id: id,
        })
        .collect();
    Ok(FlagContext::Account {
        id: account_id,
        chain,
        root: parent_id.is_none(),
    })
}

/// Load a course context; missing or deleted courses are 404.
async fn load_course_context(pool: &PgPool, course_id: i64) -> Result<FlagContext, ApiError> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT account_id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    let Some((account_id,)) = row else {
        return Err(ApiError::NotFound);
    };
    let chain: Vec<ContextNode> = match account_id {
        Some(account_id) => account_chain(pool, account_id)
            .await?
            .into_iter()
            .map(|id| ContextNode {
                context_type: "Account",
                context_id: id,
            })
            .collect(),
        None => Vec::new(),
    };
    Ok(FlagContext::Course {
        id: course_id,
        chain,
    })
}

/// Load a user context; missing users are 404 (this schema has no user
/// soft-delete).
async fn load_user_context(pool: &PgPool, user_id: i64) -> Result<FlagContext, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    if row.is_none() {
        return Err(ApiError::NotFound);
    }
    Ok(FlagContext::User { id: user_id })
}

// ---------------------------------------------------------------------------
// Flag lookup (Canvas lib/feature_flags.rb)
// ---------------------------------------------------------------------------

/// A resolved flag: a stored flag row (context set) or the feature
/// definition itself as the global default (context unset).
#[derive(Debug, Clone)]
struct LookupFlag {
    feature: String,
    state: String,
    context_type: Option<&'static str>,
    context_id: Option<i64>,
}

impl LookupFlag {
    fn default_flag(def: &FeatureDef) -> Self {
        LookupFlag {
            feature: def.feature.to_string(),
            state: def.state.to_string(),
            context_type: None,
            context_id: None,
        }
    }

    /// Canvas `Feature#default?` / `FeatureFlag#default?`.
    fn is_default(&self) -> bool {
        self.context_type.is_none()
    }

    /// Canvas `#can_override?`: inheritable states that more specific
    /// contexts may override.
    fn can_override(&self) -> bool {
        self.state == "allowed" || self.state == "allowed_on"
    }

    /// Canvas `#enabled?`.
    fn enabled(&self) -> bool {
        self.state == "on" || self.state == "allowed_on"
    }

    /// Canvas `FeatureFlag#locked?` for stored flags and `Feature#locked?`
    /// for definition defaults.
    fn locked(&self, ctx: &FlagContext, def: &FeatureDef) -> bool {
        if self.is_default() {
            !self.can_override() && !def.hidden()
        } else {
            !self.can_override()
                && !(self.context_type == Some(ctx.type_name())
                    && self.context_id == Some(ctx.id()))
        }
    }
}

/// Canvas `FeatureFlags#feature_flag`: the flag row defined directly on a
/// context, if any.
async fn direct_flag(
    pool: &PgPool,
    node: &ContextNode,
    feature: &str,
) -> Result<Option<LookupFlag>, ApiError> {
    let state: Option<String> = sqlx::query_scalar(
        "SELECT state FROM feature_flags \
         WHERE context_type = $1 AND context_id = $2 AND feature = $3 LIMIT 1",
    )
    .bind(node.context_type)
    .bind(node.context_id)
    .bind(feature)
    .fetch_optional(pool)
    .await?;
    Ok(state.map(|state| LookupFlag {
        feature: feature.to_string(),
        state,
        context_type: Some(node.context_type),
        context_id: Some(node.context_id),
    }))
}

/// Canvas `Feature#applies_to_object` (lib/feature.rb).
fn applies_to_object(def: &FeatureDef, ctx: &FlagContext) -> bool {
    match def.applies_to {
        "SiteAdmin" => false,
        "RootAccount" => ctx.is_root_account(),
        "Account" => matches!(ctx, FlagContext::Account { .. }),
        "Course" => matches!(
            ctx,
            FlagContext::Course { .. } | FlagContext::Account { .. }
        ),
        "User" => matches!(ctx, FlagContext::User { .. }),
        _ => false,
    }
}

/// Canvas `Feature.applicable_features`.
fn applicable_features(ctx: &FlagContext) -> Vec<&'static FeatureDef> {
    let allowed_types: &[&str] = match ctx {
        FlagContext::Account { root, .. } => {
            if *root {
                &["Account", "Course", "RootAccount"]
            } else {
                &["Account", "Course"]
            }
        }
        FlagContext::Course { .. } => &["Course"],
        FlagContext::User { .. } => &["User"],
    };
    FEATURES
        .iter()
        .filter(|def| allowed_types.contains(&def.applies_to))
        .collect()
}

/// Canvas `FeatureFlags#lookup_feature_flag`: resolve the flag applying to
/// the context, possibly inherited from the account chain. Returns `None`
/// when the feature does not apply to the context, or (for `root_opt_in`
/// features) when the root account has not opted in yet.
async fn lookup_feature_flag(
    pool: &PgPool,
    ctx: &FlagContext,
    def: &FeatureDef,
    inherited_only: bool,
) -> Result<Option<LookupFlag>, ApiError> {
    if !applies_to_object(def, ctx) {
        return Ok(None);
    }
    // Hard-coded definitions ("on"/"off") cannot be overridden: Canvas
    // returns the definition without walking the chain. (Every catalog
    // feature defaults to "allowed", but the branch is kept for parity.)
    if !def.can_override() && !def.hidden() {
        return Ok(Some(LookupFlag::default_flag(def)));
    }
    let mut retval = LookupFlag::default_flag(def);
    for node in ctx.walk_contexts(inherited_only) {
        if let Some(flag) = direct_flag(pool, &node, def.feature).await? {
            let overridable = flag.can_override();
            retval = flag;
            if !overridable {
                break;
            }
        }
    }
    // root_opt_in tail (Canvas lib/feature_flags.rb): until the root account
    // opts in, the feature is invisible below the root account and reads as
    // a virtual "off" flag on the root account itself. (Hidden-definition
    // handling has no catalog entries and needs the Site Admin account this
    // port does not have.)
    if def.effective_root_opt_in() && retval.is_default() && retval.state == "allowed" {
        if ctx.is_root_account() {
            retval = LookupFlag {
                feature: def.feature.to_string(),
                state: "off".to_string(),
                context_type: Some("Account"),
                context_id: Some(ctx.id()),
            };
        } else {
            return Ok(None);
        }
    }
    Ok(Some(retval))
}

// ---------------------------------------------------------------------------
// JSON builders (Canvas lib/api/v1/feature_flag.rb)
// ---------------------------------------------------------------------------

/// Canvas `Feature#default_transitions` (no custom_transition_proc hooks in
/// this port). `orig_state` None means "no current flag".
fn transitions_for(
    def: &FeatureDef,
    ctx: &FlagContext,
    orig_state: Option<&str>,
) -> Map<String, Value> {
    let account = matches!(ctx, FlagContext::Account { .. });
    let states: &[&str] = if account {
        &["off", "on", "allowed", "allowed_on"]
    } else {
        &["off", "on"]
    };
    let mut map = Map::new();
    for state in states {
        if orig_state == Some(*state) {
            continue;
        }
        let locked = (*state == "allowed" || *state == "allowed_on")
            && ((def.applies_to == "RootAccount" && ctx.is_root_account())
                || def.applies_to == "SiteAdmin");
        map.insert((*state).to_string(), json!({ "locked": locked }));
    }
    map
}

/// Canvas `Api::V1::FeatureFlag#feature_json` (catalog attributes; `shadow`
/// is only present for shadow features, which this catalog has none of).
fn feature_json(def: &FeatureDef) -> Value {
    json!({
        "feature": def.feature,
        "applies_to": def.applies_to,
        "root_opt_in": def.root_opt_in,
        "early_access_program": def.early_access_program,
        "beta": def.beta,
        "release_notes_url": def.release_notes_url,
        "autoexpand": def.autoexpand,
        "type": def.feature_type,
        "display_name": def.display_name,
        "description": def.description,
    })
}

/// Canvas `Api::V1::FeatureFlag#feature_flag_json`. Default (global) flags
/// omit context_id/context_type/locking_account_id; every flag carries
/// `transitions`, `locked`, and the `parent_state` resolved from the
/// context's ancestors.
async fn feature_flag_json(
    pool: &PgPool,
    ctx: &FlagContext,
    flag: &LookupFlag,
) -> Result<Value, ApiError> {
    let Some(def) = definition(&flag.feature) else {
        return Err(ApiError::Db(format!(
            "feature flag {} has no catalog definition",
            flag.feature
        )));
    };
    let mut obj = Map::new();
    obj.insert("feature".into(), json!(flag.feature));
    if !flag.is_default() {
        obj.insert("context_id".into(), json!(flag.context_id));
        obj.insert("context_type".into(), json!(flag.context_type));
        // Deprecated Canvas field, kept (always null) for compatibility.
        obj.insert("locking_account_id".into(), Value::Null);
    }
    obj.insert("state".into(), json!(flag.state));
    obj.insert(
        "transitions".into(),
        Value::Object(transitions_for(def, ctx, Some(&flag.state))),
    );
    obj.insert("locked".into(), json!(flag.locked(ctx, def)));
    let parent = lookup_feature_flag(pool, ctx, def, true).await?;
    obj.insert("parent_state".into(), json!(parent.map(|flag| flag.state)));
    Ok(Value::Object(obj))
}

// ---------------------------------------------------------------------------
// Permissions (Canvas authorized_action :read / :manage_feature_flags)
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas account rights flow through `account_users` grants on the account
/// or any of its ancestors (an admin of a parent account administers the
/// sub-accounts below it).
async fn account_member(
    pool: &PgPool,
    user_id: i64,
    account_ids: &[i64],
) -> Result<bool, ApiError> {
    if account_ids.is_empty() {
        return Ok(false);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE user_id = $1 AND account_id = ANY($2) LIMIT 1",
    )
    .bind(user_id)
    .bind(account_ids)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

async fn enrollment(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    role: Option<&str>,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM enrollments
        WHERE user_id = $1 AND course_id = $2 AND ($3::text IS NULL OR role = $3)
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `authorized_action(@context, @current_user, :read)`.
async fn can_read_flags(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: &FlagContext,
) -> Result<bool, ApiError> {
    match ctx {
        FlagContext::Account { .. } => {
            Ok(is_admin(auth) || account_member(pool, auth.id, &ctx.account_ids()).await?)
        }
        FlagContext::Course { .. } => Ok(is_admin(auth)
            || account_member(pool, auth.id, &ctx.account_ids()).await?
            || enrollment(pool, auth.id, ctx.id(), None).await?),
        FlagContext::User { .. } => Ok(auth.id == ctx.id() || is_admin(auth)),
    }
}

/// Canvas `authorized_action(@context, @current_user, :manage_feature_flags)`:
/// account admins, teachers (via Course :manage), or the user themself.
async fn can_manage_flags(
    pool: &PgPool,
    auth: &AuthUser,
    ctx: &FlagContext,
) -> Result<bool, ApiError> {
    match ctx {
        FlagContext::Account { .. } => {
            Ok(is_admin(auth) || account_member(pool, auth.id, &ctx.account_ids()).await?)
        }
        FlagContext::Course { .. } => Ok(is_admin(auth)
            || account_member(pool, auth.id, &ctx.account_ids()).await?
            || enrollment(pool, auth.id, ctx.id(), Some("teacher")).await?),
        FlagContext::User { .. } => Ok(auth.id == ctx.id() || is_admin(auth)),
    }
}

// ---------------------------------------------------------------------------
// Shared action bodies
// ---------------------------------------------------------------------------

/// Request body for setting a flag (`state` mirrors the Canvas param; an
/// absent/empty state fails Canvas's save-time validation).
#[derive(Debug, Deserialize)]
struct UpdateFeature {
    state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct IndexParams {
    hide_inherited_enabled: Option<String>,
}

/// Canvas `Canvas::Plugin.value_to_boolean`.
fn value_to_boolean(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    match value.to_ascii_lowercase().as_str() {
        "yes" | "y" | "true" | "t" | "on" | "1" => true,
        "no" | "n" | "false" | "f" | "off" | "0" => false,
        // Canvas falls back to `value.to_i != 0` (leading numeric prefix).
        other => other
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '+')
            .collect::<String>()
            .parse::<i64>()
            .is_ok_and(|n| n != 0),
    }
}

/// GET .../features — Canvas `FeatureFlagsController#index`.
async fn index_action(
    state: &AppState,
    auth: &AuthUser,
    ctx: &FlagContext,
    hide_inherited_enabled: bool,
) -> Result<Json<Value>, ApiError> {
    if !can_read_flags(&state.db, auth, ctx).await? {
        return Err(ApiError::Forbidden);
    }
    let mut out = Vec::new();
    for def in applicable_features(ctx) {
        let Some(flag) = lookup_feature_flag(&state.db, ctx, def, false).await? else {
            continue;
        };
        // hide_inherited_enabled: omit flags forced on by a higher context.
        if hide_inherited_enabled && flag.enabled() && flag.locked(ctx, def) {
            continue;
        }
        let mut feature = feature_json(def);
        let object = feature
            .as_object_mut()
            .expect("feature_json builds an object");
        object.insert(
            "feature_flag".to_string(),
            feature_flag_json(&state.db, ctx, &flag).await?,
        );
        out.push(feature);
    }
    Ok(Json(Value::Array(out)))
}

/// GET .../features/enabled — Canvas `#enabled_features`: the names of the
/// features whose resolved flag is enabled.
async fn enabled_action(
    state: &AppState,
    auth: &AuthUser,
    ctx: &FlagContext,
) -> Result<Json<Value>, ApiError> {
    if !can_read_flags(&state.db, auth, ctx).await? {
        return Err(ApiError::Forbidden);
    }
    let mut names = Vec::new();
    for def in applicable_features(ctx) {
        if let Some(flag) = lookup_feature_flag(&state.db, ctx, def, false).await? {
            if flag.enabled() {
                names.push(json!(def.feature));
            }
        }
    }
    Ok(Json(Value::Array(names)))
}

/// GET .../features/flags/{feature} — Canvas `#show`.
async fn show_action(
    state: &AppState,
    auth: &AuthUser,
    ctx: &FlagContext,
    feature: &str,
) -> Result<Json<Value>, ApiError> {
    if !can_read_flags(&state.db, auth, ctx).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas raises RecordNotFound for unknown features.
    let Some(def) = definition(feature) else {
        return Err(ApiError::NotFound);
    };
    let Some(flag) = lookup_feature_flag(&state.db, ctx, def, false).await? else {
        return Err(ApiError::NotFound);
    };
    Ok(Json(feature_flag_json(&state.db, ctx, &flag).await?))
}

/// PUT .../features/flags/{feature} — Canvas `#update`.
async fn update_action(
    state: &AppState,
    auth: &AuthUser,
    ctx: &FlagContext,
    feature: &str,
    body: UpdateFeature,
) -> Result<Json<Value>, ApiError> {
    if !can_manage_flags(&state.db, auth, ctx).await? {
        return Err(ApiError::Forbidden);
    }
    let Some(def) = definition(feature) else {
        return Err(ApiError::BadRequest("invalid feature".into()));
    };
    if !applies_to_object(def, ctx) {
        return Err(ApiError::BadRequest("invalid feature".into()));
    }
    let current = lookup_feature_flag(&state.db, ctx, def, false).await?;
    if let Some(flag) = &current {
        if flag.locked(ctx, def) {
            // A parent context forced the feature on/off.
            return Err(ApiError::Forbidden);
        }
    }
    let prior_state = current.as_ref().map(|flag| flag.state.clone());
    let requested = body.state.as_deref().filter(|s| !s.is_empty());

    // Early Access Program gate. No catalog feature opts into the program
    // (matching Canvas master), but the gate is kept for parity.
    if def.early_access_program == Some(true) && requested != Some("off") {
        if let Some(root_id) = ctx.root_account_id() {
            let accepted: (bool,) = sqlx::query_as(
                r#"SELECT COALESCE((settings->'early_access_program'->>'value') = 'true', false)
                   FROM accounts WHERE id = $1"#,
            )
            .bind(root_id)
            .fetch_one(&state.db)
            .await?;
            if !accepted.0 {
                return Err(ApiError::Forbidden);
            }
        }
    }

    // Hidden features can only be unhidden by a site admin (none here).
    if current.is_none() && def.hidden() {
        return Err(ApiError::BadRequest("invalid feature".into()));
    }

    // Canvas checks the transition table before the save-time validation.
    if let Some(requested) = requested {
        let transitions = transitions_for(def, ctx, prior_state.as_deref());
        if let Some(Value::Object(target)) = transitions.get(requested) {
            if target.get("locked") == Some(&Value::Bool(true)) {
                return Err(ApiError::Forbidden);
            }
        }
    }

    // Canvas FeatureFlag#valid_state: off/on anywhere; allowed/allowed_on
    // only on accounts. A missing state fails validation at save time.
    let Some(requested) = requested else {
        return Err(ApiError::BadRequest("state is not valid in context".into()));
    };
    let valid = matches!(requested, "off" | "on")
        || (matches!(requested, "allowed" | "allowed_on")
            && matches!(ctx, FlagContext::Account { .. }));
    if !valid {
        return Err(ApiError::BadRequest("state is not valid in context".into()));
    }

    let saved = upsert_flag(&state.db, ctx, def, requested).await?;
    Ok(Json(feature_flag_json(&state.db, ctx, &saved).await?))
}

/// Insert or update the flag row on the context itself (Canvas
/// `#create_or_update_feature_flag`).
async fn upsert_flag(
    pool: &PgPool,
    ctx: &FlagContext,
    def: &FeatureDef,
    state: &str,
) -> Result<LookupFlag, ApiError> {
    let row: (String,) = sqlx::query_as(
        r#"
        INSERT INTO feature_flags (context_type, context_id, feature, display_name, state)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (context_type, context_id, feature)
        DO UPDATE SET state = EXCLUDED.state, display_name = EXCLUDED.display_name,
                      updated_at = now()
        RETURNING state
        "#,
    )
    .bind(ctx.type_name())
    .bind(ctx.id())
    .bind(def.feature)
    .bind(def.display_name)
    .bind(state)
    .fetch_one(pool)
    .await?;
    Ok(LookupFlag {
        feature: def.feature.to_string(),
        state: row.0,
        context_type: Some(ctx.type_name()),
        context_id: Some(ctx.id()),
    })
}

/// DELETE .../features/flags/{feature} — Canvas `#delete`.
async fn delete_action(
    state: &AppState,
    auth: &AuthUser,
    ctx: &FlagContext,
    feature: &str,
) -> Result<Json<Value>, ApiError> {
    if !can_manage_flags(&state.db, auth, ctx).await? {
        return Err(ApiError::Forbidden);
    }
    let Some(def) = definition(feature) else {
        return Err(ApiError::BadRequest("invalid feature".into()));
    };
    if !applies_to_object(def, ctx) {
        return Err(ApiError::BadRequest("invalid feature".into()));
    }
    // Root accounts may not drop root_opt_in flags once opted in: the flag
    // row is what marks the opt-in for the inheritance lookup.
    if def.effective_root_opt_in() && ctx.is_root_account() {
        return Err(ApiError::Forbidden);
    }
    // Canvas `find_by!`: only a flag defined on this context can be deleted.
    let Some(flag) = direct_flag(&state.db, &ctx.self_node(), def.feature).await? else {
        return Err(ApiError::NotFound);
    };
    // Canvas checks flag.locked? here; a flag defined on the queried context
    // itself is never locked, so this cannot fire.
    if flag.locked(ctx, def) {
        return Err(ApiError::Forbidden);
    }
    sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = $1 AND context_id = $2 AND feature = $3",
    )
    .bind(ctx.type_name())
    .bind(ctx.id())
    .bind(def.feature)
    .execute(&state.db)
    .await?;
    // parent_state is resolved after the delete (Canvas renders the flag
    // after destroying it).
    Ok(Json(feature_flag_json(&state.db, ctx, &flag).await?))
}

// ---------------------------------------------------------------------------
// Environment (Canvas ApplicationController JS_ENV lists)
// ---------------------------------------------------------------------------

/// Canvas `ApplicationController::JS_ENV_SITE_ADMIN_FEATURES` (evaluated on
/// the Site Admin account; this port evaluates them on the root account).
const JS_ENV_SITE_ADMIN_FEATURES: &[&str] = &[
    "account_level_blackout_dates",
    "assignment_edit_placement_not_on_announcements",
    "a11y_checker_ga2_features",
    "block_content_editor_toolbar_reorder",
    "commons_new_quizzes",
    "courses_popout_sisid",
    "create_external_apps_side_tray_overrides",
    "dashboard_graphql_integration",
    "developer_key_user_agent_alert",
    "disallow_threaded_replies_fix_alert",
    "disallow_threaded_replies_manage",
    "discussion_ai_survey_link",
    "discussion_checkpoints",
    "discussion_permalink",
    "enhanced_course_creation_account_fetching",
    "explicit_latex_typesetting",
    "feature_flag_ui_sorting",
    "files_a11y_rewrite",
    "files_a11y_rewrite_toggle",
    "files_a11y_folder_duplicates",
    "horizon_course_setting",
    "instui_for_import_page",
    "instui_header",
    "media_links_use_attachment_id",
    "multiselect_gradebook_filters",
    "new_quizzes_media_type",
    "new_quizzes_navigation_updates",
    "permanent_page_links",
    "rce_a11y_resize",
    "rce_find_replace",
    "render_both_to_do_lists",
    "scheduled_feedback_releases",
    "speedgrader_studio_media_capture",
    "validate_call_to_action",
    "block_content_editor_ai_alt_text",
    "ux_list_concluded_courses_in_bp",
];

/// Canvas `ApplicationController::JS_ENV_ROOT_ACCOUNT_FEATURES` (evaluated on
/// the domain root account).
const JS_ENV_ROOT_ACCOUNT_FEATURES: &[&str] = &[
    "accessibility_automatic_scanning",
    "account_level_mastery_scales",
    "ams_root_account_integration",
    "ams_advanced_content_organization",
    "buttons_and_icons_root_account",
    "canvas_apps_sub_account_access",
    "cookie_consent_necessary",
    "course_pace_allow_bulk_pace_assign",
    "course_pace_download_document",
    "course_pace_draft_state",
    "course_pace_pacing_status_labels",
    "course_pace_pacing_with_mastery_paths",
    "course_pace_time_selection",
    "course_pace_weighted_assignments",
    "course_paces_skip_selected_days",
    "create_course_subaccount_picker",
    "disable_iframe_sandbox_file_show",
    "extended_submission_state",
    "file_verifiers_for_quiz_links",
    "grading_rubrics_pagination",
    "institutional_tags",
    "instui_nav",
    "login_registration_ui_identity",
    "lock_lti_registrations",
    "lti_apps_page_instructors",
    "lti_asset_processor",
    "lti_asset_processor_discussions",
    "lti_deactivate_registrations",
    "lti_link_to_apps_from_developer_keys",
    "lti_registrations_next",
    "lti_registrations_templates",
    "lti_registrations_usage_data",
    "lti_registrations_usage_data_dev",
    "lti_registrations_usage_data_low_usage",
    "lti_registrations_usage_tab",
    "lti_toggle_placements",
    "substitution_variable_display",
    "mobile_offline_mode",
    "modules_requirements_allow_percentage",
    "nav_menu_links",
    "non_scoring_rubrics",
    "pendo_extended",
    "product_tours",
    "rce_asr_captioning_improvements",
    "rce_lite_enabled_speedgrader_comments",
    "rce_transform_loaded_content",
    "react_discussions_post",
    "restrict_student_access",
    "rubric_criterion_range",
    "scheduled_page_publication",
    "send_usage_metrics",
    "send_usage_metrics_after_consent",
    "top_navigation_placement",
    "youtube_migration",
    "educator_dashboard",
    "widget_dashboard",
    "default_discussion_options",
    "course_navigation_and_feature_options_permissions",
];

/// Canvas `JS_ENV_ROOT_ACCOUNT_SERVICES` — this port has no services
/// subsystem, so they always evaluate false.
const JS_ENV_ROOT_ACCOUNT_SERVICES: &[&str] = &["account_survey_notifications"];

/// Canvas `JS_ENV_BRAND_ACCOUNT_FEATURES` — evaluated on the brand config
/// account, which this port does not have.
const JS_ENV_BRAND_ACCOUNT_FEATURES: &[&str] =
    &["discussion_checkpoints", "embedded_release_notes"];

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

// ---- account scope ----

async fn index_account(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    Query(params): Query<IndexParams>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_account_context(&state.db, account_id).await?;
    index_action(
        &state,
        &auth,
        &ctx,
        value_to_boolean(params.hide_inherited_enabled.as_deref()),
    )
    .await
}

async fn enabled_account(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_account_context(&state.db, account_id).await?;
    enabled_action(&state, &auth, &ctx).await
}

async fn show_account(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((account_id, feature)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_account_context(&state.db, account_id).await?;
    show_action(&state, &auth, &ctx, &feature).await
}

async fn update_account(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((account_id, feature)): Path<(i64, String)>,
    Json(body): Json<UpdateFeature>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_account_context(&state.db, account_id).await?;
    update_action(&state, &auth, &ctx, &feature, body).await
}

async fn delete_account(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((account_id, feature)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_account_context(&state.db, account_id).await?;
    delete_action(&state, &auth, &ctx, &feature).await
}

/// POST /accounts/{accountId}/features/early_access_program — Canvas
/// `#accept_early_access_terms`. Only root accounts have the endpoint;
/// Canvas raises RecordNotFound for anything else BEFORE the permission
/// check.
async fn accept_early_access_terms(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_account_context(&state.db, account_id).await?;
    if !ctx.is_root_account() {
        return Err(ApiError::NotFound);
    }
    if !can_manage_flags(&state.db, &auth, &ctx).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas stores settings[:early_access_program] = { value: true }.
    sqlx::query(
        "UPDATE accounts \
         SET settings = jsonb_set(settings, '{early_access_program,value}', 'true'::jsonb), \
             updated_at = now() \
         WHERE id = $1",
    )
    .bind(account_id)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "early_access_program": true })))
}

// ---- course scope ----

async fn index_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
    Query(params): Query<IndexParams>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_course_context(&state.db, course_id).await?;
    index_action(
        &state,
        &auth,
        &ctx,
        value_to_boolean(params.hide_inherited_enabled.as_deref()),
    )
    .await
}

async fn enabled_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(course_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_course_context(&state.db, course_id).await?;
    enabled_action(&state, &auth, &ctx).await
}

async fn show_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, feature)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_course_context(&state.db, course_id).await?;
    show_action(&state, &auth, &ctx, &feature).await
}

async fn update_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, feature)): Path<(i64, String)>,
    Json(body): Json<UpdateFeature>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_course_context(&state.db, course_id).await?;
    update_action(&state, &auth, &ctx, &feature, body).await
}

async fn delete_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, feature)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_course_context(&state.db, course_id).await?;
    delete_action(&state, &auth, &ctx, &feature).await
}

// ---- user scope ----

async fn index_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
    Query(params): Query<IndexParams>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_user_context(&state.db, user_id).await?;
    index_action(
        &state,
        &auth,
        &ctx,
        value_to_boolean(params.hide_inherited_enabled.as_deref()),
    )
    .await
}

async fn enabled_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_user_context(&state.db, user_id).await?;
    enabled_action(&state, &auth, &ctx).await
}

async fn show_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((user_id, feature)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_user_context(&state.db, user_id).await?;
    show_action(&state, &auth, &ctx, &feature).await
}

async fn update_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((user_id, feature)): Path<(i64, String)>,
    Json(body): Json<UpdateFeature>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_user_context(&state.db, user_id).await?;
    update_action(&state, &auth, &ctx, &feature, body).await
}

async fn delete_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((user_id, feature)): Path<(i64, String)>,
) -> Result<Json<Value>, ApiError> {
    let ctx = load_user_context(&state.db, user_id).await?;
    delete_action(&state, &auth, &ctx, &feature).await
}

// ---- global ----

/// GET /features/environment — Canvas `#environment`: the fixed JS_ENV
/// feature lists supplied to the web UI as ENV.FEATURES. Requires a session
/// (the auth extractor rejects anonymous calls); mobile clients are not
/// distinguished in this port.
async fn environment(
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    // Single-tenant port: both the site-admin and root-account lists evaluate
    // against the root account (account 1).
    let root = load_account_context(&state.db, 1).await.ok();
    let mut env = Map::new();
    for name in JS_ENV_SITE_ADMIN_FEATURES
        .iter()
        .chain(JS_ENV_ROOT_ACCOUNT_FEATURES.iter())
    {
        let enabled = match (&root, definition(name)) {
            (Some(ctx), Some(def)) => lookup_feature_flag(&state.db, ctx, def, false)
                .await?
                .is_some_and(|flag| flag.enabled()),
            _ => false,
        };
        env.insert((*name).to_string(), json!(enabled));
    }
    for name in JS_ENV_ROOT_ACCOUNT_SERVICES {
        env.insert((*name).to_string(), json!(false));
    }
    for name in JS_ENV_BRAND_ACCOUNT_FEATURES {
        env.insert((*name).to_string(), json!(false));
    }
    Ok(Json(Value::Object(env)))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounts/{accountId}/features", get(index_account))
        .route(
            "/accounts/{accountId}/features/enabled",
            get(enabled_account),
        )
        .route(
            "/accounts/{accountId}/features/flags/{feature}",
            get(show_account).put(update_account).delete(delete_account),
        )
        .route(
            "/accounts/{accountId}/features/early_access_program",
            post(accept_early_access_terms),
        )
        .route("/courses/{courseId}/features", get(index_course))
        .route("/courses/{courseId}/features/enabled", get(enabled_course))
        .route(
            "/courses/{courseId}/features/flags/{feature}",
            get(show_course).put(update_course).delete(delete_course),
        )
        .route("/users/{userId}/features", get(index_user))
        .route("/users/{userId}/features/enabled", get(enabled_user))
        .route(
            "/users/{userId}/features/flags/{feature}",
            get(show_user).put(update_user).delete(delete_user),
        )
        .route("/features/environment", get(environment))
}
