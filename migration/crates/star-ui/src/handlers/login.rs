//! Login / logout handlers (spec §5).
//!
//! `GET /login` renders the form and issues the CSRF cookie; `POST /login`
//! form-decodes, checks the double-submit CSRF pair, verifies the password
//! with star-auth's argon2 `verify_password`, mints the SAME JWT via
//! `star_auth::create_token` (HS256, 7-day expiry) and sets it as the
//! `star_session` cookie. `POST /logout` stamps `users.last_logged_out` —
//! which the session extractor (and the API's AuthUser) already honor — and
//! clears the cookie.

use askama::Template;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;

use star_auth::{create_token, verify_password};
use star_common::{ApiError, AppState};

use crate::env::JsEnv;
use crate::handlers::error::from_api_error;
use crate::session::{
    check_csrf, clear_session_cookie, ensure_csrf_cookie, sanitize_next, session_cookie,
};

/// The login page template context.
#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub csrf_token: String,
    pub next: String,
    pub email: String,
    pub error: String,
    /// Show the dismissible session-expiry notice (redirect carried
    /// `expired=1` because the presented token's `exp` was in the past).
    pub session_expired: bool,
}

impl IntoResponse for LoginPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("login template render failed: {err}")).into_response()
            }
        }
    }
}

#[derive(Deserialize)]
pub struct LoginQuery {
    pub next: Option<String>,
    pub login_hint: Option<String>,
    /// Present when the shell redirected here because the session token was
    /// EXPIRED (vs merely absent) — drives the dismissible expiry notice.
    pub expired: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
    pub _csrf: String,
    pub next: Option<String>,
}

/// GET /login — public; issues `star_csrf` when absent and renders the form.
///
/// `?expired=1` (set by the session extractor when a well-formed token's
/// `exp` is in the past) surfaces a dismissible "Your session has expired"
/// notice (issue #70). A fresh login never carries the flag, so the notice
/// disappears once the user re-authenticates.
pub async fn login_form(
    State(_app): State<AppState>,
    Query(query): Query<LoginQuery>,
    jar: CookieJar,
) -> Response {
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let origin = crate::request_origin();
    let page = LoginPage {
        title: "Log In".to_string(),
        body_class: "full-width ic-Login-Body no-headers".to_string(),
        css_url: crate::assets::app_css_url(),
        env_json: JsEnv::anonymous(&origin, Vec::new()).to_json_string(),
        notices: Vec::new(),
        csrf_token,
        next: sanitize_next(query.next.as_deref()),
        email: query.login_hint.unwrap_or_default(),
        error: String::new(),
        session_expired: query.expired.as_deref().is_some_and(|v| v == "1"),
    };
    (jar, page).into_response()
}

/// POST /login — form-decode, CSRF check, password verify, cookie set, 303.
pub async fn login_submit(
    State(app): State<AppState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Response {
    // Double-submit CSRF: 403 on mismatch or absence.
    if !check_csrf(&jar, &form._csrf) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
        )
            .into_response();
    }

    let row: Option<(i64, String, String)> =
        match sqlx::query_as("SELECT id, password_hash, role FROM users WHERE email = $1")
            .bind(form.email.trim().to_lowercase())
            .fetch_optional(&app.db)
            .await
        {
            Ok(row) => row,
            Err(e) => {
                return from_api_error(&ApiError::from(e), &crate::request_origin()).into_response()
            }
        };

    // Verify with the same argon2 path as the API. Unknown email and wrong
    // password produce the SAME generic re-render (no enumeration leak).
    let verified = row
        .as_ref()
        .map(|(_, hash, _)| verify_password(&form.password, hash))
        .unwrap_or(false);

    if !verified {
        let (jar, csrf_token) = ensure_csrf_cookie(jar);
        let page = LoginPage {
            title: "Log In".to_string(),
            body_class: "full-width ic-Login-Body no-headers".to_string(),
            css_url: crate::assets::app_css_url(),
            env_json: JsEnv::anonymous(&crate::request_origin(), Vec::new()).to_json_string(),
            notices: Vec::new(),
            csrf_token,
            next: sanitize_next(form.next.as_deref()),
            email: form.email.clone(),
            error: "Invalid email or password.".to_string(),
            session_expired: false,
        };
        return (StatusCode::UNAUTHORIZED, jar, page).into_response();
    }

    let (user_id, _, role) = row.expect("verified implies the row exists");

    // Mint the session JWT — same claims, secret and 7-day lifetime the API
    // login mints; only the transport differs (cookie vs Bearer).
    let token = create_token(user_id, &role, &app.jwt_secret).unwrap_or_default();
    if token.is_empty() {
        return from_api_error(
            &ApiError::Db("token mint failed".to_string()),
            &crate::request_origin(),
        )
        .into_response();
    }

    let next = sanitize_next(form.next.as_deref());
    (
        StatusCode::SEE_OTHER,
        jar.add(session_cookie(&token)),
        [("location", next)],
    )
        .into_response()
}

/// POST /logout — CSRF check, stamp `last_logged_out`, clear cookie, redirect.
///
/// The UPDATE reuses the token-invalidation mechanism the API's terminate
/// sessions route uses (star-users): every token minted before now is dead
/// for both the shell (SessionUser) and the API (AuthUser).
pub async fn logout(
    State(app): State<AppState>,
    jar: CookieJar,
    user: crate::session::SessionUser,
    Form(form): Form<LogoutForm>,
) -> Response {
    if !check_csrf(&jar, &form._csrf) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
        )
            .into_response();
    }

    let update =
        sqlx::query("UPDATE users SET last_logged_out = now(), updated_at = now() WHERE id = $1")
            .bind(user.id)
            .execute(&app.db)
            .await;

    if let Err(e) = update {
        return from_api_error(&ApiError::from(e), &crate::request_origin()).into_response();
    }

    (
        StatusCode::SEE_OTHER,
        jar.remove(clear_session_cookie()),
        [("location", "/login")],
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct LogoutForm {
    #[allow(dead_code)]
    pub _csrf: String,
}

/// GET / — Canvas `root => users#user_dashboard`: redirect to /dashboard.
pub async fn root() -> Redirect {
    Redirect::to("/dashboard")
}
