//! The RFC-5988 `Link` header Canvas answers list requests with.
//!
//! Canvas builds it in `lib/api.rb:412-413` and
//! `lib/api.rb:497-544`:
//!
//! * the base URL is the request's own URL,
//! * every query parameter except `page`, `per_page`, `access_token` and
//!   `api_key` is carried over (`EXCLUDE_IN_PAGINATION_LINKS`),
//! * each link is `<url>; rel="<name>"`, and the names come out in the order
//!   `current`, `next`, `prev`, `first`, `last` (`PAGINATION_PARAMS`),
//! * the links are joined with a comma and no space.
//!
//! ```text
//! <http://canvas/api/v1/courses?page=1&per_page=10>; rel="current",\
//! <http://canvas/api/v1/courses?page=1&per_page=10>; rel="first",\
//! <http://canvas/api/v1/courses?page=1&per_page=10>; rel="last"
//! ```
//!
//! The page token is a page number for a normal collection and the literal
//! `first` for a bookmark-paged one
//! (`gems/bookmarked_collection/lib/bookmarked_collection/collection.rb:24`).

use std::collections::BTreeMap;

use crate::routes::Route;
use crate::RequestFacts;

/// Canvas' maximum page size (`lib/api.rb:23`).
const MAX_PER_PAGE: u32 = 100;

/// The query parameters Canvas leaves out of the links
/// (`lib/api.rb:511`).
const EXCLUDED_PARAMS: [&str; 4] = ["page", "per_page", "access_token", "api_key"];

/// The `Link` header for a list answer, if the request asks for one.
pub fn link_header(route: &Route, request: &RequestFacts) -> Option<String> {
    let params = parse_query(request.query.as_deref());

    let per_page = params
        .get("per_page")
        .and_then(|value| value.parse::<u32>().ok())
        .map(|value| value.clamp(1, MAX_PER_PAGE))
        .unwrap_or(route.per_page);

    // Canvas hands `params[:page]` to the collection; a bookmark-paged
    // collection answers the literal token `first` for its first page.
    let current = match params.get("page") {
        Some(page) => page.clone(),
        None if route.bookmarked => "first".to_string(),
        None => "1".to_string(),
    };

    let base = format!("http://{}{}", request.host, request.path);

    let mut links: Vec<(&str, String)> = Vec::new();
    links.push(("current", page_url(&base, &params, &current, per_page)));

    // Canvas only sends `next` and `prev` when there is another page. The
    // port does not count the rows behind a list answer, so it can only
    // offer a previous page when the request asked for a later one.
    if let Ok(page) = current.parse::<u32>() {
        if page > 1 {
            links.push((
                "prev",
                page_url(&base, &params, &(page - 1).to_string(), per_page),
            ));
        }
    }

    let first = if route.bookmarked { "first" } else { "1" };
    links.push(("first", page_url(&base, &params, first, per_page)));
    links.push(("last", page_url(&base, &params, &current, per_page)));

    Some(
        links
            .iter()
            .map(|(rel, url)| format!("<{url}>; rel=\"{rel}\""))
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// The URL of one page of a collection.
fn page_url(base: &str, params: &BTreeMap<String, String>, page: &str, per_page: u32) -> String {
    let mut url = String::from(base);
    url.push('?');
    for (key, value) in params {
        if EXCLUDED_PARAMS.contains(&key.as_str()) {
            continue;
        }
        url.push_str(&form_escape(key));
        url.push('=');
        url.push_str(&form_escape(value));
        url.push('&');
    }
    url.push_str("page=");
    url.push_str(&form_escape(page));
    url.push_str("&per_page=");
    url.push_str(&per_page.to_string());
    url
}

/// Parse a query string into its parameters.
fn parse_query(query: Option<&str>) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    let Some(query) = query else {
        return params;
    };
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        params.insert(form_unescape(key), form_unescape(value));
    }
    params
}

/// Percent-encode a query parameter the way CGI.escape does.
fn form_escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Reverse of [`form_escape`].
fn form_unescape(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
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
