//! Conversations (inbox) handlers — Canvas `conversations#index` two-pane
//! server-rendered port (issue #72).
//!
//! Routes (registered in `lib.rs`):
//! - GET  /conversations            — inbox; `?filter=unread|starred` scope
//! - GET  /conversations/compose    — compose form (recipients select)
//! - POST /conversations            — create conversation, PRG to the thread
//! - GET  /conversations/{id}       — thread show, marks read on open
//! - POST /conversations/{id}/add_message — reply, PRG back to the thread
//! - POST /conversations/mark_all_read   — mark-all-read, PRG to the inbox
//!
//! All data access goes through the `star_conversations::ui` pub loaders,
//! which apply the same active-participant gates as the REST API. The
//! no-JS port follows canvas-reference.md §5: left pane = conversation list
//! with scope filter links, right pane = the selected thread with a reply
//! form; compose carries a recipients multi-select fed by the same recipient
//! index the API serves.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;

use star_common::{ApiError, AppState};
use star_conversations::ui::{self as conversations_ui, InboxFilter, InboxRow};

use crate::handlers::error::from_api_error;
use crate::session::{check_csrf, ensure_csrf_cookie, SessionUser};
use crate::{assets, env::JsEnv, queries};

/// CSRF rejection page shared by every POST below.
fn csrf_rejected() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
    )
        .into_response()
}

/// An inbox conversation row in the view model.
pub struct ConversationRow {
    pub id: i64,
    pub subject: String,
    pub participant_names: String,
    pub last_message_preview: String,
    pub last_message_at: Option<String>,
    pub unread: bool,
    pub starred: bool,
}

/// The inbox template context.
#[derive(Template)]
#[template(path = "conversations_index.html")]
pub struct ConversationsIndexPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub unread_badge: i64,
    pub filter: String,
    pub conversations: Vec<ConversationRow>,
}

impl IntoResponse for ConversationsIndexPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("conversations index template render failed: {err}"))
                .into_response(),
        }
    }
}

/// A message in the thread view model.
pub struct ThreadMessageView {
    pub id: i64,
    pub author_name: String,
    pub body: String,
    pub created_at: String,
    /// Whether the message author is the viewer.
    pub mine: bool,
}

/// The thread show template context.
#[derive(Template)]
#[template(path = "conversation_show.html")]
pub struct ConversationShowPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub unread_badge: i64,
    pub conversation_id: i64,
    pub subject: String,
    pub participant_names: String,
    pub starred: bool,
    pub messages: Vec<ThreadMessageView>,
}

impl IntoResponse for ConversationShowPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("conversation show template render failed: {err}"))
                .into_response(),
        }
    }
}

/// A recipient option in the compose view model.
pub struct RecipientView {
    pub id: i64,
    pub label: String,
}

/// The compose template context.
#[derive(Template)]
#[template(path = "conversation_compose.html")]
pub struct ConversationComposePage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub unread_badge: i64,
    pub recipients: Vec<RecipientView>,
    pub error: String,
}

impl IntoResponse for ConversationComposePage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!(
                "conversation compose template render failed: {err}"
            ))
            .into_response(),
        }
    }
}

/// Shared shell context for the non-course inbox pages.
struct InboxContext {
    preferences: crate::env::EnvPreferences,
    use_high_contrast: bool,
    user_name: String,
    unread_badge: i64,
}

async fn load_inbox_context(
    app: &AppState,
    user: &SessionUser,
    origin: &str,
) -> Result<InboxContext, Box<Response>> {
    let (preferences, use_high_contrast) = queries::load_preferences(&app.db, user.id)
        .await
        .map_err(|e| Box::new(from_api_error(&e, origin).into_response()))?;
    let user_name = queries::load_user_name(&app.db, user.id)
        .await
        .map_err(|e| Box::new(from_api_error(&e, origin).into_response()))?
        .unwrap_or_default();
    let unread_badge = conversations_ui::load_unread_count(&app.db, user.id)
        .await
        .map_err(|e| Box::new(from_api_error(&e, origin).into_response()))?;
    Ok(InboxContext {
        preferences,
        use_high_contrast,
        user_name,
        unread_badge,
    })
}

/// `?filter=` query for the inbox.
#[derive(Deserialize)]
pub struct InboxQuery {
    pub filter: Option<String>,
}

/// GET /conversations — the two-pane inbox. The scope filters are plain GET
/// links (`?filter=unread`, `?filter=starred`), Canvas' scope variants
/// (`/conversations/unread` etc.) folded into one query param.
pub async fn conversations_index(
    State(app): State<AppState>,
    Query(query): Query<InboxQuery>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match load_inbox_context(&app, &user, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let filter = InboxFilter::from_str_opt(query.filter.as_deref());
    let rows: Vec<InboxRow> = match conversations_ui::load_inbox(&app.db, user.id, filter).await {
        Ok(rows) => rows,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let conversations: Vec<ConversationRow> = rows
        .into_iter()
        .map(|row| ConversationRow {
            id: row.id,
            subject: row.subject,
            participant_names: if row.participant_names.is_empty() {
                "You".to_string()
            } else {
                row.participant_names
            },
            last_message_preview: truncate(row.last_message_preview.as_deref(), 90),
            last_message_at: row
                .last_message_at
                .map(|ts| ts.format("%b %-d, %Y").to_string()),
            unread: row.read_at.is_none(),
            starred: row.starred,
        })
        .collect();

    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        None,
        "",
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let filter_name = match filter {
        InboxFilter::All => "",
        InboxFilter::Unread => "unread",
        InboxFilter::Starred => "starred",
    }
    .to_string();

    let page = ConversationsIndexPage {
        title: "Conversations".to_string(),
        body_class: "primary-nav-expanded full-width conversations".to_string(),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "conversations",
        breadcrumb: vec![("Conversations".to_string(), "/conversations".to_string())],
        csrf_token,
        unread_badge: ctx.unread_badge,
        filter: filter_name,
        conversations,
    };
    (jar, page).into_response()
}

/// GET /conversations/compose — the compose form. The recipients multi-select
/// is fed by the same user index the API's find_recipients serves (capped,
/// viewer excluded).
pub async fn conversation_compose(
    State(app): State<AppState>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match load_inbox_context(&app, &user, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let recipients: Vec<RecipientView> =
        match conversations_ui::load_recipient_options(&app.db, user.id).await {
            Ok(rows) => rows
                .into_iter()
                .map(|row| RecipientView {
                    id: row.id,
                    label: format!("{} <{}>", row.name, row.email),
                })
                .collect(),
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        None,
        "",
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = ConversationComposePage {
        title: "New Conversation".to_string(),
        body_class: "primary-nav-expanded full-width conversations".to_string(),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "conversations",
        breadcrumb: vec![
            ("Conversations".to_string(), "/conversations".to_string()),
            ("New".to_string(), String::new()),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        recipients,
        error: String::new(),
    };
    (jar, page).into_response()
}

/// The compose form payload. HTML multi-selects post `recipients` once per
/// selected option; axum's `Vec<String>` form decode collects them all.
#[derive(Deserialize)]
pub struct ComposeForm {
    pub _csrf: String,
    pub recipients: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// POST /conversations — create a conversation, then redirect to the new
/// thread (PRG).
pub async fn conversation_create(
    State(app): State<AppState>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<ComposeForm>,
) -> Response {
    let origin = crate::request_origin();

    if !check_csrf(&jar, &form._csrf) {
        return csrf_rejected();
    }

    let recipients: Vec<i64> = form
        .recipients
        .iter()
        .filter_map(|raw| raw.parse::<i64>().ok())
        .collect();

    match conversations_ui::create_conversation_for_viewer(
        &app.db,
        &(star_auth::AuthUser {
            id: user.id,
            role: user.role.clone(),
        }),
        &recipients,
        &form.subject,
        &form.body,
    )
    .await
    {
        Ok(conversation_id) => {
            Redirect::to(&format!("/conversations/{conversation_id}")).into_response()
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// GET /conversations/{id} — the thread. Marks the conversation read on open
/// (the participant row's `read_at`), rendering the pre-open unread state.
pub async fn conversation_show(
    State(app): State<AppState>,
    Path(conversation_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match load_inbox_context(&app, &user, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let thread =
        match conversations_ui::load_conversation_thread(&app.db, conversation_id, user.id).await {
            Ok(thread) => thread,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    // Mark read on open; a failure is non-fatal (the badge lags one view).
    let _ = conversations_ui::mark_conversation_read_for_viewer(
        &app.db,
        conversation_id,
        user.id,
        true,
    )
    .await;

    let participant_names = thread
        .participants
        .iter()
        .map(|p| p.name.clone())
        .collect::<Vec<_>>()
        .join(", ");

    let messages: Vec<ThreadMessageView> = thread
        .messages
        .iter()
        .map(|m| ThreadMessageView {
            id: m.id,
            author_name: m.author_name.clone(),
            body: m.body.clone(),
            created_at: m.created_at.format("%b %-d, %Y %H:%M").to_string(),
            mine: m.author_id == user.id,
        })
        .collect();

    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        None,
        "",
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = ConversationShowPage {
        title: format!("{} - Conversations", thread.subject),
        body_class: "primary-nav-expanded full-width conversations".to_string(),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "conversations",
        breadcrumb: vec![
            ("Conversations".to_string(), "/conversations".to_string()),
            (thread.subject.clone(), String::new()),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        conversation_id,
        subject: thread.subject,
        participant_names,
        starred: thread.starred,
        messages,
    };
    (jar, page).into_response()
}

/// The reply form payload.
#[derive(Deserialize)]
pub struct ReplyForm {
    pub _csrf: String,
    pub body: String,
}

/// POST /conversations/{id}/add_message — append a message, redirect back to
/// the thread (PRG).
pub async fn conversation_reply(
    State(app): State<AppState>,
    Path(conversation_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<ReplyForm>,
) -> Response {
    let origin = crate::request_origin();

    if !check_csrf(&jar, &form._csrf) {
        return csrf_rejected();
    }

    let auth = star_auth::AuthUser {
        id: user.id,
        role: user.role.clone(),
    };

    match conversations_ui::add_message_for_viewer(&app.db, conversation_id, &auth, &form.body)
        .await
    {
        Ok(_) => Redirect::to(&format!("/conversations/{conversation_id}")).into_response(),
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// The mark-all-read form payload.
#[derive(Deserialize)]
pub struct MarkAllForm {
    pub _csrf: String,
}

/// POST /conversations/mark_all_read — mark every active conversation read,
/// redirect back to the inbox (PRG).
pub async fn conversations_mark_all_read(
    State(app): State<AppState>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<MarkAllForm>,
) -> Response {
    if !check_csrf(&jar, &form._csrf) {
        return csrf_rejected();
    }

    match conversations_ui::mark_all_conversations_read(&app.db, user.id).await {
        Ok(()) => Redirect::to("/conversations").into_response(),
        Err(e) => from_api_error(&e, &crate::request_origin()).into_response(),
    }
}

/// Truncate a preview string on a char boundary (no panic on multibyte).
fn truncate(text: Option<&str>, max: usize) -> String {
    let text = text.unwrap_or("").trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max).collect();
    format!("{truncated}…")
}
