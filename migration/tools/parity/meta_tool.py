#!/usr/bin/env python3
"""Handle Canvas' `x-canvas-meta` header, which cannot be compared byte-for-byte.

The problem
-----------
Canvas' `x-canvas-meta` carries process diagnostics alongside the contract:

    at=10000000000007;dk=10000000000001;o=accounts;n=index;b=0;m=0;u=0.00;y=0.00;d=0.00;rlr=600.0;

- **stable** — `at` (access token), `dk` (developer key), `o` (controller),
  `n` (action), `t` (type), `i` (id), `rlr` (rate limit remaining). These are
  the contract and must match exactly.
- **volatile** — `b` (bytes), `m` (memory), `u` (cpu seconds), `y` (memory MB),
  `d` (db seconds). These are the *Canvas server's* numbers on the machine that
  answered. A Rust port can never reproduce another process's memory bytes or
  its query timing, and the recorder itself wildcarded them inline
  (`b=[..]`), which the text differ does not treat as a pattern.

The fix
-------
1. `normalize` moves the recorded meta out of the compared headers and into a
   side file (`<case>.meta`), leaving the header value as the whole-value
   wildcard `[..]` — which the differ *does* honour.
2. `check` replays the cases against the port and compares the **stable fields
   only**, field by field, so a wrong `at=`, `dk=`, `o=` or `n=` is still a
   failure.

    python3 tools/parity/meta_tool.py normalize rust/app-cases/http
    python3 tools/parity/meta_tool.py check rust/app-cases/http --url http://127.0.0.1:8099
    python3 tools/parity/meta_tool.py restore  rust/app-cases/http
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

WILDCARD = "[..]"
META = "x-canvas-meta"

# The fields that are part of the contract. Everything else in the header is
# a number about the process that answered.
STABLE = ("at", "dk", "o", "n", "t", "i", "rlr")


def parse_meta(value: str) -> dict[str, str]:
    out: dict[str, str] = {}
    for part in value.split(";"):
        part = part.strip()
        if not part or "=" not in part:
            continue
        key, _, val = part.partition("=")
        out[key.strip()] = val.strip()
    return out


def normalise(case_dir: Path) -> int:
    changed = 0
    for toml in sorted(case_dir.glob("*.toml")):
        text = toml.read_text(encoding="utf-8")
        m = re.search(rf'^{META} = "([^"]*)"$', text, re.M)
        if not m:
            continue
        recorded = m.group(1)
        side = toml.with_suffix(".meta")
        if not side.exists():
            side.write_text(recorded + "\n", encoding="utf-8")
        text = text[: m.start()] + f'{META} = "{WILDCARD}"' + text[m.end():]
        toml.write_text(text, encoding="utf-8")
        changed += 1
    return changed


def restore(case_dir: Path) -> int:
    changed = 0
    for toml in sorted(case_dir.glob("*.toml")):
        side = toml.with_suffix(".meta")
        if not side.is_file():
            continue
        recorded = side.read_text(encoding="utf-8").strip()
        text = toml.read_text(encoding="utf-8")
        m = re.search(rf'^{META} = "{re.escape(WILDCARD)}"$', text, re.M)
        if not m:
            continue
        text = text[: m.start()] + f'{META} = "{recorded}"' + text[m.end():]
        toml.write_text(text, encoding="utf-8")
        side.unlink()
        changed += 1
    return changed


def read_cases(case_dir: Path) -> list[dict]:
    cases = []
    for toml in sorted(case_dir.glob("*.toml")):
        text = toml.read_text(encoding="utf-8")
        method = re.search(r'^method = "([^"]*)"$', text, re.M)
        path = re.search(r'^path = "([^"]*)"$', text, re.M)
        header = re.search(r'^Authorization = "([^"]*)"$', text, re.M)
        status = re.search(r"^status = (\d+)$", text, re.M)
        meta = toml.with_suffix(".meta")
        recorded_meta = meta.read_text(encoding="utf-8").strip() if meta.is_file() else ""
        if not (method and path):
            continue
        cases.append({
            "name": toml.stem,
            "method": method.group(1),
            "path": path.group(1),
            "authorization": header.group(1) if header else None,
            "status": int(status.group(1)) if status else None,
            "meta": recorded_meta,
        })
    return cases


def check(case_dir: Path, base_url: str, timeout: float) -> int:
    cases = read_cases(case_dir)
    failures: list[tuple[str, str, str, str]] = []
    checked = 0
    for case in cases:
        if not case["meta"] or not case["authorization"]:
            continue
        checked += 1
        req = urllib.request.Request(
            base_url.rstrip("/") + case["path"],
            method=case["method"],
            headers={"Authorization": case["authorization"]},
        )
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                got = resp.headers.get(META, "")
                status = resp.status
        except urllib.error.HTTPError as exc:
            got = exc.headers.get(META, "") if exc.headers else ""
            status = exc.code
        except Exception as exc:  # noqa: BLE001
            failures.append((case["name"], "request", str(exc), ""))
            continue

        want = parse_meta(case["meta"])
        have = parse_meta(got)
        for field in STABLE:
            if field not in want:
                continue
            if want[field] != have.get(field):
                failures.append((case["name"], field, want[field], have.get(field, "<missing>")))
        if case["status"] is not None and status != case["status"]:
            failures.append((case["name"], "status", str(case["status"]), str(status)))

    print(f"checked {checked} cases; {len(failures)} stable-field differences")
    by_field: dict[str, int] = {}
    for _, field, _, _ in failures:
        by_field[field] = by_field.get(field, 0) + 1
    for field, n in sorted(by_field.items(), key=lambda kv: -kv[1]):
        print(f"  {field:<8} {n}")
    limit = int(os.environ.get("META_CHECK_SHOW", "20"))
    for name, field, want, got in failures[:limit]:
        print(f"  {name}: {field} expected {want!r} got {got!r}")
    return 1 if failures else 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("action", choices=["normalize", "restore", "check"])
    ap.add_argument("case_dir", type=Path)
    ap.add_argument("--url", default="http://127.0.0.1:8099")
    ap.add_argument("--timeout", type=float, default=15.0)
    args = ap.parse_args()

    if args.action == "normalize":
        n = normalise(args.case_dir)
        print(f"normalised {n} cases; the recorded meta is in <case>.meta")
        return 0
    if args.action == "restore":
        n = restore(args.case_dir)
        print(f"restored {n} cases")
        return 0
    return check(args.case_dir, args.url, args.timeout)


if __name__ == "__main__":
    import os
    raise SystemExit(main())
