#!/usr/bin/env python3
"""Generate rust-anything `--inputs` JSONL from the Canvas route registry.

Every Canvas API route becomes one or more replay cases. The recorder
(`rust-anything app record --http`) sends each request to the running Canvas
instance and stores the status, headers and body under
`rust/app-cases/http/`. `rust-anything app replay` then re-sends them to the
Rust build and diffs the answers.

The point: coverage becomes *computable*. A route with no case is a route with
no behavioural evidence.

Usage:
    gen_inputs.py --manifest canvas_routes_manifest.json \
                  --out rust/app-cases/http-inputs.jsonl \
                  [--family courses] [--families-from core_families.txt] \
                  [--ids ids.json]
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Core families, in the order we want them migrated.
#
# Ordered by "how much of Canvas does the rest of the app depend on this".
# Accounts/users first: every route resolves the acting user through them.
# ---------------------------------------------------------------------------
CORE_FAMILIES: list[tuple[str, str]] = [
    ("accounts", "accounts"),
    ("users", "users"),
    ("courses", "courses"),
    ("enrollments_api", "enrollments"),
    ("groups", "groups"),
    ("group_categories", "group_categories"),
    ("folders", "folders"),
    ("files", "files"),
    ("assignments", "assignments"),
    ("submissions_api", "submissions"),
    ("rubrics_api", "rubrics"),
    ("discussion_topics_api", "discussion_topics"),
    ("wiki_pages_api", "wiki_pages"),
    ("calendar_events_api", "calendar_events"),
    ("conversations", "conversations"),
    ("outcome_groups_api", "outcome_groups"),
    ("external_tools", "external_tools"),
    ("lti/registrations", "lti_registrations"),
]

# Verbs that mutate state. Recorded in this order so a create is always
# available to the reads that follow it, and the reads come last so they see
# the state the writes produced.
MUTATING_VERBS = ("POST", "PUT", "PATCH", "DELETE")
READ_VERBS = ("GET",)

# Canvas uses a bare `:id` for 416 routes. What it points at depends on the
# controller, so resolve it from the family.
FAMILY_ID_KEY = {
    "accounts": "account_id",
    "users": "user_id",
    "courses": "course_id",
    "groups": "group_id",
    "group_categories": "group_category_id",
    "folders": "folder_id",
    "files": "file_id",
    "assignments": "assignment_id",
    "submissions_api": "submission_id",
    "rubrics_api": "rubric_id",
    "discussion_topics_api": "discussion_topic_id",
    "wiki_pages_api": "wiki_page_id",
    "calendar_events_api": "calendar_event_id",
    "conversations": "conversation_id",
    "outcome_groups_api": "outcome_group_id",
    "external_tools": "external_tool_id",
    "lti/registrations": "lti_registration_id",
    "enrollments_api": "enrollment_id",
}

# Params that must hold a real id for the route to return anything useful.
# The values come from the seeded database (see seed/ids.json).
ID_PARAMS = {
    "course_id",
    "user_id",
    "account_id",
    "group_id",
    "group_category_id",
    "folder_id",
    "assignment_id",
    "submission_id",
    "rubric_id",
    "discussion_topic_id",
    "wiki_page_id",
    "calendar_event_id",
    "conversation_id",
    "outcome_group_id",
    "external_tool_id",
    "registration_id",
    "enrollment_id",
    "context_id",
    "lti_registration_id",
    "planner_id",
    "assignment_group_id",
    "assessment_id",
    "asset_id",
    "media_object_id",
    "message_id",
    "external_asset_id",
    "topic_id",
    "section_id",
    "course_section_id",
    "module_id",
    "context_module_id",
    "student_id",
    "grader_id",
    "developer_key_id",
    "context_external_tool_id",
    "outcome_id",
    "quiz_id",
    "poll_id",
    "entry_id",
    "attachment_id",
    "communication_channel_id",
    "grading_standard_id",
    "quiz_submission_id",
    "tool_proxy_id",
    "tool_proxy_guid",
    "content_migration_id",
    "ai_experience_id",
    "file_id",
}

# These ids are read-only in Canvas and are not exposed as path params.
IGNORED_PARAMS = {"page", "per_page", "include", "exclude", "search_term"}


def substitute_params(
    path: str, ids: dict[str, object], family: str
) -> tuple[str, list[str]]:
    """Replace `:param` / `{param}` segments with ids from the seed.

    Returns the concrete path and the list of params that had no seed value
    (those routes are skipped, because a request with a placeholder id proves
    nothing).
    """
    missing: list[str] = []
    out: list[str] = []
    for segment in path.split("/"):
        if ":" in segment:
            name = segment.split(":", 1)[1]
        elif segment.startswith("{") and segment.endswith("}"):
            name = segment.strip("{}")
        else:
            out.append(segment)
            continue

        if name in IGNORED_PARAMS:
            out.append(segment)
            continue

        # A bare `:id` is polymorphic: the controller decides what it is.
        if name == "id":
            name = FAMILY_ID_KEY.get(family, "")

        if name and name in ids:
            out.append(str(ids[name]))
        elif name.endswith("_id") or name == "id":
            missing.append(name or "id")
            out.append(segment)
        else:
            out.append(segment)
    return "/".join(out), missing


def route_to_case(
    route: dict,
    ids: dict[str, object],
    family: str,
) -> dict | None:
    """Turn one registry route into one input case, or None to skip it."""
    verb = route["verb"]
    if verb == "OPTIONS":
        return None

    concrete, missing = substitute_params(route["path"], ids, family)
    if missing:
        # A route whose ids we cannot seed cannot produce a meaningful body.
        # Skip it rather than record a 404 that proves nothing.
        return None

    # Only the verbs we can drive without a hand-built body.
    if verb in READ_VERBS:
        method, body = "GET", None
    elif verb in MUTATING_VERBS:
        # A mutation without a body is either a no-op or a validation error.
        # Canvas's own behaviour for that IS the contract, so record it as-is:
        # the Rust port must answer the same thing for the same empty request.
        method, body = verb, None
    else:
        return None

    name = f"{family}__{verb.lower()}__{route['normalized_path']}".replace(
        "/", "_"
    ).replace("{", "").replace("}", "").replace(":", "")

    case = {
        "name": name,
        "method": method,
        "path": concrete,
    }
    if body is not None:
        case["body"] = body
    return case


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--family", action="append", default=[],
                    help="controller to include (repeatable)")
    ap.add_argument("--all", action="store_true",
                    help="every route, not just the core families")
    ap.add_argument("--ids", type=Path,
                    help="JSON of seed ids: {\"course_id\": 1, ...}")
    args = ap.parse_args()

    manifest = json.loads(args.manifest.read_text())
    routes = manifest["routes"]

    ids: dict[str, object] = {}
    if args.ids and args.ids.is_file():
        ids = json.loads(args.ids.read_text())

    if args.all:
        families = sorted({r["controller"] for r in routes})
    elif args.family:
        families = args.family
    else:
        families = [c for c, _ in CORE_FAMILIES]

    by_controller: dict[str, list[dict]] = {}
    for route in routes:
        by_controller.setdefault(route["controller"], []).append(route)

    cases: list[dict] = []
    skipped_missing: list[str] = []
    for family in families:
        for route in by_controller.get(family, []):
            case = route_to_case(route, ids, family)
            if case is None:
                if route["verb"] != "OPTIONS":
                    skipped_missing.append(
                        f"{route['verb']} {route['path']} "
                        f"(no seed id for its params)"
                    )
                continue
            cases.append(case)

    # Writes before reads within a family: a create makes the following reads
    # meaningful. Reads within a family keep registry order.
    def sort_key(case: dict) -> tuple:
        method = case["method"]
        rank = 0 if method in MUTATING_VERBS else 1
        return (rank, case["path"], method)

    cases.sort(key=sort_key)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    with args.out.open("w", encoding="utf-8") as fh:
        for case in cases:
            fh.write(json.dumps(case, sort_keys=True) + "\n")

    print(f"families: {len(families)}")
    print(f"cases written: {len(cases)} -> {args.out}")
    if skipped_missing:
        print(f"skipped (no seed id): {len(skipped_missing)}")
        if "-v" in sys.argv:
            for line in skipped_missing[:40]:
                print(f"  {line}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
