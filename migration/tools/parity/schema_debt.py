#!/usr/bin/env python3
"""Report the schema debt the recorded Canvas responses expose.

The recorded bodies are the behaviour contract: they show exactly which
fields Canvas returns for each route. The Rust port's tables are much
thinner, so every key in a recorded body that has no column in the
corresponding Rust table is a field the port still owes Canvas.

This turns "the port is 35% implemented" into a per-family list of missing
columns.

Usage:
    schema_debt.py [--cases rust/app-cases/http] [--migrations crates/star-db/migrations]
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

# Which Rust table backs a recorded case's top-level object. The case name
# starts with the controller, which maps to the table that owns the row.
# The recorder names cases after the request path, so the family is read
# from the path's first segment, not from the case name.
PATH_TABLE = {
    "accounts": "accounts",
    "users": "users",
    "courses": "courses",
    "enrollments": "enrollments",
    "groups": "groups",
    "group_categories": "group_categories",
    "folders": "folders",
    "files": "attachments",
    "assignments": "assignments",
    "submissions": "submissions",
    "rubrics": "rubrics",
    "rubric_assessments": "rubric_assessments",
    "discussion_topics": "discussion_topics",
    "pages": "wiki_pages",
    "calendar_events": "calendar_events",
    "conversations": "conversations",
    "outcome_groups": "outcome_groups",
    "external_tools": "external_tools",
    "lti_registrations": "lti_registrations",
    "announcements": "announcements",
    "quizzes": "quizzes",
    "modules": "modules",
    "sections": "course_sections",
    "terms": "enrollment_terms",
}

# Segments that are not a table of their own: the response is an embedded
# object (permissions, settings, statistics), not a row.
NON_RESOURCE = {
    "permissions", "settings", "stats", "statistics", "grades", "grade",
    "enrollment_state", "read", "status", "available", "self",
}

CONTROLLER_TABLE = {
    "accounts": "accounts",
    "users": "users",
    "courses": "courses",
    "enrollments_api": "enrollments",
    "groups": "groups",
    "group_categories": "group_categories",
    "folders": "folders",
    "files": "attachments",
    "assignments": "assignments",
    "submissions_api": "submissions",
    "rubrics_api": "rubrics",
    "discussion_topics_api": "discussion_topics",
    "wiki_pages_api": "wiki_pages",
    "calendar_events_api": "calendar_events",
    "conversations": "conversations",
    "outcome_groups_api": "outcome_groups",
    "external_tools": "external_tools",
    "lti_registrations": "lti_registrations",
}

CREATE = re.compile(
    r"CREATE TABLE IF NOT EXISTS\s+(\w+)\s*\((.*?)\n\);", re.S | re.I
)


def rust_columns(migrations: Path) -> dict[str, set[str]]:
    cols: dict[str, set[str]] = {}
    for f in sorted(migrations.glob("*.sql")):
        text = f.read_text(errors="ignore")
        for m in CREATE.finditer(text):
            table, body = m.group(1), m.group(2)
            names = set()
            for line in body.split("\n"):
                line = line.strip()
                if not line or line.startswith("--"):
                    continue
                col = line.split()[0].strip('"')
                if col.upper() in {"PRIMARY", "FOREIGN", "UNIQUE", "CHECK", "CONSTRAINT"}:
                    continue
                names.add(col)
            cols.setdefault(table, set()).update(names)
    return cols


def json_keys(body: str) -> list[str]:
    """Keys of the first object in a recorded body, if it is JSON.

    An error envelope is not a row: Canvas answers `{"errors": [...]}` or
    `{"error": ...}` for a denied or missing request, and those keys are not
    columns of anything. They are skipped, or every table would appear to owe
    an `errors` column.
    """
    text = body.strip()
    if not text or text == "[..]":
        return []
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        return []
    if isinstance(data, list):
        data = data[0] if data else {}
    if not isinstance(data, dict):
        return []
    if "errors" in data or "error" in data:
        return []
    return sorted(data.keys())


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", type=Path, default=Path("rust/app-cases/http"))
    ap.add_argument("--migrations", type=Path, default=Path("crates/star-db/migrations"))
    args = ap.parse_args()

    cols = rust_columns(args.migrations)
    debt: dict[str, set[str]] = {}
    seen: dict[str, int] = {}

    for body_file in sorted(args.cases.glob("*.body")):
        toml = body_file.with_suffix(".toml")
        if not toml.is_file():
            continue
        req = toml.read_text(errors="ignore")
        m = re.search(r'path = "([^"]*)"', req)
        if not m:
            continue
        segments = [s for s in m.group(1).split("/") if s]
        if segments and segments[0] == "api" and len(segments) > 1:
            segments = segments[2:]
        # The response is the LAST resource segment: /courses/1/folders
        # returns folders, not courses. An id-like segment is skipped.
        table = None
        for seg in reversed(segments):
            if seg.isdigit() or seg.startswith("{") or seg.startswith(":"):
                continue
            if seg in NON_RESOURCE:
                # The response is an embedded object, not a row: stop here.
                break
            if seg in PATH_TABLE:
                table = PATH_TABLE[seg]
                break
        if not table:
            continue
        keys = json_keys(body_file.read_text(errors="ignore"))
        if not keys:
            continue
        seen[table] = seen.get(table, 0) + 1
        have = cols.get(table, set())
        missing = {k for k in keys if k not in have}
        debt.setdefault(table, set()).update(missing)

    print(f"{'table':<22} {'cases':>6}  missing columns (Canvas returns them, Rust has no column)")
    print("-" * 100)
    for table in sorted(debt, key=lambda t: -len(debt[t])):
        print(f"{table:<22} {seen.get(table,0):>6}  {len(debt[table])}")
        for k in sorted(debt[table]):
            print(f"{'':<30}{k}")
    print()
    total = sum(len(v) for v in debt.values())
    print(f"total missing columns across {len(debt)} tables: {total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
