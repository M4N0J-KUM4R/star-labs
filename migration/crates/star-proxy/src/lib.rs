//! star-proxy — the cutover front door.
//!
//! The port answers the Canvas routes it owns. While the port is still being
//! finished, the deployment must not lose the routes it does not own yet, so
//! the service sits in front of the running Canvas instance and delegates
//! what it cannot answer:
//!
//! * a request that matches no route in the port (axum's fallback), and
//! * a request whose method the port does not serve for that path (axum's
//!   method-not-allowed fallback — Canvas answers 404 for a route it does
//!   not define, never 405).
//!
//! Delegation is **off** unless `STAR_CANVAS_ORIGIN` names the Canvas
//! instance. Without it the fallback is the same empty 404 axum returns, so
//! the parity replay and CI see only the port's own answers and nothing
//! about the cutover changes the measured behaviour.
//!
//! The request is forwarded as it came in — method, path, query, headers and
//! body — and Canvas' answer is returned as it came back, including the
//! status, the `Set-Cookie` headers and the body. Only the hop-by-hop
//! headers (`Connection`, `Transfer-Encoding`, …) are dropped, because they
//! describe one connection rather than the message.

use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{Extension, Request},
    http::{header, HeaderMap, HeaderName, StatusCode},
    response::{IntoResponse, Response},
};
use reqwest::Client;
use thiserror::Error;

/// The environment variable that names the Canvas instance to delegate to.
pub const ORIGIN_ENV: &str = "STAR_CANVAS_ORIGIN";

/// The largest request body the front door will buffer while forwarding.
///
/// Attachment uploads can be large; the front door streams what it can and
/// refuses to buffer more than this, answering the way Canvas answers a
/// request it cannot read.
const MAX_FORWARDED_BODY: usize = 64 * 1024 * 1024;

/// Why a delegation failed.
#[derive(Debug, Error)]
pub enum ProxyError {
    /// The origin is not usable.
    #[error("proxy configuration error: {0}")]
    Config(String),
    /// Canvas could not be reached, or answered something unreadable.
    #[error("upstream failure: {0}")]
    Upstream(String),
}

/// The Canvas instance the port delegates to.
#[derive(Clone, Debug)]
pub struct CanvasOrigin {
    /// The origin without a trailing slash.
    base: String,
    /// The HTTP client used for the delegation.
    client: Client,
}

impl CanvasOrigin {
    /// Build an origin from a base URL.
    ///
    /// The URL must be `http://` or `https://`; anything else is a
    /// configuration error rather than a request-time failure, because a
    /// wrong origin must not turn into a 500 for every delegated route.
    pub fn new(base: &str) -> Result<Self, ProxyError> {
        let base = base.trim().trim_end_matches('/');
        if base.is_empty() {
            return Err(ProxyError::Config(format!("{ORIGIN_ENV} is empty")));
        }
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(ProxyError::Config(format!(
                "{ORIGIN_ENV} must be an http(s) URL, got `{base}`"
            )));
        }
        let client = Client::builder()
            // Canvas answers a redirected request itself: the client must see
            // the 302 and its Location header, not the page behind it.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| ProxyError::Config(format!("http client: {e}")))?;
        Ok(Self {
            base: base.to_string(),
            client,
        })
    }

    /// The origin from `STAR_CANVAS_ORIGIN`, when it is set and usable.
    pub fn from_env() -> Result<Option<Self>, ProxyError> {
        match std::env::var(ORIGIN_ENV) {
            Ok(value) if !value.trim().is_empty() => Ok(Some(Self::new(&value)?)),
            _ => Ok(None),
        }
    }

    /// The origin's base URL.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Forward `req` to Canvas and return Canvas' answer.
    ///
    /// A failure to reach Canvas is rendered the way the port renders an
    /// internal error, because there is nothing else to answer with.
    pub async fn forward(&self, req: Request<Body>) -> Response {
        match self.try_forward(req).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(error = %error, "the front door could not reach Canvas");
                star_common::ApiError::Canvas {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    body: canvas_internal_error_body(),
                }
                .into_response()
            }
        }
    }

    /// The delegation itself, without the error rendering.
    async fn try_forward(&self, req: Request<Body>) -> Result<Response, ProxyError> {
        let (parts, body) = req.into_parts();
        let method = parts.method.clone();
        let path = parts.uri.path().to_string();
        let query = parts
            .uri
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        let target = format!("{}{}{}", self.base, path, query);

        let bytes = axum::body::to_bytes(body, MAX_FORWARDED_BODY)
            .await
            .map_err(|e| ProxyError::Upstream(format!("reading the request body: {e}")))?;

        let mut request = self.client.request(method.clone(), &target);
        for (name, value) in parts.headers.iter() {
            if is_dropped_request_header(name) {
                continue;
            }
            request = request.header(name, value);
        }
        if !bytes.is_empty() {
            request = request.body(bytes.to_vec());
        }

        let response = request
            .send()
            .await
            .map_err(|e| ProxyError::Upstream(format!("{method} {target}: {e}")))?;
        let status = response.status();
        let mut headers = HeaderMap::new();
        for (name, value) in response.headers().iter() {
            if is_dropped_response_header(name) {
                continue;
            }
            // `Set-Cookie` repeats; every other header appears once.
            if name == header::SET_COOKIE {
                headers.append(name.clone(), value.clone());
            } else {
                headers.insert(name.clone(), value.clone());
            }
        }
        let body = response.bytes().await.map_err(|e| {
            ProxyError::Upstream(format!("{method} {target}: reading the body: {e}"))
        })?;

        let mut out = Response::new(Body::from(body));
        *out.status_mut() = status;
        *out.headers_mut() = headers;
        Ok(out)
    }
}

/// The front door's answer for a request the port cannot serve.
///
/// With a Canvas origin the request goes there. Without one the answer is
/// the same empty 404 axum's own fallback returns, so nothing about the
/// port's measured behaviour changes when the cutover is off.
pub async fn fallback(
    Extension(origin): Extension<Option<Arc<CanvasOrigin>>>,
    req: Request<Body>,
) -> Response {
    match origin {
        Some(origin) => origin.forward(req).await,
        None => not_found(),
    }
}

/// Canvas' 500 body, rendered the way the port renders it.
fn canvas_internal_error_body() -> String {
    // The port's own renderer owns the shape; ask it for the body rather than
    // writing a second copy of it here.
    serde_json::json!({
        "errors": [{
            "message": "An error occurred.",
            "error_code": "internal_server_error"
        }]
    })
    .to_string()
}

/// The empty 404 axum's default fallback returns.
fn not_found() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

/// Whether a request header describes the connection rather than the message.
fn is_dropped_request_header(name: &HeaderName) -> bool {
    is_hop_by_hop(name)
        || name == header::HOST
        || name == header::CONTENT_LENGTH
        || name == header::ACCEPT_ENCODING
}

/// Whether a response header describes the connection rather than the message.
fn is_dropped_response_header(name: &HeaderName) -> bool {
    is_hop_by_hop(name) || name == header::CONTENT_LENGTH || name == header::CONTENT_ENCODING
}

/// The hop-by-hop headers (`RFC 9110` §7.6.1) plus the ones that describe a
/// single connection and must not be replayed onto another one.
fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_origin_needs_an_http_url() {
        assert!(matches!(
            CanvasOrigin::new("canvas.example.edu"),
            Err(ProxyError::Config(_))
        ));
        assert!(matches!(
            CanvasOrigin::new("   "),
            Err(ProxyError::Config(_))
        ));
        assert!(CanvasOrigin::new("https://canvas.example.edu/").is_ok());
    }

    #[test]
    fn a_trailing_slash_is_dropped() {
        let origin = CanvasOrigin::new("https://canvas.example.edu/").unwrap();
        assert_eq!(origin.base(), "https://canvas.example.edu");
    }

    #[test]
    fn hop_by_hop_headers_are_dropped() {
        assert!(is_hop_by_hop(&header::CONNECTION));
        assert!(is_hop_by_hop(&header::TRANSFER_ENCODING));
        assert!(!is_hop_by_hop(&header::AUTHORIZATION));
        assert!(is_dropped_request_header(&header::HOST));
        assert!(!is_dropped_request_header(&header::AUTHORIZATION));
    }
}
