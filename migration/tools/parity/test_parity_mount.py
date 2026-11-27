#!/usr/bin/env python3
"""Tests for mount-aware classification in parity.py (repo task #30).

`web_*` vs `api_*` is decided by where the axum router is ACTUALLY mounted —
learned by walking the composition in star-http's `main()` through every
`.merge(...)` / `.nest(...)` / `.fallback_service(...)` and router-returning
tail calls — not by the shape of the path string. Each test pins one rule; a
future route-table edit must not be able to flip a page into an API extra
(or an API endpoint into a web port) without editing these tests.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import parity  # noqa: E402

# 9 real `.route(...)` calls: 1 in main(), 2 in the page shell, 5 in
# api-mounted routers, 1 in the root-merged login family.
MAIN_RS = '''
use axum::{Router, routing::get};

fn main() {
    let api_router = build_api_router();
    let app = Router::new()
        .route("/health", get(health))
        // Rails' pages: the shell is the fallback, NOT under /api/v1.
        .fallback_service(star_ui::app(state.clone()))
        .merge(star_auth::login_router())
        .nest("/api/v1", api_router.layer(noop_middleware))
        .nest("/api/graphql", star_graphql::router())
        .with_state(state);
}

fn build_api_router() -> Router<AppState> {
    Router::new()
        .merge(star_things::router())
        .merge(star_pages_api::router())
        .merge(star_tail::router())
}

async fn health() {}
'''

# star-ui is the fallback_service web router; its asset-style comment below
# literally says `.route()` and must never count as a registration (that
# comment is the one unparsed call parity reported on main, line 325).
UI_LIB_RS = '''
use axum::{Router, routing::get};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", get(login_page))
        .route("/courses/{id}/grades", get(grades_page))
        // assets are served by a fallback, not `.route()` calls.
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .fallback_service(router().with_state(state))
}

async fn login_page() {}
async fn grades_page() {}
'''

THINGS_LIB_RS = '''
use axum::{Router, routing::get};

mod things_api;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(things_api::router())
        .route("/widgets/{id}", get(show_widget))
}

async fn show_widget() {}
'''

THINGS_API_RS = '''
use axum::{Router, routing::get};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gadgets", get(list_gadgets))
}

async fn list_gadgets() {}
'''

# The conferences-crate shape: the crate-root builder is a plain tail call
# into the module that owns the routes; the mount must still propagate.
TAIL_LIB_RS = '''
use axum::Router;

mod tail_api;

pub fn router() -> Router<AppState> {
    tail_api::router()
}
'''

TAIL_API_RS = '''
use axum::{Router, routing::get};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tail_thing", get(show_tail))
}

async fn show_tail() {}
'''

AUTH_LIB_RS = '''
use axum::{Router, routing::post};

pub fn login_router() -> Router<AppState> {
    Router::new()
        .route("/login/oauth2/token", post(oauth2_token))
}

async fn oauth2_token() {}
'''

GRAPHQL_LIB_RS = '''
use axum::{Router, routing::post};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/query", post(graphql))
}

async fn graphql() {}
'''

PAGES_API_RS = '''
use axum::{Router, routing::get};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users/self/profile", get(profile))
}

async fn profile() {}
'''

FILES = {
    "crates/star-http/src/main.rs": MAIN_RS,
    "crates/star-ui/src/lib.rs": UI_LIB_RS,
    "crates/star-things/src/lib.rs": THINGS_LIB_RS,
    "crates/star-things/src/things_api.rs": THINGS_API_RS,
    "crates/star-tail/src/lib.rs": TAIL_LIB_RS,
    "crates/star-tail/src/tail_api.rs": TAIL_API_RS,
    "crates/star-auth/src/lib.rs": AUTH_LIB_RS,
    "crates/star-graphql/src/lib.rs": GRAPHQL_LIB_RS,
    "crates/star-pages-api/src/lib.rs": PAGES_API_RS,
}


def write_tree(root: Path, main_rs=None):
    files = dict(FILES)
    if main_rs is not None:
        files["crates/star-http/src/main.rs"] = main_rs
    for rel, body in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)


def api_route(verb, path):
    return {"verb": verb, "path": path,
            "normalized_path": parity.normalize_path(path),
            "controller": None, "action": None}


def extras_for(root: Path, canvas_routes, web_routes):
    """Same pipeline as build_coverage: parse, subtract, classify."""
    star_routes, calls, unparsed, _tests, _unreach = \
        parity.parse_star_routes(root)
    star_keys = {(r["verb"], r["normalized_path"]) for r in star_routes}
    canvas_keys = {(r["verb"], r["normalized_path"]) for r in canvas_routes}
    extras = [dict(r) for r in star_routes
              if (r["verb"], r["normalized_path"]) not in canvas_keys]
    counts = parity.classify_extras(extras, canvas_routes, web_routes,
                                    star_keys)
    return star_routes, calls, unparsed, extras, counts


def mounts(routes):
    return {(r["verb"], r["path"]): r["mount"] for r in routes}


def test_mounts_follow_the_real_composition(tmp_path):
    write_tree(tmp_path)
    routes, calls, unparsed, _extras, _counts = extras_for(tmp_path, [], [])
    assert calls == 9 and unparsed == 0
    m = mounts(routes)
    # nest("/api/v1", build_api_router) -> merge(star_things::router) ->
    # merge(things_api::router): direct and transitive registrations are
    # api-mounted even when the path string looks like a page or a plain
    # noun (the `PUT /assignments/{id}` mislabel this task fixes).
    assert m[("GET", "/widgets/{id}")] == "api"
    assert m[("GET", "/gadgets")] == "api"
    # ... including through a tail-call builder (the star-conferences shape).
    assert m[("GET", "/tail_thing")] == "api"
    assert m[("GET", "/users/self/profile")] == "api"
    # The fallback shell, the root-merged login family, the /api/graphql
    # nest, and main()'s own .route() are all on the web (top-level) side.
    assert m[("GET", "/login")] == "web"
    assert m[("GET", "/courses/{id}/grades")] == "web"
    assert m[("POST", "/login/oauth2/token")] == "web"
    assert m[("POST", "/query")] == "web"
    assert m[("GET", "/health")] == "web"


def test_comment_mentioning_route_is_not_a_registration(tmp_path):
    write_tree(tmp_path)
    routes, calls, unparsed, _e, _c = extras_for(tmp_path, [], [])
    assert unparsed == 0
    assert calls == 9  # the comment's `.route()` adds neither a call nor an unparsed
    assert all(r["mount"] in ("api", "web") for r in routes)


def test_page_is_never_an_api_extra(tmp_path):
    write_tree(tmp_path)
    # Trap for the old path-shape logic: the API table has the page's path
    # with a different verb, so a shape-first classifier scored the page as
    # API surface. Mount-based classification asks the web table instead.
    canvas = [api_route("PUT", "/widgets/{id}")]
    web = [api_route("GET", "/login"), api_route("GET", "/courses/{id}/grades")]
    routes, _calls, _unparsed, extras, counts = extras_for(tmp_path, canvas,
                                                           web)
    grades = next(e for e in extras if e["path"] == "/courses/{id}/grades")
    assert grades["classification"] == "web_exact"
    assert counts["web_exact"] == 2
    # the API-mounted widgets row is the only api-side verb gap
    assert counts["api_verb_mismatch"] == 1
    widget = next(e for e in extras if e["path"] == "/widgets/{id}")
    assert widget["classification"] == "api_verb_mismatch"


def test_api_endpoint_is_never_a_web_port(tmp_path):
    write_tree(tmp_path)
    # The mirror trap: `/gadgets` exists in Canvas' WEB table, but star
    # registers it inside the /api/v1 chain, so it must be scored against
    # the API table (here: no Canvas API path at all -> absent), never
    # `web_exact` the way the old shape-first check said.
    canvas = [api_route("GET", "/widgets/{id}")]
    web = [api_route("GET", "/gadgets"), api_route("GET", "/login")]
    _routes, _calls, _unparsed, extras, _counts = extras_for(tmp_path, canvas,
                                                             web)
    gadgets = next(e for e in extras if e["path"] == "/gadgets")
    assert gadgets["classification"] == "absent"
    login = next(e for e in extras if e["path"] == "/login")
    assert login["classification"] == "web_exact"


def test_self_alias_handling_survives_the_mount_split(tmp_path):
    write_tree(tmp_path)
    # `/users/self/profile` is api-mounted; the alias logic is untouched.
    canvas = [api_route("GET", "/api/v1/users/{user_id}/profile")]
    _routes, _calls, _unparsed, extras, counts = extras_for(tmp_path, canvas,
                                                            [])
    assert counts["api_alias"] == 1
    alias = next(e for e in extras if e["path"] == "/users/self/profile")
    assert alias["classification"] == "api_alias"


def test_unmounted_router_fails_loudly(tmp_path):
    write_tree(tmp_path)
    orphan = tmp_path / "crates/star-orphan/src/lib.rs"
    orphan.parent.mkdir(parents=True)
    orphan.write_text('''
use axum::{Router, routing::get};

pub fn orphan_router() -> Router<AppState> {
    Router::new()
        .route("/orphans", get(list_orphans))
}

async fn list_orphans() {}
''')
    try:
        parity.parse_star_routes(tmp_path)
    except SystemExit as error:
        assert "star-orphan" in str(error) and "orphan_router" in str(error)
    else:
        raise AssertionError("an unmounted router must fail the parse loudly")


def test_double_mount_fails_loudly(tmp_path):
    # star_things::router() merged at the root AND nested under /api/v1:
    # the same registrations would answer on both sides, so the mount is
    # genuinely ambiguous and the tool must not guess.
    double = MAIN_RS.replace(
        '.merge(star_auth::login_router())',
        '.merge(star_auth::login_router())\n        .merge(star_things::router())')
    write_tree(tmp_path, main_rs=double)
    try:
        parity.parse_star_routes(tmp_path)
    except SystemExit as error:
        assert "two different mounts" in str(error)
    else:
        raise AssertionError("an ambiguous mount must fail the parse loudly")
