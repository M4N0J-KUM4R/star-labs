//! Error pages for the shell — HTML renderings of ApiError's statuses.

use askama::Template;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use std::fmt::Write as _;

use star_common::ApiError;

use crate::assets;
use crate::env::JsEnv;

/// The error page template context.
#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorPage {
    /// Base layout fields.
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    /// Error fields.
    pub status: u16,
    pub message: String,
}

impl IntoResponse for ErrorPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("error template render failed: {err}")).into_response()
            }
        }
    }
}

/// Render an error page for a shell URL. The ENV block is anonymous (no user
/// keys leak on error pages) and the origin mirrors the request scheme/host.
pub fn error_page(status: StatusCode, message: &str, origin: &str) -> ErrorPage {
    let mut title = String::new();
    let _ = write!(title, "{status}");
    ErrorPage {
        title,
        body_class: "error-page".to_string(),
        css_url: assets::app_css_url(),
        env_json: JsEnv::anonymous(origin, Vec::new()).to_json_string(),
        notices: Vec::new(),
        status: status.as_u16(),
        message: message.to_string(),
    }
}

/// Map an ApiError to a user-facing error page (HTML equivalents of the API's
/// plain-text statuses).
pub fn from_api_error(err: &ApiError, origin: &str) -> ErrorPage {
    let (status, message) = match err {
        ApiError::NotFound => (
            StatusCode::NOT_FOUND,
            "The page you were looking for doesn't exist.",
        ),
        ApiError::Unauthorized => (
            StatusCode::UNAUTHORIZED,
            "You need to be logged in to see this page.",
        ),
        ApiError::Forbidden => (
            StatusCode::FORBIDDEN,
            "You don't have permission to access this page.",
        ),
        ApiError::Conflict(_) => (
            StatusCode::CONFLICT,
            "The request conflicted with the current state.",
        ),
        ApiError::Validation(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg),
        ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
        ApiError::Db(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong on our side.",
        ),
    };
    error_page(status, message, origin)
}
