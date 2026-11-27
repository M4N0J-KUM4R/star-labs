//! The bytes of a file: downloading them, and the attachment JWT that lets a
//! browser fetch them without an API token.
//!
//! Canvas keeps the bytes of an attachment in its store and serves them from
//! `files_controller#show` (`app/controllers/files_controller.rb:665-764`):
//! the response carries the content type, the length and the disposition
//! Canvas computes (`app/helpers/attachment_helper.rb:226-228`).
//!
//! The attachment JWT is Canvas' own short-lived token: it carries the
//! attachment id and lets a third party (the DocViewer) talk about a file
//! without an API token (`app/models/attachment.rb:2159`).

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{AUTHORIZATION, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use star_common::{ApiError, AppState};
use star_storage::{global_store, StorageError};

use crate::user_enrolled_or_admin;

/// The claims of an attachment JWT.
///
/// Canvas signs `{ id: <attachment id> }` with an expiry
/// (`CanvasSecurity.create_jwt({ id: }, 1.hour.from_now)`,
/// `app/models/attachment.rb:2159`); the port signs the same claim set with
/// its own secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentClaims {
    /// The attachment the token is about.
    pub id: i64,
    /// When the token stops being valid (seconds since the epoch).
    pub exp: usize,
}

/// How long an attachment JWT is good for — Canvas uses an hour.
const ATTACHMENT_JWT_TTL_SECS: i64 = 3600;

/// Mint an attachment JWT for `attachment_id`.
pub fn create_attachment_jwt(attachment_id: i64, secret: &str) -> Result<String, ApiError> {
    let exp = (Utc::now().timestamp() + ATTACHMENT_JWT_TTL_SECS) as usize;
    let claims = AttachmentClaims {
        id: attachment_id,
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| ApiError::canvas_internal_error())
}

/// Read an attachment JWT.
pub fn read_attachment_jwt(token: &str, secret: &str) -> Option<AttachmentClaims> {
    decode::<AttachmentClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .ok()
    .map(|data| data.claims)
}

/// The query of the download route.
#[derive(Debug, Default, Deserialize)]
pub struct DownloadQuery {
    /// The attachment JWT, when the caller has no API token.
    pub attachment_jwt: Option<String>,
    /// Canvas serves the file inline when the caller asks for it.
    pub inline: Option<String>,
    /// Canvas forces a download when this is present.
    pub download_frd: Option<String>,
}

/// The row the download needs.
#[derive(Debug, sqlx::FromRow)]
struct ContentRow {
    course_id: i64,
    uploader_id: i64,
    filename: String,
    display_name: String,
    content_type: String,
    storage_key: Option<String>,
    encoding: Option<String>,
}

/// GET /files/{id}/download — the bytes of a file.
///
/// Canvas serves them from `files_controller#show` with
/// `send_file(..., type: content_type_with_encoding, disposition: ...,
/// filename: display_name)` (`app/helpers/attachment_helper.rb:228`).
pub async fn download_file(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Query(query): Query<DownloadQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let row: Option<ContentRow> = sqlx::query_as(
        "SELECT course_id, uploader_id, filename, display_name, content_type, \
                storage_key, encoding \
         FROM files WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(file_id)
    .fetch_optional(&state.db)
    .await?;

    let row = row.ok_or(ApiError::NotFound)?;

    // The attachment JWT stands in for the API token: it names the file, so
    // it is all the caller needs.
    let has_jwt = query
        .attachment_jwt
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .and_then(|token| read_attachment_jwt(token, &state.jwt_secret))
        .is_some_and(|claims| claims.id == file_id);

    if !has_jwt {
        let path = format!("/api/v1/files/{file_id}/download");
        let Some(token) = bearer_token(&headers) else {
            return Err(ApiError::Unauthorized);
        };
        let Some(auth) =
            star_auth::resolve_bearer(&state.db, &state.jwt_secret, token, &path).await?
        else {
            return Err(ApiError::Unauthorized);
        };
        let allowed =
            user_enrolled_or_admin(&state.db, auth.user_id, row.course_id, &auth.role).await?;
        if row.uploader_id != auth.user_id && !allowed {
            return Err(ApiError::Forbidden);
        }
    }

    let Some(storage_key) = row.storage_key.clone() else {
        return Err(ApiError::canvas_message(
            StatusCode::NOT_FOUND,
            "File data could not be found",
        ));
    };

    let store = global_store().map_err(|_| ApiError::canvas_internal_error())?;
    let bytes = match store.get(&storage_key).await {
        Ok(bytes) => bytes,
        Err(StorageError::NotFound(_)) => {
            return Err(ApiError::canvas_message(
                StatusCode::NOT_FOUND,
                "File data could not be found",
            ))
        }
        Err(_) => return Err(ApiError::canvas_internal_error()),
    };

    let content_type = match row.encoding.as_deref() {
        Some(encoding) if !encoding.is_empty() => {
            format!("{}; charset={}", row.content_type, encoding)
        }
        _ => row.content_type.clone(),
    };

    let inline = inline_content(&row.content_type, &row.filename, query.inline.is_some())
        && query.download_frd.is_none();
    let disposition = format!(
        "{}; {}",
        if inline { "inline" } else { "attachment" },
        disposition_filename(&row.display_name)
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, bytes.len().to_string())
        .header(CONTENT_DISPOSITION, disposition)
        // Canvas keeps these pages out of the search engines
        // (`app/controllers/files_controller.rb:667`).
        .header("x-robots-tag", "noindex, nofollow")
        .body(Body::from(bytes))
        .map_err(|_| ApiError::canvas_internal_error())
}

/// POST /files/{id}/attachment_jwt — mint the token that lets a browser
/// fetch the file without an API token.
///
/// Canvas mints this token where it needs one (the DocViewer upload,
/// `app/models/attachment.rb:2159`); the port exposes it so a client can ask
/// for one.
pub async fn mint_attachment_jwt(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: Option<(i64, i64, i64)> = sqlx::query_as(
        "SELECT id, course_id, uploader_id FROM files WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(file_id)
    .fetch_optional(&state.db)
    .await?;

    let (_, course_id, uploader_id) = row.ok_or(ApiError::NotFound)?;

    let path = format!("/api/v1/files/{file_id}/attachment_jwt");
    let Some(token) = bearer_token(&headers) else {
        return Err(ApiError::Unauthorized);
    };
    let Some(auth) = star_auth::resolve_bearer(&state.db, &state.jwt_secret, token, &path).await?
    else {
        return Err(ApiError::Unauthorized);
    };
    let allowed = user_enrolled_or_admin(&state.db, auth.user_id, course_id, &auth.role).await?;
    if uploader_id != auth.user_id && !allowed {
        return Err(ApiError::Forbidden);
    }

    let token = create_attachment_jwt(file_id, &state.jwt_secret)?;
    let expires_at = Utc::now() + chrono::Duration::seconds(ATTACHMENT_JWT_TTL_SECS);
    Ok(Json(serde_json::json!({
        "attachment_jwt": token,
        "expires_at": expires_at.to_rfc3339(),
    })))
}

/// The bearer token of a request.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
}

/// Whether Canvas serves the file inline.
///
/// `Attachment#inline_content?` (`app/models/attachment.rb:933`): a text
/// file, or an HTML/SWF one. With `inline=1` Canvas also serves images
/// inline (`app/controllers/files_controller.rb:900`).
pub fn inline_content(content_type: &str, filename: &str, forced: bool) -> bool {
    let text = content_type.starts_with("text");
    let image = content_type.starts_with("image/");
    let markup =
        filename.ends_with(".html") || filename.ends_with(".htm") || filename.ends_with(".swf");
    if forced {
        text || image
    } else {
        text || markup
    }
}

/// The file name part of a `Content-Disposition` header.
///
/// Canvas builds it in `Attachment#disposition_filename`
/// (`app/models/attachment.rb:1272-1284`): the readable name in quotes, and
/// the real name in the RFC 5987 form.
pub fn disposition_filename(display_name: &str) -> String {
    let ascii: String = display_name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c
            } else {
                '_'
            }
        })
        .collect();
    let quoted_ascii = escape_quoted(&ascii);
    format!(
        "filename=\"{quoted_ascii}\"; filename*=UTF-8''{}",
        escape_unicode(display_name)
    )
}

/// Escape the characters Canvas escapes in a quoted string.
fn escape_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_ascii_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Percent-encode everything but letters, digits and dots, the way
/// `URI::DEFAULT_PARSER.escape(name, /[^A-Za-z0-9.]/)` does.
fn escape_unicode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' => out.push(*byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_matches_canvas() {
        assert_eq!(
            disposition_filename("report.pdf"),
            "filename=\"report.pdf\"; filename*=UTF-8''report.pdf"
        );
        assert_eq!(
            disposition_filename("rapport été.pdf"),
            "filename=\"rapport _t_.pdf\"; filename*=UTF-8''rapport%20%C3%A9t%C3%A9.pdf"
        );
    }

    #[test]
    fn inline_content_matches_canvas() {
        assert!(inline_content("text/plain", "a.txt", false));
        assert!(inline_content("text/html", "a.htm", false));
        assert!(!inline_content("application/pdf", "a.pdf", false));
        assert!(inline_content("image/png", "a.png", true));
    }

    #[test]
    fn attachment_jwt_round_trips() {
        let token = create_attachment_jwt(569, "secret").unwrap();
        let claims = read_attachment_jwt(&token, "secret").unwrap();
        assert_eq!(claims.id, 569);
        assert!(read_attachment_jwt(&token, "other secret").is_none());
    }
}
