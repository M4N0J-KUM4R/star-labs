//! Matching a request onto the Canvas route it would have hit.
//!
//! The route table in [`crate::routes`] comes from Canvas' own route
//! registry (`migration/parity/canvas_manifest.json`), so a path like
//! `/api/v1/courses/1/users` resolves to the Canvas route
//! `/api/v1/courses/:course_id/users` and therefore to the controller and
//! action Canvas would have run.
//!
//! The match is deliberately the same one Canvas' router makes: the number of
//! path segments has to be equal, and every literal segment has to be equal.
//! When more than one template matches, the one with the most literal
//! segments wins — `/api/v1/users/self/groups` beats `/api/v1/users/:id/groups`.

use crate::routes::{Route, ROUTES};

/// The Canvas route a request would have hit, if the port knows it.
pub fn match_route(verb: &str, path: &str) -> Option<&'static Route> {
    if let Some(route) = best_match(verb, path, false) {
        return Some(route);
    }
    // A few recorded Canvas paths are stored without the `/api/v1` prefix.
    // Fall back to matching the remainder against the API routes.
    let rest = path.strip_prefix("/api/v1")?;
    best_match(verb, rest, true)
}

/// The best route for a request out of the route table.
///
/// `strip_api_prefix` drops the `/api/v1` prefix from the templates, for
/// requests that were recorded without it.
fn best_match(verb: &str, path: &str, strip_api_prefix: bool) -> Option<&'static Route> {
    let request_segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut best: Option<&Route> = None;
    let mut best_literals = 0usize;
    let mut best_params = usize::MAX;

    for route in ROUTES {
        if route.verb != verb {
            continue;
        }
        let template = if strip_api_prefix {
            route.path.strip_prefix("/api/v1").unwrap_or(route.path)
        } else {
            route.path
        };
        let template_segments: Vec<&str> = template.split('/').filter(|s| !s.is_empty()).collect();
        if template_segments.len() != request_segments.len() {
            continue;
        }
        let mut literals = 0usize;
        let mut params = 0usize;
        let mut matches = true;
        for (actual, template_segment) in request_segments.iter().zip(template_segments.iter()) {
            if template_segment.starts_with(':') || template_segment.starts_with('*') {
                params += 1;
                continue;
            }
            literals += 1;
            if actual != template_segment {
                matches = false;
                break;
            }
        }
        if !matches {
            continue;
        }
        if literals > best_literals || (literals == best_literals && params < best_params) {
            best = Some(route);
            best_literals = literals;
            best_params = params;
        }
    }
    best
}
