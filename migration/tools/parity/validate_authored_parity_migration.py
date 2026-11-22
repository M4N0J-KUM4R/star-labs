#!/usr/bin/env python3
"""Static validation of migration 0231 (authored-cohort column parity).

No local PostgreSQL is available in this sandbox, so this script proves
the migration is safe by construction:

1. every statement is `ALTER TABLE <t> ADD COLUMN IF NOT EXISTS <c> <type>;`
   (additive, nullable, no default, no backfill, no rename, no drop);
2. every referenced table exists at the end of the chain (created in an
   earlier migration);
3. no statement re-adds a column that already exists at the end of the
   chain (the IF NOT EXISTS guard makes that harmless, but a re-add
   would mean the backlog diff was wrong);
4. types are from the Rails→PG whitelist the canvas-schema files use.

Exit 0 = safe. Used before pushing; CI's real PG run is the final gate.
"""
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
MIGRATIONS = HERE.parents[1] / "crates" / "star-db" / "migrations"
TARGET = MIGRATIONS / "0231_canvas_authored_column_parity.sql"

CREATE_RE = re.compile(r"CREATE TABLE (?:IF NOT EXISTS )?(\w+)\s*\(", re.I)
ALTER_ADD_RE = re.compile(
    r"ALTER TABLE (?:IF EXISTS )?(\w+)\s+ADD COLUMN (?:IF NOT EXISTS )?(\w+)",
    re.I)
STMT_RE = re.compile(
    r"^ALTER TABLE (\w+) ADD COLUMN IF NOT EXISTS (\w+) ([a-z0-9()]+);$")
TYPE_WHITELIST = {
    "boolean", "text", "integer", "bigint", "float8", "numeric",
    "timestamp", "timestamptz", "date", "json", "jsonb",
    "varchar(255)", "varchar(16)",
}


def chain_tables_and_columns():
    """Replay the chain in filename order up to (not including) 0231."""
    tables = {}
    for f in sorted(MIGRATIONS.glob("*.sql")):
        if f.name.startswith("0231"):
            continue
        text = f.read_text(encoding="utf-8", errors="replace")
        for m in CREATE_RE.finditer(text):
            start = m.end()
            depth, i = 1, start
            while i < len(text) and depth:
                if text[i] == "(":
                    depth += 1
                elif text[i] == ")":
                    depth -= 1
                i += 1
            body = text[start:i]
            cols = tables.setdefault(m.group(1).lower(), set())
            for line in body.splitlines():
                cm = re.match(r"\s{2,}(\w+)\s+\w", line)
                if cm:
                    cols.add(cm.group(1))
        for m in ALTER_ADD_RE.finditer(text):
            tables.setdefault(m.group(1).lower(), set()).add(m.group(2))
    return tables


def main():
    problems = []
    stmts = 0
    state = chain_tables_and_columns()

    for lineno, line in enumerate(
            TARGET.read_text().splitlines(), start=1):
        line = line.strip()
        if not line or line.startswith("--"):
            continue
        m = STMT_RE.match(line)
        if not m:
            problems.append(f"line {lineno}: not a plain ADD COLUMN "
                            f"statement: {line[:70]}")
            continue
        table, col, typ = m.group(1), m.group(2), m.group(3)
        stmts += 1
        if typ not in TYPE_WHITELIST:
            problems.append(f"line {lineno}: type {typ} not in whitelist")
        if table not in state:
            problems.append(f"line {lineno}: table {table} does not exist "
                            "in the chain before 0231")
        elif col in state[table]:
            problems.append(f"line {lineno}: {table}.{col} already exists "
                            "(backlog diff bug)")

    print(f"statements checked: {stmts}")
    if problems:
        for p in problems:
            print("FAIL:", p)
        sys.exit(1)
    print("OK: all statements additive, typed, targeting existing tables, "
          "no duplicate columns")


if __name__ == "__main__":
    main()
