//! Canvas response middleware.
//!
//! Canvas answers *every* request with a fixed set of response headers. The
//! Rust port answered with none of them, which is why the parity replay
//! reported "missing key" for sixteen headers on all 430 recorded cases.
//! This crate is that middleware.
//!
//! Where the values come from (Canvas checkout at `canvas-src/`):
//!
//! | Header | Canvas source |
//! |---|---|
//! | `Content-Security-Policy` | `app/controllers/application_controller.rb:1236` (`set_response_headers`) |
//! | `X-Canvas-User-Id` | `app/controllers/application_controller.rb:1172` (`set_user_id_header`) |
//! | `Pragma`, `Cache-Control` | `app/controllers/application_controller.rb:1944` (`set_no_cache_headers`) |
//! | `X-Request-Cost`, `X-Rate-Limit-Remaining` | `app/middleware/request_throttle.rb:91-98` |
//! | `X-Session-Id`, `X-Request-Context-Id`, `X-Canvas-Meta` | `gems/request_context/lib/request_context/generator.rb:80-86` |
//! | `Referrer-Policy` | `config/application.rb:60` |
//! | `X-Content-Type-Options`, `X-Download-Options`, `X-Permitted-Cross-Domain-Policies`, `X-XSS-Protection` | Rails' `ActionDispatch` default headers |
//! | `Link` | `lib/api.rb:412-413` (`Api.paginate`) |
//!
//! `X-Frame-Options` is deliberately *not* sent: Canvas deletes it from the
//! Rails defaults (`config/application.rb:59`) and the recorded corpus has
//! none.
//!
//! The middleware also renders errors the way Canvas renders them —
//! `{"errors":[{"message":"..."}]}` — because the port used to answer
//! `text/plain`. See [`canvas_error_body`].

pub mod matcher;
pub mod meta;
pub mod pagination;
pub mod procinfo;
pub mod routes;

use axum::{
    body::{to_bytes, Body, HttpBody},
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use star_common::AppState;

use crate::{
    matcher::match_route, meta::MetaHeader, pagination::link_header, procinfo::ProcSample,
};

/// The largest response body the middleware is willing to buffer so it can
/// look at it (to detect a list body for the `Link` header, or to convert a
/// plain-text error into Canvas' JSON shape).
const MAX_BUFFERED_BODY: usize = 8 * 1024 * 1024;

/// Canvas' leaky bucket starts full (`app/middleware/request_throttle.rb:366`
/// sets `hwm: 600`), so a fresh bucket reports 600 units left.
const RATE_LIMIT_REMAINING: &str = "600.0";

/// Canvas' global ids are the shard id followed by the record id, so the
/// default shard (1) puts user 1 at `10000000000001`.
const GLOBAL_ID_OFFSET: i64 = 10_000_000_000_000;

/// `application/json; charset=utf-8` — what Canvas answers with.
const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// The Canvas response middleware.
///
/// Apply it to the whole application router:
///
/// ```ignore
/// .layer(axum::middleware::from_fn_with_state(state.clone(), star_canvas_response::canvas_response))
/// ```
pub async fn canvas_response(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Everything the middleware needs is read out of the request before it
    // is handed to the handler: a borrow of the request cannot be held
    // across an await, and the caller is resolved from the database.
    let verb = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(|q| q.to_string());
    let host = header_value(req.headers(), header::HOST).unwrap_or_else(|| "localhost".to_string());
    let bearer = bearer_token(&req);

    let auth = match &bearer {
        Some(token) => resolve_caller(&state, token, &path).await,
        None => None,
    };
    let request = RequestFacts::capture(verb, path, query, host, auth);
    let before = ProcSample::capture();

    let response = next.run(req).await;

    let after = ProcSample::capture();
    let elapsed = request.started.elapsed();

    let (mut parts, body) = response.into_parts();
    let status = parts.status;
    let content_type = header_value(&parts.headers, header::CONTENT_TYPE);

    let bufferable = should_buffer(&body, content_type.as_deref());
    let mut body: Option<Body> = Some(body);

    if bufferable {
        // The body is consumed here. `should_buffer` only lets a body through
        // when its content length is known and small, so a failure means the
        // connection broke and there is nothing left to return anyway.
        let bytes = to_bytes(body.take().unwrap_or_else(Body::empty), MAX_BUFFERED_BODY)
            .await
            .unwrap_or_default();

        if (status.is_client_error() || status.is_server_error())
            && is_plain_text(content_type.as_deref())
        {
            let text = String::from_utf8_lossy(&bytes);
            if let Some(rendered) = canvas_error_body(status, &text) {
                parts.headers.insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(JSON_CONTENT_TYPE),
                );
                parts.headers.remove(header::CONTENT_LENGTH);
                body = Some(Body::from(rendered));
            }
        }

        if body.is_none() {
            body = Some(Body::from(bytes));
        }
    }

    let mut response = Response::from_parts(parts, body.unwrap_or_else(Body::empty));

    // ------------------------------------------------------------------
    // Canvas answers JSON with `application/json; charset=utf-8`.
    // ------------------------------------------------------------------
    if is_json(header_value(response.headers(), header::CONTENT_TYPE).as_deref()) {
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(JSON_CONTENT_TYPE),
        );
    } else if header_value(response.headers(), header::CONTENT_TYPE).is_none()
        && (response.status().is_client_error() || response.status().is_server_error())
    {
        // Canvas puts the JSON content type on every error answer, even one
        // that carries no body (`app/controllers/application_controller.rb`
        // renders the error bodies as JSON). A handler that answers a bare
        // `StatusCode` would otherwise leave the header out entirely.
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(JSON_CONTENT_TYPE),
        );
    }

    // ------------------------------------------------------------------
    // Headers Canvas sets on every answer.
    // ------------------------------------------------------------------
    apply_canvas_headers(&mut response, &request, &before, &after, elapsed);

    // ------------------------------------------------------------------
    // RFC-5988 pagination links for list answers.
    // ------------------------------------------------------------------
    if response.status() == StatusCode::OK {
        if let Some(route) = request.route {
            if route.paginated {
                if let Some(link) = link_header(route, &request) {
                    if let Ok(value) = HeaderValue::from_str(&link) {
                        response.headers_mut().insert(header::LINK, value);
                    }
                }
            }
        }
    }

    response
}

/// Everything the middleware needs to know about the request it is
/// answering.
pub struct RequestFacts {
    /// Request path, used to pick the Canvas route and to build links.
    pub path: String,
    /// Raw query string, if any.
    pub query: Option<String>,
    /// `Host` header, used for the CSP header and the `Link` header.
    pub host: String,
    /// The acting user's id, when the request carried a valid bearer token.
    pub user_id: Option<i64>,
    /// The `access_tokens` row the bearer token belongs to — the `at` of the
    /// `x-canvas-meta` header (`lib/authentication_methods.rb:173`).
    pub access_token_id: Option<i64>,
    /// The developer key that access token was issued for — the `dk` of the
    /// header (`lib/authentication_methods.rb:174`).
    pub developer_key_id: Option<i64>,
    /// The Canvas route the request maps to, when the port knows it.
    pub route: Option<&'static crate::routes::Route>,
    /// When the request started.
    pub started: std::time::Instant,
}

impl RequestFacts {
    fn capture(
        verb: String,
        path: String,
        query: Option<String>,
        host: String,
        auth: Option<star_auth::BearerAuth>,
    ) -> Self {
        let user_id = auth.as_ref().map(|auth| auth.user_id);
        let access_token_id = auth.as_ref().and_then(|auth| auth.access_token_id);
        let developer_key_id = auth.as_ref().and_then(|auth| auth.developer_key_id);
        let route = match_route(&verb, &path);
        Self {
            path,
            query,
            host,
            user_id,
            access_token_id,
            developer_key_id,
            route,
            started: std::time::Instant::now(),
        }
    }

    /// Canvas' global id for the acting user.
    pub fn canvas_user_id(&self) -> Option<String> {
        self.user_id.map(|id| format!("{}", global_id(id)))
    }
}

/// Canvas' global id for a record.
///
/// A global id is the shard id followed by the record id, so the default
/// shard (1) puts user 1 at `10000000000001`
/// (`config/initializers/active_record.rb:110-112`, `Shard::IDS_PER_SHARD`).
pub fn global_id(id: i64) -> i64 {
    GLOBAL_ID_OFFSET + id
}

/// The bearer token of the request, if it carries one.
fn bearer_token(req: &Request) -> Option<String> {
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())?;
    header
        .strip_prefix("Bearer ")
        .map(|token| token.to_string())
}

/// The caller the bearer token of the request names, if any.
///
/// Canvas resolves the caller from the `access_tokens` row the token belongs
/// to (`lib/authentication_methods.rb:144-177`), which is also where the ids
/// of the `x-canvas-meta` header come from. The port resolves it the same
/// way, so the middleware and the handlers agree on who the caller is.
async fn resolve_caller(
    state: &AppState,
    token: &str,
    path: &str,
) -> Option<star_auth::BearerAuth> {
    match star_auth::resolve_bearer(&state.db, &state.jwt_secret, token, path).await {
        Ok(auth) => auth,
        Err(err) => {
            eprintln!("canvas-response: could not resolve the bearer token: {err}");
            None
        }
    }
}

/// Set every header Canvas sets on a response.
fn apply_canvas_headers(
    response: &mut Response,
    request: &RequestFacts,
    before: &ProcSample,
    after: &ProcSample,
    elapsed: std::time::Duration,
) {
    let status = response.status();
    let headers = response.headers_mut();

    // Rails' default headers (ActionDispatch::Response). Canvas keeps all of
    // them except X-Frame-Options, which it deletes (config/application.rb:59).
    set(headers, "x-content-type-options", "nosniff");
    set(headers, "x-download-options", "noopen");
    set(headers, "x-permitted-cross-domain-policies", "none");
    set(headers, "x-xss-protection", "1; mode=block");

    // config/application.rb:60
    set(headers, "referrer-policy", "no-referrer-when-downgrade");

    // app/controllers/application_controller.rb:1236 (set_response_headers)
    // The frame ancestor is Canvas' own domain
    // (`lib/host_url.rb:65-80` reads `config/domain.yml`), not the host the
    // request came in on.
    set(
        headers,
        "content-security-policy",
        &format!("frame-ancestors 'self' {};", csp_host(&request.host)),
    );

    // app/controllers/application_controller.rb:1944 (set_no_cache_headers)
    // and the Rails default cache policy for successful answers.
    // A handler that set its own cache policy keeps it: Canvas' asset
    // pipeline serves the stylesheets with
    // `public, max-age=31536000, immutable`, and Rails only adds the
    // no-cache headers when the controller asks for them
    // (`set_no_cache_headers`, application_controller.rb:1944). Overwriting
    // them would make every immutable asset revalidate on every page view.
    if !headers.contains_key("cache-control") {
        match status {
            StatusCode::OK => set(
                headers,
                "cache-control",
                "max-age=0, private, must-revalidate",
            ),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                set(headers, "cache-control", "no-store");
                set(headers, "pragma", "no-cache");
            }
            _ => set(headers, "cache-control", "no-cache"),
        }
    }

    // gems/request_context/lib/request_context/generator.rb:84
    set(
        headers,
        "x-request-context-id",
        &uuid::Uuid::new_v4().to_string(),
    );

    // Rails' `X-Runtime`.
    set(
        headers,
        "x-runtime",
        &format!("{:.2}", elapsed.as_secs_f64()),
    );

    // app/middleware/request_throttle.rb:96
    set(
        headers,
        "x-request-cost",
        &format!("{:.4}", elapsed.as_secs_f64()),
    );

    // app/middleware/request_throttle.rb:97 — the port has no leaky bucket,
    // so it always reports a full one.
    set(headers, "x-rate-limit-remaining", RATE_LIMIT_REMAINING);

    // app/controllers/application_controller.rb:1172 (set_user_id_header)
    if let Some(user_id) = request.canvas_user_id() {
        set(headers, "x-canvas-user-id", &user_id);
    }

    // gems/request_context/lib/request_context/generator.rb:80-86
    let session_id = session_id(request);
    set(headers, "x-session-id", &session_id);
    for cookie in session_cookies(&session_id) {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            headers.append(header::SET_COOKIE, value);
        }
    }

    // gems/request_context/lib/request_context/generator.rb:85
    let meta = MetaHeader::build(request, before, after);
    set(headers, "x-canvas-meta", &meta.to_string());
}

/// The host Canvas puts in the `Content-Security-Policy` header.
///
/// Canvas uses its own configured domain (`config/domain.yml`, read by
/// `lib/host_url.rb:65-80`), not the host the request arrived on. The port
/// has no domain setting, so `STAR_CANVAS_DOMAIN` plays that role; without
/// it the request's own host is used, without its port.
fn csp_host(request_host: &str) -> String {
    static DOMAIN: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let configured = DOMAIN.get_or_init(|| {
        std::env::var("STAR_CANVAS_DOMAIN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    });
    match configured {
        Some(domain) => domain.clone(),
        None => match request_host.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host.to_string(),
            _ => request_host.to_string(),
        },
    }
}

/// The session id Canvas would use for this request.
///
/// Canvas' session id is the rack session id, which is stable for a session.
/// The port has no server-side session — it authenticates with a bearer JWT —
/// so the session id is derived from the acting user: the same user always
/// maps to the same session id, and an unauthenticated request gets a fresh
/// one.
fn session_id(request: &RequestFacts) -> String {
    match request.user_id {
        Some(user_id) => {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(user_id.to_le_bytes());
            hasher.update(b"star-canvas-response session");
            hex(&hasher.finalize()[..16])
        }
        None => {
            use rand::RngCore;
            let mut bytes = [0u8; 16];
            rand::thread_rng().fill_bytes(&mut bytes);
            hex(&bytes)
        }
    }
}

/// The cookies Canvas writes alongside the session id.
///
/// * `_csrf_token` — Rails' masked CSRF token, written by the session store
///   configured in `config/initializers/session_store.rb`.
/// * `log_session_id` — the id Canvas logs requests under
///   (`gems/request_context/lib/request_context/session.rb`).
/// * `_normandy_session` — the session cookie itself
///   (`config/initializers/session_store.rb:40`).
fn session_cookies(session_id: &str) -> Vec<String> {
    use base64::Engine as _;
    use rand::RngCore;

    let mut csrf_bytes = [0u8; 31];
    rand::thread_rng().fill_bytes(&mut csrf_bytes);
    let csrf = url_escape(&base64::engine::general_purpose::STANDARD.encode(csrf_bytes));

    vec![
        format!("_csrf_token={csrf}; path=/"),
        format!("log_session_id={session_id}; path=/; httponly"),
        format!("_normandy_session={session_id}.{session_id}; path=/; httponly"),
    ]
}

/// Percent-encode the characters Rails escapes in a cookie value.
fn url_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Lowercase hex of a byte slice.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Render an error the way Canvas renders it.
///
/// Canvas' API errors are JSON objects with an `errors` array
/// (`app/controllers/application_controller.rb:2296` and
/// `lib/authentication_methods.rb:414-425`):
///
/// * 401 — `{"status":"unauthenticated","errors":[{"message":"user
///   authorization required"}]}`
/// * 403 — `{"status":"unauthorized","errors":[{"message":"user not
///   authorized to perform that action"}]}`
/// * 404 — `{"errors":[{"message":"The specified resource does not
///   exist."}]}`
/// * 500 — `{"errors":[{"message":"An error occurred.","error_code":
///   "internal_server_error"}]}`
/// * everything else — `{"errors":[{"message":"<the message>"}]}`
pub fn canvas_error_body(status: StatusCode, text: &str) -> Option<Vec<u8>> {
    let trimmed = text.trim();
    let detail = strip_error_prefix(trimmed);
    let body = match status {
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
            if detail.is_empty() {
                return None;
            }
            let escaped = serde_json::to_string(detail).unwrap_or_else(|_| "\"\"".to_string());
            format!("{{\"errors\":[{{\"message\":{escaped}}}]}}")
        }
    };
    Some(body.into_bytes())
}

/// Strip the prefix the port's error type puts on its message.
fn strip_error_prefix(text: &str) -> &str {
    for prefix in ["validation: ", "bad request: ", "conflict: "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return rest;
        }
    }
    text
}

/// Whether the middleware should buffer the body.
fn should_buffer(body: &Body, content_type: Option<&str>) -> bool {
    if !is_json(content_type) && !is_plain_text(content_type) {
        return false;
    }
    match body.size_hint().exact() {
        Some(length) => length <= MAX_BUFFERED_BODY as u64,
        // The size is not known: the body is streamed, so leave it alone.
        None => false,
    }
}

fn is_json(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
    })
}

fn is_plain_text(content_type: Option<&str>) -> bool {
    content_type.is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/plain"))
    })
}

/// Read a header as a string.
fn header_value(headers: &axum::http::HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

/// Set a header, replacing any value that is already there.
fn set(headers: &mut axum::http::HeaderMap, name: &'static str, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}
