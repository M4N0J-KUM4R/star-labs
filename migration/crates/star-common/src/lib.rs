//! Shared state, error type, and JWT claims for star-lms.
//!
//! Keep this small: state + errors + claims only. Domain logic belongs in
//! the domain crates.

use axum::{
    extract::{FromRequest, Request},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// Application state shared by all handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt_secret: String,
}

/// Single error type for the whole API. Convert to HTTP responses here.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("validation: {0}")]
    Validation(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("database error")]
    Db(String),
    /// Canvas' parameter-validation error.
    ///
    /// Canvas renders this as a **bare array** of `{attribute, message}` pairs,
    /// not as an object with an `errors` key
    /// (`app/controllers/conversations_controller.rb:1149`):
    ///
    /// ```json
    /// [{"attribute":"recipients","message":"blank"}]
    /// ```
    #[error("parameter validation: {0:?}")]
    ParameterValidation(Vec<ParamError>),
    /// A Canvas error body the port must render verbatim.
    ///
    /// Canvas renders several different shapes for a 400 depending on the
    /// controller — `{"errors":[{"message":"..."}]}` from the generic handler
    /// (`app/controllers/application_controller.rb:2321`), `{"message":"..."}`
    /// or `{"error":"..."}` from controllers that render their own hash. Use
    /// this variant when the shape is not one of the fixed ones.
    #[error("canvas error: {body}")]
    Canvas { status: StatusCode, body: String },
}

/// One entry of Canvas' parameter-validation error array.
/// The `message` vocabulary comes from Canvas' own validators: `"blank"`,
/// `"invalid"`, `"not_found"`, …
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParamError {
    pub attribute: String,
    pub message: String,
}

/// Canvas' `reject!` body — `RequestError#error_json`
/// (`lib/request_error.rb:28-33`).
///
/// The field order is Canvas': `status` before `message`.
#[derive(Debug, Clone, Serialize)]
struct RejectBody {
    status: &'static str,
    message: String,
}

/// Canvas' generic 500 body with an `error_report_id`.
///
/// The field order is Canvas': `errors` before `error_report_id`.
#[derive(Debug, Clone, Serialize)]
struct InternalErrorBody {
    errors: Vec<InternalErrorEntry>,
    error_report_id: String,
}

/// One entry of Canvas' generic 500 `errors` array.
#[derive(Debug, Clone, Serialize)]
struct InternalErrorEntry {
    message: &'static str,
    error_code: &'static str,
}

impl ParamError {
    pub fn new(attribute: impl Into<String>, message: impl Into<String>) -> Self {
        ParamError {
            attribute: attribute.into(),
            message: message.into(),
        }
    }
}

impl ApiError {
    /// Maps each variant to its HTTP status code.
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::Conflict(_) => StatusCode::CONFLICT,
            ApiError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::ParameterValidation(_) => StatusCode::BAD_REQUEST,
            ApiError::Canvas { status, .. } => *status,
        }
    }

    /// The message Canvas puts in the `errors` array of an API error.
    ///
    /// The variants that carry a message keep it; the others have a message
    /// Canvas fixes for them (`The specified resource does not exist.` and
    /// friends).
    pub fn detail(&self) -> &str {
        match self {
            ApiError::Conflict(message) => message,
            ApiError::Validation(message) => message,
            ApiError::BadRequest(message) => message,
            _ => "",
        }
    }

    /// A Canvas error body the port renders verbatim.
    ///
    /// Use it for the 400 shapes Canvas renders outside the generic handler
    /// (`{"message":"..."}`, `{"error":"..."}`).
    ///
    /// The body is serialized here rather than built as a
    /// `serde_json::Value`: a `Value`'s map sorts its keys alphabetically,
    /// while Canvas renders `status` before `errors` and `errors` before
    /// `error_report_id`. A `#[derive(Serialize)]` struct keeps the field
    /// order of its declaration.
    pub fn canvas_json<T: serde::Serialize>(status: StatusCode, body: T) -> Self {
        let body = serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string());
        ApiError::Canvas { status, body }
    }

    /// Canvas' parameter-validation error: a bare array of
    /// `{attribute, message}` pairs.
    pub fn parameter_validation(errors: Vec<ParamError>) -> Self {
        ApiError::ParameterValidation(errors)
    }

    /// Canvas' generic 500 body.
    ///
    /// Canvas' exception handler answers an unhandled exception with
    /// `{"errors":[{"message":"An error occurred.","error_code":
    /// "internal_server_error"}]}`
    /// (`app/controllers/application_controller.rb:2325`).
    pub fn canvas_internal_error() -> Self {
        ApiError::Canvas {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: "{\"errors\":[{\"message\":\"An error occurred.\",\"error_code\":\"internal_server_error\"}]}"
                .to_string(),
        }
    }

    /// Canvas' `reject!` body — `RequestError#error_json`
    /// (`lib/request_error.rb:28-33`).
    ///
    /// ```json
    /// {"status":"bad_request","message":"..."}
    /// ```
    pub fn canvas_reject(status: StatusCode, message: impl Into<String>) -> Self {
        // A struct keeps the field order Canvas renders; a `serde_json::Value`
        // map sorts its keys alphabetically.
        let body = RejectBody {
            status: status_symbol(status),
            message: message.into(),
        };
        ApiError::canvas_json(status, &body)
    }

    /// Canvas' generic 500 body, with the `error_report_id` Canvas adds when
    /// the failure was reported to its error reporter
    /// (`app/controllers/application_controller.rb:2284-2286`).
    ///
    /// ```json
    /// {"errors":[{"message":"An error occurred.","error_code":"internal_server_error"}],"error_report_id":"..."}
    /// ```
    pub fn canvas_internal_error_with_report(report_id: impl Into<String>) -> Self {
        let body = InternalErrorBody {
            errors: vec![InternalErrorEntry {
                message: "An error occurred.",
                error_code: "internal_server_error",
            }],
            error_report_id: report_id.into(),
        };
        ApiError::canvas_json(StatusCode::INTERNAL_SERVER_ERROR, &body)
    }

    /// Canvas' `{"message":"..."}` body.
    ///
    /// A number of Canvas controllers answer a failure with a single
    /// `message` key of their own, not with the generic `errors` array:
    ///
    /// * `{"message":"Nothing to split off of this user"}` — 400
    ///   (`app/controllers/users_controller.rb:2835`)
    /// * `{"message":"Developer key not authorized"}` — 403
    ///   (`app/controllers/users_controller.rb:2955`)
    /// * `{"message":"No front page has been set"}` — 404
    ///   (`app/controllers/wiki_pages_api_controller.rb:781`)
    pub fn canvas_message(status: StatusCode, message: impl Into<String>) -> Self {
        let message = message.into();
        let body = serde_json::json!({ "message": message });
        ApiError::canvas_json(status, body)
    }

    /// Canvas' `{"error":"..."}` body.
    ///
    /// * `{"error":"no current invitation"}` — 400
    ///   (`app/controllers/enrollments_api_controller.rb:1001`)
    /// * `{"error":"No rubric IDs provided"}` — 400
    ///   (`app/controllers/rubrics_api_controller.rb:392`)
    /// * `{"error":"'grade_data' parameter required"}` — 400
    ///   (`app/controllers/submissions_api_controller.rb:1390`)
    pub fn canvas_error(status: StatusCode, message: impl Into<String>) -> Self {
        let message = message.into();
        let body = serde_json::json!({ "error": message });
        ApiError::canvas_json(status, body)
    }

    /// Canvas' `{"error":{"message":"..."}}` body.
    ///
    /// `{"error":{"message":"No progress available because this course is not
    /// module based ..."}}` — 400
    /// (`app/controllers/courses_controller.rb` `bulk_user_progress`).
    pub fn canvas_error_object(status: StatusCode, message: impl Into<String>) -> Self {
        let message = message.into();
        let body = serde_json::json!({ "error": { "message": message } });
        ApiError::canvas_json(status, body)
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        eprintln!("database error: {e}");
        ApiError::Db(e.to_string())
    }
}

impl IntoResponse for ApiError {
    /// Canvas answers API errors with a JSON body, never with plain text.
    ///
    /// * 401 — `{"status":"unauthenticated","errors":[{"message":"user
    ///   authorization required"}]}` (`lib/authentication_methods.rb:414-425`)
    /// * 403 — `{"status":"unauthorized","errors":[{"message":"user not
    ///   authorized to perform that action"}]}` (same place)
    /// * 404 — `{"errors":[{"message":"The specified resource does not
    ///   exist."}]}` (`app/controllers/application_controller.rb:2296`)
    /// * 500 — `{"errors":[{"message":"An error occurred.","error_code":
    ///   "internal_server_error"}]}` (same place)
    /// * 400 (parameter validation) — a bare array of `{attribute, message}`
    ///   pairs (`app/controllers/conversations_controller.rb:1149`)
    /// * everything else — `{"errors":[{"message":"<the message>"}]}`
    fn into_response(self) -> Response {
        let status = self.status();
        let body = match &self {
            ApiError::Canvas { body, .. } => body.clone(),
            ApiError::ParameterValidation(errors) => param_validation_json(errors),
            _ => canvas_error_json(status, self.detail()),
        };
        let mut response = (status, body).into_response();
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json; charset=utf-8"),
        );
        if status == StatusCode::UNAUTHORIZED {
            // Canvas tells a client without a session to authenticate again.
            response.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static("Bearer realm=\"canvas-lms\""),
            );
        }
        response
    }
}

/// `Rack::Utils::SYMBOL_TO_STATUS_CODE` key for a status code.
///
/// Canvas' `RequestError#error_json` puts that symbol in the body's
/// `status` key (`lib/request_error.rb:30`).
pub fn status_symbol(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "bad_request",
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::UNPROCESSABLE_ENTITY => "unprocessable_entity",
        StatusCode::INTERNAL_SERVER_ERROR => "internal_server_error",
        _ => "error",
    }
}

/// Canvas' parameter-validation body: a bare array of `{attribute, message}`.
///
/// `app/controllers/conversations_controller.rb:1149`:
///
/// ```ruby
/// def render_error(attribute, message, status = :bad_request)
///   render json: [{ attribute:, message: }], status:
/// end
/// ```
fn param_validation_json(errors: &[ParamError]) -> String {
    serde_json::to_string(errors).unwrap_or_else(|_| "[]".to_string())
}

/// The JSON body Canvas answers an API error with.
///
/// Canvas renders the error objects itself, so the key order matters for a
/// byte-for-byte comparison:
///
/// * 401 — `{"status":"unauthenticated","errors":[{"message":"user
///   authorization required"}]}` (`lib/authentication_methods.rb:414-425`)
/// * 403 — `{"status":"unauthorized","errors":[{"message":"user not
///   authorized to perform that action"}]}` (same place)
/// * 404 — `{"errors":[{"message":"The specified resource does not
///   exist."}]}` (`app/controllers/application_controller.rb:2296`)
/// * 500 — `{"errors":[{"message":"An error occurred.","error_code":
///   "internal_server_error"}]}` (same place)
/// * everything else — `{"errors":[{"message":"<the message>"}]}`
fn canvas_error_json(status: StatusCode, message: &str) -> String {
    match status {
        StatusCode::UNAUTHORIZED => {
            "{\"status\":\"unauthenticated\",\"errors\":[{\"message\":\"user authorization required\"}]}"
                .to_string()
        }
        StatusCode::FORBIDDEN => {
            "{\"status\":\"unauthorized\",\"errors\":[{\"message\":\"user not authorized to perform that action\"}]}"
                .to_string()
        }
        StatusCode::NOT_FOUND => {
            "{\"errors\":[{\"message\":\"The specified resource does not exist.\"}]}".to_string()
        }
        StatusCode::INTERNAL_SERVER_ERROR => {
            "{\"errors\":[{\"message\":\"An error occurred.\",\"error_code\":\"internal_server_error\"}]}"
                .to_string()
        }
        _ => {
            let escaped = serde_json::to_string(message).unwrap_or_else(|_| "\"\"".to_string());
            format!("{{\"errors\":[{{\"message\":{escaped}}}]}}")
        }
    }
}

/// JWT claims for session tokens.
#[derive(Debug, Deserialize, Serialize)]
pub struct Claims {
    pub sub: i64,
    pub role: String,
    pub exp: usize,
}

/// Canvas-compatible request-body extractor.
///
/// axum's `Json<T>` rejects a request that has no
/// `Content-Type: application/json` **before** the handler body runs, so the
/// permission check that would have produced Canvas' 403 never happens
/// (`docs/knowledge/traps/axum-json-extractor-415.md`). Canvas does not do
/// that: it reads `params[:xyz]` out of whatever the client sent — JSON, a
/// form-encoded body, or nothing at all — and answers 400/403/404 as the
/// route dictates.
///
/// `ApiJson<T>` therefore accepts a body with no content type, a
/// form-encoded body, or a JSON body, and deserializes it into `T`. An empty
/// body deserializes to `T::default()`, so every parameter reads as absent
/// and the handler applies Canvas' own parameter validation.
///
/// Like `Json<T>`, it must be the **last** extractor in a handler: it
/// consumes the request body.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned + Default,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = axum::body::Bytes::from_request(req, state)
            .await
            .map_err(|e| ApiError::BadRequest(format!("Failed to read the request body: {e}")))?;
        Ok(ApiJson(parse_canvas_body(&bytes)?))
    }
}

/// Parses a request body the way Canvas parses `params[:xyz]`.
///
/// * an empty body — every parameter is absent (`T::default()`)
/// * a JSON object — deserialized as-is
/// * a form-encoded body — decoded into the equivalent JSON object
fn parse_canvas_body<T: DeserializeOwned + Default>(bytes: &[u8]) -> Result<T, ApiError> {
    let body = trim_ascii(bytes);
    if body.is_empty() {
        return Ok(T::default());
    }

    if body.first() == Some(&b'{') || body.first() == Some(&b'[') {
        return serde_json::from_slice(body).map_err(|e| {
            ApiError::BadRequest(format!("Failed to parse the request body as JSON: {e}"))
        });
    }

    // Form-encoded: Rails reads `a=1&b[]=2` into params[:a] / params[:b].
    let pairs = parse_form_pairs(body);
    if pairs.is_empty() {
        return Ok(T::default());
    }
    let value = form_pairs_to_json(pairs);
    serde_json::from_value(value)
        .map_err(|e| ApiError::BadRequest(format!("Failed to parse the request body as JSON: {e}")))
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

/// One `key=value` pair of a form-encoded body. `key` keeps its `[]` suffix
/// so a repeated key can be recognised as an array.
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
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Turns form pairs into the JSON object Canvas' params would be.
///
/// A repeated `key[]` becomes an array; a single value stays a string and is
/// coerced to a number or a boolean when it reads as one, so that a form
/// body deserializes into the same shape as a JSON body.
fn form_pairs_to_json(pairs: Vec<(String, String)>) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    for (key, value) in pairs {
        let (name, is_array) = match key.strip_suffix("[]") {
            Some(name) => (name.to_string(), true),
            None => (key, false),
        };
        let coerced = coerce_form_value(&value);
        if is_array {
            let entry = object
                .entry(name)
                .or_insert_with(|| serde_json::Value::Array(Vec::new()));
            if let serde_json::Value::Array(items) = entry {
                items.push(coerced);
            }
        } else {
            object.insert(name, coerced);
        }
    }
    serde_json::Value::Object(object)
}

fn coerce_form_value(value: &str) -> serde_json::Value {
    if value.is_empty() {
        return serde_json::Value::String(String::new());
    }
    if let Ok(number) = value.parse::<i64>() {
        return serde_json::Value::Number(number.into());
    }
    if let Ok(number) = value.parse::<f64>() {
        if let Some(number) = serde_json::Number::from_f64(number) {
            return serde_json::Value::Number(number);
        }
    }
    match value {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        _ => serde_json::Value::String(value.to_string()),
    }
}
