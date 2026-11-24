//! Embeds the built front end into the `star-ui` binary.
//!
//! Two inputs, both committed on this branch:
//!
//! * `migration/ui/build/` — the `adapter-static` output (`index.html` plus
//!   `_app/immutable/**`, whose filenames Vite already content-hashes), and
//! * `migration/ui/static/` — the vendored Canvas layer: the compiled bundles from
//!   `npm run styles:canvas`, the generated `tokens.css`, and the fonts and images
//!   those sheets reference (`npm run assets:canvas`).
//!
//! Embedding rather than `tower_http::ServeDir` keeps the property `assets.rs`
//! documents (spec §2.3): one binary, no filesystem dependency at runtime, no Node
//! process. The alternative would make every deployment a repo checkout.
//!
//! Each file also gets the first 8 hex chars of its sha256. Vite's own output needs
//! it not, but the Canvas sheets have fixed names — so the revision is what lets the
//! shell hand out a `max-age=31536000, immutable` URL that changes the moment a sheet
//! changes.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Extension → Content-Type. Unknown extensions abort the build: a wrong type on a
/// JS module is a silent "the page renders nothing" bug, not a visible one.
fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("<no extension>")
    {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "map" | "json" => "application/json; charset=utf-8",
        "html" => "text/html; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "txt" => "text/plain; charset=utf-8",
        other => panic!("no Content-Type for `.{other}` in {}", path.display()),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// The cache-busting revision: 32 bits of sha256, enough to make a changed file a
/// different URL and short enough to stay out of the way in a rendered tag.
fn revision(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_lower(&digest[..4])
}

/// Walk `root`, filing every file under the URL it will be served at.
fn collect(root: &Path, url_prefix: &str, out: &mut BTreeMap<String, (String, PathBuf)>) {
    if !root.is_dir() {
        return;
    }
    let entries = fs::read_dir(root).unwrap_or_else(|e| panic!("read_dir {}: {e}", root.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("readdir entry under {}: {e}", root.display()))
            .path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if path.is_dir() {
            collect(&path, &format!("{url_prefix}/{name}"), out);
            continue;
        }
        let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        out.insert(format!("{url_prefix}/{name}"), (revision(&bytes), path));
    }
}

/// `embed/shell-map.txt` → the shell's stylesheet table.
///
/// Malformed rows and unknown bundle names are build failures. A typo there reaches
/// the browser as a 404 `<link>` and shows up as "the dashboard looks slightly
/// wrong", which is the hardest class of CSS bug to find from a screenshot.
fn parse_shell_map(
    file: &Path,
    sheets: &BTreeMap<String, (String, PathBuf)>,
) -> (Vec<String>, Vec<(String, Vec<String>, Vec<String>)>) {
    let text = fs::read_to_string(file).unwrap_or_else(|e| {
        panic!(
            "read {}: {e} — the shell cannot pick a page's stylesheets without its table",
            file.display()
        )
    });
    let mut always: Vec<String> = Vec::new();
    let mut entries: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('|').map(str::trim);
        let (pattern, bundle_field, class_field) =
            match (fields.next(), fields.next(), fields.next()) {
                (Some(p), Some(b), Some(c)) => (p, b, c),
                _ => {
                    panic!("shell-map.txt:{line_no}: expected 3 `|`-separated fields, got {line:?}")
                }
            };
        let split = |field: &str| -> Vec<String> {
            field
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        };
        let (bundles, classes) = (split(bundle_field), split(class_field));

        for name in &bundles {
            for variant in ["normal_contrast", "high_contrast"] {
                let key = format!("/canvas/{variant}/{name}.css");
                if !sheets.contains_key(&key) {
                    panic!(
                        "shell-map.txt:{line_no}: bundle `{name}` has no compiled sheet at {key} \
                         — run `npm run styles:canvas` and `npm run tokens:canvas` in \
                         migration/ui, then commit static/canvas"
                    );
                }
            }
        }

        if pattern == "**" {
            always = bundles;
        } else {
            entries.push((pattern.to_string(), bundles, classes));
        }
    }

    if always.is_empty() {
        panic!("shell-map.txt: no `**` record — Canvas' common sheets (tokens, fonts, common) must load on every page");
    }
    (always, entries)
}

const GENERATED: &str = "// @generated by build.rs - the embedded front end. Do not edit.";
const CLOSE: &str = "];";

fn quoted(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("{item:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let ui = manifest.join("../../ui");
    let build_dir = ui.join("build");
    let static_dir = ui.join("static");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=embed/shell-map.txt");
    println!("cargo:rerun-if-changed={}", ui.join("build").display());
    println!("cargo:rerun-if-changed={}", static_dir.display());

    if !build_dir.join("index.html").is_file() || !build_dir.join("_app").is_dir() {
        panic!(
            "migration/ui/build/{{index.html,_app}} is missing. The bundle is embedded at \
             compile time, so it has to be built and committed: `cd migration/ui && npm ci \
             && npm run build`, then add build/index.html and build/_app/."
        );
    }
    if !static_dir.join("canvas").is_dir() {
        panic!(
            "migration/ui/static/canvas is missing. Generate it with `cd migration/ui && npm \
             run styles:canvas && npm run tokens:canvas && npm run assets:canvas` and commit it."
        );
    }

    // Keyed by the URL it is served at. `/_app/**` is Vite-hashed and served at its
    // literal name; the Canvas sheets keep theirs, because the `url()` references
    // inside them are rooted at /fonts and /images — renaming those files would mean
    // rewriting Canvas' own CSS.
    let mut assets: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
    collect(&build_dir.join("_app"), "/_app", &mut assets);
    for dir in ["canvas", "fonts", "images"] {
        collect(&static_dir.join(dir), &format!("/{dir}"), &mut assets);
    }
    for name in ["favicon.svg", "favicon.ico", "robots.txt"] {
        for root in [&static_dir, &build_dir] {
            let file = root.join(name);
            if file.is_file() && !assets.contains_key(&format!("/{name}")) {
                let bytes =
                    fs::read(&file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
                assets.insert(format!("/{name}"), (revision(&bytes), file));
            }
        }
    }

    let (always, entries) = parse_shell_map(&manifest.join("embed/shell-map.txt"), &assets);

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let mut out =
        fs::File::create(out_dir.join("embedded.rs")).expect("write $OUT_DIR/embedded.rs");

    writeln!(out, "{GENERATED}").unwrap();
    writeln!(
        out,
        "pub struct Asset {{\n    pub path: &'static str,\n    pub mime: &'static str,\n    pub rev: &'static str,\n    pub bytes: &'static [u8],\n}}\n"
    )
    .unwrap();
    writeln!(out, "pub static ASSETS: &[Asset] = &[").unwrap();
    for (url, (rev, file)) in &assets {
        writeln!(
            out,
            "    Asset {{\n        path: {:?},\n        mime: {:?},\n        rev: {:?},\n        bytes: include_bytes!({:?}),\n    }},",
            url,
            mime_for(file),
            rev,
            file.display().to_string()
        )
        .unwrap();
    }
    writeln!(out, "{CLOSE}\n").unwrap();

    writeln!(
        out,
        "pub struct ShellEntry {{\n    pub pattern: &'static str,\n    pub bundles: &'static [&'static str],\n    pub body_classes: &'static [&'static str],\n}}\n"
    )
    .unwrap();
    // `quoted`, not `{:?}`: a Vec's Debug prints `["a", "b"]`, which inside `&[...]`
    // nests an array and fails to compile against `&[&str]`.
    writeln!(
        out,
        "pub static SHELL_ALWAYS: &[&str] = &[{}];\n",
        quoted(&always)
    )
    .unwrap();
    writeln!(out, "pub static SHELL_ENTRIES: &[ShellEntry] = &[").unwrap();
    for (pattern, bundles, classes) in &entries {
        writeln!(
            out,
            "    ShellEntry {{ pattern: {:?}, bundles: &[{}], body_classes: &[{}] }},",
            pattern,
            quoted(bundles),
            quoted(classes)
        )
        .unwrap();
    }
    writeln!(out, "{CLOSE}").unwrap();
    writeln!(
        out,
        "\npub static SHELL_INDEX: &str = include_str!({:?});",
        build_dir.join("index.html").display().to_string()
    )
    .unwrap();
    writeln!(
        out,
        "\npub const EMBEDDED_ASSET_COUNT: usize = {};",
        assets.len()
    )
    .unwrap();
}
