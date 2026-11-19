#!/usr/bin/env python3
"""Classify a pull request's changed files for the CI fast path (wave 4).

Input:  a GitHub `compare` API JSON payload (``base...head``) on stdin or as
the first positional argument. The payload's ``files`` array is what we
classify; ``truncated: true`` (or an unreadable payload) degrades to the
full, slower run.

Output: ``rust=true|false`` and ``lab=true|false`` lines, appended to the
file named by ``--github-output`` (pass ``$GITHUB_OUTPUT``) so downstream
jobs can gate on ``needs.changes.outputs.rust`` / ``.lab``. A human-readable
summary always goes to stdout.

Rules — single source of truth for all three workflows that use the fast
path (rust-migration, production-build, bootstrap):

``rust`` (anything that can change compilation, tests, or the CI machinery
that gates them):
  - ``migration/**`` except ``*.md`` documentation
  - ``tools/ci/**``
  - ``.github/workflows/rust-migration.yml``
  - ``.github/workflows/production-build.yml``

``lab`` (anything the bootstrap lab-constraint checks validate):
  - ``infra/**``, ``scripts/**``
  - ``.github/workflows/bootstrap.yml``

Non-pull_request events never reach this script; the workflows short-circuit
them to a full run.
"""
from __future__ import annotations

import argparse
import json
import sys

RUST_EXACT = {
    ".github/workflows/rust-migration.yml",
    ".github/workflows/production-build.yml",
}
RUST_PREFIXES = ("migration/", "tools/ci/")

LAB_EXACT = {".github/workflows/bootstrap.yml"}
LAB_PREFIXES = ("infra/", "scripts/")

# The compare API returns at most 300 files; treat the cap as truncation.
COMPARE_FILE_CAP = 300


def classify(files: list[str]) -> tuple[bool, bool]:
    """Return (rust, lab) booleans for the given changed-file paths."""
    rust = False
    lab = False
    for path in files:
        is_doc = path.endswith(".md")
        if path in RUST_EXACT or (path.startswith(RUST_PREFIXES) and not is_doc):
            rust = True
        if path in LAB_EXACT or path.startswith(LAB_PREFIXES):
            lab = True
    return rust, lab


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "payload",
        nargs="?",
        default="-",
        help="compare API JSON payload path, or '-' / omitted for stdin",
    )
    parser.add_argument(
        "--github-output",
        default=None,
        help="file to append 'rust=' / 'lab=' outputs to ($GITHUB_OUTPUT)",
    )
    args = parser.parse_args()

    try:
        raw = sys.stdin.read() if args.payload in ("-", "") else open(args.payload).read()
        payload = json.loads(raw)
    except (OSError, json.JSONDecodeError) as exc:
        print(f"::warning::could not read compare payload ({exc}); defaulting to full run")
        payload = None

    if not isinstance(payload, dict) or not isinstance(payload.get("files"), list):
        # A real compare payload always carries a `files` array; anything else
        # is an unknown shape — degrade to the full run.
        print("::warning::payload is not a compare response; defaulting to full run")
        payload = {"truncated": True, "files": []}

    files = [f.get("filename", "") for f in payload.get("files") or [] if isinstance(f, dict)]
    truncated = bool(payload.get("truncated")) or len(files) >= COMPARE_FILE_CAP

    if truncated:
        print("::warning::compare payload truncated (>=300 files); defaulting to full run")
        rust, lab = True, True
    else:
        rust, lab = classify(files)

    lines = [f"rust={str(rust).lower()}", f"lab={str(lab).lower()}"]
    for line in lines:
        if args.github_output:
            with open(args.github_output, "a") as fh:
                fh.write(line + "\n")
        print(line)

    if not truncated:
        matched = [p for p in files if classify([p]) != (False, False)]
        print(f"classified {len(files)} changed files; "
              f"{len(matched)} match a fast-path class:")
        for path in matched[:20]:
            print(f"  {path}")
        if len(matched) > 20:
            print(f"  ... and {len(matched) - 20} more")
    return 0


if __name__ == "__main__":
    sys.exit(main())
