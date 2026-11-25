#!/usr/bin/env python3
"""CI-side Canvas schema conformance checker.

Replays star-db's migrations into a live PostgreSQL (one transaction per
file, mirroring sqlx's MIGRATOR — via psql, no drivers needed), introspects
the resulting schema through pg_catalog (attnotnull / format_type /
pg_get_expr defaults / pg_indexes / pg_constraint), and diffs it against the
generated Canvas reference (parity/canvas_schema_reference.json, produced by
tools/schema/canvas_reference.py).

Outputs parity/schema_gap.json (machine-readable full diff) and
parity/schema_gap.md (human summary, also cat'ed into $GITHUB_STEP_SUMMARY
by .github/workflows/schema-parity.yml).

Exit code: 0 unless a reference TABLE is missing from the live database AND
is not listed in parity/schema_gap_allowlist.json. Missing reference COLUMNS
are reported per table (lists and counts in schema_gap.md) but do NOT gate on
day one — they are the additive Wave 1.2 backlog and must shrink
monotonically. Type, nullability, default, index and foreign-key deltas are
reported, not gating, on day one.

Usage:
  DATABASE_URL=postgres://... python3 tools/schema/verify_schema_parity.py
  python3 tools/schema/verify_schema_parity.py --database-url ... \
      [--reference PATH] [--migrations-dir PATH] [--out-dir PATH] \
      [--allowlist PATH] [--keep-db] [--skip-apply]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
MIGRATION_DIR = HERE.parents[1]          # migration/
DEFAULT_REFERENCE = MIGRATION_DIR / "parity" / "canvas_schema_reference.json"
DEFAULT_MIGRATIONS = MIGRATION_DIR / "crates" / "star-db" / "migrations"
DEFAULT_OUT_DIR = MIGRATION_DIR / "parity"
DEFAULT_ALLOWLIST = MIGRATION_DIR / "parity" / "schema_gap_allowlist.json"

SQLX_LEDGER_DDL = """
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT now(),
    success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL,
    execution_time BIGINT NOT NULL
);
"""

SNAPSHOT_SQL = """
SELECT json_build_object(
  'tables', (SELECT COALESCE(json_object_agg(relname, true), '{}'::json)
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = current_schema() AND c.relkind IN ('r','p')),
  'columns', (SELECT COALESCE(json_object_agg(tab, cols), '{}'::json) FROM (
       SELECT c.relname AS tab, json_object_agg(a.attname, json_build_object(
                  'type', format_type(a.atttypid, a.atttypmod),
                  'not_null', a.attnotnull,
                  'default', COALESCE(pg_get_expr(d.adbin, d.adrelid), ''))) AS cols
       FROM pg_attribute a
       JOIN pg_class c ON c.oid = a.attrelid
       JOIN pg_namespace n ON n.oid = c.relnamespace
       LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
       WHERE n.nspname = current_schema() AND c.relkind IN ('r','p')
         AND a.attnum > 0 AND NOT a.attisdropped
       GROUP BY c.relname) x),
  'indexes', (SELECT COALESCE(json_object_agg(schemaname || '.' || indexname,
                 json_build_object('def', indexdef, 'table', tablename)),
                 '{}'::json)
              FROM pg_indexes WHERE schemaname = current_schema()),
  'fks', (SELECT COALESCE(json_agg(json_build_object(
              'name', con.conname,
              'table', sf.relname,
              'ref', f.relname,
              'cols', (SELECT json_agg(sa.attname) FROM unnest(con.conkey) ck
                       JOIN pg_attribute sa
                         ON sa.attrelid = con.conrelid AND sa.attnum = ck),
              'ref_cols', (SELECT json_agg(ra.attname) FROM unnest(con.confkey) rk
                       JOIN pg_attribute ra
                         ON ra.attrelid = con.confrelid AND ra.attnum = rk))),
              '[]'::json)
          FROM pg_constraint con
          JOIN pg_class sf ON sf.oid = con.conrelid
          JOIN pg_namespace n ON n.oid = sf.relnamespace
          JOIN pg_class f ON f.oid = con.confrelid
          WHERE con.contype = 'f' AND n.nspname = current_schema())
) AS snapshot;
"""

# --------------------------------------------------------------------------
# psql plumbing
# --------------------------------------------------------------------------

def _psql_cmd(url: str, sql: str | None = None, file: Path | None = None,
              single_txn: bool = False, tuples_only: bool = False) -> list[str]:
    cmd = ["psql", url, "-v", "ON_ERROR_STOP=1", "-q", "-X"]
    if single_txn:
        cmd.append("-1")
    if tuples_only:
        # -t -A: tuples-only + unaligned — the output of a single-value
        # SELECT is then exactly the value (no header, no "(1 row)" footer,
        # no wrapping), so json.loads sees the bare JSON document.
        cmd += ["-t", "-A"]
    if file is not None:
        cmd += ["-f", str(file)]
    elif sql is not None:
        cmd += ["-c", sql]
    return cmd


def psql(url: str, sql: str | None = None, file: Path | None = None,
         single_txn: bool = False, tuples_only: bool = False) -> str:
    proc = subprocess.run(
        _psql_cmd(url, sql, file, single_txn, tuples_only),
        capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"psql failed ({proc.returncode}): "
                           f"{proc.stderr.strip()[:2000]}")
    return proc.stdout


def apply_migrations(url: str, migrations_dir: Path, keep_db: bool) -> list[str]:
    """Apply every migration file in lexicographic order, one transaction per
    file (psql -1), recording each in _sqlx_migrations like sqlx's MIGRATOR."""
    files = sorted(migrations_dir.glob("*.sql"), key=lambda p: p.name)
    if not files:
        raise RuntimeError(f"no *.sql migrations under {migrations_dir}")
    applied = []
    psql(url, SQLX_LEDGER_DDL)
    have = {row.strip() for row in
            psql(url, "SELECT version FROM _sqlx_migrations;").splitlines()
            if row.strip()}
    for f in files:
        m = re.match(r"(\d+)_", f.name)
        if not m:
            raise RuntimeError(f"migration file without numeric version: {f.name}")
        version = int(m.group(1))
        if str(version) in have:
            continue
        description = f.name[m.end():].rsplit(".", 1)[0]
        checksum = hashlib.sha384(f.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory() as td:
            combined = Path(td) / "migration.sql"
            combined.write_text(
                f.read_text(encoding="utf-8", errors="replace")
                + "\n"
                + "INSERT INTO _sqlx_migrations (version, description, "
                "success, checksum, execution_time) VALUES "
                f"({version}, '{description}', true, "
                f"'\\x{checksum}'::bytea, 0);\n")
            psql(url, file=combined, single_txn=True)
        applied.append(f.name)
    if not keep_db:
        pass  # service container is throwaway; nothing to tear down
    return applied


def snapshot(url: str) -> dict:
    raw = psql(url, SNAPSHOT_SQL, tuples_only=True).strip()
    try:
        return json.loads(raw)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"snapshot output is not JSON ({exc}); first 300 "
                           f"chars: {raw[:300]!r}") from exc


# --------------------------------------------------------------------------
# live-side parsing / normalization
# --------------------------------------------------------------------------

def parse_indexdef(defn: str) -> dict | None:
    s = " ".join(defn.replace("\n", " ").split())
    m = re.match(r"CREATE (UNIQUE )?INDEX \S+ ON \S+?\.(?P<tbl>\S+)\s+"
                 r"USING (?P<using>\S+) \(", s)
    if not m:
        return None
    i, depth = m.end(), 1
    while i < len(s) and depth:
        if s[i] == "(":
            depth += 1
        elif s[i] == ")":
            depth -= 1
        i += 1
    cols_str = s[m.end():i - 1]
    rest = s[i:]
    wm = re.match(r"\s*WHERE\s+(.*)$", rest)
    cols, sub, cur = [], 0, []
    for ch in cols_str:
        if ch == "(":
            sub += 1
        elif ch == ")":
            sub -= 1
        if ch == "," and sub == 0:
            cols.append("".join(cur).strip())
            cur = []
        else:
            cur.append(ch)
    if cur:
        cols.append("".join(cur).strip())
    return {
        "columns": cols,
        "unique": bool(m.group(1)),
        "using": m.group("using"),
        "where": wm.group(1) if wm else None,
    }


def normalize_pg_type(t: str) -> str:
    # timestamp(6) == timestamp: Rails' default precision is not schema-visible
    return re.sub(r"(timestamp\(6\)|timestamptz\(6\)|time\(6\))",
                  lambda m: m.group(1).split("(")[0], t)


def rails_type_to_pg(col: dict) -> str:
    """Expected pg format_type string for a reference column."""
    t = col["type"]
    limit, prec, scale = col.get("limit"), col.get("precision"), col.get("scale")
    if t == "string":
        return f"character varying({limit or 255})"
    if t == "text":
        return "text"
    if t == "bigint":
        return "bigint"
    if t in ("integer", "primary_key", "serial"):
        return "integer"
    if t == "boolean":
        return "boolean"
    if t in ("datetime", "timestamp"):
        if prec is None or prec == 6:
            return "timestamp without time zone"
        return f"timestamp({prec}) without time zone"
    if t == "time":
        return "time without time zone"
    if t == "date":
        return "date"
    if t == "float":
        return "double precision"
    if t == "decimal":
        if prec is not None and scale is not None:
            return f"numeric({prec},{scale})"
        if prec is not None:
            return f"numeric({prec})"
        return "numeric"
    if t == "json":
        return "json"
    if t == "jsonb":
        return "jsonb"
    if t == "uuid":
        return "uuid"
    if t == "binary":
        return "bytea"
    if t == "interval":
        return "interval"
    return t  # extension types (e.g. vector) pass through


def normalize_default(value) -> str:
    """Loose default comparison (reported, not gating)."""
    if value is None:
        return ""
    s = str(value)
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, (int, float)):
        return s
    s = s.lower().strip().strip("'\"")
    s = re.sub(r"::[\w\s\[\]]+$", "", s)
    s = s.strip("'\"")
    if s in ("now()", "current_timestamp"):
        return "now()"
    return s


# --------------------------------------------------------------------------
# diff
# --------------------------------------------------------------------------

def diff(reference: dict, live: dict, allowlist: dict) -> dict:
    ref_tables = reference["tables"]
    live_tables = live.get("tables", {})
    live_cols = live.get("columns", {})

    live_indexes: dict[str, dict[str, dict]] = {}
    for _key, obj in (live.get("indexes") or {}).items():
        parsed = parse_indexdef(obj["def"])
        if parsed:
            live_indexes.setdefault(obj["table"], {})[_key.split(".", 1)[-1]] = parsed

    live_fks: dict[str, set] = {}
    for fk in live.get("fks") or []:
        live_fks.setdefault(fk["table"], set()).add(
            (tuple(fk["cols"] or []), fk["ref"]))

    allow_tables = allowlist.get("tables", {})

    result = {
        "missing_tables": {},
        "star_only_tables": sorted(t for t in live_tables if t not in ref_tables),
        "tables": {},
    }
    gating = 0
    for table in sorted(ref_tables):
        spec = ref_tables[table]
        if table not in live_tables:
            if table in allow_tables:
                result["missing_tables"][table] = {
                    "gating": False,
                    "reason": allow_tables[table].get("reason", ""),
                }
            else:
                result["missing_tables"][table] = {"gating": True, "reason": None}
                gating += 1
            continue
        entry: dict = {}
        lcols = live_cols.get(table, {})
        miss_cols = [c for c in spec["columns"] if c not in lcols]
        if miss_cols:
            # Reported, not gating (day one): the additive Wave 1.2 backlog.
            entry["missing_columns"] = {"columns": miss_cols}
        extra_cols = [c for c in lcols if c not in spec["columns"]]
        if extra_cols:
            entry["extra_columns"] = extra_cols
        types, nulls, defaults = [], [], []
        for col, cspec in spec["columns"].items():
            lcol = lcols.get(col)
            if not lcol:
                continue
            expected = normalize_pg_type(rails_type_to_pg(cspec))
            actual = normalize_pg_type(lcol["type"])
            if expected != actual:
                types.append({"column": col, "reference": expected,
                              "live": actual})
            if bool(cspec["not_null"]) != bool(lcol["not_null"]):
                nulls.append({"column": col,
                              "reference_not_null": bool(cspec["not_null"]),
                              "live_not_null": bool(lcol["not_null"])})
            ref_def = cspec.get("default")
            if ref_def is not None:
                if normalize_default(ref_def) != normalize_default(
                        lcol.get("default")):
                    defaults.append({"column": col,
                                     "reference": ref_def,
                                     "live": lcol.get("default")})
        if types:
            entry["type_mismatch"] = types
        if nulls:
            entry["not_null_mismatch"] = nulls
        if defaults:
            entry["default_mismatch"] = defaults
        # indexes
        lidx = live_indexes.get(table, {})
        missing_idx = [name for name in spec["indexes"] if name not in lidx]
        if missing_idx:
            entry["missing_indexes"] = [
                {"name": name, **spec["indexes"][name]} for name in missing_idx]
        extra_idx = [name for name in lidx if name not in spec["indexes"]]
        if extra_idx:
            entry["extra_indexes"] = extra_idx
        # foreign keys (pair identity: column set + target table)
        ref_fk_pairs = {(tuple(fk["column"]) if isinstance(fk["column"], list)
                         else (fk["column"],), fk["to_table"])
                        for fk in spec["foreign_keys"]}
        have_fk_pairs = live_fks.get(table, set())
        missing_fks = [pair for pair in sorted(ref_fk_pairs,
                                              key=lambda p: (p[1], p[0]))
                       if pair not in have_fk_pairs]
        if missing_fks:
            entry["missing_foreign_keys"] = [
                {"column": list(p[0]), "to_table": p[1]} for p in missing_fks]
        if entry:
            result["tables"][table] = entry
    result["gating_missing_table_count"] = gating
    result["missing_column_count"] = sum(
        len(t["missing_columns"]["columns"])
        for t in result["tables"].values() if "missing_columns" in t)
    result["missing_column_tables"] = sum(
        1 for t in result["tables"].values() if "missing_columns" in t)
    return result


# --------------------------------------------------------------------------
# report
# --------------------------------------------------------------------------

def write_md(gap: dict, out_path: Path):
    lines = ["# Schema conformance gap (Canvas reference vs live star-db)\n"]
    gated = gap["gating_missing_table_count"]
    lines.append(f"**Gating: missing non-allowlisted reference tables: {gated}**"
                 f" — exit code {'1' if gated else '0'}.\n")
    lines.append(f"**Missing reference columns: "
                 f"{gap['missing_column_count']} across "
                 f"{gap['missing_column_tables']} tables (reported, not yet "
                 "gating)** — the additive Wave 1.2 backlog; this number must "
                 "shrink monotonically.\n")
    missing = gap["missing_tables"]
    if missing:
        lines.append("## Missing tables\n")
        lines.append("| table | gating | reason |")
        lines.append("|---|---|---|")
        for t, info in missing.items():
            lines.append(f"| {t} | {'yes' if info['gating'] else 'no'} "
                         f"| {info['reason'] or '—'} |")
        lines.append("")
    if gap["star_only_tables"]:
        lines.append(f"## Star-only tables ({len(gap['star_only_tables'])})\n")
        lines.append(", ".join(gap["star_only_tables"]))
        lines.append("")
    if gap["tables"]:
        lines.append("## Per-table deltas\n")
        for table, entry in gap["tables"].items():
            lines.append(f"### {table}")
            mc = entry.get("missing_columns")
            if mc:
                cols = mc["columns"]
                head = ", ".join(cols[:15])
                more = "" if len(cols) <= 15 else f" …(+{len(cols) - 15} more)"
                lines.append(f"- missing columns ({len(cols)}): {head}{more}")
            for key, label in (("extra_columns", "extra columns"),
                               ("type_mismatch", "type mismatch"),
                               ("not_null_mismatch", "NOT NULL mismatch"),
                               ("default_mismatch", "default mismatch"),
                               ("missing_indexes", "missing index"),
                               ("extra_indexes", "extra index"),
                               ("missing_foreign_keys", "missing FK")):
                if key in entry:
                    val = entry[key]
                    detail = json.dumps(val) if not isinstance(val, list) else \
                        "; ".join(json.dumps(v) for v in val[:10])
                    lines.append(f"- {label}: {detail}")
            lines.append("")
    out_path.write_text("\n".join(lines) + "\n")


# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------

def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--database-url", default=os.environ.get("DATABASE_URL"),
                    help="postgres URL (defaults to $DATABASE_URL)")
    ap.add_argument("--reference", type=Path, default=DEFAULT_REFERENCE)
    ap.add_argument("--migrations-dir", type=Path, default=DEFAULT_MIGRATIONS)
    ap.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    ap.add_argument("--allowlist", type=Path, default=DEFAULT_ALLOWLIST)
    ap.add_argument("--skip-apply", action="store_true",
                    help="introspect only (schema already applied)")
    ap.add_argument("--keep-db", action="store_true", help=argparse.SUPPRESS)
    args = ap.parse_args(argv)

    if not args.database_url:
        print("error: --database-url or $DATABASE_URL required", file=sys.stderr)
        return 2
    if not args.reference.exists():
        print(f"error: reference not found: {args.reference}", file=sys.stderr)
        return 2

    try:
        if not args.skip_apply:
            applied = apply_migrations(args.database_url, args.migrations_dir,
                                       args.keep_db)
            print(f"applied {len(applied)} migrations")
            for name in applied[:5]:
                print(f"  {name}")
            if len(applied) > 5:
                print(f"  …(+{len(applied) - 5} more)")
        live = snapshot(args.database_url)
    except (RuntimeError, json.JSONDecodeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    reference = json.loads(args.reference.read_text())
    allowlist = (json.loads(args.allowlist.read_text())
                 if args.allowlist.exists() else {})

    gap = diff(reference, live, allowlist)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    (args.out_dir / "schema_gap.json").write_text(json.dumps(gap, indent=2) + "\n")
    write_md(gap, args.out_dir / "schema_gap.md")

    gated = gap["gating_missing_table_count"]
    print(f"missing tables: {len(gap['missing_tables'])} "
          f"(gating {sum(1 for v in gap['missing_tables'].values() if v['gating'])})")
    print(f"star-only tables: {len(gap['star_only_tables'])}")
    print(f"tables with deltas: {len(gap['tables'])}")
    print(f"missing reference columns (reported, not gating): "
          f"{gap['missing_column_count']} across "
          f"{gap['missing_column_tables']} tables")
    print(f"reports: {args.out_dir / 'schema_gap.json'}, "
          f"{args.out_dir / 'schema_gap.md'}")
    return 1 if gated else 0


if __name__ == "__main__":
    sys.exit(main())
