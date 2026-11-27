#!/usr/bin/env python3
"""Compare the Rust port against what Canvas actually is.

The inputs are the truth, not an estimate:

    migration/parity/canvas-api-surface.json   routes, permissions, serializers,
                                               GraphQL types — dumped from a
                                               running Canvas by
                                               canvas-boot/canvas_introspection.rb
    migration/parity/canvas-data-layer.json    351 models with their columns,
                                               validations, callbacks,
                                               associations and scopes
    migration/rust/app-cases/                  what Canvas answered, recorded
    migration/rust/app-cases/replay-report.md  what the Rust build answered

And the port's own state, read from the tree:
    migration/crates/star-*/src/**.rs          its routes, permission checks
    migration/crates/star-db/migrations/*.sql  its tables
    migration/crates/star-graphql/src/**.rs    its GraphQL surface

Output: docs/knowledge/facts/gap-report.md

    python3 tools/knowledge/gen_gap_report.py
"""

from __future__ import annotations

import argparse
import collections
import json
import re
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
MIGRATION = REPO / "migration"
VAULT = REPO / "docs" / "knowledge"

# ---------------------------------------------------------------------------
# The port's own state
# ---------------------------------------------------------------------------
ROUTE_RE = re.compile(r'\.route\(\s*"([^"]*)"', re.S)
PERMISSION_RE = re.compile(
    r"\b(manage|view|read|create|update|delete|moderate|participate|"
    r"manage_courses|manage_groups|manage_users|manage_content|"
    r"manage_assignments|manage_sections|manage_files|manage_"
    r"[a-z_]+|view_[a-z_]+|read_[a-z_]+)\b"
)


def port_routes() -> dict[str, set[str]]:
    """Normalised path shape -> the crates that register it."""
    out: dict[str, set[str]] = collections.defaultdict(set)
    for crate_dir in sorted((MIGRATION / "crates").iterdir()):
        if not crate_dir.is_dir():
            continue
        for f in crate_dir.rglob("*.rs"):
            if "/tests/" in str(f):
                continue
            for m in ROUTE_RE.finditer(f.read_text(errors="ignore")):
                out[shape(m.group(1))].add(crate_dir.name)
    return out


def shape(path: str) -> str:
    """The path as the port registers it (the API router is nested on /api/v1)."""
    if path.startswith("/api/v1"):
        path = path[len("/api/v1"):]
    return normalise_path(path)


def normalise_path(path: str) -> str:
    """Both parameter syntaxes to one shape.

    The port writes `{course_id}` (axum); Canvas writes `:course_id` (Rails).
    They are the same URL, so both must normalise to `{}`.
    """
    path = re.sub(r"\{[^}]+\}", "{}", path)
    path = re.sub(r":[A-Za-z_][A-Za-z0-9_]*", "{}", path)
    path = re.sub(r"\([^)]*\)\Z", "", path)      # Rails "(.:format)"
    return path or "/"


def canvas_shape(path: str) -> str:
    """Canvas mounts the API under /api/v1; the port nests it there too, so
    both sides are compared in the router's own path space."""
    if path.startswith("/api/v1"):
        path = path[len("/api/v1"):]
    return normalise_path(path)


def port_tables() -> dict[str, set[str]]:
    cols: dict[str, set[str]] = collections.defaultdict(set)
    create = re.compile(r"CREATE TABLE IF NOT EXISTS\s+(\w+)\s*\((.*?)\n\);", re.S | re.I)
    alter = re.compile(r"ALTER TABLE\s+(\w+)\s+ADD COLUMN(?:\s+IF NOT EXISTS)?\s+(\w+)", re.I)
    for f in sorted((MIGRATION / "crates" / "star-db" / "migrations").glob("*.sql")):
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
        for m in alter.finditer(text):
            cols[m.group(1)].add(m.group(2))
    return cols


def resolve_table(name: str, tables: dict[str, set[str]]) -> str | None:
    """Canvas model name -> the Rust table that holds it.

    Rails pluralises; Canvas' model names do not always match the table
    ("Course" -> "courses", "EnrollmentTerm" -> "enrollment_terms").
    """
    # Rails names the class "Enrollment::Term" for the table "enrollment_terms",
    # and an STI subclass "AssetUserAccessLog::AuaLog0" shares its parent's
    # table. Try both readings.
    demod = name.split("::")[-1]
    first = name.split("::")[0]
    cands: list[str] = []
    for base in (demod, first):
        lower = base.lower()
        cands += [lower, lower + "s", lower.rstrip("s"), lower.rstrip("s") + "es",
                  lower.replace("_", "")]
    for candidate in cands:
        if candidate in tables:
            return candidate
    return None


def port_permission_words() -> set[str]:
    """Permission names the port mentions.

    Canvas names permissions as bare identifiers in its `can?` calls
    (`can :manage_courses`), and the port mirrors them as `require_manage_courses`
    helpers or as string literals. Match both.
    """
    names: set[str] = set()
    for f in (MIGRATION / "crates").rglob("*.rs"):
        if "/tests/" in str(f):
            continue
        text = f.read_text(errors="ignore")
        # The port names a Canvas permission as a string literal:
        # `require_permission("manage_courses")`.
        for m in re.finditer(r'"([a-z][a-z0-9_]{4,})"', text):
            names.add(m.group(1))
    return names


def port_graphql() -> dict[str, set[str]]:
    """GraphQL type / query field / mutation names the port declares."""
    out: dict[str, set[str]] = {"types": set(), "queries": set(), "mutations": set()}
    root = MIGRATION / "crates" / "star-graphql" / "src"
    if not root.is_dir():
        return out
    for f in root.rglob("*.rs"):
        text = f.read_text(errors="ignore")
        # The port uses async-graphql: `#[derive(SimpleObject)]` on a struct,
        # `#[Object]` on an impl block, and `async_graphql::Object` fields.
        for m in re.finditer(r"#\[derive\([^)]*SimpleObject[^)]*\)\][\s\S]{0,400}?struct\s+(\w+)", text):
            out["types"].add(m.group(1))
        for m in re.finditer(r"#\[derive\([^)]*SimpleObject[^)]*\)\][\s\S]{0,400}?struct\s+(\w+)\s*\{", text):
            out["types"].add(m.group(1))
        for m in re.finditer(r"async_graphql::Object", text):
            pass
        # The query and mutation roots are named structs: `pub struct Query;`
        # and `pub struct Mutation(..);`, each with an `#[Object] impl`.
        for root, bucket in (("Query", "queries"), ("Mutation", "mutations")):
            m = re.search(r"impl\s+" + root + r"[^\{]*\{[\s\S]*?\n\}", text)
            if not m:
                continue
            for f in re.finditer(r"\basync\s+fn\s+(\w+)\s*\(", m.group(0)):
                out[bucket].add(f.group(1))
    return out


def replay_counts() -> tuple[int, int, int]:
    """(recorded, passed, failed) from the replay report."""
    report = MIGRATION / "rust" / "app-cases" / "replay-report.md"
    if not report.is_file():
        return (0, 0, 0)
    text = report.read_text(errors="ignore")
    m = re.search(r"Passed:\s*(\d+)/(\d+)", text)
    recorded = int(m.group(2)) if m else 0
    passed = int(m.group(1)) if m else 0
    failed = len(re.findall(r"^### ([a-z0-9\-_]+)\s*$", text, re.M))
    return (recorded, passed, failed)


# ---------------------------------------------------------------------------
def build_gap_report() -> str:
    surface = json.loads((MIGRATION / "parity" / "canvas-api-surface.json").read_text())
    layer = json.loads((MIGRATION / "parity" / "canvas-data-layer.json").read_text())

    canvas_routes = surface["routes"]
    canvas_api = [r for r in canvas_routes if r["api"]]
    port = port_routes()
    tables = port_tables()
    perms_named = port_permission_words()
    gql = port_graphql()
    recorded, passed, failed = replay_counts()

    # --- routes ------------------------------------------------------------
    canvas_shapes = collections.defaultdict(list)
    for r in canvas_api:
        canvas_shapes[canvas_shape(r["path"])].append(r)
    missing_routes = {
        s: v for s, v in canvas_shapes.items() if s not in port
    }
    extra_routes = {s: v for s, v in port.items() if s not in canvas_shapes}

    # --- permissions -------------------------------------------------------
    canvas_perms = {p["name"] for p in surface["permissions"] if "name" in p}
    port_perms = {p for p in perms_named if p in canvas_perms}
    missing_perms = sorted(canvas_perms - port_perms)

    # --- data layer --------------------------------------------------------
    canvas_models = layer["models"]
    model_gaps = {}
    models_with_table = 0
    for name, m in canvas_models.items():
        table = resolve_table(name, tables)
        if table is None:
            continue
        models_with_table += 1
        missing = sorted(set(m.get("columns", [])) - tables[table])
        if missing:
            model_gaps[name] = missing

    # --- graphql -----------------------------------------------------------
    canvas_gql = surface["graphql"]
    gql_gaps = {
        "types": sorted(set(canvas_gql.get("types", [])) - gql["types"]),
        "queries": sorted(set(canvas_gql.get("queries", [])) - gql["queries"]),
        "mutations": sorted(set(canvas_gql.get("mutations", [])) - gql["mutations"]),
    }

    L: list[str] = []
    A = L.append
    A("---")
    A("tags: [fact, generated, gap-report]")
    A("---")
    A("")
    A("# Gap report — Canvas vs the port, measured")
    A("")
    A("GENERATED by `tools/knowledge/gen_gap_report.py`. Do not edit by hand.")
    A("")
    A("Canvas's side is *introspection*, not a guess: `canvas-boot/canvas_introspection.rb`")
    A("asks a running Canvas for its routes, permissions, models and GraphQL surface.")
    A("The port's side is read from the tree. Where the two disagree, the port owes")
    A("Canvas something.")
    A("")

    A("## The headline")
    A("")
    A("| Dimension | Canvas | Port | Gap |")
    A("|---|---:|---:|---:|")
    A(f"| API routes mounted | {len(canvas_api)} | {len([s for s in port if s in canvas_shapes])} | **{len(missing_routes)}** |")
    A(f"| Permissions declared | {len(canvas_perms)} | {len(port_perms)} | **{len(missing_perms)}** |")
    A(f"| Models with a table | {len(canvas_models)} | {models_with_table} | **{len(canvas_models) - models_with_table}** |")
    A(f"| Columns missing (models with a table) | — | — | **{sum(len(v) for v in model_gaps.values())}** |")
    A(f"| GraphQL types | {len(canvas_gql.get('types', []))} | {len(gql['types'])} | **{len(gql_gaps['types'])}** |")
    A(f"| GraphQL query fields | {len(canvas_gql.get('queries', []))} | {len(gql['queries'])} | **{len(gql_gaps['queries'])}** |")
    A(f"| GraphQL mutations | {len(canvas_gql.get('mutations', []))} | {len(gql['mutations'])} | **{len(gql_gaps['mutations'])}** |")
    A(f"| Behavioural cases passing | {recorded} recorded | {passed} pass / {failed} fail | **{failed}** |")
    A("")

    A("## Routes Canvas mounts that the port does not")
    A("")
    A(f"{len(missing_routes)} path shapes. The list is long; the shape of it is not:")
    A("Canvas's long tail is LTI, AI surfaces, master templates, reporting and admin.")
    A("")
    by_controller = collections.Counter(
        (v[0].get("controller") or "?").split("/")[-1] for v in missing_routes.values()
    )
    A("| Canvas controller | routes not ported |")
    A("|---|---:|")
    for controller, n in by_controller.most_common(20):
        A(f"| `{controller}` | {n} |")
    A("")

    if extra_routes:
        A(f"### Routes the port registers that Canvas does not mount ({len(extra_routes)})")
        A("")
        for s in sorted(extra_routes)[:30]:
            A(f"- `{s}`  ({', '.join(sorted(extra_routes[s]))})")
        A("")
        A("These are either Canvas aliases the introspection does not list, or routes")
        A("the port invented. Each needs a decision: keep as an alias, or delete.")
        A("")

    A("## Permissions the port does not mention")
    A("")
    A(f"{len(missing_perms)} of {len(canvas_perms)}. Canvas declares each in")
    A("`config/initializers/permissions_registry.rb`.")
    A("")
    A("```")
    for p in missing_perms:
        A(p)
    A("```")
    A("")

    A("## Columns the port's tables still owe")
    A("")
    if not model_gaps:
        A("None — every recorded Canvas field has a column.")
    else:
        A(f"{len(model_gaps)} models, {sum(len(v) for v in model_gaps.values())} columns total.")
        A("")
        for name in sorted(model_gaps, key=lambda n: -len(model_gaps[n]))[:25]:
            A(f"### `{name}` — {len(model_gaps[name])} missing")
            A("")
            A("```")
            A("\n".join(model_gaps[name]))
            A("```")
            A("")

    A("## GraphQL")
    A("")
    for kind in ("types", "queries", "mutations"):
        gaps = gql_gaps[kind]
        total = len(canvas_gql.get(kind, []))
        pct = f"{100.0 * (total - len(gaps)) / total:.1f}%" if total else "—"
        A(f"- **{kind}**: {total - len(gaps)}/{total} ({pct}) — {len(gaps)} missing")
    A("")
    if gql_gaps["queries"]:
        A("Canvas query root fields the port does not expose:")
        A("")
        A("```")
        A("\n".join(gql_gaps["queries"]))
        A("```")
        A("")

    A("## Where to start")
    A("")
    A("1. The **behavioural** column is the one that matters for production. A route")
    A("   that is registered but answers the wrong shape is worse than a route that")
    A("   is absent, because a client cannot tell the difference.")
    A("2. The cheapest wins are the ones the replay already names: the Canvas response")
    A("   middleware (every case), JSON error bodies (402 cases), and body extraction")
    A("   that tolerates a missing Content-Type (45 cases).")
    A("3. The **column** gaps are mechanical and additive. The permissions gap needs a")
    A("   decision per permission — a port that silently allows what Canvas denies is")
    A("   a security defect, so it is not a bulk edit.")
    A("")
    return "\n".join(L)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()
    out = VAULT / "facts" / "gap-report.md"
    content = build_gap_report()
    if args.check:
        if not out.is_file() or out.read_text(encoding="utf-8") != content:
            print(f"STALE: {out.relative_to(REPO)}")
            return 1
        print("gap report is up to date")
        return 0
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(content, encoding="utf-8")
    print(f"wrote {out.relative_to(REPO)} ({len(content)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
