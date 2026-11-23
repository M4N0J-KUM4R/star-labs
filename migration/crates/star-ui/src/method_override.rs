//! `Rack::MethodOverride` for the server-rendered shell.
//!
//! HTML forms can only submit GET and POST, so Rails' form helpers
//! (`form_for …, method: :patch`) emit `POST <member path>` with a hidden
//! `_method=patch` field, and `Rack::MethodOverride` — which sits in
//! Canvas' middleware stack (`config/application.rb`, the default Rails
//! stack) — rewrites `REQUEST_METHOD` from that field *before routing*, then
//! deletes the field from the params so controllers never see it. That is why
//! Canvas' route table only has `PATCH`/`PUT`/`DELETE` for `resources`
//! updates and destroys, with no POST twin.
//!
//! Without this shim the shell's write forms had to be registered on made-up
//! paths (the parity report read `POST /courses/{id}/assignments/{aid}/edit`
//! as a verb Canvas does not answer there). With it, forms post to the path
//! Rails draws and land on the verb Rails draws.
//!
//! Deviations from Rack, both deliberate:
//!
//! * Only `PATCH`, `PUT` and `DELETE` are honoured. Rack accepts any verb
//!   string; a shell that let a form pick `GET` would turn authenticated
//!   reads into a CSRF-free write-shaped request, and no Canvas form needs
//!   anything beyond these three.
//! * Only `application/x-www-form-urlencoded` POST bodies are inspected,
//!   which is exactly Rack's own condition (`content_type =~ urlencoded`), so
//!   multipart uploads and JSON API calls are never buffered.

use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, TRANSFER_ENCODING};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Same ceiling as the body extractors downstream (`axum`'s default 2 MiB for
/// `Form`/`Bytes`), so buffering here cannot accept a body the handler would
/// have refused.
const MAX_BODY: usize = 2 * 1024 * 1024;

/// The form field Rails' helpers write and Rack reads.
const METHOD_FIELD: &str = "_method";

fn is_urlencoded(content_type: Option<&HeaderValue>) -> bool {
    let Some(Ok(value)) = content_type.map(|v| v.to_str()) else {
        return false;
    };
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("application/x-www-form-urlencoded")
}

/// Rewrite a form POST's method from its `_method` field, then route.
///
/// Applied by `star_ui::app` to the shell's whole-router service, so it runs
/// before any path or verb is matched — which is the whole point: the verb the
/// route table carries is the verb the request ends up matching. Layering it
/// with `Router::layer` instead would be too late (see `app`).
pub async fn method_override(req: Request, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();

    if parts.method != Method::POST || !is_urlencoded(parts.headers.get(CONTENT_TYPE)) {
        return next.run(Request::from_parts(parts, body)).await;
    }

    let raw = match to_bytes(body, MAX_BODY).await {
        Ok(raw) => raw,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                "request body is larger than the shell's form limit",
            )
                .into_response();
        }
    };

    let Some(method) = override_method(&raw) else {
        set_body_length(&mut parts.headers, raw.len());
        return next.run(Request::from_parts(parts, Body::from(raw))).await;
    };

    let encoded = encode_without_method(&raw);
    parts.method = method;
    set_body_length(&mut parts.headers, encoded.len());

    next.run(Request::from_parts(parts, Body::from(encoded)))
        .await
}

/// The rebuilt body has a known length, which replaces whatever framing the
/// streamed request carried.
fn set_body_length(headers: &mut HeaderMap, len: usize) {
    headers.remove(TRANSFER_ENCODING);
    headers.insert(CONTENT_LENGTH, HeaderValue::from(len));
}

/// The verb a body asks for, or `None` when it carries no usable `_method`.
///
/// Last value wins, matching Rack's `params.delete("_method")` on a hash.
fn override_method(raw: &[u8]) -> Option<Method> {
    let value = url::form_urlencoded::parse(raw)
        .filter(|(key, _)| &key[..] == METHOD_FIELD)
        .map(|(_, value)| value.into_owned())
        .last()?;
    match value.to_ascii_uppercase().as_str() {
        "PATCH" => Some(Method::PATCH),
        "PUT" => Some(Method::PUT),
        "DELETE" => Some(Method::DELETE),
        _ => None,
    }
}

/// Re-encode the form pairs minus `_method`, so handlers never see the field
/// — Rack removes it from params for the same reason. Key order and repeated
/// keys (Rails' `[]` array params) are preserved.
fn encode_without_method(raw: &[u8]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in url::form_urlencoded::parse(raw) {
        if key == METHOD_FIELD {
            continue;
        }
        serializer.append_pair(&key, &value);
    }
    serializer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_values_are_the_three_write_verbs() {
        assert_eq!(override_method(b"_method=patch").unwrap(), Method::PATCH);
        assert_eq!(override_method(b"_method=PUT").unwrap(), Method::PUT);
        assert_eq!(
            override_method(b"a=1&_method=delete&b=2").unwrap(),
            Method::DELETE
        );
        // Anything else — including verbs Rack would accept but this port
        // deliberately refuses — leaves the request alone.
        assert!(override_method(b"_method=get").is_none());
        assert!(override_method(b"_method=head").is_none());
        assert!(override_method(b"name=_method").is_none());
        // Last value wins, like Rack's params hash.
        assert_eq!(
            override_method(b"_method=put&_method=delete").unwrap(),
            Method::DELETE
        );
    }

    #[test]
    fn method_field_is_stripped_but_everything_else_survives() {
        let out = encode_without_method(
            b"_method=patch&name=Project+One&due_at%5B%5D=1&due_at%5B%5D=2&_csrf=abc",
        );
        assert_eq!(
            out,
            "name=Project+One&due_at%5B%5D=1&due_at%5B%5D=2&_csrf=abc"
        );
        // A body that is only `_method` re-encodes to empty, not to `_method=`.
        assert_eq!(encode_without_method(b"_method=patch"), "");
    }

    // The routing half of the contract — a POST with `_method=patch` reaching
    // the PATCH handler, and a POST without it staying a POST — is covered
    // end-to-end in tests/ui_integration.rs, because only `star_ui::app()` has
    // the Canvas route table and the override ahead of it to match against.
}
