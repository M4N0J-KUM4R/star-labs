//! Registration + password-reset handlers (issue #70, canvas-reference §2).
//!
//! * `GET /register` (+ its alias `GET /register_from_website`) renders the
//!   self-registration form and `POST /users` submits it — Canvas' split,
//!   `get "register" => "users#new"` (config/routes.rb:1009-1010) with
//!   `resources :users` (:1016) providing the create verb. Reuses
//!   star-auth's public-register semantics verbatim: role forced to
//!   `student`, email must contain `@`, password >= 8 chars, argon2 hashing
//!   via `star_auth::hash_password`, and the same `INSERT ... sortable_name`
//!   SQL (unique-violation 23505 = "email already in use"). Success issues a
//!   session (auto-login) and 303s to /dashboard, exactly like POST /login.
//! * `GET/POST /forgot_password` — always-200 generic confirmation; a token
//!   row is created only when the email exists, but the response is
//!   byte-identical either way (no user enumeration). No mailer exists in
//!   star-lms, so the link is written to the server log — documented in the
//!   PR as the first password-reset path in the repo.
//! * `GET/POST /reset_password/:token` — single-use, 2h-expiry token from
//!   `password_reset_tokens` (migration 0222); sets the new argon2 hash,
//!   stamps `used_at`, and logs the user straight in (Canvas parity:
//!   change_password "logs user in").

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;

use star_auth::{create_token, hash_password};
use star_common::{ApiError, AppState};

use crate::env::JsEnv;
use crate::handlers::error::from_api_error;
use crate::session::{
    check_csrf, ensure_csrf_cookie, generate_password_reset_token, session_cookie,
};

/// How long a reset token stays valid (2 hours — Canvas' confirmation-code
/// window; far tighter than the 7-day session JWT on purpose).
const RESET_TOKEN_TTL_SECS: i64 = 2 * 60 * 60;

/// Shared "bare layout" scaffold for the auth pages: (body_class, css_url,
/// env_json) — same values the login page uses (Canvas "bare" layout).
fn auth_page_shell() -> (String, String, String) {
    (
        "full-width ic-Login-Body no-headers".to_string(),
        crate::assets::app_css_url(),
        JsEnv::anonymous(&crate::request_origin(), Vec::new()).to_json_string(),
    )
}

fn html(status: StatusCode, body: String) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

fn csrf_403() -> Response {
    html(
        StatusCode::FORBIDDEN,
        "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
    )
}

// ---------------------------------------------------------------------------
// GET /register — the form; POST /users — the submit
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "register.html")]
pub struct RegisterPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub csrf_token: String,
    pub name: String,
    pub email: String,
    pub error: String,
    pub name_error: String,
    pub email_error: String,
    pub password_error: String,
}

impl IntoResponse for RegisterPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(body) => html(StatusCode::OK, body),
            Err(err) => {
                ApiError::Db(format!("register template render failed: {err}")).into_response()
            }
        }
    }
}

#[derive(Deserialize, Default)]
pub struct RegisterQuery {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Deserialize)]
pub struct RegisterForm {
    pub name: String,
    pub email: String,
    pub password: String,
    pub password_confirmation: String,
    pub _csrf: String,
}

/// GET /register — the self-registration form (Canvas users#new, no-JS port).
pub async fn register_form(Query(query): Query<RegisterQuery>, jar: CookieJar) -> Response {
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let (body_class, css_url, env_json) = auth_page_shell();
    let page = RegisterPage {
        title: "Register".to_string(),
        body_class,
        css_url,
        env_json,
        notices: Vec::new(),
        csrf_token,
        name: query.name.unwrap_or_default(),
        email: query.email.unwrap_or_default(),
        error: String::new(),
        name_error: String::new(),
        email_error: String::new(),
        password_error: String::new(),
    };
    (jar, page).into_response()
}

/// POST /users — validate, hash with star-auth's argon2, insert (role
/// forced to student), auto-login, 303 /dashboard.
///
/// Response-shape deviation, unchanged by the route move: Canvas' `create_user`
/// answers every success with `render json: data` (users_controller.rb:3478,
/// where the React signup page reads `data["destination"]`) and every failure
/// with `render json: errors, status: :bad_request` (:3489). This page is the
/// no-JS port, so the server completes the login and redirects instead of
/// handing a destination to a client.
pub async fn register_submit(
    State(app): State<AppState>,
    jar: CookieJar,
    Form(form): Form<RegisterForm>,
) -> Response {
    if !check_csrf(&jar, &form._csrf) {
        return csrf_403();
    }

    let name = form.name.trim().to_string();
    let email = form.email.trim().to_lowercase();
    let mut name_error = String::new();
    let mut email_error = String::new();
    let mut password_error = String::new();

    // Field validation mirrors star-auth's register handler.
    if name.is_empty() {
        name_error = "Name is required.".to_string();
    }
    if !email.contains('@') {
        email_error = "Email must contain '@'.".to_string();
    }
    if form.password.len() < 8 {
        password_error = "Password must be at least 8 characters.".to_string();
    } else if form.password != form.password_confirmation {
        password_error = "Password confirmation does not match.".to_string();
    }

    if !name_error.is_empty() || !email_error.is_empty() || !password_error.is_empty() {
        let (jar, csrf_token) = ensure_csrf_cookie(jar);
        let (body_class, css_url, env_json) = auth_page_shell();
        let page = RegisterPage {
            title: "Register".to_string(),
            body_class,
            css_url,
            env_json,
            notices: Vec::new(),
            csrf_token,
            name,
            email,
            error: String::new(),
            name_error,
            email_error,
            password_error,
        };
        return (StatusCode::UNPROCESSABLE_ENTITY, jar, page).into_response();
    }

    // Hash through the SAME star-auth primitive — no reimplementation.
    let password_hash = match hash_password(&form.password) {
        Ok(h) => h,
        Err(e) => {
            return from_api_error(&ApiError::BadRequest(e), &crate::request_origin())
                .into_response()
        }
    };

    // Role is FORCED to student: star-auth's public registration policy
    // (resolve_public_registration_role) only ever grants 'student'.
    let insert: Result<i64, sqlx::Error> = sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES (
            $1,
            TRIM(SPLIT_PART($1, ' ', -1) || ', ' || LEFT($1, LENGTH($1) - LENGTH(SPLIT_PART($1, ' ', -1)) - 1)),
            $2, $3, 'student'
        )
        RETURNING id
        "#,
    )
    .bind(&name)
    .bind(&email)
    .bind(&password_hash)
    .fetch_one(&app.db)
    .await;

    let user_id = match insert {
        Ok(id) => id,
        Err(e) => {
            // Unique violation 23505 = duplicate email (star-auth maps this to
            // Conflict "email already in use"; the form re-renders with the
            // field error instead).
            if let sqlx::Error::Database(ref db) = e {
                if db.code().map(|c| c == "23505").unwrap_or(false) {
                    let (jar, csrf_token) = ensure_csrf_cookie(jar);
                    let (body_class, css_url, env_json) = auth_page_shell();
                    let page = RegisterPage {
                        title: "Register".to_string(),
                        body_class,
                        css_url,
                        env_json,
                        notices: Vec::new(),
                        csrf_token,
                        name,
                        email,
                        error: String::new(),
                        name_error: String::new(),
                        email_error: "Email already in use.".to_string(),
                        password_error: String::new(),
                    };
                    return (StatusCode::CONFLICT, jar, page).into_response();
                }
            }
            return from_api_error(&ApiError::from(e), &crate::request_origin()).into_response();
        }
    };

    // Auto-login: mint the session exactly like POST /login does.
    let token = create_token(user_id, "student", &app.jwt_secret).unwrap_or_default();
    if token.is_empty() {
        return from_api_error(
            &ApiError::Db("token mint failed".to_string()),
            &crate::request_origin(),
        )
        .into_response();
    }
    (
        StatusCode::SEE_OTHER,
        jar.add(session_cookie(&token)),
        [("location", "/dashboard")],
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// GET/POST /forgot_password
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "forgot_password.html")]
pub struct ForgotPasswordPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub csrf_token: String,
    pub email: String,
    pub error: String,
    pub confirmation: String,
}

impl IntoResponse for ForgotPasswordPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(body) => html(StatusCode::OK, body),
            Err(err) => ApiError::Db(format!("forgot_password template render failed: {err}"))
                .into_response(),
        }
    }
}

#[derive(Deserialize)]
pub struct ForgotPasswordForm {
    pub email: String,
    pub _csrf: String,
}

/// The generic confirmation — IDENTICAL for known and unknown emails so the
/// response cannot be used to enumerate accounts (Canvas parity: the same
/// flash is shown whether or not the channel was found).
const FORGOT_CONFIRMATION: &str =
    "If that account exists, a password reset link was sent. Check your spam box.";

fn forgot_page(jar: CookieJar, email: String, confirmation: String, error: String) -> Response {
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let (body_class, css_url, env_json) = auth_page_shell();
    let page = ForgotPasswordPage {
        title: "Forgot Password".to_string(),
        body_class,
        css_url,
        env_json,
        notices: Vec::new(),
        csrf_token,
        email,
        error,
        confirmation,
    };
    (jar, page).into_response()
}

/// GET /forgot_password — the email form.
pub async fn forgot_password_form(jar: CookieJar) -> Response {
    forgot_page(jar, String::new(), String::new(), String::new())
}

/// POST /forgot_password — always 200 with the generic confirmation. When the
/// email exists a single-use, 2h reset token row is created; the reset link
/// is logged server-side (star-lms has no mailer yet — documented deviation).
pub async fn forgot_password_submit(
    State(app): State<AppState>,
    jar: CookieJar,
    Form(form): Form<ForgotPasswordForm>,
) -> Response {
    if !check_csrf(&jar, &form._csrf) {
        return csrf_403();
    }
    let email = form.email.trim().to_lowercase();

    if let Ok(Some(user_id)) = sqlx::query_scalar::<_, i64>("SELECT id FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(&app.db)
        .await
    {
        let token = generate_password_reset_token();
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(RESET_TOKEN_TTL_SECS);
        let inserted = sqlx::query(
            "INSERT INTO password_reset_tokens (user_id, token, expires_at) \
             VALUES ($1, $2, $3)",
        )
        .bind(user_id)
        .bind(&token)
        .bind(expires_at)
        .execute(&app.db)
        .await;
        match inserted {
            Ok(_) => {
                // No mailer in star-lms: the link goes to the server log.
                // Reset links are single-use and expire in 2 hours.
                println!("password reset link for {email}: /reset_password/{token}");
            }
            Err(e) => {
                return from_api_error(&ApiError::from(e), &crate::request_origin()).into_response()
            }
        }
    }
    // Known AND unknown email take the SAME branch shape and body.
    forgot_page(jar, email, FORGOT_CONFIRMATION.to_string(), String::new())
}

// ---------------------------------------------------------------------------
// GET/POST /reset_password/:token
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "reset_password.html")]
pub struct ResetPasswordPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub csrf_token: String,
    pub token: String,
    pub valid: bool,
    pub error: String,
    pub password_error: String,
}

impl IntoResponse for ResetPasswordPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(body) => html(StatusCode::OK, body),
            Err(err) => ApiError::Db(format!("reset_password template render failed: {err}"))
                .into_response(),
        }
    }
}

#[derive(Deserialize)]
pub struct ResetPasswordForm {
    pub password: String,
    pub password_confirmation: String,
    pub _csrf: String,
}

/// The row shape the token lookup returns.
type ResetRow = (
    i64,
    chrono::DateTime<chrono::Utc>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// Load a token row that is unused and unexpired.
async fn load_valid_token(app: &AppState, token: &str) -> Result<Option<ResetRow>, ApiError> {
    sqlx::query_as::<_, ResetRow>(
        "SELECT user_id, expires_at, used_at FROM password_reset_tokens \
         WHERE token = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(token)
    .fetch_optional(&app.db)
    .await
    .map_err(ApiError::from)
}

/// GET /reset_password/:token — render the new-password form when the token
/// is valid, or the generic expired/invalid message otherwise (never say
/// which).
pub async fn reset_password_form(
    State(app): State<AppState>,
    Path(token): Path<String>,
    jar: CookieJar,
) -> Response {
    let row = match load_valid_token(&app, &token).await {
        Ok(row) => row,
        Err(e) => return from_api_error(&e, &crate::request_origin()).into_response(),
    };
    let (jar, csrf_token) = ensure_csrf_cookie(jar);
    let (body_class, css_url, env_json) = auth_page_shell();
    let page = ResetPasswordPage {
        title: "Change Password".to_string(),
        body_class,
        css_url,
        env_json,
        notices: Vec::new(),
        csrf_token,
        valid: row.is_some(),
        error: match row {
            Some(_) => String::new(),
            None => "This password reset link is invalid or has expired.".to_string(),
        },
        password_error: String::new(),
        token,
    };
    (jar, page).into_response()
}

/// POST /reset_password/:token — set the new password (argon2 via star-auth),
/// mark the token used (single use), log the user in.
pub async fn reset_password_submit(
    State(app): State<AppState>,
    Path(token): Path<String>,
    jar: CookieJar,
    Form(form): Form<ResetPasswordForm>,
) -> Response {
    if !check_csrf(&jar, &form._csrf) {
        return csrf_403();
    }

    // Validation errors re-render the form (token stays valid).
    let mut password_error = String::new();
    if form.password.len() < 8 {
        password_error = "Password must be at least 8 characters.".to_string();
    } else if form.password != form.password_confirmation {
        password_error = "Password confirmation does not match.".to_string();
    }
    if !password_error.is_empty() {
        let (jar, csrf_token) = ensure_csrf_cookie(jar);
        let (body_class, css_url, env_json) = auth_page_shell();
        let page = ResetPasswordPage {
            title: "Change Password".to_string(),
            body_class,
            css_url,
            env_json,
            notices: Vec::new(),
            csrf_token,
            valid: true,
            error: String::new(),
            password_error,
            token,
        };
        return (StatusCode::UNPROCESSABLE_ENTITY, jar, page).into_response();
    }

    // Atomically consume the token: the WHERE clause re-checks
    // unused+unexpired, so a replayed or racing second POST updates zero
    // rows and is rejected. The password hash is set in the same statement
    // so a failure cannot consume the token without changing the password.
    let password_hash = match hash_password(&form.password) {
        Ok(h) => h,
        Err(e) => {
            return from_api_error(&ApiError::BadRequest(e), &crate::request_origin())
                .into_response()
        }
    };
    let consumed: Result<(i64,), sqlx::Error> = sqlx::query_as(
        "UPDATE password_reset_tokens SET used_at = now() \
         WHERE token = $1 AND used_at IS NULL AND expires_at > now() \
         RETURNING user_id",
    )
    .bind(&token)
    .fetch_one(&app.db)
    .await;

    let Some(user_id) = consumed.ok().map(|(id,)| id) else {
        let (jar, csrf_token) = ensure_csrf_cookie(jar);
        let (body_class, css_url, env_json) = auth_page_shell();
        let page = ResetPasswordPage {
            title: "Change Password".to_string(),
            body_class,
            css_url,
            env_json,
            notices: Vec::new(),
            csrf_token,
            valid: false,
            error: "This password reset link is invalid or has expired.".to_string(),
            password_error: String::new(),
            token,
        };
        return (StatusCode::OK, jar, page).into_response();
    };

    let updated =
        sqlx::query("UPDATE users SET password_hash = $1, updated_at = now() WHERE id = $2")
            .bind(&password_hash)
            .bind(user_id)
            .execute(&app.db)
            .await;
    if let Err(e) = updated {
        return from_api_error(&ApiError::from(e), &crate::request_origin()).into_response();
    }

    // Canvas parity: change_password logs the user in. Fresh role from DB.
    let role: String = match sqlx::query_scalar("SELECT role FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&app.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return from_api_error(&ApiError::from(e), &crate::request_origin()).into_response()
        }
    };
    let jwt = create_token(user_id, &role, &app.jwt_secret).unwrap_or_default();
    if jwt.is_empty() {
        return from_api_error(
            &ApiError::Db("token mint failed".to_string()),
            &crate::request_origin(),
        )
        .into_response();
    }
    (
        StatusCode::SEE_OTHER,
        jar.add(session_cookie(&jwt)),
        [("location", "/dashboard")],
    )
        .into_response()
}
