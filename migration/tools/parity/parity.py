#!/usr/bin/env python3
"""Canvas -> Rust parity scoreboard.

Two deterministic subcommands:

    manifest  extract Canvas API routes and source-size counts from a
              canvas-lms checkout into canvas_manifest.json
    report    compare the Rust workspace's axum routes against that manifest
              and write coverage.json, report.md, baseline.json

Stdlib only. No network, no timestamps, no absolute paths in the output.
"""

import argparse
import json
import re
import sys
from pathlib import Path

DRAW_RE = re.compile(r"^\s*ApiRouteSet(?:::V1)?\.draw\(self(?:,\s*\"([^\"]+)\")?\)\s+do\b")
VERB_RE = re.compile(r"^\s*(get|post|put|delete|patch|head|options|match)\b")
BLOCK_RE = re.compile(r"\bdo\b(?:\s*\|[^|]*\|)?\s*$")
COND_RE = re.compile(r"^\s*(if|unless|case|begin|while|until)\b")
END_RE = re.compile(r"^\s*end\b")
ASSIGN_RE = re.compile(r"^\s*([a-z_][a-z0-9_]*)\s*=\s*\"([^\"]*)\"\s*$")
SCOPE_CTRL_RE = re.compile(r"controller:\s*(?::([A-Za-z0-9_]+)|[\"']([A-Za-z0-9_/]+)[\"'])")
ACTION_RE = re.compile(r"action:\s*(?::([A-Za-z0-9_]+)|[\"']([A-Za-z0-9_]+)[\"'])")
TO_RE = re.compile(r"to:\s*[\"']([A-Za-z0-9_/]+)#([A-Za-z0-9_]+)[\"']")
STR_RE = re.compile(r"[\"']([^\"']*)[\"']")
STAR_METHOD_RE = re.compile(r"\b(get|post|put|delete|patch|head|options)\s*\(")
JS_ENV_RE = re.compile(r"\bjs_env\b")
UI_API_RE = re.compile(r"[\"'`](/api/v1/[^\"'`\s]*)")


# ---------------------------------------------------------------------------
# Path normalization
# ---------------------------------------------------------------------------

def normalize_path(path):
    """Strip /api/v1, collapse :params / {params} / interpolations to {}."""
    p = path.strip().split("?")[0]
    p = re.sub(r"^https?://[^/]+", "", p)
    p = re.sub(r"^/api/v1(?=/|$)", "", p)
    p = re.sub(r"[#$]\{[^}]*\}", ":__param__", p)
    p = re.sub(r"#.*$", "", p)  # URI fragment, after #{} interpolation is gone
    p = re.sub(r"\{[^/{}]*\}", ":__param__", p)
    p = re.sub(r":[A-Za-z_][A-Za-z0-9_]*", "{}", p)
    p = p.replace(":{}", "{}")  # :#{...} became ::__param__
    p = re.sub(r"\([^)]*\)", "", p)  # ruby glob fragments such as (/*scope)
    p = re.sub(r"/{2,}", "/", p)
    return p.strip("/")


def normalize_ui_reference(path):
    """UI literals often embed concrete ids; treat digit-only segments as params."""
    p = normalize_path(path)
    p = re.sub(r"(?<=/)\d+(?=/|$)", "{}", p)
    return p


# ---------------------------------------------------------------------------
# Canvas routes.rb parsing
# ---------------------------------------------------------------------------

def _draw_prefix(line):
    m = DRAW_RE.match(line)
    if not m:
        return None
    return m.group(1) or "/api/v1"


def _needs_continuation(text):
    if text.rstrip().endswith(","):
        return True
    depth = 0
    quote = None
    escaped = False
    for ch in text:
        if quote:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == quote:
                quote = None
        elif ch in "\"'":
            quote = ch
        elif ch in "({[":
            depth += 1
        elif ch in ")}]":
            depth -= 1
    return depth > 0


def _join_statement(lines, index):
    """Join a Ruby route declaration that continues over several lines."""
    text = lines[index].rstrip("\n")
    while _needs_continuation(text) and index + 1 < len(lines):
        index += 1
        text += " " + lines[index].strip()
    return text, index


def parse_canvas_routes(routes_file):
    lines = routes_file.read_text(encoding="utf-8", errors="replace").splitlines()
    routes = []
    i = 0
    while i < len(lines):
        prefix = _draw_prefix(lines[i])
        if prefix is None:
            i += 1
            continue

        start_indent = len(lines[i]) - len(lines[i].lstrip())
        stack = [(start_indent, None, {})]  # (indent, controller, path vars)
        i += 1
        while i < len(lines):
            line = lines[i]
            stripped = line.strip()
            indent = len(line) - len(line.lstrip())

            if END_RE.match(line):
                if indent <= start_indent:
                    break
                while len(stack) > 1 and stack[-1][0] >= indent:
                    stack.pop()
                i += 1
                continue

            if not stripped or stripped.startswith("#"):
                i += 1
                continue

            if VERB_RE.match(line):
                statement, i = _join_statement(lines, i)
                i += 1
                routes.extend(
                    _parse_route_statement(statement, prefix, stack))
                continue

            assign = ASSIGN_RE.match(line)
            if assign:
                stack[-1][2][assign.group(1)] = assign.group(2)
                i += 1
                continue

            if BLOCK_RE.search(line) or COND_RE.match(line):
                ctrl = None
                m = SCOPE_CTRL_RE.search(line)
                if m:
                    ctrl = m.group(1) or m.group(2)
                stack.append((indent, ctrl, dict(stack[-1][2])))
            i += 1
        i += 1

    return routes


def _parse_route_statement(statement, prefix, stack):
    m = VERB_RE.match(statement)
    if not m:
        return []
    token = m.group(1)
    if token == "match":
        via = re.search(r"via:\s*\[([^\]]*)\]", statement)
        verbs = re.findall(r":?([A-Za-z]+)", via.group(1)) if via else []
        verbs = [v.upper() for v in verbs] or ["MATCH"]
    else:
        verbs = [token.upper()]

    rest = statement[m.end():].lstrip()
    if rest[:1] in ("\"", "'"):
        sm = STR_RE.match(rest)
        path = sm.group(1) if sm else None
    else:
        var = re.match(r"([A-Za-z_][A-Za-z0-9_]*)(?:\.to_s)?\s*[,)]", rest)
        path = stack[-1][2].get(var.group(1)) if var else None
    if not path:
        return []

    controller = None
    cm = re.search(r"controller:\s*(?::([A-Za-z0-9_]+)|[\"']([A-Za-z0-9_/]+)[\"'])",
                   statement)
    if cm:
        controller = cm.group(1) or cm.group(2)
    if controller is None:
        for _, ctrl, _ in reversed(stack):
            if ctrl:
                controller = ctrl
                break

    am = ACTION_RE.search(statement)
    action = (am.group(1) or am.group(2)) if am else None
    if action is None or controller is None:
        tm = TO_RE.search(statement)
        if tm:
            controller = controller or tm.group(1)
            action = action or tm.group(2)

    if path.startswith("/"):
        original = path
    else:
        original = prefix.rstrip("/") + "/" + path.lstrip("/")
    original = re.sub(r"/{2,}", "/", original)

    return [
        {
            "verb": verb,
            "path": original,
            "normalized_path": normalize_path(original),
            "controller": controller,
            "action": action,
        }
        for verb in verbs
    ]


# ---------------------------------------------------------------------------
# Canvas source-size counts and UI references
# ---------------------------------------------------------------------------

def _walk_files(root, suffixes):
    if not root.is_dir():
        return []
    found = []
    for path in root.rglob("*"):
        if path.is_file() and path.suffix in suffixes:
            found.append(path)
    return sorted(found)


def count_canvas_sources(canvas_src):
    graphql = _walk_files(canvas_src / "app" / "graphql", {".rb"})
    mutations = _walk_files(canvas_src / "app" / "graphql" / "mutations", {".rb"})
    types = [p for p in graphql if p.name.endswith("_type.rb")]

    features_dir = canvas_src / "ui" / "features"
    feature_dirs = sorted(p for p in features_dir.iterdir() if p.is_dir()) \
        if features_dir.is_dir() else []

    ui_files = [
        p for p in _walk_files(canvas_src / "ui", {".js", ".jsx", ".ts", ".tsx"})
        if "__tests__" not in p.parts and not p.name.endswith(".d.ts")
    ]
    erb_views = _walk_files(canvas_src / "app" / "views", {".erb"})

    js_env_files = 0
    js_env_sites = 0
    for top in ("app", "ui", "lib", "config"):
        for path in _walk_files(canvas_src / top, {".rb", ".erb", ".js", ".jsx",
                                                  ".ts", ".tsx"}):
            text = path.read_text(encoding="utf-8", errors="replace")
            sites = len(JS_ENV_RE.findall(text))
            if sites:
                js_env_files += 1
                js_env_sites += sites

    return {
        "graphql_files": len(graphql),
        "graphql_types": len(types),
        "graphql_mutations": len(mutations),
        "models": len(_walk_files(canvas_src / "app" / "models", {".rb"})),
        "ui_feature_dirs": len(feature_dirs),
        "js_tsx_files": len(ui_files),
        "erb_views": len(erb_views),
        "js_env_files": js_env_files,
        "js_env_call_sites": js_env_sites,
    }


def scan_ui_api_references(canvas_src):
    refs = set()
    literal_count = 0
    for path in _walk_files(canvas_src / "ui", {".js", ".jsx", ".ts", ".tsx"}):
        text = path.read_text(encoding="utf-8", errors="replace")
        for match in UI_API_RE.finditer(text):
            literal_count += 1
            ref = normalize_ui_reference(match.group(1))
            if ref:
                refs.add(ref)
    return refs, literal_count


def route_referenced(normalized_path, ui_refs):
    if normalized_path in ui_refs:
        return True
    tail = normalized_path + "/"
    return any(ref.startswith(tail) and
               all(seg == "{}" for seg in ref[len(tail):].split("/") if seg)
               for ref in ui_refs)


# ---------------------------------------------------------------------------
# Star (Rust) parsing
# ---------------------------------------------------------------------------

def _extract_route_calls(text):
    """Yield the argument text of every `.route(...)` call, paren-balanced."""
    pos = 0
    while True:
        start = text.find(".route(", pos)
        if start < 0:
            break
        i = start + len(".route(")
        depth = 1
        in_string = False
        escaped = False
        while i < len(text) and depth:
            ch = text[i]
            if in_string:
                if escaped:
                    escaped = False
                elif ch == "\\":
                    escaped = True
                elif ch == '"':
                    in_string = False
            elif ch == '"':
                in_string = True
            elif ch == "(":
                depth += 1
            elif ch == ")":
                depth -= 1
            i += 1
        yield text[start + len(".route("):i - 1]
        pos = i


def parse_star_routes(star_src):
    routes = []
    calls = 0
    unparsed = 0
    for path in sorted((star_src / "crates").glob("*/src/*.rs")):
        crate = path.parent.parent.name
        text = path.read_text(encoding="utf-8", errors="replace")
        for args in _extract_route_calls(text):
            calls += 1
            sm = STR_RE.search(args)
            if not sm:
                unparsed += 1
                continue
            route_path = sm.group(1)
            methods = STAR_METHOD_RE.findall(args[sm.end():])
            if not methods:
                unparsed += 1
                continue
            for method in methods:
                routes.append({
                    "verb": method.upper(),
                    "path": route_path,
                    "normalized_path": normalize_path(route_path),
                    "crate": crate,
                    "source": str(path.relative_to(star_src)),
                })
    return sorted(routes, key=lambda r: (r["normalized_path"], r["verb"],
                                         r["crate"])), calls, unparsed


def count_star_sources(star_src):
    impl = {
        "graphql_files": 0,
        "ui_feature_dirs": 0,
        "js_tsx_files": 0,
        "erb_views": 0,
        "js_env_files": 0,
    }
    for path in star_src.rglob("*"):
        if not path.is_file():
            continue
        if path.suffix in {".graphql", ".graphqls"}:
            impl["graphql_files"] += 1
        elif path.suffix == ".erb":
            impl["erb_views"] += 1
        elif path.suffix in {".js", ".jsx", ".ts", ".tsx"}:
            impl["js_tsx_files"] += 1
        elif path.name == "Cargo.toml" and "async-graphql" in path.read_text(
                encoding="utf-8", errors="replace"):
            impl["graphql_files"] += 1
    features = star_src / "ui" / "features"
    if features.is_dir():
        impl["ui_feature_dirs"] = sum(1 for p in features.iterdir() if p.is_dir())
    if (star_src / "ui").is_dir():
        for path in _walk_files(star_src / "ui", {".rb", ".erb", ".js", ".jsx",
                                                  ".ts", ".tsx"}):
            if JS_ENV_RE.search(path.read_text(encoding="utf-8", errors="replace")):
                impl["js_env_files"] += 1

    tables = set()
    migrations = star_src / "crates" / "star-db" / "migrations"
    for path in sorted(migrations.glob("*.sql")) if migrations.is_dir() else []:
        text = path.read_text(encoding="utf-8", errors="replace")
        tables.update(m.group(1).lower() for m in re.finditer(
            r"CREATE TABLE(?:\s+IF NOT EXISTS)?\s+([A-Za-z_][A-Za-z0-9_]*)",
            text, re.IGNORECASE))
    impl["tables"] = len(tables)
    return impl


# ---------------------------------------------------------------------------
# Percent helper
# ---------------------------------------------------------------------------

def pct(part, total):
    return round(100.0 * part / total, 1) if total else 0.0


def fmt_pct(part, total):
    return "%.1f%%" % pct(part, total)


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n",
                    encoding="utf-8")


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# manifest
# ---------------------------------------------------------------------------

def build_manifest(canvas_src):
    routes_file = canvas_src / "config" / "routes.rb"
    if not routes_file.is_file():
        raise SystemExit("not a Canvas checkout: %s" % routes_file)

    routes = parse_canvas_routes(routes_file)
    ui_refs, ui_literals = scan_ui_api_references(canvas_src)
    for route in routes:
        route["ui_referenced"] = route_referenced(route["normalized_path"], ui_refs)

    counts = count_canvas_sources(canvas_src)
    counts["api_routes"] = len(routes)
    counts["controllers"] = len({r["controller"] for r in routes if r["controller"]})
    counts["ui_api_ref_strings"] = ui_literals
    counts["ui_api_ref_paths"] = len(ui_refs)

    return {
        "source": "canvas-lms config/routes.rb (ApiRouteSet blocks)",
        "normalization": "strip /api/v1; :param, {param}, #{...} -> {}",
        "routes": routes,
        "counts": counts,
    }


def cmd_manifest(args):
    manifest = build_manifest(Path(args.canvas_src).resolve())
    write_json(Path(args.out), manifest)
    counts = manifest["counts"]
    print("canvas routes: %d (%d controllers)" %
          (counts["api_routes"], counts["controllers"]))
    print("ui-referenced: %d routes from %d /api/v1 literals" %
          (sum(1 for r in manifest["routes"] if r["ui_referenced"]),
           counts["ui_api_ref_strings"]))
    print("wrote %s" % args.out)


# ---------------------------------------------------------------------------
# report
# ---------------------------------------------------------------------------

def build_coverage(manifest, star_src):
    canvas_routes = manifest["routes"]
    star_routes, calls, unparsed = parse_star_routes(star_src)
    star_keys = {(r["verb"], r["normalized_path"]) for r in star_routes}

    implemented = [r for r in canvas_routes if (r["verb"], r["normalized_path"]) in star_keys]
    ui_routes = [r for r in canvas_routes if r["ui_referenced"]]
    ui_implemented = [r for r in ui_routes if (r["verb"], r["normalized_path"]) in star_keys]

    canvas_keys = {(r["verb"], r["normalized_path"]) for r in canvas_routes}
    extras = [r for r in star_routes if (r["verb"], r["normalized_path"]) not in canvas_keys]

    controllers = {}
    for route in canvas_routes:
        ctrl = route["controller"] or "(unknown)"
        entry = controllers.setdefault(ctrl, {"total": 0, "implemented": 0})
        entry["total"] += 1
        if (route["verb"], route["normalized_path"]) in star_keys:
            entry["implemented"] += 1
    controllers = {
        name: {
            "total": data["total"],
            "implemented": data["implemented"],
            "pct": pct(data["implemented"], data["total"]),
        }
        for name, data in controllers.items() if data["implemented"] > 0
    }

    star_impl = count_star_sources(star_src)
    counts = manifest["counts"]

    return {
        "summary": {
            "canvas_api_routes": len(canvas_routes),
            "canvas_controllers": counts["controllers"],
            "implemented_rest_routes": len(implemented),
            "implemented_rest_pct": pct(len(implemented), len(canvas_routes)),
            "ui_referenced_routes": len(ui_routes),
            "ui_referenced_implemented": len(ui_implemented),
            "ui_referenced_pct": pct(len(ui_implemented), len(ui_routes)),
            "star_route_registrations": len(star_routes),
            "star_route_calls": calls,
            "star_unparsed_calls": unparsed,
            "star_extras": len(extras),
            "graphql_files": counts["graphql_files"],
            "graphql_files_implemented": star_impl["graphql_files"],
            "graphql_types": counts["graphql_types"],
            "graphql_types_implemented": 0,
            "graphql_mutations": counts["graphql_mutations"],
            "graphql_mutations_implemented": 0,
            "ui_feature_dirs": counts["ui_feature_dirs"],
            "ui_feature_dirs_implemented": star_impl["ui_feature_dirs"],
            "js_tsx_files": counts["js_tsx_files"],
            "js_tsx_files_implemented": star_impl["js_tsx_files"],
            "erb_views": counts["erb_views"],
            "erb_views_implemented": star_impl["erb_views"],
            "js_env_files": counts["js_env_files"],
            "js_env_files_implemented": star_impl["js_env_files"],
            "js_env_call_sites": counts["js_env_call_sites"],
            "canvas_models": counts["models"],
            "star_tables": star_impl["tables"],
        },
        "controllers": controllers,
        "implemented_routes": implemented,
        "unmatched_star_routes": extras,
    }


def render_report(coverage):
    s = coverage["summary"]
    lines = []
    add = lines.append
    add("# Canvas to Rust parity scoreboard")
    add("")
    add("Measured, deterministic output. Canvas source is not committed; refresh "
        "the manifest locally with `tools/parity/parity.py manifest`.")
    add("")
    add("## Summary")
    add("")
    add("| Area | Implemented | Total | Coverage |")
    add("|---|---:|---:|---:|")
    rows = [
        ("REST API routes", s["implemented_rest_routes"], s["canvas_api_routes"]),
        ("REST routes referenced from `ui/`", s["ui_referenced_implemented"],
         s["ui_referenced_routes"]),
        ("GraphQL files", s["graphql_files_implemented"], s["graphql_files"]),
        ("GraphQL object types", s["graphql_types_implemented"], s["graphql_types"]),
        ("GraphQL mutations", s["graphql_mutations_implemented"],
         s["graphql_mutations"]),
        ("UI feature directories", s["ui_feature_dirs_implemented"],
         s["ui_feature_dirs"]),
        ("JS/JSX/TS/TSX files", s["js_tsx_files_implemented"], s["js_tsx_files"]),
        ("ERB views", s["erb_views_implemented"], s["erb_views"]),
        ("js_env files", s["js_env_files_implemented"], s["js_env_files"]),
    ]
    for label, part, total in rows:
        add("| %s | %d | %d | %s |" % (label, part, total, fmt_pct(part, total)))
    add("| Schema tables (vs %d Canvas models) | %d | %d | n/a |" %
        (s["canvas_models"], s["star_tables"], s["canvas_models"]))
    add("")
    add("Implemented REST route count: **%d / %d** (%s)." %
        (s["implemented_rest_routes"], s["canvas_api_routes"],
         fmt_pct(s["implemented_rest_routes"], s["canvas_api_routes"])))
    add("")
    add("Star registrations: %d route/method pairs from %d `.route(...)` calls "
        "(%d unparsed); %d have no Canvas match and are listed as extras below." %
        (s["star_route_registrations"], s["star_route_calls"],
         s["star_unparsed_calls"], s["star_extras"]))
    add("")
    add("Canvas source sizes: %d GraphQL files (%d object types, %d mutations), "
        "%d models, %d UI feature dirs, %d JS/TSX files, %d ERB views, "
        "%d js_env files (%d call sites)." %
        (s["graphql_files"], s["graphql_types"], s["graphql_mutations"],
         s["canvas_models"], s["ui_feature_dirs"], s["js_tsx_files"],
         s["erb_views"], s["js_env_files"], s["js_env_call_sites"]))
    add("")

    ctrls = sorted(coverage["controllers"].items(),
                   key=lambda kv: (-kv[1]["implemented"], kv[0]))
    add("## Controllers with coverage (%d)" % len(ctrls))
    add("")
    add("| Controller | Implemented | Total | Coverage |")
    add("|---|---:|---:|---:|")
    for name, data in ctrls:
        add("| `%s` | %d | %d | %s |" %
            (name, data["implemented"], data["total"],
             fmt_pct(data["implemented"], data["total"])))
    add("")

    extras = coverage["unmatched_star_routes"]
    add("## Star routes with no Canvas match (%d)" % len(extras))
    add("")
    add("| Verb | Path | Crate |")
    add("|---|---|---|")
    for route in extras:
        add("| %s | `%s` | `%s` |" %
            (route["verb"], route["path"], route["crate"]))
    add("")
    add("## Parser scope and limitations")
    add("")
    add("- Canvas routes are explicit verb declarations inside `ApiRouteSet.draw` "
        "blocks; `resources` expansions are not enumerated.")
    add("- Ruby interpolation (`#{...}`) and dynamic prefixes are normalized to "
        "`{}` and cannot match concrete Rust paths.")
    add("- UI references are `/api/v1/...` string literals in `ui/`; runtime-built "
        "paths and hard-coded ids that cannot be normalized are missed.")
    add("- Rust paths are matched by `(verb, normalized path)`; axum's "
        "`nest(\"/api/v1\", ...)` mount is assumed for all crate routers.")
    add("- Extras include deliberate flat-path deviations (for example "
        "`/assignments/{id}` where Canvas uses "
        "`/courses/{course_id}/assignments/{id}`) and Canvas routes generated "
        "by `resources`, which are not enumerated.")
    add("- The schema row counts `CREATE TABLE` statements in `star-db` migrations "
        "(36 authored tables); sqlx adds `_sqlx_migrations`, so a live database "
        "has 37 tables.")
    add("")
    return "\n".join(lines)


def cmd_report(args):
    manifest = read_json(Path(args.manifest))
    star_src = Path(args.star_src).resolve()
    out_dir = Path(args.out)

    coverage = build_coverage(manifest, star_src)
    current = coverage["summary"]["implemented_rest_routes"]

    baseline_path = out_dir / "baseline.json"
    previous = read_json(baseline_path)["implemented_rest_routes"] \
        if baseline_path.is_file() else None

    write_json(out_dir / "coverage.json", coverage)
    (out_dir / "report.md").write_text(render_report(coverage), encoding="utf-8")

    regressed = previous is not None and current < previous
    if not regressed:
        write_json(baseline_path, {
            "implemented_rest_routes": current,
            "policy": "report fails when implemented REST routes fall below this number",
        })

    s = coverage["summary"]
    print("REST: %d/%d (%s)" % (current, s["canvas_api_routes"],
                                fmt_pct(current, s["canvas_api_routes"])))
    print("UI-referenced: %d/%d (%s)" %
          (s["ui_referenced_implemented"], s["ui_referenced_routes"],
           fmt_pct(s["ui_referenced_implemented"], s["ui_referenced_routes"])))
    print("star registrations: %d (%d extras)" %
          (s["star_route_registrations"], s["star_extras"]))
    print("wrote %s" % out_dir)

    if regressed:
        print("parity regression: implemented %d < baseline %d" %
              (current, previous), file=sys.stderr)
        return 1
    return 0


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("manifest", help="extract Canvas routes and counts")
    p.add_argument("--canvas-src", required=True, help="Canvas LMS checkout")
    p.add_argument("--out", required=True, help="output JSON file")
    p.set_defaults(func=cmd_manifest)

    p = sub.add_parser("report", help="compare Rust routes to the manifest")
    p.add_argument("--manifest", required=True, help="canvas_manifest.json")
    p.add_argument("--star-src", required=True, help="migration workspace root")
    p.add_argument("--out", required=True, help="output directory")
    p.set_defaults(func=cmd_report)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
