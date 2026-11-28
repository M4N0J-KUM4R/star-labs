#!/usr/bin/env python3
"""Generate `src/routes.rs` — the Canvas route table the middleware needs.

The middleware has to answer two Canvas-specific questions for every request:

* which controller/action Canvas would have run, so it can write
  `o=<controller>;n=<action>;` into the `x-canvas-meta` header
  (canvas-src/gems/request_context/lib/request_context/generator.rb:123-124),
* whether the response is a paginated list, so it can write the RFC-5988
  `Link` header Canvas writes for `Api.paginate` calls
  (canvas-src/lib/api.rb:412-413).

Both answers come from Canvas' own route registry, which this repository
already keeps in `migration/parity/canvas_manifest.json` (verb, path,
controller, action for all 1403 API routes).

Usage:
    python3 tools/gen_routes.py \
        --manifest ../../parity/canvas_manifest.json \
        --out ../src/routes.rs

`--canvas-src` optionally re-scans the Canvas tree for the actions that call
`Api.paginate` / `api_render_stream`, so the "this route is a paginated list"
flag is derived from Canvas source rather than hand-maintained. Without it the
baked-in list below is used.
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import re
from pathlib import Path

# ---------------------------------------------------------------------------
# Canvas actions that paginate but do not call Api.paginate directly.
#
# canvas-src/app/controllers/courses_controller.rb:530 (index) delegates to
# `courses_for_user`, and :772 (user_index) to the same helper, so neither
# action contains an `Api.paginate` call of its own.
# canvas-src/app/controllers/calendar_events_api_controller.rb:372 (index) and
# :421 (user_index) delegate to a helper at :451.
# The `activity_stream` actions call `api_render_stream`
# (canvas-src/lib/api/v1/stream_item.rb:131), which paginates internally.
# ---------------------------------------------------------------------------
EXTRA_PAGINATED_ACTIONS = {
    ("courses", "index"),
    ("courses", "user_index"),
    ("calendar_events_api", "index"),
    ("calendar_events_api", "user_index"),
    ("courses", "activity_stream"),
    ("groups", "activity_stream"),
    ("users", "activity_stream"),
}

# ---------------------------------------------------------------------------
# Canvas actions whose collection is a *bookmarked* collection.
#
# A bookmarked collection pages with an opaque bookmark token instead of a
# page number, so its first page is the literal token "first"
# (canvas-src/gems/bookmarked_collection/lib/bookmarked_collection/collection.rb:24
# and :100-106). Canvas reaches for one in two ways:
#
#   * `ShardedBookmarkedCollection.build(..., always_use_bookmarks: true)`
#     - canvas-src/app/controllers/enrollments_api_controller.rb:528
#     - canvas-src/app/controllers/accounts_controller.rb:487
#   * a plain `BookmarkedCollection.wrap`/`BookmarkedCollection.merge`
#     - canvas-src/app/controllers/courses_controller.rb:1495-1526 (todo_items)
#     - canvas-src/app/controllers/users_controller.rb:983-1052 (todo_items)
#     - canvas-src/app/controllers/users_controller.rb:3012-3016
#       (user_graded_submissions)
#
# A `ShardedBookmarkedCollection` *without* `always_use_bookmarks` pages
# numerically (canvas-src/app/controllers/groups_controller.rb:265), so
# groups#index is deliberately absent here.
# ---------------------------------------------------------------------------
BOOKMARKED_ACTIONS = {
    ("accounts", "course_accounts"),
    ("courses", "todo_items"),
    ("enrollments_api", "index"),
    ("groups", "todo_items"),
    ("users", "todo_items"),
    ("users", "user_graded_submissions"),
}

# ---------------------------------------------------------------------------
# Canvas actions the source scan marks as paginated, but which do not answer
# with a `Link` header.
#
# The recorded corpus is the contract, and it shows no `Link` header for
# these four (all recorded with status 200):
#
#   /api/v1/accounts/:account_id/sub_accounts
#   /api/v1/accounts/:account_id/quiz_ip_filters
#   /api/v1/courses/:course_id/recent_students
#   /api/v1/conversations/find_recipients
#
# `search#recipients` is the odd one out: the action lives in
# `search_controller.rb`, but the route is the conversations one, and
# Canvas does not paginate it.
# ---------------------------------------------------------------------------
NOT_PAGINATED_ACTIONS = {
    ("accounts", "sub_accounts"),
    ("accounts", "quiz_ip_filters"),
    ("courses", "recent_students"),
    ("search", "recipients"),
}

# ---------------------------------------------------------------------------
# Per-page defaults.
#
# canvas-src/lib/api.rb:22 (PER_PAGE = 10) is the default for every paginated
# collection. `api_render_stream` overrides it with 21
# (canvas-src/lib/api/v1/stream_item.rb:160).
# ---------------------------------------------------------------------------
DEFAULT_PER_PAGE = 10
ACTIVITY_STREAM_PER_PAGE = 21
ACTIVITY_STREAM_ACTIONS = {
    ("courses", "activity_stream"),
    ("groups", "activity_stream"),
    ("users", "activity_stream"),
}


def scan_canvas_paginated(canvas_src: Path, controllers: list[str]) -> set[tuple[str, str]]:
    """Return the (controller, action) pairs that paginate in Canvas.

    `controllers` are the controller names the manifest uses (for example
    `enrollments_api`); the matching Canvas file is
    `app/controllers/<name>_controller.rb`.
    """
    root = canvas_src / "app" / "controllers"
    found: set[tuple[str, str]] = set()
    for name in controllers:
        path = root / (name.replace("/", "_") + "_controller.rb")
        if not path.is_file():
            continue
        action = None
        for line in path.read_text().splitlines():
            match = re.match(r"\s*def ([A-Za-z0-9_?!]+)", line)
            if match:
                action = match.group(1)
            if action and re.search(
                r"Api\.(paginate|jsonapi_paginate)\(|api_render_stream\(", line
            ):
                found.add((name, action))
    return found


def build_rows(manifest: dict, paginated: set[tuple[str, str]]) -> list[dict]:
    rows = []
    for route in manifest["routes"]:
        controller = route["controller"]
        action = route["action"]
        key = (controller, action)
        is_paginated = (
            key in paginated or key in EXTRA_PAGINATED_ACTIONS
        ) and key not in NOT_PAGINATED_ACTIONS
        per_page = DEFAULT_PER_PAGE
        if key in ACTIVITY_STREAM_ACTIONS:
            per_page = ACTIVITY_STREAM_PER_PAGE
        rows.append(
            {
                "verb": route["verb"],
                "path": route["path"],
                "controller": controller,
                "action": action,
                "paginated": is_paginated,
                "bookmarked": is_paginated and key in BOOKMARKED_ACTIONS,
                "per_page": per_page,
            }
        )
    rows.sort(key=lambda r: (r["path"], r["verb"]))
    return rows


def render(rows: list[dict]) -> str:
    out = [
        "// @generated by tools/gen_routes.py — do not edit by hand.",
        "//",
        "// Source: migration/parity/canvas_manifest.json (the Canvas route registry:",
        "// verb, path, controller and action for every API route), plus the Canvas",
        "// source citations in tools/gen_routes.py for the pagination flags.",
        "//",
        "// Regenerate with:",
        "//   python3 tools/gen_routes.py --manifest ../../parity/canvas_manifest.json \\",
        "//       --out ../src/routes.rs",
        "",
        "/// One Canvas API route, with the pagination facts the middleware needs.",
        "pub struct Route {",
        "    /// HTTP verb, as Canvas declares it.",
        "    pub verb: &'static str,",
        "    /// Canvas route template, e.g. `/api/v1/courses/:course_id/users`.",
        "    pub path: &'static str,",
        "    /// Canvas controller name — the `o=` field of `x-canvas-meta`.",
        "    pub controller: &'static str,",
        "    /// Canvas action name — the `n=` field of `x-canvas-meta`.",
        "    pub action: &'static str,",
        "    /// True when Canvas paginates this action and answers with a",
        "    /// RFC-5988 `Link` header.",
        "    pub paginated: bool,",
        "    /// True when the collection is bookmark-paged, so the page token is",
        "    /// the literal `first` instead of a page number.",
        "    pub bookmarked: bool,",
        "    /// Canvas' default page size for this action.",
        "    pub per_page: u32,",
        "}",
        "",
        "/// Every Canvas API route the port knows about.",
        "pub static ROUTES: &[Route] = &[",
    ]
    for row in rows:
        out.append(
            "    Route {{ verb: \"{verb}\", path: \"{path}\", controller: \"{controller}\", "
            "action: \"{action}\", paginated: {paginated}, bookmarked: {bookmarked}, "
            "per_page: {per_page} }},".format(
                verb=row["verb"],
                path=row["path"],
                controller=row["controller"],
                action=row["action"],
                paginated=str(row["paginated"]).lower(),
                bookmarked=str(row["bookmarked"]).lower(),
                per_page=row["per_page"],
            )
        )
    out.append("];")
    out.append("")
    return "\n".join(out)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--canvas-src", type=Path, default=None)
    args = parser.parse_args()

    manifest = json.loads(args.manifest.read_text())
    controllers = sorted({r["controller"] for r in manifest["routes"]})
    if args.canvas_src and args.canvas_src.is_dir():
        paginated = scan_canvas_paginated(args.canvas_src, controllers)
    else:
        paginated = set()
    rows = build_rows(manifest, paginated)
    args.out.write_text(render(rows))
    paginated_count = sum(1 for r in rows if r["paginated"])
    print(f"routes: {len(rows)} (paginated: {paginated_count}) -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
