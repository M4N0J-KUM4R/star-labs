#!/usr/bin/env python3
"""Generate the agent knowledge vault's fact sheets.

The vault (`docs/knowledge/`) is what an agent reads BEFORE it touches code.
Its facts are derived from artefacts that are already committed and
machine-readable, so they cannot drift:

    migration/parity/canvas_manifest.json   the Canvas route registry
    migration/parity/coverage.json           which routes the port implements
    migration/rust/app-cases/http/*.toml    what Canvas actually answered
    migration/rust/app-cases/replay-report.md  what the Rust build answered
    migration/crates/star-db/migrations/    the Rust schema

Run:

    python3 tools/knowledge/gen_knowledge.py            # write the facts
    python3 tools/knowledge/gen_knowledge.py --check    # exit 1 if stale

CI runs the `--check` form: a fact sheet that no longer matches its source
fails the build instead of quietly misleading an agent.
"""

from __future__ import annotations

import argparse
import collections
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
VAULT = REPO / "docs" / "knowledge"

# ---------------------------------------------------------------------------
# The crate that owns each Canvas controller.
#
# Derived from the port's own registrations: a Canvas controller belongs to
# whichever crate registers its paths. Kept as a table so a wrong mapping is
# visible and fixable, rather than inferred wrongly at runtime.
# ---------------------------------------------------------------------------
CRATE_BY_CONTROLLER = {
    "accounts": "star-accounts",
    "account_calendars": "star-calendar",
    "assignment_groups": "star-assignment-groups",
    "assignments": "star-assignments",
    "announcements": "star-announcements",
    "assignment_overrides": "star-assignments",
    "audit": "star-audit",
    "calendar_events_api": "star-calendar",
    "collaborations": "star-collaborations",
    "conferences": "star-conferences",
    "content_exports": "star-content",
    "content_imports": "star-content",
    "content_migrations": "star-content",
    "content_shares": "star-content",
    "conversation": "star-conversations",
    "conversations": "star-conversations",
    "courses": "star-courses",
    "discussion_topics_api": "star-discussions",
    "eportfolios": "star-eportfolios",
    "external_tools": "star-lti",
    "feature_flags": "star-features",
    "files": "star-files",
    "folders": "star-files",
    "grading_periods": "star-grading-standards",
    "grading_standards": "star-grading-standards",
    "groups": "star-groups",
    "group_categories": "star-groups",
    "learning_dates": "star-learning-dates",
    "lti/registrations": "star-lti",
    "lti_apps": "star-lti",
    "master_courses/master_templates": "star-courses",
    "modules": "star-modules",
    "notifications": "star-notifications",
    "outcomes": "star-outcomes",
    "pages": "star-pages",
    "permissions": "star-permissions",
    "polling": "star-polling",
    "quiz_statistics": "star-quizzes",
    "quizzes": "star-quizzes",
    "rubrics_api": "star-rubrics",
    "search": "star-search",
    "sis": "star-sis",
    "submissions_api": "star-submissions",
    "submission_comments_api": "star-submission-comments",
    "terms": "star-courses",
    "users": "star-users",
    "jobs": "star-jobs",
    "eportfolio": "star-eportfolios",
}

# Canvas controller -> the file to open. Canvas names controller files after the
# controller path, with `_controller.rb`.
def canvas_controller_file(controller: str) -> str:
    if "/" in controller:
        head, tail = controller.split("/", 1)
        return f"app/controllers/{head}/{tail}_controller.rb"
    return f"app/controllers/{controller}_controller.rb"


def route_shape(path: str) -> str:
    """Both parameter syntaxes to one shape.

    The port writes `{course_id}` (axum); Canvas writes `:course_id` (Rails).
    They are the same URL, so both must normalise to `{}`.
    """
    path = re.sub(r"\{[^}]+\}", "{}", path)
    path = re.sub(r":[A-Za-z_][A-Za-z0-9_]*", "{}", path)
    path = re.sub(r"\([^)]*\)\Z", "", path)
    return path or "/"


# ---------------------------------------------------------------------------
# Sources
# ---------------------------------------------------------------------------
def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def api_shape(path: str) -> str:
    """The path shape as the port registers it.

    The API router is nested under `/api/v1` in `star-http/src/main.rs`, so a
    route registered as `/courses/{}` there serves Canvas' `/api/v1/courses`.
    """
    if path.startswith("/api/v1"):
        path = path[len("/api/v1"):]
    return route_shape(path) or "/"


def star_registrations() -> dict[str, list[str]]:
    """Rust-registered path shape -> the crates that register it."""
    out: dict[str, list[str]] = collections.defaultdict(list)
    crates = REPO / "migration" / "crates"
    for crate_dir in sorted(crates.iterdir()):
        if not crate_dir.is_dir():
            continue
        for f in crate_dir.rglob("*.rs"):
            if "/tests/" in str(f):
                continue
            text = f.read_text(errors="ignore")
            for m in re.finditer(r'\.route\(\s*"([^"]*)"\s*,', text):
                out[api_shape(m.group(1))].append(crate_dir.name)
    return out


def replay_status() -> dict[str, str]:
    """Recorded case -> its replay verdict."""
    report = REPO / "migration" / "rust" / "app-cases" / "replay-report.md"
    if not report.is_file():
        return {}
    text = report.read_text(errors="ignore")
    passed = set()
    total = None
    m = re.search(r"Passed:\s*(\d+)/(\d+)", text)
    if m:
        passed_count, total = int(m.group(1)), int(m.group(2))
    # The report lists only failures in detail, each under a `### <case>`
    # heading; a case not listed passed.
    failures = set(re.findall(r"^### ([a-z0-9\-_]+)\s*$", text, re.M))
    status = {}
    for toml in sorted((REPO / "migration" / "rust" / "app-cases" / "http").glob("*.toml")):
        name = toml.stem
        status[name] = "fail" if name in failures else "pass"
    return status


def recorded_cases() -> dict[str, dict]:
    """Recorded case -> request path and the status Canvas answered."""
    out = {}
    base = REPO / "migration" / "rust" / "app-cases" / "http"
    for toml in sorted(base.glob("*.toml")):
        text = toml.read_text(errors="ignore")
        path = re.search(r'path = "([^"]*)"', text)
        status = re.search(r"status = (\d+)", text)
        if path:
            out[toml.stem] = {"path": path.group(1), "status": int(status.group(1)) if status else None}
    return out


def rust_tables() -> dict[str, set[str]]:
    cols: dict[str, set[str]] = collections.defaultdict(set)
    create = re.compile(r"CREATE TABLE IF NOT EXISTS\s+(\w+)\s*\((.*?)\n\);", re.S | re.I)
    alter = re.compile(r"ALTER TABLE\s+(\w+)\s+(.*?);", re.S | re.I)
    add_column = re.compile(r"ADD COLUMN(?:\s+IF NOT EXISTS)?\s+(\w+)", re.I)
    for f in sorted((REPO / "migration" / "crates" / "star-db" / "migrations").glob("*.sql")):
        text = f.read_text(errors="ignore")
        for m in create.finditer(text):
            for line in m.group(2).split("\n"):
                line = line.strip()
                if not line or line.startswith("--"):
                    continue
                col = line.split()[0].strip('"')
                if col.upper() in {"PRIMARY", "FOREIGN", "UNIQUE", "CHECK", "CONSTRAINT"}:
                    continue
                cols[m.group(1)].add(col)
        # A table that grew through `ALTER TABLE ... ADD COLUMN` has the same
        # columns as one created with them — and one statement may add several
        # columns at once.
        code = "\n".join(
            line for line in text.split("\n") if not line.strip().startswith("--")
        )
        for m in alter.finditer(code):
            for add in add_column.finditer(m.group(2)):
                cols[m.group(1)].add(add.group(1))
    return cols


# ---------------------------------------------------------------------------
# Generators
# ---------------------------------------------------------------------------
def gen_route_map(manifest: dict, coverage: dict, star: dict, cases: dict, replay: dict) -> str:
    implemented = {
        (r["verb"], r["normalized_path"]) for r in coverage.get("implemented_routes", [])
    }
    by_controller: dict[str, list] = collections.defaultdict(list)
    for r in manifest["routes"]:
        by_controller[r["controller"]].append(r)

    lines = [
        "---",
        "tags: [fact, generated, routes]",
        "---",
        "",
        "# Route map — Canvas route → Rust owner → Canvas source",
        "",
        "GENERATED by `tools/knowledge/gen_knowledge.py`. Do not edit by hand.",
        "",
        "Every Canvas API route, the Rust crate that owns it, the Canvas file to read,",
        "and what the parity replay says about it. An agent working on a route should",
        "read this file, then the Canvas file, then the Rust crate — not the whole tree.",
        "",
        f"Canvas routes: **{len(manifest['routes'])}** in **{len(by_controller)}** controllers.",
        f"Implemented in the port: **{len(implemented)}**.",
        "",
    ]

    # Which routes have a recorded case, and how the replay went.
    case_by_path = collections.defaultdict(list)
    for name, info in cases.items():
        shape = route_shape(info["path"])
        case_by_path[shape].append((name, info["status"], replay.get(name, "-")))

    for controller in sorted(by_controller):
        routes = sorted(by_controller[controller], key=lambda r: (r["verb"], r["path"]))
        crate = CRATE_BY_CONTROLLER.get(controller)
        if crate is None:
            # Derive it from the port: whichever crate registers this
            # controller's paths owns them.
            owners = collections.Counter()
            for r in routes:
                for c in star.get(api_shape(r["path"]), []):
                    owners[c] += 1
            crate = owners.most_common(1)[0][0] if owners else "?"
        src = canvas_controller_file(controller)
        n_impl = sum(1 for r in routes if (r["verb"], r["normalized_path"]) in implemented)
        lines.append(f"## `{controller}` → `{crate}`")
        lines.append("")
        lines.append(f"Canvas source: `{src}`  ·  routes: {len(routes)}  ·  implemented: {n_impl}")
        lines.append("")
        lines.append("| verb | Canvas path | implemented | recorded case | replay |")
        lines.append("|---|---|---|---|---|")
        for r in routes:
            impl = "yes" if (r["verb"], r["normalized_path"]) in implemented else "**no**"
            shape = route_shape(r["path"])
            case = case_by_path.get(shape, [])
            if case:
                cname, cstatus, rstat = case[0]
                rec = f"`{cstatus}`"
                rp = {"pass": "pass", "fail": "**FAIL**", "-": "-"}[rstat]
            else:
                rec, rp = "-", "-"
            lines.append(f"| {r['verb']} | `{r['path']}` | {impl} | {rec} | {rp} |")
        lines.append("")
    return "\n".join(lines)


def gen_schema_truth(tables: dict[str, set[str]], cases: dict) -> str:
    """The Rust schema per table, plus the fields Canvas returns that it lacks."""
    # Which recorded cases map to which table, by the request path's last
    # resource segment.
    seg_table = {
        "accounts": "accounts", "users": "users", "courses": "courses",
        "enrollments": "enrollments", "groups": "groups",
        "group_categories": "group_categories", "folders": "folders",
        "files": "files", "assignments": "assignments",
        "submissions": "submissions", "rubrics": "rubrics",
        "rubric_assessments": "rubric_assessments",
        "discussion_topics": "discussion_topics", "pages": "wiki_pages",
        "calendar_events": "calendar_events", "conversations": "conversations",
        "outcome_groups": "outcome_groups", "external_tools": "external_tools",
        "lti_registrations": "lti_registrations", "announcements": "announcements",
        "quizzes": "quizzes", "modules": "modules",
    }
    want = {
        "accounts", "users", "courses", "enrollments", "groups",
        "group_categories", "folders", "attachments", "assignments",
        "submissions", "conversations", "calendar_events", "rubrics",
    }
    # Segments that are not a table row of their own: the response is an
    # embedded object (a permissions map, the course settings document) or
    # it is not a row at all (preview_html renders a document, ping answers
    # with a flag).
    non_resource = {
        "permissions", "settings", "stats", "statistics", "grades", "grade",
        "preview_html", "ping",
    }
    # Sub-resources that answer with a row of their own table: `/courses/1/
    # students` answers with users, `/courses/1/front_page` with a wiki page.
    sub_resource_table = {
        "students": "users", "users": "users", "recent_students": "users",
        "search_users": "users", "student_view_student": "users",
        "content_share_users": "users", "front_page": "wiki_pages",
    }
    canvas_fields: dict[str, set[str]] = collections.defaultdict(set)
    for name, info in cases.items():
        segs = [s for s in info["path"].split("/") if s]
        if segs and segs[0] == "api" and len(segs) > 2:
            segs = segs[2:]
        table = None
        for s in reversed(segs):
            if s.isdigit() or s.startswith("{"):
                continue
            if s in non_resource:
                break
            if s in sub_resource_table:
                table = sub_resource_table[s]
                break
            if s in seg_table:
                table = seg_table[s]
                break
        if not table or table not in want:
            continue
        body = (REPO / "migration" / "rust" / "app-cases" / "http" / f"{name}.body")
        if not body.is_file():
            continue
        text = body.read_text(errors="ignore").strip()
        if not text or text == "[..]":
            continue
        try:
            data = json.loads(text)
        except json.JSONDecodeError:
            continue
        if isinstance(data, list):
            data = data[0] if data else {}
        if not isinstance(data, dict) or "errors" in data or "error" in data:
            continue
        # Canvas' single-message error body is {"message": "..."} — that is an
        # error, not a field of the resource.
        if set(data.keys()) <= {"message", "status", "ok"}:
            continue
        # Only scalar fields are columns. A nested object or list is a
        # sub-resource (a group's `permissions` map, a course's `enrollments`
        # array), not a column of this table.
        for key, value in data.items():
            if value is None or isinstance(value, (str, int, float, bool)):
                canvas_fields[table].add(key)

    lines = [
        "---",
        "tags: [fact, generated, schema]",
        "---",
        "",
        "# Schema truth — what the Rust tables carry vs what Canvas returns",
        "",
        "GENERATED by `tools/knowledge/gen_knowledge.py`. Do not edit by hand.",
        "",
        "The right-hand column is measured from the recorded Canvas responses:",
        "fields Canvas returns that the Rust table has no column for. Each one is a",
        "field the port still owes Canvas — the recorded body is the spec.",
        "",
    ]
    total_missing = 0
    for table in sorted(want):
        have = tables.get(table, set())
        missing = sorted(canvas_fields.get(table, set()) - have)
        total_missing += len(missing)
        lines.append(f"## `{table}`")
        lines.append("")
        if not canvas_fields.get(table):
            lines.append("_No non-error recorded response for this table yet._")
            lines.append("")
            continue
        lines.append(f"Rust columns: {len(have)}  ·  missing vs Canvas: **{len(missing)}**")
        lines.append("")
        if missing:
            lines.append("```")
            lines.extend(missing)
            lines.append("```")
        else:
            lines.append("All recorded fields exist as columns.")
        lines.append("")
    lines.append(f"**Total missing columns across these tables: {total_missing}.**")
    lines.append("")
    lines.append("To close one: add an idempotent migration in")
    lines.append("`migration/crates/star-db/migrations/` (never edit an existing one),")
    lines.append("then extend the handler's SELECT and JSON construction.")
    lines.append("")
    return "\n".join(lines)


def gen_parity_state(coverage: dict, replay: dict, cases: dict) -> str:
    summary = coverage.get("summary", {})
    total = len(cases)
    fails = sum(1 for v in replay.values() if v == "fail")
    passes = sum(1 for v in replay.values() if v == "pass")
    lines = [
        "---",
        "tags: [fact, generated, parity]",
        "---",
        "",
        "# Parity state — where the port actually stands",
        "",
        "GENERATED by `tools/knowledge/gen_knowledge.py`. Do not edit by hand.",
        "",
        "## Route coverage",
        "",
        "| Area | Implemented | Total | Coverage |",
        "|---|---:|---:|---:|",
    ]
    rows = [
        ("REST API routes", "implemented_rest_routes", "canvas_api_routes"),
        ("REST routes used by the UI", "ui_referenced_implemented", "ui_referenced_routes"),
        ("GraphQL object types", "graphql_types_implemented", "graphql_types"),
        ("GraphQL mutations", "graphql_mutations_implemented", "graphql_mutations"),
    ]
    for label, done_key, total_key in rows:
        done, tot = summary.get(done_key), summary.get(total_key)
        pct = f"{(100.0 * done / tot):.1f}%" if done and tot else "—"
        lines.append(f"| {label} | {done} | {tot} | {pct} |")
    lines += [
        "",
        "## Behavioural parity (the replay)",
        "",
        f"- recorded cases: **{total}**",
        f"- replay passed: **{passes}**  ·  failed: **{fails}**",
        "",
        "A case passes only when the status, the headers Canvas sets and the body",
        "all match. The report is `migration/rust/app-cases/replay-report.md`;",
        "CI fails when the committed report and a fresh run disagree.",
        "",
        "## What to read for a given task",
        "",
        "| You are working on | Read |",
        "|---|---|",
        "| a route | `facts/route-map.md` for that controller → the Canvas file it names |",
        "| a table's fields | `facts/schema-truth.md` for that table |",
        "| a failing case | `migration/rust/app-cases/replay-report.md` |",
        "| the response headers | `traps/canvas-response-headers.md` |",
        "| anything that has bitten you before | `traps/` |",
        "",
    ]
    return "\n".join(lines)


# ---------------------------------------------------------------------------
def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true",
                    help="exit 1 if the committed facts are stale")
    args = ap.parse_args()

    manifest = load_json(REPO / "migration" / "parity" / "canvas_manifest.json")
    coverage = load_json(REPO / "migration" / "parity" / "coverage.json")
    star = star_registrations()
    cases = recorded_cases()
    replay = replay_status()
    tables = rust_tables()

    outputs = {
        VAULT / "facts" / "route-map.md": gen_route_map(manifest, coverage, star, cases, replay),
        VAULT / "facts" / "schema-truth.md": gen_schema_truth(tables, cases),
        VAULT / "facts" / "parity-state.md": gen_parity_state(coverage, replay, cases),
    }

    stale = []
    for path, content in outputs.items():
        if args.check:
            if not path.is_file() or path.read_text(encoding="utf-8") != content:
                stale.append(path)
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        print(f"wrote {path.relative_to(REPO)} ({len(content)} bytes)")

    if args.check:
        if stale:
            for p in stale:
                print(f"STALE: {p.relative_to(REPO)}")
            print("run: python3 tools/knowledge/gen_knowledge.py")
            return 1
        print("knowledge facts are up to date")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
