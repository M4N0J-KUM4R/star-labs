//! The document the browser boots the Svelte app with.
//!
//! Canvas renders its outer document server-side — `layouts/_head.html.erb` puts
//! `window.ENV` and the stylesheets in `<head>`, and
//! `application.html.erb:83-110` computes the `<body>` classes. This module does the
//! same three jobs for the client-rendered pages: it takes the `index.html` that
//! `adapter-static` emitted (embedded by `build.rs`, so the boot script, the
//! modulepreload list and the hashed entry filenames stay SvelteKit's business) and
//! injects the per-request parts into it.
//!
//! Injecting rather than maintaining a parallel shell template is deliberate: the
//! bundle's entry filenames change on every rebuild, and a hand-written shell would
//! silently point at an entry that no longer exists.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::assets::canvas_href;
use crate::embed;

/// Which contrast sheet set the page gets.
///
/// Mirrors `js_env`'s `use_high_contrast` (application_controller.rb:292) and the
/// variant directories `scripts/build-canvas-css.sh` compiles from
/// `variants/new_styles_high_contrast`.
pub fn variant(high_contrast: bool) -> &'static str {
    if high_contrast {
        "high_contrast"
    } else {
        "normal_contrast"
    }
}

/// The `embed/shell-map.txt` record for a path.
///
/// Most literal segments wins, then the longer pattern, so `/courses/42/assignments`
/// takes the assignments row over the `/courses/*` row. `/courses/42/1a`-style
/// nonsense matches the `/courses/*` row, which is fine: the row only chooses
/// stylesheets, and the handler has already decided the request is worth answering.
pub fn match_shell(path: &str) -> Option<&'static embed::ShellEntry> {
    let request: Vec<&str> = path.trim_matches('/').split('/').collect();
    let score = |entry: &embed::ShellEntry| -> Option<(usize, usize)> {
        let pattern: Vec<&str> = entry.pattern.trim_matches('/').split('/').collect();
        if pattern.len() != request.len() {
            return None;
        }
        let mut literals = 0;
        for (want, got) in pattern.iter().zip(&request) {
            if *want == "*" {
                continue;
            }
            if want != got {
                return None;
            }
            literals += 1;
        }
        Some((literals, pattern.len()))
    };

    let mut best: Option<((usize, usize), &embed::ShellEntry)> = None;
    for entry in embed::SHELL_ENTRIES {
        let Some(s) = score(entry) else { continue };
        if best.is_none_or(|(previous, _)| s > previous) {
            best = Some((s, entry));
        }
    }
    best.map(|(_, entry)| entry)
}

/// Escape text for an HTML text node. Titles come from course and user names, which
/// are user-supplied, so this is not optional.
fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// Break `</script>` sequences inside the ENV JSON.
///
/// `serde_json` does not escape a forward slash, so a course named `</script><img
/// src=x onerror=…>` would close the script tag and run. `<\/` is the same string in
/// JSON, which is why this fix costs nothing: the JSON stays valid and the HTML stops
/// being parseable as a script terminator. (The `notices` and `title` fields come from
/// the database, so the threat is user input reaching a `<script>` block — the same
/// reason Canvas' `render_js_env` goes through `to_json` in a `javascript_tag`.)
fn json_for_script_block(json: &str) -> String {
    json.replace("</", "<\\/")
}

/// The `<head>` additions for one request: ENV, the title, and the stylesheets Canvas
/// would have emitted for this page.
fn head_fragment(path: &str, title: &str, env_json: &str, high_contrast: bool) -> String {
    let variant = variant(high_contrast);
    let mut out = String::from("<title>");
    out.push_str(&escape_text(title));
    out.push_str("</title><script>window.ENV = ");
    out.push_str(&json_for_script_block(env_json));
    out.push_str(";</script>");

    let page_bundles: &[&str] = match_shell(path).map_or(&[], |entry| entry.bundles);
    let mut emitted: Vec<&str> = Vec::new();
    for name in embed::SHELL_ALWAYS
        .iter()
        .chain(page_bundles.iter())
        .copied()
    {
        if emitted.contains(&name) {
            continue;
        }
        emitted.push(name);
        out.push_str(&format!(
            "<link rel=\"stylesheet\" href=\"{}\" data-canvas-bundle=\"{}\">",
            canvas_href(variant, name),
            escape_text(name)
        ));
    }
    out
}

/// Build the document. `path` selects the stylesheets and body classes, `title` and
/// `env_json` come from the handler, and the two accessibility flags are the same
/// values that go into ENV — the browser must not have to run JS to learn them.
pub fn render(
    path: &str,
    title: &str,
    env_json: &str,
    high_contrast: bool,
    dyslexic: bool,
) -> String {
    let mut html = embed::SHELL_INDEX.to_string();

    // Injected at the top of `<head>`, ahead of `<meta charset>`. That is safe because
    // the response's own `Content-Type: text/html; charset=utf-8` outranks the meta
    // (shell::response), and it keeps the sheet order Canvas uses: its stylesheets
    // precede the app's, so a project-authored rule wins over a Canvas one only where
    // we wrote it on purpose — which `src/app.css` says is three declarations.
    html = html.replacen(
        "<head>",
        &format!(
            "<head>{}",
            head_fragment(path, title, env_json, high_contrast)
        ),
        1,
    );

    // `<html lang="en">` gains the attributes +layout.svelte would otherwise toggle
    // after boot. Doing it here is what removes the high-contrast/dyslexic first-paint
    // flash, and the client toggling them again is idempotent.
    let mut root_attrs = String::new();
    if high_contrast {
        root_attrs.push_str(" data-use-high-contrast");
    }
    if dyslexic {
        root_attrs.push_str(" data-use-dyslexic-font");
    }
    if !root_attrs.is_empty() {
        html = html.replacen("<html", &format!("<html{root_attrs}"), 1);
    }

    let classes = match_shell(path)
        .map(|entry| entry.body_classes.join(" "))
        .unwrap_or_default();
    if !classes.is_empty() {
        html = html.replacen("<body", &format!("<body class=\"{classes}\""), 1);
    }
    html
}

/// A shell response. `Cache-Control` is `no-store` for the same reason the Askama
/// pages carry it (see `handlers::login::login_page_is_no_store`): the document holds
/// `window.ENV` for one user's session, so a shared cache must not keep it.
pub fn response(html: String) -> Response {
    (
        StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                axum::http::header::CACHE_CONTROL,
                "no-cache, no-store, must-revalidate",
            ),
        ],
        html,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_specific_row_beats_the_prefix_row() {
        assert_eq!(
            match_shell("/courses/42/assignments").map(|e| e.pattern),
            Some("/courses/*/assignments")
        );
        assert_eq!(
            match_shell("/courses/42").map(|e| e.pattern),
            Some("/courses/*")
        );
        assert_eq!(match_shell("/profile").map(|e| e.pattern), Some("/profile"));
        assert_eq!(
            match_shell("/login").map(|e| e.body_classes.join(" ")),
            Some("full-width ic-Login-Body".to_string())
        );
        // A path with no row still gets the common sheets, and no body classes.
        assert!(match_shell("/conversations/7").is_none());
        assert!(match_shell("/").is_none());
    }

    #[test]
    fn a_star_matches_one_segment_only() {
        assert_eq!(
            match_shell("/reset_password/abc").map(|e| e.pattern),
            Some("/reset_password/*")
        );
        assert!(match_shell("/reset_password/abc/edit").is_none());
    }

    #[test]
    fn env_json_cannot_close_its_own_script_tag() {
        let doc = render(
            "/profile",
            "Profile",
            r#"{"current_user_id":1,"notices":["</script><img src=x onerror=alert(1)>"]}"#,
            false,
            false,
        );
        assert!(
            !doc.contains("</script><img"),
            "escaped ENV must not terminate the block"
        );
        assert!(
            doc.contains("<\\/script>"),
            "the escape must stay valid JSON"
        );
    }

    #[test]
    fn the_title_is_escaped() {
        let doc = render("/profile", "A & B <c>", "{}", false, false);
        assert!(doc.contains("<title>A &amp; B &lt;c&gt;</title>"));
    }

    #[test]
    fn common_sheets_come_first_and_the_page_bundle_after() {
        let doc = render("/dashboard", "Dashboard", "{}", false, false);
        let at = |needle: &str| doc.find(needle).unwrap_or(usize::MAX);
        let tokens = at("data-canvas-bundle=\"tokens\"");
        let fonts = at("data-canvas-bundle=\"fonts\"");
        let common = at("data-canvas-bundle=\"common\"");
        let dash = at("data-canvas-bundle=\"dashboard\"");
        assert!(tokens < fonts && fonts < common && common < dash);
        // Cascade order is decided by document order, so a repeated bundle would make
        // the winner depend on which row listed it first.
        assert_eq!(doc.matches("data-canvas-bundle=\"common\"").count(), 1);
    }

    #[test]
    fn the_high_contrast_request_gets_the_other_directory() {
        let normal = render("/profile", "Profile", "{}", false, false);
        let contrast = render("/profile", "Profile", "{}", true, false);
        assert!(normal.contains("/normal_contrast/"));
        assert!(!normal.contains("/high_contrast/"));
        assert!(contrast.contains("/high_contrast/"));
        assert!(contrast.contains("<html data-use-high-contrast"));
    }

    #[test]
    fn the_body_class_is_on_the_opening_tag() {
        let doc = render("/dashboard", "Dashboard", "{}", false, false);
        // `with-right-side` is `shell-map.txt`'s own column for this route, because
        // Canvas' layout adds it when a page provides :right_side
        // (app/views/users/user_dashboard.html.erb:84 -> layouts/application.html.erb:94)
        // and it is the only thing that un-hides #right-side-wrapper.
        assert!(
            doc.contains(
                "<body class=\"primary-nav-expanded with-right-side\" data-sveltekit-preload-data"
            ),
            "dashboard body tag: {:?}",
            doc.lines().find(|line| line.contains("<body"))
        );
        // Every sheet link is versioned, which is what lets the handler cache it forever.
        assert!(
            doc.matches("/canvas/").count() >= 4,
            "expected several versioned canvas links"
        );
    }

    #[test]
    fn the_injection_anchors_exist_in_the_built_document() {
        // render() replaces three anchors. If a SvelteKit upgrade renames one, the
        // `replacen` calls match nothing and the page silently loses its title, its
        // ENV and every Canvas stylesheet — a 200 with unstyled text, not an error.
        assert!(embed::SHELL_INDEX.contains("<head>"));
        assert!(embed::SHELL_INDEX.contains("<html"));
        assert!(embed::SHELL_INDEX.contains("<body"));
    }

    #[test]
    fn the_sveltekit_boot_document_survives_injection() {
        // The bundle mounts through the script adapter-static emitted; if the injection
        // ever drops it the page is blank with a 200 and no error anywhere.
        let doc = render("/profile", "Profile", "{}", false, true);
        assert!(doc.contains("kit.start(app, element)"));
        assert!(doc.contains("/_app/immutable/entry/start."));
        assert!(doc.contains("data-use-dyslexic-font"));
    }
}
