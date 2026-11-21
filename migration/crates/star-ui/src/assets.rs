//! Content-hashed static assets for the UI shell.
//!
//! Phase 1 ships one hand-authored stylesheet (`static/app.css`, embedded via
//! `include_str!`). Assets are served from `/static/{hash}/{file}` where
//! `{hash}` is the full sha256 of the file contents, computed once per
//! process behind a `OnceLock`. Because the URL changes whenever the content
//! changes, responses carry `Cache-Control: public, max-age=31536000,
//! immutable`; a hash mismatch is a 404 so a stale URL can never serve the
//! wrong bytes.
//!
//! This is deliberately a plain handler rather than `tower_http::ServeDir`:
//! the embedded single-file approach needs no `fs` feature on tower-http and
//! keeps the binary self-contained (spec §2.3: no node build, no runtime
//! template or asset dependency).

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

use star_common::{ApiError, AppState};

/// The shell stylesheet, embedded at compile time.
pub const APP_CSS: &str = include_str!("../static/app.css");

/// Full sha256 hex digest of [`APP_CSS`], computed once.
pub fn app_css_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| hex_sha256(APP_CSS.as_bytes()))
}

/// Absolute URL for the stylesheet, including its content hash.
pub fn app_css_url() -> String {
    format!("/static/{}/app.css", app_css_hash())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Serve an embedded asset by content hash. `GET /static/{hash}/{file}`.
///
/// 404s when either the file name is unknown or the hash does not match the
/// current content — both mean "the client is holding a URL from a previous
/// build", which a cache-immutable response must never answer with different
/// bytes. On match, the response is immutable-cacheable.
pub async fn serve_static(
    State(_state): State<AppState>,
    Path((hash, file)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let (body, content_type) = match file.as_str() {
        "app.css" => (APP_CSS, "text/css; charset=utf-8"),
        _ => return Err(ApiError::NotFound),
    };

    if hash != app_css_hash() {
        return Err(ApiError::NotFound);
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("public, max-age=31536000, immutable"),
    );

    Ok((StatusCode::OK, headers, body).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_css_hash_is_a_sha256_hex_digest() {
        let hash = app_css_hash();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        // Stable across calls (OnceLock) and equal to a fresh computation.
        assert_eq!(hash, hex_sha256(APP_CSS.as_bytes()));
    }

    #[test]
    fn app_css_url_embeds_the_hash() {
        assert_eq!(app_css_url(), format!("/static/{}/app.css", app_css_hash()));
    }

    #[test]
    fn hex_sha256_matches_known_vector() {
        // sha256("") — well-known digest, guards the hex encoding loop.
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
