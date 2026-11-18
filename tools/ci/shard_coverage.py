#!/usr/bin/env python3
"""Assert the CI test-shard map covers every workspace crate exactly once.

A crate added to `migration/Cargo.toml` but not to a shard in
`.github/workflows/rust-migration.yml` would silently stop being tested. This
guard makes that a hard CI failure instead.

Dependency-free on purpose (no PyYAML on the runner): both files are parsed with
targeted regular expressions.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKSPACE = ROOT / "migration" / "Cargo.toml"
WORKFLOW = ROOT / ".github" / "workflows" / "rust-migration.yml"


def workspace_members(text: str) -> list[str]:
    block = re.search(r"^members\s*=\s*\[(.*?)\]", text, re.S | re.M)
    if not block:
        raise SystemExit("could not find [workspace] members in migration/Cargo.toml")
    return [
        Path(m).name
        for m in re.findall(r'"([^"]+)"', block.group(1))
    ]


def shard_crates(text: str) -> list[tuple[int, str]]:
    """Return (shard number, crate) pairs from the matrix `crates:` entries.

    Line-based scan: a `- shard: N` line opens an entry, the following `crates:`
    block scalar continues until the next entry or a dedent out of the matrix.
    """
    pairs: list[tuple[int, str]] = []
    shard: int | None = None
    in_crates = False
    for line in text.splitlines():
        stripped = line.strip()
        head = re.match(r"-\s*shard:\s*(\d+)$", stripped)
        if head:
            shard = int(head.group(1))
            in_crates = False
            continue
        if shard is None:
            continue
        if stripped.startswith("crates:"):
            in_crates = True
            tail = stripped.split(":", 1)[1].strip()
            if tail not in (">-", ">", "|", "|-", ""):
                pairs.extend((shard, c) for c in re.findall(r"-p\s+([A-Za-z0-9_-]+)", tail))
                in_crates = False
            continue
        if in_crates:
            if not line.startswith(" " * 12) or not stripped:
                in_crates = False
                continue
            pairs.extend((shard, c) for c in re.findall(r"-p\s+([A-Za-z0-9_-]+)", stripped))
    return pairs


def main() -> int:
    members = workspace_members(WORKSPACE.read_text())
    pairs = shard_crates(WORKFLOW.read_text())
    if not pairs:
        print("::error::no shard matrix entries parsed from rust-migration.yml")
        return 1

    assigned = [crate for _, crate in pairs]
    duplicates = sorted({c for c in assigned if assigned.count(c) > 1})
    missing = sorted(set(members) - set(assigned))
    unknown = sorted(set(assigned) - set(members))

    print(f"workspace members: {len(members)}")
    print(f"shard assignments: {len(assigned)} across {len({s for s, _ in pairs})} shards")
    for shard in sorted({s for s, _ in pairs}):
        crates = [c for s, c in pairs if s == shard]
        print(f"  shard {shard}: {len(crates)} crates")

    failed = False
    if duplicates:
        print(f"::error::crates assigned to more than one shard: {', '.join(duplicates)}")
        failed = True
    if missing:
        print(f"::error::crates in no shard (untested): {', '.join(missing)}")
        failed = True
    if unknown:
        print(f"::error::shard references unknown crates: {', '.join(unknown)}")
        failed = True
    if failed:
        return 1
    print("shard map is a complete, non-overlapping partition")
    return 0


if __name__ == "__main__":
    sys.exit(main())
