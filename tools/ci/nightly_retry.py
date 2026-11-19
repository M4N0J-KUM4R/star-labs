#!/usr/bin/env python3
"""Classify cargo-test failures for the nightly Supabase shards.

Reads a combined cargo test log (stdout+stderr, ANSI colors allowed) and
decides whether the failed tests may be retried.

    python3 tools/ci/nightly_retry.py <logfile>

stdout: the failed test names, one per line (empty when none were collected).
exit 0: every failed test's stdout section looks like infrastructure noise
        (session-pooler saturation / connection errors) -> a retry is allowed.
exit 1: at least one failure looks real -> no retry, fail the shard now.

The gate can only cause a *pointless retry*, never mask a failure: whatever
it lets through is re-run, and a persistent failure fails the retry too.

Dependency-free on purpose (runs on the CI runner inside the shard job).
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
# Optional GitHub Actions log prefix ("2026-11-19T15:04:11.0568100Z "),
# so downloaded job logs can be classified locally too.
GH_PREFIX = re.compile(r"^\d{4}-\d{2}-\d{2}T[\d:.]+Z ?")

# Downstream messages that infra grazes produce in this workspace. A section
# matching any of these is treated as retriable infrastructure noise.
INFRA_PATTERNS = (
    "EMAXCONNSESSION",
    "max clients reached",
    "database error",
    "connect to postgres",
    "error attempting to connect",
    "pool timed out while waiting",
)


def strip_ansi(line: str) -> str:
    return GH_PREFIX.sub("", ANSI.sub("", line))


def parse(text: str) -> tuple[list[str], dict[str, str]]:
    """Return (failed test names, stdout-section bodies by test name).

    libtest emits, per failing binary:

        failures:

        ---- <name> stdout ----
        <body>

        failures:
            <name1>
            <name2>

        test result: FAILED. ...

    The indented block after the *second* `failures:` is the authoritative
    name list; the `---- name stdout ----` sections carry the reasons.
    """
    lines = [strip_ansi(ln).rstrip("\n") for ln in text.splitlines()]

    bodies: dict[str, str] = {}
    current: str | None = None
    buf: list[str] = []
    names: list[str] = []
    expect_names = False  # right after a `failures:` header

    name_re = re.compile(r"^[A-Za-z0-9_:]+$")

    def flush() -> None:
        nonlocal current, buf
        if current is not None:
            bodies[current] = "\n".join(buf)
        current, buf = None, []

    for ln in lines:
        m = re.match(r"^---- (\S+) stdout ----$", ln)
        if m:
            flush()
            expect_names = False
            current = m.group(1)
            continue
        if ln == "failures:":
            flush()
            expect_names = True
            continue
        if expect_names:
            if ln.startswith("    ") and name_re.match(ln.strip()):
                names.append(ln.strip())
                continue
            expect_names = False  # blank or non-name line ends the block
            if not ln.strip():
                continue
        if current is not None:
            if ln.strip():
                buf.append(ln)
            elif buf:
                flush()  # blank after body content ends the section
            # else: blank line right after the header — keep collecting
    flush()
    return names, bodies


def is_infra(body: str | None) -> bool:
    if body is None:
        return False
    low = body.lower()
    return any(p.lower() in low for p in INFRA_PATTERNS)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    text = Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")

    names, bodies = parse(text)
    # keep the last occurrence (re-parse safety) and preserve order
    seen: dict[str, None] = {}
    for n in names:
        seen.setdefault(n, None)
    failed = list(seen)

    real = [n for n in failed if not is_infra(bodies.get(n))]
    for n in failed:
        verdict = "infra" if is_infra(bodies.get(n)) else (
            "REAL" if n in real else "no-stdout"
        )
        print(f"[{verdict:8s}] {n}", file=sys.stderr)

    for n in failed:
        print(n)
    return 1 if real else 0


if __name__ == "__main__":
    sys.exit(main())
