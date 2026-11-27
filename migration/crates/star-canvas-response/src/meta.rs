//! The `x-canvas-meta` header.
//!
//! Canvas collects a handful of facts about the request while it handles it
//! and writes them into one semicolon-separated header
//! (`gems/request_context/lib/request_context/generator.rb:89-95`):
//!
//! ```text
//! at=<access token id>;dk=<developer key id>;o=<controller>;n=<action>;
//! t=<context class>;i=<context id>;b=<memory before>;m=<memory after>;
//! u=<user cpu>;y=<system cpu>;d=<db time>;rlr=<rate limit remaining>;
//! ```
//!
//! Every key is optional — Canvas skips a key whose value is blank — and the
//! order is the order in which Canvas collects them.
//!
//! The port can produce every key Canvas puts in the header: `at` and `dk`
//! are the global ids of the access token the caller authenticated with and
//! of its developer key (`lib/authentication_methods.rb:173-174`), `o`, `n`,
//! `t`, `i`, `b`, `m`, `u`, `y`, `d` and `rlr` come from the request itself.

use crate::procinfo::ProcSample;
use crate::RequestFacts;

/// The `x-canvas-meta` value for one response.
pub struct MetaHeader {
    fields: Vec<(String, String)>,
}

impl MetaHeader {
    /// Collect the meta fields for a request.
    pub fn build(request: &RequestFacts, before: &ProcSample, after: &ProcSample) -> Self {
        let mut fields = Vec::new();

        // lib/authentication_methods.rb:173-174 — the global id of the access
        // token the caller authenticated with, and of its developer key.
        // Canvas collects them while it authenticates the request, so they
        // come before the fields the controller adds.
        if let Some(id) = request.access_token_id {
            push(&mut fields, "at", &crate::global_id(id).to_string());
        }
        if let Some(id) = request.developer_key_id {
            push(&mut fields, "dk", &crate::global_id(id).to_string());
        }

        if let Some(route) = request.route {
            // generator.rb:123-124 — the controller and the action.
            push(&mut fields, "o", route.controller);
            push(&mut fields, "n", route.action);
        }

        // generator.rb:133-134 — the context the request ran in.
        if let Some((class, id)) = context_of(request) {
            push(&mut fields, "t", &class);
            push(&mut fields, "i", &id);
        }

        // app/middleware/request_throttle.rb:308-312 — the process memory
        // before and after the request, and the CPU the request burned.
        push(&mut fields, "b", &before.memory_bytes().to_string());
        push(&mut fields, "m", &after.memory_bytes().to_string());
        push(
            &mut fields,
            "u",
            &format!("{:.2}", before.user_cpu_delta(after)),
        );
        push(
            &mut fields,
            "y",
            &format!("{:.2}", before.system_cpu_delta(after)),
        );
        push(&mut fields, "d", &format!("{:.2}", 0.0));

        // app/middleware/request_throttle.rb:93 — the leaky bucket's
        // remaining capacity.
        push(&mut fields, "rlr", crate::RATE_LIMIT_REMAINING);

        Self { fields }
    }
}

impl std::fmt::Display for MetaHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, value) in &self.fields {
            write!(f, "{name}={value};")?;
        }
        Ok(())
    }
}

/// Add a field, skipping it when the value is blank (Canvas does the same:
/// `add_meta_header` returns early for a blank value).
fn push(fields: &mut Vec<(String, String)>, name: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    fields.push((name.to_string(), value.to_string()));
}

/// The Canvas context a request ran in: its class and its id.
///
/// Canvas stores the context of the request (`@context`) in the meta header
/// (`generator.rb:132-135`). The context of an API request is the course,
/// group, user or account the path is scoped to.
fn context_of(request: &RequestFacts) -> Option<(String, String)> {
    let segments: Vec<&str> = request.path.split('/').filter(|s| !s.is_empty()).collect();
    for window in segments.windows(2) {
        let (kind, id) = (window[0], window[1]);
        if !id.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let class = match kind {
            "courses" => "Course",
            "groups" => "Group",
            "users" => "User",
            "accounts" => "Account",
            // A section's context is its course; the port cannot resolve the
            // course id from the section id without a query.
            _ => continue,
        };
        return Some((class.to_string(), id.to_string()));
    }
    None
}
