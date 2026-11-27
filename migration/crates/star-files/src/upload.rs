//! Reading an upload into bytes.
//!
//! Canvas uploads the bytes of an attachment in a multipart body
//! (`app/controllers/files_controller.rb:1059` — `params[:file]`), while the
//! port's metadata-only upload takes JSON or a form-encoded body
//! (`ApiJson`, `crates/star-common/src/lib.rs:366`). This extractor accepts
//! all three: a multipart body carries the bytes, the other two keep the
//! metadata-only behaviour.

use axum::body::Bytes;
use axum::extract::FromRequest;
use axum::extract::Request;
use axum::http::header::CONTENT_TYPE;
use serde_json::Value;

use star_common::{ApiError, AppState};

use crate::CreateFileRequest;

/// One part of a multipart body.
#[derive(Debug, Clone)]
pub struct MultipartPart {
    /// The name of the form field.
    pub name: String,
    /// The file name the client sent, when the part carried a file.
    pub filename: Option<String>,
    /// The content type the client announced, when the part carried one.
    pub content_type: Option<String>,
    /// The bytes of the part.
    pub data: Vec<u8>,
}

/// The body of an upload: the metadata plus, when the client sent bytes, the
/// bytes themselves.
#[derive(Debug, Default)]
pub struct UploadRequest {
    /// The metadata Canvas reads out of `params[:attachment]`.
    pub metadata: Option<CreateFileRequest>,
    /// The bytes of the uploaded file.
    pub data: Option<Vec<u8>>,
    /// The name of the uploaded file, when it came with the bytes.
    pub filename: Option<String>,
    /// The content type of the uploaded file, when it came with the bytes.
    pub content_type: Option<String>,
}

impl UploadRequest {
    /// The name of the file being uploaded.
    pub fn file_name(&self) -> Option<&str> {
        if let Some(metadata) = &self.metadata {
            if !metadata.filename.trim().is_empty() {
                return Some(metadata.filename.as_str());
            }
        }
        self.filename.as_deref()
    }

    /// The content type of the file being uploaded.
    pub fn file_content_type(&self) -> Option<&str> {
        if let Some(metadata) = &self.metadata {
            if let Some(content_type) = &metadata.content_type {
                if !content_type.trim().is_empty() {
                    return Some(content_type.as_str());
                }
            }
        }
        self.content_type.as_deref()
    }
}

impl FromRequest<AppState> for UploadRequest {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|e| ApiError::BadRequest(format!("Failed to read the request body: {e}")))?;

        if content_type.starts_with("multipart/form-data") {
            let boundary = boundary_of(&content_type).ok_or_else(|| {
                ApiError::BadRequest("The multipart body has no boundary".to_string())
            })?;
            let parts = parse_multipart(&bytes, &boundary)
                .map_err(|e| ApiError::BadRequest(format!("Failed to parse the body: {e}")))?;
            return Ok(UploadRequest::from_parts(parts));
        }

        if content_type.starts_with("application/json")
            || content_type.starts_with("application/x-www-form-urlencoded")
            || content_type.is_empty()
        {
            return Ok(UploadRequest {
                metadata: parse_metadata(&bytes)?,
                ..Default::default()
            });
        }

        // Anything else is the file itself.
        Ok(UploadRequest {
            data: Some(bytes.to_vec()),
            ..Default::default()
        })
    }
}

impl UploadRequest {
    /// Build the upload out of the parts of a multipart body.
    fn from_parts(parts: Vec<MultipartPart>) -> Self {
        let mut metadata = CreateFileRequest::default();
        let mut data = None;
        let mut filename = None;
        let mut content_type = None;

        for part in parts {
            match part.name.as_str() {
                // The bytes of the file.
                "file" | "uploaded_data" | "attachment[uploaded_data]" | "attachment[file]" => {
                    filename = part.filename.clone().or(filename);
                    content_type = part.content_type.clone().or(content_type);
                    data = Some(part.data);
                }
                "filename" | "attachment[filename]" => {
                    metadata.filename = text_of(&part.data);
                }
                "display_name" | "attachment[display_name]" => {
                    metadata.display_name = Some(text_of(&part.data));
                }
                "content_type" | "attachment[content_type]" => {
                    metadata.content_type = Some(text_of(&part.data));
                }
                "size" | "attachment[size]" => {
                    metadata.size = text_of(&part.data).trim().parse::<i64>().ok();
                }
                "folder" => {
                    metadata.folder = Some(text_of(&part.data));
                }
                "folder_id" | "attachment[folder_id]" => {
                    metadata.folder_id = text_of(&part.data).trim().parse::<i64>().ok();
                }
                _ => {}
            }
        }

        UploadRequest {
            metadata: Some(metadata),
            data,
            filename,
            content_type,
        }
    }
}

/// The text of a part.
fn text_of(data: &[u8]) -> String {
    String::from_utf8_lossy(data).trim().to_string()
}

/// Parse a JSON or form-encoded body the way `ApiJson` does.
fn parse_metadata(bytes: &[u8]) -> Result<Option<CreateFileRequest>, ApiError> {
    let body = trim_ascii(bytes);
    if body.is_empty() {
        return Ok(None);
    }

    if body.first() == Some(&b'{') || body.first() == Some(&b'[') {
        return serde_json::from_slice(body).map(Some).map_err(|e| {
            ApiError::BadRequest(format!("Failed to parse the request body as JSON: {e}"))
        });
    }

    // Form-encoded: Rails reads `a=1` into params[:a].
    let mut map = serde_json::Map::new();
    for (key, value) in parse_form_pairs(body) {
        let key = key.trim_end_matches("[]");
        let value = coerce_form_value(&value);
        match key {
            "filename" | "attachment[filename]" => {
                map.insert("filename".into(), value);
            }
            "display_name" | "attachment[display_name]" => {
                map.insert("display_name".into(), value);
            }
            "content_type" | "attachment[content_type]" => {
                map.insert("content_type".into(), value);
            }
            "size" | "attachment[size]" => {
                map.insert("size".into(), value);
            }
            "folder" => {
                map.insert("folder".into(), value);
            }
            "folder_id" | "attachment[folder_id]" => {
                map.insert("folder_id".into(), value);
            }
            _ => {}
        }
    }

    if map.is_empty() {
        return Ok(None);
    }

    serde_json::from_value(Value::Object(map))
        .map(Some)
        .map_err(|e| ApiError::BadRequest(format!("Failed to parse the request body as JSON: {e}")))
}

/// The value of a form field, coerced the way `ApiJson` coerces it
/// (`crates/star-common/src/lib.rs:514`).
fn coerce_form_value(value: &str) -> Value {
    if value.is_empty() {
        return Value::String(String::new());
    }
    if let Ok(number) = value.parse::<i64>() {
        return Value::Number(number.into());
    }
    if let Ok(number) = value.parse::<f64>() {
        if let Some(number) = serde_json::Number::from_f64(number) {
            return Value::Number(number);
        }
    }
    match value {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(value.to_string()),
    }
}

/// Trim the ASCII whitespace around a body.
fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

/// The `key=value` pairs of a form-encoded body.
fn parse_form_pairs(body: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(body);
    text.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = match part.split_once('=') {
                Some((key, value)) => (key, value),
                None => (part, ""),
            };
            (
                percent_decode(key.replace('+', " ").as_bytes()),
                percent_decode(value.replace('+', " ").as_bytes()),
            )
        })
        .collect()
}

/// Percent-decode a form value.
fn percent_decode(bytes: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// The boundary of a `multipart/form-data` content type.
fn boundary_of(content_type: &str) -> Option<String> {
    for piece in content_type.split(';').skip(1) {
        let piece = piece.trim();
        if let Some(boundary) = piece.strip_prefix("boundary=") {
            let boundary = boundary.trim_matches('"');
            if !boundary.is_empty() {
                return Some(boundary.to_string());
            }
        }
    }
    None
}

/// Split a multipart body into its parts.
fn parse_multipart(body: &[u8], boundary: &str) -> Result<Vec<MultipartPart>, String> {
    let delimiter = format!("--{boundary}");
    let delimiter = delimiter.as_bytes();

    let mut parts = Vec::new();
    let mut cursor = match find(body, delimiter, 0) {
        Some(index) => index + delimiter.len(),
        None => return Err("the body has no boundary".to_string()),
    };

    loop {
        // The closing delimiter is `--boundary--`.
        if body[cursor..].starts_with(b"--") {
            break;
        }
        // Skip the CRLF that ends the delimiter line.
        if body[cursor..].starts_with(b"\r\n") {
            cursor += 2;
        } else if body[cursor..].starts_with(b"\n") {
            cursor += 1;
        } else {
            break;
        }

        let next = match find(body, delimiter, cursor) {
            Some(index) => index,
            None => break,
        };
        // The part ends 2 bytes before the delimiter (the CRLF).
        let end = next.saturating_sub(2).max(cursor);
        let chunk = &body[cursor..end];

        let separator = find(chunk, b"\r\n\r\n", 0)
            .map(|index| (index, 4))
            .or_else(|| find(chunk, b"\n\n", 0).map(|index| (index, 2)))
            .ok_or_else(|| "a part has no header block".to_string())?;

        let headers = String::from_utf8_lossy(&chunk[..separator.0]).to_string();
        let data = chunk[separator.0 + separator.1..].to_vec();

        let mut name = String::new();
        let mut filename = None;
        let mut content_type = None;
        for line in headers.lines() {
            let (header, value) = match line.split_once(':') {
                Some((header, value)) => (header.trim().to_ascii_lowercase(), value.trim()),
                None => continue,
            };
            match header.as_str() {
                "content-disposition" => {
                    for piece in value.split(';').skip(1) {
                        let piece = piece.trim();
                        if let Some(value) = piece.strip_prefix("name=") {
                            name = value.trim_matches('"').to_string();
                        } else if let Some(value) = piece.strip_prefix("filename=") {
                            filename = Some(value.trim_matches('"').to_string());
                        }
                    }
                }
                "content-type" => content_type = Some(value.to_string()),
                _ => {}
            }
        }

        parts.push(MultipartPart {
            name,
            filename,
            content_type,
            data,
        });

        cursor = next + delimiter.len();
    }

    Ok(parts)
}

/// Find `needle` in `haystack` at or after `from`.
fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|index| index + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_boundary() {
        assert_eq!(
            boundary_of("multipart/form-data; boundary=abc123"),
            Some("abc123".to_string())
        );
        assert_eq!(
            boundary_of("multipart/form-data; boundary=\"abc 123\""),
            Some("abc 123".to_string())
        );
        assert_eq!(boundary_of("application/json"), None);
    }

    #[test]
    fn splits_a_multipart_body() {
        let body = b"--X\r\nContent-Disposition: form-data; name=\"filename\"\r\n\r\nreport.pdf\r\n--X\r\nContent-Disposition: form-data; name=\"file\"; filename=\"report.pdf\"\r\nContent-Type: application/pdf\r\n\r\n%PDF-1.4\r\n--X--\r\n";
        let parts = parse_multipart(body, "X").unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].name, "filename");
        assert_eq!(text_of(&parts[0].data), "report.pdf");
        assert_eq!(parts[1].name, "file");
        assert_eq!(parts[1].filename.as_deref(), Some("report.pdf"));
        assert_eq!(parts[1].content_type.as_deref(), Some("application/pdf"));
        assert_eq!(parts[1].data, b"%PDF-1.4".to_vec());
    }

    #[test]
    fn parses_json_and_form_metadata() {
        let json = parse_metadata(br#"{"filename":"a.pdf"}"#).unwrap().unwrap();
        assert_eq!(json.filename, "a.pdf");

        let form = parse_metadata(b"filename=a.pdf&size=12").unwrap().unwrap();
        assert_eq!(form.filename, "a.pdf");
        assert_eq!(form.size, Some(12));

        assert!(parse_metadata(b"").unwrap().is_none());
    }
}
