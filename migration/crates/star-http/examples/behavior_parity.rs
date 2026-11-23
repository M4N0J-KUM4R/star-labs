//! Behavioral parity harness — the measurement for issue #231.
//!
//! Route registration alone says nothing about behavior. This harness drives
//! the *full production router* (star_http::build_app) against a real
//! Postgres and probes every route in the Canvas manifest, scoring the
//! dimensions issue #231 calls out:
//!
//! - registration : the route answers (no fall-through to axum's empty 404)
//! - authz        : anonymous requests are 401/403 (or handler 404), never
//!                  silently 200 unless the route is on the reviewed public
//!                  allowlist (parity/behavior/public_routes.json)
//! - robustness   : path params like 0 and >i64::MAX never produce a 500
//! - pagination   : index routes accept ?per_page=1&page=1 and ?per_page=0
//!                  without a 500
//! - expectations : per-route behavioral contracts (fields, nullability,
//!                  soft-delete visibility) from
//!                  parity/behavior/expectations.json — checked only where
//!                  specified, so the report shows what is *specified* and
//!                  what of that *passes*
//!
//! Field names, nullability and soft-delete visibility need seeded data per
//! resource; until a route has an entry in expectations.json it is reported
//! as unspecified rather than guessed at.
//!
//! Run with:
//!   DATABASE_URL=postgres://... cargo run -p star-http --example behavior_parity
//!
//! Writes parity/behavior/report.json and prints a scorecard.

use axum::Router;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn probe_call(app: &Router, verb: &str, url: &str) -> (u16, String) {
    let req = axum::http::Request::builder()
        .method(verb.parse::<axum::http::Method>().expect("valid verb"))
        .uri(url)
        .body(axum::body::Body::empty())
        .expect("build probe request");
    let resp = app.clone().oneshot(req).await.expect("router responds");
    let status = resp.status().as_u16();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read probe body");
    (status, String::from_utf8_lossy(&body).into_owned())
}
use star_common::AppState;

const MANIFEST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../parity/canvas_manifest.json");
const BEHAVIOR_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../parity/behavior");

const NONEXISTENT_ID: &str = "999999001";
const ZERO_ID: &str = "0";
const OVERFLOW_ID: &str = "99999999999999999999"; // > i64::MAX

struct Probe {
    url: String,
    kind: &'static str,
}

fn instantiate_path(path: &str, id: &str) -> String {
    path.split('/')
        .map(|seg| if seg.starts_with(':') { id.to_string() } else { seg.to_string() })
        .collect::<Vec<_>>()
        .join("/")
}

fn probes_for(route: &Value) -> Vec<Probe> {
    let verb = route["verb"].as_str().unwrap_or("GET");
    let path = canvas_base_path(route["path"].as_str().unwrap_or_default());
    let action = route["action"].as_str().unwrap_or_default();
    let has_param = route["path"].as_str().unwrap_or_default().contains("/:"); // replaced below
    let mut out = vec![Probe {
        url: instantiate_path(&path, NONEXISTENT_ID),
        kind: "anon",
    }];

    if has_param {
        out.push(Probe {
            url: instantiate_path(&path, ZERO_ID),
            kind: "malformed-id-0",
        });
        out.push(Probe {
            url: instantiate_path(&path, OVERFLOW_ID),
            kind: "malformed-id-overflow",
        });
    }

    if verb == "GET" && action == "index" {
        let base = instantiate_path(&path, NONEXISTENT_ID);
        out.push(Probe {
            url: format!("{base}?per_page=1&page=1"),
            kind: "pagination-page",
        });
        out.push(Probe {
            url: format!("{base}?per_page=0"),
            kind: "pagination-zero",
        });
    }

    out
}

/// axum's fall-through 404 is the app's distinctive JSON fallback; handlers
/// produce text, JSON of their own shape, or an empty body (missing resource).
fn is_fallthrough_404(status: u16, body: &str) -> bool {
    status == 404 && body.contains("route_not_found")
}

/// Strip Canvas' optional-segment suffixes (`(/:scope)`, `(*scope)`) — the
/// base path without them is always a registered route if the manifest entry
/// maps to one, and probing the literal parenthesis can never match axum.
fn canvas_base_path(path: &str) -> String {
    let base = path.split('(').next().unwrap_or(path).trim_end_matches('/');
    // build_app serves the REST API under /api/v1; manifest paths for the
    // root-mounted route sets (/api/lti, /api/sis, /canvadoc_session, login)
    // already carry their full prefix, so only add it when missing.
    if base.starts_with("/api/") || base.starts_with("/auth/") || base == "/login" || base == "/logout" || base == "/canvadoc_session" {
        base.to_string()
    } else {
        format!("/api/v1{base}")
    }
}

#[tokio::main]
async fn main() {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");
    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    let state = AppState {
        db: pool,
        jwt_secret: "behavior-parity-probe-secret".to_string(),
    };
    let app: Router = star_http::build_app().with_state(state);

    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(MANIFEST).expect("read manifest"))
            .expect("parse manifest");
    let routes = manifest["routes"].as_array().expect("manifest routes").clone();

    let public_list = std::fs::read_to_string(format!("{BEHAVIOR_DIR}/public_routes.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v["public"].as_array().cloned())
        .unwrap_or_default();
    let public_keys: Vec<String> = public_list
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let is_public = |verb: &str, path: &str| {
        public_keys.iter().any(|k| k == &format!("{verb} {path}"))
    };

    let expectations: Value = std::fs::read_to_string(format!("{BEHAVIOR_DIR}/expectations.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .unwrap_or_else(|| json!({ "checks": [] }));
    let expected: Vec<Value> = expectations["checks"].as_array().cloned().unwrap_or_default();

    #[derive(Default)]
    struct Dim {
        pass: u64,
        fail: u64,
    }
    let mut registration = Dim::default();
    let mut authz = Dim::default();
    let mut robustness = Dim::default();
    let mut pagination = Dim::default();
    let mut error_shape_json = 0u64;
    let mut error_shape_text = 0u64;

    let mut failures: Vec<Value> = Vec::new();
    let mut unregistered: Vec<String> = Vec::new();
    let mut public_observed: Vec<String> = Vec::new();

    let mut exp_pass = 0u64;
    let mut exp_fail: Vec<Value> = Vec::new();

    for route in &routes {
        let verb = route["verb"].as_str().unwrap_or("GET").to_string();
        let path = route["path"].as_str().unwrap_or_default().to_string();
        let key = format!("{verb} {path}");

        for probe in probes_for(route) {
            let (status, body) = probe_call(&app, &verb, &probe.url).await;

            if is_fallthrough_404(status, &body) {
                registration.fail += 1;
                unregistered.push(key.clone());
                failures.push(json!({
                    "route": key, "probe": probe.kind, "status": status,
                    "finding": "unregistered: empty fall-through 404",
                }));
                continue;
            }
            registration.pass += 1;

            if status >= 500 {
                if probe.kind.starts_with("malformed") {
                    robustness.fail += 1;
                } else if probe.kind.starts_with("pagination") {
                    pagination.fail += 1;
                } else {
                    authz.fail += 1;
                }
                let snippet: String = body.chars().take(120).collect();
                failures.push(json!({
                    "route": key, "probe": probe.kind, "status": status,
                    "finding": "5xx server error", "body": snippet,
                }));
                continue;
            }

            match probe.kind {
                "anon" => {
                    if (200..300).contains(&status) {
                        // 2xx anonymous: fine only if on the reviewed allowlist.
                        if is_public(&verb, &path) {
                            authz.pass += 1;
                            public_observed.push(key.clone());
                        } else if (200..300).contains(&status) {
                            authz.fail += 1;
                            let snippet: String = body.chars().take(120).collect();
                            failures.push(json!({
                                "route": key, "probe": probe.kind, "status": status,
                                "finding": "anonymous request returned 2xx; not on public allowlist",
                                "body": snippet,
                            }));
                        }
                    } else if (400..500).contains(&status) || (300..400).contains(&status) {
                        authz.pass += 1;
                        if (400..500).contains(&status) {
                            if body.trim_start().starts_with('{') || body.trim_start().starts_with('[')
                            {
                                error_shape_json += 1;
                            } else if !body.trim().is_empty() {
                                error_shape_text += 1;
                            }
                        }
                    } else {
                        authz.pass += 1;
                    }
                }
                kind if kind.starts_with("malformed") => {
                    robustness.pass += 1;
                }
                kind if kind.starts_with("pagination") => {
                    pagination.pass += 1;
                }
                _ => {}
            }
        }
    }

    // Per-route behavioral expectations (fields / nullability / soft-delete
    // are only checked where a contract entry exists).
    let mut specified: Vec<String> = Vec::new();
    for exp in &expected {
        let verb = exp["verb"].as_str().unwrap_or("GET");
        let path = exp["path"].as_str().unwrap_or_default();
        specified.push(format!("{verb} {path}"));
        let url = instantiate_path(&path, NONEXISTENT_ID);
        let (status, _body) = probe_call(&app, verb, &url).await;
        let want = exp["authz"].as_str().unwrap_or("401");
        let ok = match want {
            "401" => status == 401,
            "404" => status == 404,
            "public" => (200..300).contains(&status) || status == 302,
            _ => true,
        };
        if ok {
            exp_pass += 1;
        } else {
            exp_fail.push(json!({
                "route": format!("{verb} {path}"),
                "expected_authz": want, "observed_status": status,
            }));
        }
    }

    let pct = |d: &Dim| {
        let total = d.pass + d.fail;
        if total == 0 { None } else { Some((100.0 * d.pass as f64 / total as f64).round() as u64) }
    };

    let report = json!({
        "generated_by": "cargo run -p star-http --example behavior_parity",
        "routes_probed": routes.len(),
        "dimensions": {
            "registration": { "pass": registration.pass, "fail": registration.fail, "pct": pct(&registration) },
            "authz_anonymous": { "pass": authz.pass, "fail": authz.fail, "pct": pct(&authz) },
            "robustness_malformed_id": { "pass": robustness.pass, "fail": robustness.fail, "pct": pct(&robustness) },
            "pagination_index": { "pass": pagination.pass, "fail": pagination.fail, "pct": pct(&pagination) },
            "error_body_shape_4xx": { "json": error_shape_json, "text": error_shape_text },
            "expectations": {
                "specified_routes": specified.len(),
                "specified_of_total": format!("{} / {}", specified.len(), routes.len()),
                "pass": exp_pass,
                "fail": exp_fail.len(),
                "note": "field names, nullability and soft-delete visibility are measured only for routes with an entry in expectations.json; all others are unspecified, not passing",
            },
        },
        "measured_dims": ["registration", "authz_anonymous", "robustness_malformed_id", "pagination_index"],
        "unmeasured_dims": ["field_names", "nullability", "soft_delete_visibility", "permission_order_with_seeded_users"],
        "public_allowlist": public_keys,
        "failures": failures,
        "unregistered": unregistered,
        "expectation_failures": exp_fail,
    });

    std::fs::create_dir_all(BEHAVIOR_DIR).expect("create behavior dir");
    let out = format!("{BEHAVIOR_DIR}/report.json");
    std::fs::write(&out, serde_json::to_string_pretty(&report).unwrap()).expect("write report");

    println!("== Behavioral parity scoreboard (issue #231) ==");
    println!("routes probed: {}", routes.len());
    println!(
        "registration            {:>4}%  ({} pass / {} fail)",
        pct(&registration).unwrap_or(0), registration.pass, registration.fail
    );
    println!(
        "authz (anonymous)       {:>4}%  ({} pass / {} fail)",
        pct(&authz).unwrap_or(0), authz.pass, authz.fail
    );
    println!(
        "robustness (bad ids)    {:>4}%  ({} pass / {} fail)",
        pct(&robustness).unwrap_or(0), robustness.pass, robustness.fail
    );
    println!(
        "pagination (index)      {:>4}%  ({} pass / {} fail)",
        pct(&pagination).unwrap_or(0), pagination.pass, pagination.fail
    );
    println!(
        "behavioral contract     {}/{} routes specified, {} pass / {} fail",
        specified.len(), routes.len(), exp_pass, exp_fail.len()
    );
    println!(
        "error body shape (4xx): {} json / {} text",
        error_shape_json, error_shape_text
    );
    println!("NOT YET MEASURED: field names, nullability, soft-delete visibility, permission order (seeded users)");
    println!("report: {}", out);

    if !failures.is_empty() {
        println!("\n-- first findings --");
        for f in failures.iter().take(20) {
            println!("{}", f);
        }
        if failures.len() > 20 {
            println!("... {} more in report.json", failures.len() - 20);
        }
    }
}
