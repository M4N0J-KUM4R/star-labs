//! Live parity tests are ignored by default; run explicitly with `-- --ignored` against a local service.
//! Live-API Canvas parity tests (files + dashboard).
//!
//! These run against a real star-lms server (Canvas-shaped API) and are
//! SKIPPED entirely unless STAR_PARITY_URL is set, e.g.:
//!
//! ```text
//! STAR_PARITY_URL=http://127.0.0.1:8080 cargo test -p star-files --test canvas_parity
//! ```
//!
//! Canvas has no single dashboard API (the dashboard is composed client-side
//! from /users/self/... + /courses/... endpoints); star-lms's GET /dashboard
//! is a pragmatic composition with three sections: upcoming, courses,
//! recent_announcements. Files parity follows Canvas attachment_json
//! (lib/api/v1/attachment.rb): display_name defaults to filename when not
//! provided (attachment.rb infer_display_name).
//!
//! HTTP is done with a ~40-line std-only HTTP/1.1 client (TcpStream +
//! `Connection: close`) so the test adds no new dependencies.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn base_url() -> Option<(String, u16, String)> {
    let raw = std::env::var("STAR_PARITY_URL").ok()?;
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return None;
    }
    // Accept "host[:port]" or "http(s)://host[:port]".
    let after_scheme = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .unwrap_or(raw);
    let (host, port) = match after_scheme.split_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse().expect("STAR_PARITY_URL port must be numeric"),
        ),
        None => (after_scheme.to_string(), 80),
    };
    Some((host, port, raw.to_string()))
}

/// Minimal blocking HTTP/1.1 client — JSON in, JSON out.
struct Client {
    host: String,
    port: u16,
    token: String,
}

struct Response {
    status: u16,
    body: Value,
}

impl Client {
    fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Response, String> {
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{payload}",
            host = self.host,
            port = self.port,
            token = self.token,
            len = payload.len(),
        );

        let mut stream = TcpStream::connect((self.host.as_str(), self.port)).map_err(|e| {
            format!(
                "connect {host}:{port} failed: {e}",
                host = self.host,
                port = self.port
            )
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .map_err(|e| e.to_string())?;
        stream
            .write_all(req.as_bytes())
            .map_err(|e| format!("{method} {path} write failed: {e}"))?;

        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|e| format!("{method} {path} read failed: {e}"))?;

        let text = String::from_utf8_lossy(&raw);
        // Status line + headers, then body (split on first blank line).
        let (head, body_text) = text.split_once("\r\n\r\n").unwrap_or((text.as_ref(), ""));
        let status_line = head.lines().next().unwrap_or("");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("{method} {path}: bad status line '{status_line}'"))?;

        // Chunked transfer bodies are reassembled if present (Connection: close
        // means EOF-terminated; handle chunked just in case).
        let body_text = if head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
        {
            dechunk(body_text)
        } else {
            body_text.to_string()
        };

        let parsed: Value = if body_text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(body_text.trim())
                .map_err(|e| format!("{method} {path}: body parse failed: {e}: {body_text}"))?
        };
        Ok(Response {
            status,
            body: parsed,
        })
    }

    fn get(&self, path: &str) -> Result<Response, String> {
        self.request("GET", path, None)
    }

    fn post(&self, path: &str, body: Value) -> Result<Response, String> {
        self.request("POST", path, Some(body))
    }

    fn login(_base: &str, email: &str, password: &str) -> Result<Self, String> {
        let (host, port, _) = base_url().ok_or_else(|| "STAR_PARITY_URL unset".to_string())?;
        let anon = Client {
            host,
            port,
            token: String::new(),
        };
        let res = anon.request(
            "POST",
            "/api/v1/auth/login",
            Some(json!({ "email": email, "password": password })),
        )?;
        if res.status != 200 {
            return Err(format!("login returned {}: {}", res.status, res.body));
        }
        let token = res.body["token"]
            .as_str()
            .ok_or_else(|| format!("no token in login response: {}", res.body))?
            .to_string();
        let (host, port, _) = base_url().unwrap();
        Ok(Client { host, port, token })
    }
}

fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size_line, remainder)) = rest.split_once("\r\n") {
        let size =
            match usize::from_str_radix(size_line.trim().split(';').next().unwrap_or("0"), 16) {
                Ok(s) => s,
                Err(_) => break,
            };
        if size == 0 {
            break;
        }
        let end = remainder.len().min(size);
        out.push_str(&remainder[..end]);
        rest = &remainder[end..];
        rest = rest.strip_prefix("\r\n").unwrap_or(rest);
    }
    out
}

/// Admin user on the live deployment.
const ADMIN_EMAIL: &str = "admin@star.edu";

fn expect_client() -> Client {
    let (host, port, _) = base_url().expect("STAR_PARITY_URL not set");
    match Client::login(&format!("{host}:{port}"), ADMIN_EMAIL, &parity_password()) {
        Ok(c) => c,
        Err(e) => panic!("server at {host}:{port} unreachable or login failed: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Files parity: display_name defaults to filename (Canvas infer_display_name)
// ---------------------------------------------------------------------------

fn parity_password() -> String {
    std::env::var("STAR_ADMIN_PASSWORD")
        .expect("STAR_ADMIN_PASSWORD is required for explicit live parity runs")
}

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn file_metadata_create_defaults_display_name_to_filename() {
    let Some(_) = base_url() else {
        eprintln!("skipping: STAR_PARITY_URL not set");
        return;
    };
    let client = expect_client();

    // Pick an existing course; admin can act on any course.
    let res = client
        .get("/api/v1/courses")
        .unwrap_or_else(|e| panic!("list courses failed: {e}"));
    let courses = res
        .body
        .as_array()
        .or_else(|| res.body["courses"].as_array())
        .expect("courses list should be an array");
    assert!(
        !courses.is_empty(),
        "need at least one course for the file test"
    );
    let course_id = courses[0]["id"].as_i64().expect("course id");

    // Create file metadata WITHOUT display_name -> must default to filename
    // (Canvas attachment.rb: self.display_name ||= unencoded_filename).
    let created = client
        .post(
            &format!("/api/v1/courses/{course_id}/files"),
            json!({
                "filename": "parity-test-display-name.txt",
                "content_type": "text/plain",
                "size": 42
            }),
        )
        .unwrap_or_else(|e| panic!("create file metadata failed: {e}"));
    assert_eq!(
        created.status, 201,
        "create file should return 201, got {}: {}",
        created.status, created.body
    );

    assert!(
        created.body["id"].as_i64().is_some(),
        "created file should have an id, got: {}",
        created.body
    );
    let display_name = created.body["display_name"]
        .as_str()
        .expect("response must include display_name");
    assert_eq!(
        display_name, "parity-test-display-name.txt",
        "display_name should default to filename (Canvas infer_display_name); got: {}",
        created.body
    );
    // Canvas File model fields that must be present in the response.
    for field in ["filename", "content_type", "size", "folder"] {
        assert!(
            !created.body[field].is_null(),
            "created file missing '{field}': {}",
            created.body
        );
    }

    // GET /files/{id} must also surface display_name.
    let id = created.body["id"].as_i64().unwrap();
    let fetched = client
        .get(&format!("/api/v1/files/{id}"))
        .unwrap_or_else(|e| panic!("get file failed: {e}"));
    assert_eq!(fetched.status, 200, "GET file status: {}", fetched.status);
    assert_eq!(
        fetched.body["display_name"].as_str(),
        Some("parity-test-display-name.txt"),
        "GET /files/:id display_name mismatch: {}",
        fetched.body
    );
}

// ---------------------------------------------------------------------------
// Dashboard parity: 3 sections with expected keys
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires STAR_API_BASE/STAR_PARITY_API_URL pointing to a local migration service"]
fn dashboard_returns_three_sections_with_expected_keys() {
    let Some(_) = base_url() else {
        eprintln!("skipping: STAR_PARITY_URL not set");
        return;
    };
    let client = expect_client();

    let dash = client
        .get("/api/v1/dashboard")
        .unwrap_or_else(|e| panic!("GET /dashboard failed: {e}"));
    assert_eq!(
        dash.status, 200,
        "dashboard status {}: {}",
        dash.status, dash.body
    );

    // The three pragmatic-composition sections must all be present arrays.
    for section in ["upcoming", "courses", "recent_announcements"] {
        let items = dash.body[section]
            .as_array()
            .unwrap_or_else(|| panic!("dashboard.{section} must be an array, got: {}", dash.body));
        eprintln!("dashboard.{section}: {} items", items.len());
    }

    // Section element shapes (Canvas-informed; courses includes workflow_state).
    if let Some(items) = dash.body["upcoming"].as_array() {
        for item in items {
            for key in ["id", "title", "type", "date", "course_name"] {
                assert!(
                    item.get(key).is_some_and(|v| !v.is_null()),
                    "upcoming item missing '{key}': {item}"
                );
            }
        }
    }
    if let Some(courses) = dash.body["courses"].as_array() {
        for course in courses {
            for key in ["id", "name", "course_code", "role", "workflow_state"] {
                assert!(
                    course.get(key).is_some_and(|v| !v.is_null()),
                    "dashboard course missing '{key}': {course}"
                );
            }
        }
    }
    if let Some(anns) = dash.body["recent_announcements"].as_array() {
        for ann in anns {
            for key in [
                "id",
                "course_id",
                "course_name",
                "title",
                "message",
                "created_at",
            ] {
                assert!(
                    ann.get(key).is_some_and(|v| !v.is_null()),
                    "recent_announcements item missing '{key}': {ann}"
                );
            }
        }
    }
}
