#!/usr/bin/env python3
"""Diff star-db's AUTHORED-cohort tables against the Canvas column
reference (canvas_authored_tables_reference.json) and emit the additive
ADD COLUMN backlog.

The reference lists Canvas's columns for the ~34 core tables that
migrations 0001-0222 define (PR #200's canvas-schema files skipped them
because the tables already existed). The diff is name-level; types come
from the reference (Rails→PG type map). Every emitted column is added
nullable without default so no live row is backfilled — the established
additive-only pattern in this repo.

Usage:
  python3 tools/parity/authored_column_backlog.py            # report
  python3 tools/parity/authored_column_backlog.py --sql OUT  # write SQL
"""
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
MIGRATIONS = HERE.parents[1] / "crates" / "star-db" / "migrations"
REFERENCE = HERE / "canvas_authored_tables_reference.json"

CREATE_RE = re.compile(r"CREATE TABLE (?:IF NOT EXISTS )?(\w+)\s*\(", re.I)
ALTER_ADD_RE = re.compile(
    r"ALTER TABLE (?:IF EXISTS )?(\w+)\s+ADD COLUMN (?:IF NOT EXISTS )?(\w+)",
    re.I)
CANVAS_FILES = {
    "0223_canvas_schema_tables_a.sql",
    "0224_canvas_schema_tables_b.sql",
    "0225_canvas_schema_tables_c.sql",
}


def star_columns():
    """table -> set(columns) as of the end of the migration chain."""
    cols = {}
    for f in sorted(MIGRATIONS.glob("*.sql")):
        text = f.read_text(encoding="utf-8", errors="replace")
        for m in CREATE_RE.finditer(text):
            start = m.end()
            depth = 1
            i = start
            while i < len(text) and depth:
                if text[i] == "(":
                    depth += 1
                elif text[i] == ")":
                    depth -= 1
                i += 1
            body = text[start:i]
            names = cols.setdefault(m.group(1).lower(), set())
            for line in body.splitlines():
                cm = re.match(r"\s{2,}(\w+)\s+\w", line)
                if cm:
                    names.add(cm.group(1))
        for m in ALTER_ADD_RE.finditer(text):
            cols.setdefault(m.group(1).lower(), set()).add(m.group(2))
    return cols


def main():
    write_sql = "--sql" in sys.argv
    out_path = None
    if write_sql:
        out_path = Path(sys.argv[sys.argv.index("--sql") + 1])

    reference = json.loads(REFERENCE.read_text())
    reference.pop("_comment", None)
    star = star_columns()

    missing = {}
    total = 0
    for table, columns in sorted(reference.items()):
        have = star.get(table)
        if have is None:
            print(f"!! table {table} not in star-db — reference lists "
                  f"{len(columns)} columns, SKIPPED (new-table work, not "
                  "additive)", file=sys.stderr)
            continue
        miss = [c for c in columns if c not in have]
        if miss:
            missing[table] = miss
            total += len(miss)

    for table, miss in missing.items():
        print(f"{table}: {len(miss)} missing  ({', '.join(miss[:6])}"
              + ("…" if len(miss) > 6 else "") + ")")
    print(f"\nTOTAL missing columns: {total} across {len(missing)} tables")

    if write_sql:
        lines = [
            "-- Migration 0231: Canvas column parity for the authored cohort.",
            "--",
            "-- PR #200's canvas-schema files (0223-0225) added every Canvas",
            "-- table star-db did not define; tables from migrations",
            "-- 0001-0222 were skipped because they already existed, so their",
            "-- Canvas columns were never added. This closes that backlog:",
            "-- every reference column missing from an authored table, added",
            "-- nullable with no default (additive-only; no backfill; the",
            "-- additive pattern is exactly `ALTER TABLE ... ADD COLUMN IF",
            "-- NOT EXISTS` used throughout 0030-0229).",
            "--",
            "-- Reference: tools/parity/canvas_authored_tables_reference.json",
            "-- (Canvas LMS db/migrate replay, master 2026-04 — the same",
            "-- reference the 0223-0225 files were generated from).",
            "",
        ]
        for table, miss in missing.items():
            lines.append(f"-- {table}: {len(miss)} columns")
            for c in miss:
                typ = reference[table][c]
                lines.append(
                    f"ALTER TABLE {table} ADD COLUMN IF NOT EXISTS "
                    f"{c} {typ};")
            lines.append("")
        out_path.write_text("\n".join(lines))
        print(f"SQL written → {out_path}")


if __name__ == "__main__":
    main()
