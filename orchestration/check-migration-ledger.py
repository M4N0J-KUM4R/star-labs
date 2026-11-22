#!/usr/bin/env python3
"""Consistency check for the migration reservation ledger (issue #136).

Proves the acceptance invariants of the ledger rewrite:

  1. Every number present in migration/crates/star-db/migrations/ is marked
     'merged' in orchestration/migration-reservations.json — and conversely,
     every 'merged' reservation has a file on disk.
  2. No reservation number below next_available is listed free: the
     reservations map and the free list are disjoint, and free contains no
     merged number and no duplicate.
  3. No file under specs/** contains a zero-padded four-digit token
     (regex 0[0-9]{3}, not embedded in a longer digit run). In this repo
     every such token is a migration number, so specs must carry none.

Additional ledger-shape invariants validated along the way:

  - numbers are 4-digit strings; status is 'open' or 'merged';
  - max_merged equals the highest migration file number;
  - every reservation and free number is numerically below next_available;
  - every 'open' reservation carries a GitHub issue number ('merged' entries
    may predate the issue protocol and carry issue: null).

Exit code 0 = all invariants hold. Exit code 1 = violations listed.

Usage: python3 orchestration/check-migration-ledger.py [repo-root]
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

MIGRATIONS_DIR = Path("migration/crates/star-db/migrations")
LEDGER_FILE = Path("orchestration/migration-reservations.json")
SPECS_DIR = Path("specs")

NUM_RE = re.compile(r"^[0-9]{4}$")
# Acceptance regex 0[0-9]{3}, anchored so it does not fire inside longer
# digit runs (e.g. a GitHub Actions run id such as 44365960767) while still
# catching bare numbers ("0107"), ranges ("0101-0106"), suffixes
# ("0108+", "0110_assignment_workflow_state.sql"). Non-concrete shorthand
# like "010x" cannot match 0[0-9]{3} by construction (fourth char is 'x').
SPEC_TOKEN_RE = re.compile(r"(?<![0-9])0[0-9]{3}(?![0-9])")
MIGRATION_FILE_RE = re.compile(r"^([0-9]{4})_[a-z0-9_]+\.sql$")

errors: list[str] = []


def err(msg: str) -> None:
    errors.append(msg)


def check_number_string(value: object, what: str) -> bool:
    if not isinstance(value, str) or not NUM_RE.match(value):
        err(f"{what}: expected 4-digit string, got {value!r}")
        return False
    return True


def read_migration_numbers(root: Path) -> set[str]:
    numbers: set[str] = set()
    for entry in sorted((root / MIGRATIONS_DIR).iterdir()):
        if not entry.is_file():
            err(f"unexpected non-file in migrations dir: {entry.name}")
            continue
        match = MIGRATION_FILE_RE.match(entry.name)
        if match is None:
            err(f"unexpected migration filename: {entry.name}")
            continue
        numbers.add(match.group(1))
    return numbers


def check_ledger(root: Path, merged_files: set[str]) -> dict:
    ledger_path = root / LEDGER_FILE
    try:
        ledger = json.loads(ledger_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        err(f"cannot load ledger {ledger_path}: {exc}")
        return {}

    if not isinstance(ledger, dict):
        err("ledger root must be a JSON object")
        return ledger if isinstance(ledger, dict) else {}

    for key in ("max_merged", "next_available", "free", "reservations"):
        if key not in ledger:
            err(f"ledger missing required key: {key}")

    max_merged = ledger.get("max_merged")
    next_available = ledger.get("next_available")
    free = ledger.get("free")
    reservations = ledger.get("reservations")

    check_number_string(max_merged, "max_merged")
    check_number_string(next_available, "next_available")

    if not isinstance(free, list):
        err("free must be a list")
        free = []
    if not isinstance(reservations, dict):
        err("reservations must be an object")
        reservations = {}

    if isinstance(next_available, str) and NUM_RE.match(next_available):
        next_avail_int = int(next_available)
        for value in free:
            if not check_number_string(value, "free entry"):
                continue
            if int(value) >= next_avail_int:
                err(f"free number {value} is not below next_available {next_available}")
        for key in reservations:
            if not check_number_string(key, "reservation key"):
                continue
            if int(key) >= next_avail_int:
                err(f"reservation {key} is not below next_available {next_available}")

    # Acceptance 1a: every file number is marked merged.
    for number in sorted(merged_files):
        entry = reservations.get(number)
        if entry is None:
            err(f"migration file {number}_*.sql exists but has no ledger reservation")
        elif not isinstance(entry, dict) or entry.get("status") != "merged":
            err(f"migration file {number}_*.sql exists but ledger status is "
                f"{entry.get('status') if isinstance(entry, dict) else entry!r}, expected 'merged'")

    # Acceptance 1b (converse): merged reservations must have files.
    for key, entry in sorted(reservations.items()):
        if isinstance(entry, dict) and entry.get("status") == "merged" and key not in merged_files:
            err(f"ledger marks {key} as merged but no {key}_*.sql file exists")

    # Entry shape: status open|merged; open entries need an issue number.
    for key, entry in sorted(reservations.items()):
        if not isinstance(entry, dict):
            err(f"reservation {key} must be an object, got {entry!r}")
            continue
        status = entry.get("status")
        if status not in ("open", "merged"):
            err(f"reservation {key} has invalid status {status!r}")
        issue = entry.get("issue")
        if status == "open" and not isinstance(issue, int):
            err(f"open reservation {key} must reference an issue number, got {issue!r}")
        if issue is not None and not isinstance(issue, int):
            err(f"reservation {key} issue must be an integer or null, got {issue!r}")

    # Acceptance 2: reservations and free are disjoint; free has no dupes and
    # no merged numbers.
    if len(free) != len(set(free)):
        err(f"free contains duplicates: {sorted(free)}")
    for value in free:
        if value in reservations:
            err(f"free number {value} is also a reservation")
        if value in merged_files:
            err(f"free number {value} has a merged migration file")

    # Sanity: max_merged must equal the highest file number.
    if isinstance(max_merged, str) and NUM_RE.match(max_merged) and merged_files:
        highest = max(merged_files, key=int)
        if max_merged != highest:
            err(f"max_merged is {max_merged} but the highest migration file number is {highest}")

    return ledger


def check_specs(root: Path) -> int:
    specs_root = root / SPECS_DIR
    if not specs_root.is_dir():
        err(f"specs directory missing: {specs_root}")
        return 0
    scanned = 0
    for path in sorted(specs_root.rglob("*")):
        if not path.is_file():
            continue
        scanned += 1
        text = path.read_text(encoding="utf-8", errors="replace")
        for lineno, line in enumerate(text.splitlines(), start=1):
            for match in SPEC_TOKEN_RE.finditer(line):
                rel = path.relative_to(root)
                err(f"{rel}:{lineno}: migration-context token {match.group(0)!r} "
                    f"in specs is forbidden (ledger numbers live only in "
                    f"orchestration/migration-reservations.json): ...{line.strip()[:100]}")
    return scanned


def main() -> int:
    root = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent

    if not (root / MIGRATIONS_DIR).is_dir():
        err(f"migrations directory missing: {root / MIGRATIONS_DIR}")

    merged_files = read_migration_numbers(root)
    ledger = check_ledger(root, merged_files)
    scanned = check_specs(root)

    reservations = ledger.get("reservations", {}) if isinstance(ledger, dict) else {}
    open_count = sum(1 for e in reservations.values()
                     if isinstance(e, dict) and e.get("status") == "open")
    merged_count = sum(1 for e in reservations.values()
                       if isinstance(e, dict) and e.get("status") == "merged")
    free = ledger.get("free", []) if isinstance(ledger, dict) else []

    print(f"migration files on disk : {len(merged_files)}")
    print(f"ledger merged/open/free : {merged_count}/{open_count}/{len(free) if isinstance(free, list) else '?'}")
    print(f"specs files scanned     : {scanned}")

    if errors:
        print(f"\nFAIL: {len(errors)} violation(s):")
        for message in errors:
            print(f"  - {message}")
        return 1
    print("\nOK: ledger matches the migrations dir, reservations and free are "
          "disjoint, and specs carry no migration numbers.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
