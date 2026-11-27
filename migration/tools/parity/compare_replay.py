#!/usr/bin/env python3
"""Compare the committed replay report with a fresh one.

The report's detail depends on the database it ran against: the diff of a
case shows the bodies, and the bodies differ when the database holds
different rows. The set of failing cases is the contract, not the prose, so
this compares the two sets.

Usage:
    compare_replay.py --committed rust/app-cases/replay-report.md \
                      --fresh rust/app-cases/replay-report.md
"""
from __future__ import annotations

import argparse
import re
import sys

PASSED = re.compile(r"^Passed:\s*(\d+)/(\d+)\s*$", re.MULTILINE)
FAILING = re.compile(r"^###\s+([A-Za-z0-9_.-]+)\s*$", re.MULTILINE)


def read(path: str) -> tuple[set[str], int | None, int | None]:
    try:
        text = open(path, encoding="utf-8", errors="replace").read()
    except FileNotFoundError:
        return set(), None, None
    m = PASSED.search(text)
    passed = int(m.group(1)) if m else None
    total = int(m.group(2)) if m else None
    return set(FAILING.findall(text)), passed, total


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--committed", required=True)
    parser.add_argument("--fresh", required=True)
    args = parser.parse_args()

    committed, c_passed, c_total = read(args.committed)
    fresh, f_passed, f_total = read(args.fresh)

    if not fresh:
        print("compare_replay: no fresh report to compare against", file=sys.stderr)
        return 2

    newly_failing = sorted(fresh - committed)
    newly_passing = sorted(committed - fresh)

    print(f"committed: {c_passed}/{c_total} passing, {len(committed)} failing")
    print(f"fresh:     {f_passed}/{f_total} passing, {len(fresh)} failing")
    if newly_failing:
        print(f"\n{len(newly_failing)} case(s) that pass in the committed report fail now:")
        for name in newly_failing:
            print(f"  {name}")
    if newly_passing:
        print(f"\n{len(newly_passing)} case(s) that fail in the committed report pass now:")
        for name in newly_passing:
            print(f"  {name}")

    if newly_failing:
        print("\ncompare_replay: the fresh run regressed", file=sys.stderr)
        return 1
    if newly_passing:
        # The committed report is the baseline: a case that passes now has to
        # be recorded as passing, or the next run compares against a stale
        # contract and a real regression hides behind it.
        print(
            "\ncompare_replay: the committed report is stale — commit the fresh one",
            file=sys.stderr,
        )
        return 1
    print("\ncompare_replay: the fresh run matches the committed report")
    return 0


if __name__ == "__main__":
    sys.exit(main())
