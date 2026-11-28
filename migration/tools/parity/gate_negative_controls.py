#!/usr/bin/env python3
"""Prove that the replay gate can turn red, using the real corpus.

The gate compares a fresh replay report with the committed baseline. A gate
that only ever says "the known failure set was reproduced" is a ratchet, not
a release gate: it cannot tell a port that lost behaviour from a port that
kept it. This script feeds the gate deliberately broken inputs and asserts
that each one is rejected.

The inputs are the real ones — the committed 460-case corpus and the
committed baseline report — so the controls exercise the same parser the
CI job uses. Nothing here is committed, moved or rewritten: every mutation
happens in a temporary directory.

Usage:
    python3 tools/parity/gate_negative_controls.py \
        --cases rust/app-cases/http \
        --baseline rust/app-cases/replay-report.md
"""

from __future__ import annotations

import argparse
import contextlib
import io
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import compare_replay  # noqa: E402

PASSED = re.compile(r"^Passed: (\d+)/(\d+)$", re.MULTILINE)
FAILING = re.compile(r"^### ([A-Za-z0-9_.-]+)$", re.MULTILINE)


class Control:
    """One controlled mutation of the inputs and the verdict it must get."""

    def __init__(self, name: str, must_pass: bool, why: str):
        self.name = name
        self.must_pass = must_pass
        self.why = why


def record(failures: list[str], control: Control, verdict: int) -> None:
    """Record whether the gate behaved the way this control demands."""
    ok = (verdict == 0) if control.must_pass else (verdict != 0)
    if not ok:
        wanted = "green" if control.must_pass else "red"
        failures.append(f"{control.name}: exit {verdict}, wanted {wanted}")


def run_gate(baseline: Path, fresh: Path, cases: Path, exit_code: int) -> int:
    argv = [
        "compare_replay.py",
        "--committed", str(baseline),
        "--fresh", str(fresh),
        "--cases", str(cases),
        "--exit-code", str(exit_code),
    ]
    old = sys.argv
    sys.argv = argv
    try:
        with contextlib.redirect_stderr(io.StringIO()), contextlib.redirect_stdout(io.StringIO()):
            return compare_replay.main()
    finally:
        sys.argv = old


def failing_cases(report: str) -> list[str]:
    _, _, details = report.partition("## Failing cases\n")
    return FAILING.findall(details)


def passing_cases(report: str, cases: Path) -> list[str]:
    failing = set(failing_cases(report))
    return sorted(p.stem for p in cases.glob("*.toml") if p.stem not in failing)


def rewrite_report(text: str, *, passed: int | None = None, drop: str | None = None,
                   add: str | None = None, skipped: int | None = None) -> str:
    """A copy of a report with one controlled change."""
    out = text
    if passed is not None:
        out = PASSED.sub(f"Passed: {passed}/{PASSED.search(text).group(2)}", out, count=1)
    if skipped is not None:
        out = out.replace("Passed: ", f"Skipped: {skipped}\nPassed: ", 1)
    if drop is not None:
        # Remove the whole section of that failing case.
        out = re.sub(
            rf"^### {re.escape(drop)}\n[\s\S]*?(?=^### |\Z)",
            "",
            out,
            flags=re.MULTILINE,
        )
    if add is not None:
        out += f"\n### {add}\n\nArguments: `GET /api/v1/synthetic/probe`\n\nat status (200→404)\n"
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    args = parser.parse_args()

    baseline_text = args.baseline.read_text(encoding="utf-8")
    cases = args.cases
    failing = failing_cases(baseline_text)
    passing = passing_cases(baseline_text, cases)
    total = len(list(cases.glob("*.toml")))
    if not failing or not passing:
        print("gate_negative_controls: the baseline has no failing or no passing case")
        return 2
    m = PASSED.search(baseline_text)
    if not m or int(m.group(2)) != total:
        print(f"gate_negative_controls: the baseline covers {m and m.group(2)}, corpus has {total}")
        return 2

    controls = [
        Control("reproduced baseline is accepted", True,
                "the gate must accept a run that reproduces the committed baseline"),
        Control("a missing capture is rejected", False,
                "a recorded case deleted from the corpus must not go unnoticed"),
        Control("a case dropped from the baseline is rejected", False,
                "a PR must not be able to erase a regression by editing its own baseline"),
        Control("a formerly passing case that now fails is rejected", False,
                "the gate must catch a regression, not only a difference in count"),
        Control("a partial replay is rejected", False,
                "a report whose pass count does not reconcile with its headings is not evidence"),
        Control("a skipped case is rejected", False,
                "a case the replay skipped is not parity evidence"),
        Control("an inconsistent runner exit code is rejected", False,
                "a broken runner must not be able to report success"),
        Control("an improvement that the baseline has not recorded is rejected", False,
                "the baseline has to be updated when a case starts passing"),
        Control("an all-pass run is accepted", True,
                "a genuinely clean run must be able to go green"),
    ]

    failures: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        corpus = root / "http"
        shutil.copytree(cases, corpus)
        baseline = root / "baseline.md"
        fresh = root / "fresh.md"

        def write_fresh(text: str) -> None:
            fresh.write_text(text, encoding="utf-8")

        def run(text: str, exit_code: int, corpus_dir: Path = corpus) -> int:
            baseline.write_text(baseline_text, encoding="utf-8")
            write_fresh(text)
            return run_gate(baseline, fresh, corpus_dir, exit_code)

        # 1 — the control: the baseline reproduced.
        verdict = run(baseline_text, 1)
        record(failures, controls[0], verdict)

        # 2 — a missing capture: one recorded case deleted from the corpus.
        victim = next(corpus.glob("*.toml"))
        victim.unlink()
        verdict = run(baseline_text, 1)
        record(failures, controls[1], verdict)
        shutil.copy2(cases / victim.name, corpus / victim.name)

        # 3 — the baseline edited to forget a failing case.
        verdict = run(rewrite_report(baseline_text, drop=failing[0]), 1)
        record(failures, controls[2], verdict)

        # 4 — a case that used to pass now fails.
        verdict = run(rewrite_report(baseline_text, add=passing[0]), 1)
        record(failures, controls[3], verdict)

        # 5 — a partial replay: the pass count does not reconcile.
        verdict = run(rewrite_report(baseline_text, passed=int(PASSED.search(baseline_text).group(1)) + 1), 1)
        record(failures, controls[4], verdict)

        # 6 — a skipped case.
        verdict = run(rewrite_report(baseline_text, skipped=1), 1)
        record(failures, controls[5], verdict)

        # 7 — the runner's exit code disagrees with the report.
        verdict = run(baseline_text, 0)
        record(failures, controls[6], verdict)

        # 8 — an improvement the baseline has not recorded.
        improved = rewrite_report(baseline_text, drop=failing[0], passed=int(PASSED.search(baseline_text).group(1)) + 1)
        verdict = run(improved, 1)
        record(failures, controls[7], verdict)

        # 9 — a genuinely clean run.
        clean = f"Passed: {total}/{total}\n\nEvery case matches.\n"
        baseline.write_text(clean, encoding="utf-8")
        write_fresh(clean)
        verdict = run_gate(baseline, fresh, corpus, 0)
        record(failures, controls[8], verdict)

    for control in controls:
        state = "ok" if control.name not in " ".join(failures) else "FAILED"
        print(f"[{state}] {control.name} — {control.why}")
    if failures:
        print("\ngate_negative_controls: the gate did not behave as required:")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print(f"\ngate_negative_controls: {len(controls)} controls behaved as required")
    return 0


if __name__ == "__main__":
    sys.exit(main())
