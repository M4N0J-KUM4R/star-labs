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
import tomllib
from pathlib import Path

PASSED = re.compile(r"^Passed: (\d+)/(\d+)$", re.MULTILINE)
FAILING = re.compile(r"^### ([A-Za-z0-9_.-]+)$", re.MULTILINE)


FLAKY_LIST = Path(__file__).resolve().parent / "replay_flaky_cases.txt"


def flaky_cases(path: Path) -> set[str]:
    """The cases whose answer depends on what the session mutated.

    The corpus's cases share one database, so a case that renders a whole
    table — the CSV exports — sees the rows the other cases created or
    deleted, and its answer moves with the order they ran in. Those cases
    cannot be compared exactly, so they are listed here and excluded from
    the gate; they are still reported.
    """
    if not path.is_file():
        return set()
    out: set[str] = set()
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        out.add(line.split()[0])
    return out


def read(path: Path, cases: set[str]) -> set[str]:
    text = path.read_text(encoding="utf-8")
    header, _, details = text.partition("## Failing cases\n")
    counts = PASSED.findall(header)
    if len(counts) != 1:
        raise ValueError(f"{path}: expected one Passed: N/M line")
    passed, total = map(int, counts[0])
    if not cases or total != len(cases):
        raise ValueError(f"{path}: report covers {total}, corpus contains {len(cases)}")
    if re.search(r"^Skipped: [1-9]\d*$", header, re.MULTILINE):
        raise ValueError(f"{path}: skipped cases are not parity evidence")
    names = FAILING.findall(details)
    failures = set(names)
    if len(names) != len(failures) or passed + len(failures) != total:
        raise ValueError(f"{path}: pass count and unique failure headings do not reconcile")
    if failures - cases:
        raise ValueError(f"{path}: unrecorded case(s): {', '.join(sorted(failures - cases))}")
    if not failures and "Every case matches." not in header.splitlines():
        raise ValueError(f"{path}: missing all-pass verdict")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--committed", type=Path, required=True)
    parser.add_argument("--fresh", type=Path, required=True)
    parser.add_argument("--cases", type=Path, help="default: http directory beside fresh report")
    parser.add_argument("--exit-code", type=int, help="exit status of rust-anything app replay")
    parser.add_argument("--summary", type=Path, help="append a GitHub Actions job summary")
    args = parser.parse_args()
    corpus = args.cases or args.fresh.parent / "http"
    try:
        cases = {p.stem for p in corpus.glob("*.toml")}
        committed = read(args.committed, cases)
        fresh = read(args.fresh, cases)
        if args.exit_code is not None and args.exit_code != int(bool(fresh)):
            raise ValueError(f"replay exit {args.exit_code} inconsistent with {len(fresh)} failures")
    except (OSError, ValueError) as error:
        print(f"compare_replay: {error}", file=sys.stderr)
        return 2

    total = len(cases)
    passed = total - len(fresh)
    if args.summary:
        with args.summary.open("a", encoding="utf-8") as summary:
            successful = set()
            for case in sorted(cases):
                with (corpus / f"{case}.toml").open("rb") as fixture:
                    if 200 <= tomllib.load(fixture)["response"]["status"] < 300:
                        successful.add(case)
            print(f"### Rails → Rust: recorded HTTP behaviour\n\n"
                  f"**{passed}/{total} matched ({passed / total:.1%}); "
                  f"{len(fresh)} mismatched.** Of cases where Rails returned 2xx, "
                  f"**{len(successful - fresh)}/{len(successful)}** matched. "
                  "This measures replay cases, not "
                  "registered routes or overall migration completion. "
                  "Green means the known failure set was reproduced. "
                  "See the replay-report artifact for case details.", file=summary)

    flaky = flaky_cases(FLAKY_LIST)
    newly_failing = sorted((fresh - committed) - flaky)
    newly_passing = sorted((committed - fresh) - flaky)
    moved = sorted(((fresh - committed) | (committed - fresh)) & flaky)

    print(f"committed: {total - len(committed)}/{total} passing, {len(committed)} failing")
    print(f"fresh:     {passed}/{total} passing, {len(fresh)} failing")
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
    if moved:
        print(f"\n{len(moved)} order-dependent case(s) moved (not gated):")
        for name in moved:
            print(f"  {name}")
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
