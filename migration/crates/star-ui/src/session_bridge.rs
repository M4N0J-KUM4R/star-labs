//! Presenting the web session to `/api/v1`.
//!
//! Canvas' browser code does not carry an API token. Its front end calls the API
//! same-origin and the API authenticates the web session: the controllers the pages
//! hit inherit the *web* base class — `class AnnouncementsApiController <
//! ApplicationController` (app/controllers/announcements_api_controller.rb:26) — so the
//! `require_user` path that reads the session cookie is the one in play, and
//! `window.ENV` (application_controller.rb:264-309) carries no access token at all. Our
//! `AuthUser` extractor has one transport, `Authorization: Bearer`, so the shell gets
//! the same behaviour by rewriting the cookie into that header in front of the API
//! router and letting `AuthUser` verify it exactly as it verifies a CLI token: HS256,
//! role reloaded from the DB, `users.last_logged_out` honoured. Nothing about
//! verification is duplicated here, which is the point.
//!
//! The rewrite is what re-opens CSRF: a cookie is sent without the user's involvement.
//! Two conditions, both of them Rails' own answer to the same problem:
//!
//! 1. **Same-origin.** `Origin`, else `Referer`, must name the request host. A form
//!    posted from another site carries neither for the methods that matter, and where it
//!    does carry one it must match.
//! 2. **Double-submit CSRF on unsafe methods.** The `X-CSRF-Token` header must equal the
//!    `star_csrf` cookie — the same pair `session::check_csrf` compares for the shell's
//!    own POST forms, and the reason that cookie is deliberately not HttpOnly.
//!
//! A request that fails either is rejected here rather than passed through unauthenticated:
//! silently dropping the credential would turn a CSRF attempt into a 404-by-permission
//! from the handler, which is a confusing answer to a hostile question.

use axum::http::{header, HeaderMap, Method, Request, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use star_common::ApiError;

use crate::session::{check_csrf_header, CSRF_COOKIE, CSRF_HEADER, SESSION_COOKIE};

/// The header `AuthUser` reads.
const BEARER_PREFIX: &str = "Bearer ";

/// What the bridge decided about one request.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Rewrite `Authorization` and continue.
    Authenticate(String),
    /// No cookie session to offer; leave the request exactly as it is (a client that
    /// means to be anonymous, or is already using `Authorization`).
    Passthrough,
    /// A cookie session was offered but the request is not trustworthy.
    Reject(&'static str),
}

/// Pure decision function — no I/O, so the security-relevant branch is unit-testable
/// without a database or a server.
pub fn decide(method: &Method, headers: &HeaderMap, uri: &Uri) -> Decision {
    let mut offered: Option<String> = None;
    let mut csrf_cookie: Option<String> = None;
    for value in headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
    {
        for pair in value.split(';') {
            let Some((name, contents)) = pair.split_once('=') else {
                continue;
            };
            let name = name.trim();
            let contents = contents.trim().to_string();
            if name == SESSION_COOKIE && offered.is_none() {
                offered = Some(contents);
            } else if name == CSRF_COOKIE && csrf_cookie.is_none() {
                csrf_cookie = Some(contents);
            }
        }
    }

    // `Authorization` always wins: an explicit credential is unambiguous, and API
    // clients (the CLI, LTI tools, every golden-response test) must behave exactly as
    // they did before this module existed.
    if offered.is_none() || headers.contains_key(header::AUTHORIZATION) {
        return Decision::Passthrough;
    }

    if let Err(reason) = same_origin(headers, uri) {
        return Decision::Reject(reason);
    }

    if !is_safe(method) {
        let sent = headers
            .get(CSRF_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        if !check_csrf_header(csrf_cookie.as_deref(), sent) {
            return Decision::Reject("unsafe API request is missing a matching CSRF token");
        }
    }

    Decision::Authenticate(offered.expect("checked above"))
}

/// Rails' `protect_from_forgery` semantics: only these verbs may change nothing.
fn is_safe(method: &Method) -> bool {
    *method == Method::GET || *method == Method::HEAD || *method == Method::OPTIONS
}

/// Compare the request's origin against its host.
///
/// `Origin` is sent by every current browser on POST/PUT/PATCH/DELETE and on CORS
/// GETs; `Referer` covers the rest. Absent both, the request is same-origin by
/// construction (a curl call, or a GET from the address bar) and nothing about the
/// cookie transport makes it cross-site.
///
/// The comparison is host-only, so `http://lms.example:4000` and `http://lms.example`
/// match. That is deliberate: the port is a deployment detail (star-http binds one
/// port in CI, another behind the proxy), and an attacker who can read a victim's
/// port has already beaten same-origin policy. What must hold is that the claim names
/// *this* host — a page on another domain cannot forge it, and `Origin: null` (a
/// sandboxed iframe, a `file://` document) is rejected rather than guessed at.
fn same_origin(headers: &HeaderMap, uri: &Uri) -> Result<(), &'static str> {
    let claimed_host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty());
    let host = match claimed_host {
        Some(value) => value,
        None => uri.host().unwrap_or_default(),
    };

    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        // "null" is the opaque origin: it says the requester is sandboxed, not that it
        // is this host. Treat it as hostile rather than as an absent claim.
        if origin == "null" {
            return Err("API request with a session cookie came from an opaque origin");
        }
        return host_matches(origin, host);
    }

    if let Some(referer) = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())
    {
        return host_matches(referer, host);
    }

    Ok(())
}

/// True when a URL-shaped `Origin`/`Referer` value names `host`, ignoring scheme and
/// port. `Referer` carries a path, so the host is the first segment after the authority.
fn host_matches(claim: &str, host: &str) -> Result<(), &'static str> {
    let claimed_host = claim
        .split("://")
        .nth(1)
        .unwrap_or(claim)
        .split('/')
        .next()
        .unwrap_or("");
    if claimed_host.split(':').next().unwrap_or("") == host.split(':').next().unwrap_or("") {
        Ok(())
    } else {
        Err("API request with a session cookie came from another origin")
    }
}

/// Middleware for the `/api/v1` router: see the module header.
///
/// The rejection is a 403, matching what the shell's own form handlers answer for a
/// CSRF mismatch (`login_csrf_mismatch_is_403`) rather than inventing a second shape.
pub async fn bridge_session_to_bearer(
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    match decide(request.method(), request.headers(), request.uri()) {
        Decision::Authenticate(token) => {
            if let Ok(value) = format!("{BEARER_PREFIX}{token}").parse() {
                request.headers_mut().insert(header::AUTHORIZATION, value);
            }
        }
        Decision::Reject(reason) => {
            eprintln!("session bridge rejected a request: {reason}");
            return ApiError::Forbidden.into_response();
        }
        Decision::Passthrough => {}
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    use axum::http::HeaderName;

    fn cookie(csrf: Option<&str>) -> String {
        let mut value = format!("{SESSION_COOKIE}=jwt-here");
        if let Some(token) = csrf {
            value.push_str(&format!("; {CSRF_COOKIE}={token}"));
        }
        value
    }

    fn uri(path: &str) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority("lms.example")
            .path_and_query(path)
            .build()
            .unwrap()
    }

    #[test]
    fn a_bearer_request_is_left_alone_even_with_a_cookie() {
        let map = headers(&[
            ("cookie", &cookie(None)),
            ("authorization", "Bearer explicit-token"),
            ("host", "lms.example"),
        ]);
        assert_eq!(
            decide(&Method::GET, &map, &uri("/api/v1/courses")),
            Decision::Passthrough
        );
    }

    #[test]
    fn no_session_cookie_means_no_rewrite() {
        let map = headers(&[("host", "lms.example")]);
        assert_eq!(
            decide(&Method::GET, &map, &uri("/api/v1/courses")),
            Decision::Passthrough
        );
    }

    #[test]
    fn a_get_with_the_session_cookie_becomes_a_bearer_credential() {
        let map = headers(&[("cookie", &cookie(None)), ("host", "lms.example")]);
        assert_eq!(
            decide(&Method::GET, &map, &uri("/api/v1/courses/1")),
            Decision::Authenticate("jwt-here".to_string())
        );
    }

    #[test]
    fn unsafe_methods_need_the_double_submit_pair() {
        let uri = uri("/api/v1/courses/1");
        let without_header =
            headers(&[("cookie", &cookie(Some("csrf-1"))), ("host", "lms.example")]);
        assert!(matches!(
            decide(&Method::POST, &without_header, &uri),
            Decision::Reject(_)
        ));

        let mismatched = headers(&[
            ("cookie", &cookie(Some("csrf-1"))),
            ("x-csrf-token", "csrf-2"),
            ("host", "lms.example"),
        ]);
        assert!(matches!(
            decide(&Method::DELETE, &mismatched, &uri),
            Decision::Reject(_)
        ));

        // No cookie session at all still passes through: an API client posting with a
        // bearer token has no reason to know about CSRF.
        let anonymous = headers(&[("host", "lms.example")]);
        assert_eq!(
            decide(&Method::POST, &anonymous, &uri),
            Decision::Passthrough
        );
    }

    #[test]
    fn matching_csrf_headers_are_enough_for_an_unsafe_method() {
        let map = headers(&[
            ("cookie", &cookie(Some("csrf-1"))),
            ("x-csrf-token", "csrf-1"),
            ("host", "lms.example"),
        ]);
        assert_eq!(
            decide(&Method::PATCH, &map, &uri("/api/v1/courses/1")),
            Decision::Authenticate("jwt-here".to_string())
        );
    }

    #[test]
    fn a_foreign_origin_is_rejected() {
        for (name, value) in [
            ("origin", "https://evil.example"),
            ("referer", "https://evil.example/page"),
        ] {
            let map = headers(&[
                ("cookie", &cookie(None)),
                (name, value),
                ("host", "lms.example"),
            ]);
            assert!(
                matches!(
                    decide(&Method::GET, &map, &uri("/api/v1/courses")),
                    Decision::Reject(_)
                ),
                "{name}: {value}"
            );
        }
    }

    #[test]
    fn same_origin_claims_and_host_without_a_port_match() {
        let map = headers(&[
            ("cookie", &cookie(Some("c"))),
            ("x-csrf-token", "c"),
            ("origin", "http://lms.example:4000"),
            ("host", "lms.example:4000"),
        ]);
        assert_eq!(
            decide(&Method::POST, &map, &uri("/api/v1/x")),
            Decision::Authenticate("jwt-here".to_string())
        );
    }

    #[test]
    fn the_opaque_origin_is_not_treated_as_same_origin() {
        // `Origin: null` is what a sandboxed iframe or a file:// page sends. The bridge
        // rejects rather than guesses.
        let map = headers(&[
            ("cookie", &cookie(None)),
            ("origin", "null"),
            ("host", "lms.example"),
        ]);
        assert!(matches!(
            decide(&Method::GET, &map, &uri("/api/v1/x")),
            Decision::Reject(_)
        ));
    }

    #[test]
    fn multiple_cookie_headers_merge() {
        // RFC 6265 allows a client to send several Cookie headers; the bridge must find
        // the session in whichever one carries it.
        let map = headers(&[
            ("cookie", "other=1"),
            ("cookie", &format!("{SESSION_COOKIE}=jwt-split")),
            ("host", "lms.example"),
        ]);
        assert_eq!(
            decide(&Method::GET, &map, &uri("/api/v1/x")),
            Decision::Authenticate("jwt-split".to_string())
        );
    }
}
