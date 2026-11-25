#!/usr/bin/env python3
"""Canvas db/migrate -> JSON schema reference generator.

Replays canvas-src/db/migrate/*.rb in filename order (the order Rails runs
them) and emits the schema those migrations define: per table — columns
(Rails type + limit/precision/scale + explicit not_null + default), indexes
(name, columns, unique, using, where) and foreign keys (column, to_table,
to_column, name, on_delete, on_update).

Rails semantics reproduced:
  * t.timestamps -> created_at + updated_at, NOT NULL, default now()
    (Rails 7 precision 6 unless precision: nil is given).
  * t.references :x -> column x_id + default index unless index: false; a
    foreign key only when foreign_key: is truthy (true or a hash, possibly
    with to_table:).
  * Default names follow Rails 7.1 exactly:
      - index: "index_<table>_on_<cols joined _and_>" (63-byte fallback with
        a SHA256 suffix; expression columns are word-scanned).
      - foreign key: "fk_rails_" + SHA256("<from>_<cols joined _and_>_fk")
        [0..10]; default column "<to_table singularized>_id".
  * Rails pluralization (default :en inflections + Canvas'
    criterion/criteria) so :sis_batch resolves to sis_batches, not
    sis_batchs — the 45 dangling FK targets of the pr-200 reference were
    all pluralizer bugs (receipt pr-200-schema-parity-verification.md).
  * FKs whose target table is absent from the reference are dropped and
    recorded in _notes, never silently kept.
  * create_table adds an id column (bigint NOT NULL) unless id: false;
    t.primary_keys columns are NOT NULL (a PG primary key implies it).

execute/say noise, check constraints and runtime helpers are ignored;
everything else the parser could not interpret is counted in "_ignored" or,
when it looks like a schema op, reported in _notes.

Usage:
  python3 tools/schema/canvas_reference.py \
      --canvas-dir /path/to/canvas-src \
      --out parity/canvas_schema_reference.json \
      [--validate-authored tools/parity/canvas_authored_tables_reference.json]

--validate-authored cross-checks the generated reference against the authored
34-table reference: every column there must exist in this reference and its
Rails->PG type must agree. Mismatches are reported explicitly (non-zero
exit), never silently adjusted.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent

# --------------------------------------------------------------------------
# Rails inflection (default :en rules + Canvas' criterion/criteria)
# --------------------------------------------------------------------------

_UNCOUNTABLE = {
    "equipment", "information", "rice", "money", "species", "series",
    "fish", "sheep", "jeans", "police",
}

_IRREGULAR = {
    "person": "people",
    "man": "men",
    "child": "children",
    "sex": "sexes",
    "move": "moves",
    "zombie": "zombies",
    "criterion": "criteria",  # config/initializers/inflections.rb
}

# Definition order; Rails prepends later rules, so the list is walked in
# reverse and the first match wins.
_PLURAL_RULES = [
    (r"$", "s"),
    (r"s$", "s"),
    (r"^(ax|test)is$", r"\1es"),
    (r"(octop|vir)us$", r"\1i"),
    (r"(octop|vir)i$", r"\1i"),
    (r"(alias|status)$", r"\1es"),
    (r"(bu)s$", r"\1ses"),
    (r"(buffal|tomat)o$", r"\1oes"),
    (r"([ti])um$", r"\1a"),
    (r"([ti])a$", r"\1a"),
    (r"sis$", "ses"),
    (r"(?:([^f])fe|([lr])f)$", r"\1\2ves"),
    (r"(hive)$", r"\1s"),
    (r"([^aeiouy]|qu)y$", r"\1ies"),
    (r"(x|ch|ss|sh)$", r"\1es"),
    (r"(matr|vert|ind)(?:ix|ex)$", r"\1ices"),
    (r"^(m|l)ouse$", r"\1ice"),
    (r"^(m|l)ice$", r"\1ice"),
    (r"^(ox)$", r"\1en"),
    (r"^(oxen)$", r"\1"),
    (r"(quiz)$", r"\1zes"),
]

_SINGULAR_RULES = [
    (r"s$", ""),
    (r"(ss)$", r"\1"),
    (r"(n)ews$", r"\1ews"),
    (r"([ti])a$", r"\1um"),
    (r"((a)naly|(b)a|(d)iagno|(p)arenthe|(p)rogno|(s)ynop|(t)he)(sis|ses)$",
     r"\1sis"),
    (r"(^analy)(sis|ses)$", r"\1sis"),
    (r"([^f])ves$", r"\1fe"),
    (r"(hive)s$", r"\1"),
    (r"([lr])ves$", r"\1f"),
    (r"([^aeiouy]|qu)ies$", r"\1y"),
    (r"(x|ch|ss|sh)es$", r"\1"),
    (r"(matr|vert|ind)ices$", r"\1ix"),
    (r"(m|l)ice$", r"\1ouse"),
    (r"(bus)es$", r"\1"),
    (r"(o)es$", r"\1"),
    (r"(shoe)s$", r"\1"),
    (r"(cris|ax|test)(is|es)$", r"\1is"),
]


def _apply_rules(word: str, rules) -> str | None:
    for pattern, repl in reversed(rules):
        if re.search(pattern, word, re.I):
            return re.sub(pattern, repl, word, count=1, flags=re.I)
    return None


def _restore_case(word: str, out: str) -> str:
    if word and word[0].isupper():
        out = out[0].upper() + out[1:]
    return out


def pluralize(word: str) -> str:
    low = word.lower()
    if low in _UNCOUNTABLE:
        return word
    if low in _IRREGULAR:
        return _restore_case(word, _IRREGULAR[low])
    out = _apply_rules(word, _PLURAL_RULES) or word
    return _restore_case(word, out)


def singularize(word: str) -> str:
    low = word.lower()
    reverse = {v: k for k, v in _IRREGULAR.items()}
    if low in _UNCOUNTABLE:
        return word
    if low in reverse:
        return _restore_case(word, reverse[low])
    out = _apply_rules(word, _SINGULAR_RULES) or word
    return _restore_case(word, out)


# --------------------------------------------------------------------------
# Ruby literal lexer
# --------------------------------------------------------------------------

_LITERAL_RE = re.compile(r"__R([SYLFI])(\d+)__")


class LiteralPool:
    """Replaces Ruby literals with placeholder tokens; resolves them later."""

    def __init__(self):
        self.items: list[tuple[str, str]] = []  # (kind, raw body)

    def add(self, kind: str, raw: str) -> str:
        self.items.append((kind, raw))
        return f"__R{kind}{len(self.items) - 1}__"

    def get(self, kind: str, idx: int) -> str:
        return self.items[idx][1]

    @staticmethod
    def resolve_string(raw: str, notes: list) -> str:
        """Resolve a (possibly interpolated) Ruby string body."""

        def sub(m):
            body = m.group(1)
            ext = re.match(r"connection\.extension\(\s*[:\"'](\w+)", body)
            if ext:
                return "public"
            if re.fullmatch(r"[\w?&.]+", body):
                return "public"  # local var holding an extension schema
            notes.append({"kind": "unresolved_interpolation", "expr": body})
            return m.group(0)  # keep the raw #⏋...} text, never invent a value

        return re.sub(r"#\{([^{}]*)\}", sub, raw)


def _skip_quoted(text: str, i: int) -> int:
    """Return the index just past the quoted literal starting at text[i]."""
    quote = text[i]
    n = len(text)
    j = i + 1
    while j < n:
        if text[j] == "\\":
            j += 2
            continue
        if text[j] == quote:
            return j + 1
        j += 1
    return j


def _scan_quoted(text: str, i: int, interp: bool) -> tuple[str, int]:
    """Scan a quoted literal starting at text[i] == quote; return (body, next)."""
    quote = text[i]
    n = len(text)
    j = i + 1
    depth = 0  # #{ } nesting
    buf: list[str] = []
    while j < n:
        c = text[j]
        if c == "\\" and j + 1 < n:
            nxt = text[j + 1]
            buf.append("\\" + nxt if nxt in "nt" else nxt)
            j += 2
            continue
        if interp and c == "#" and text[j:j + 2] == "#{":
            depth += 1
            buf.append("#{")
            j += 2
            continue
        if depth:
            if c == "}":
                depth -= 1
                buf.append(c)
                j += 1
                continue
            if c in "'\"":
                # keep nested literal text so interpolation resolvers can
                # see it (e.g. "#{connection.extension(\"vector\").schema}")
                k = _skip_quoted(text, j)
                buf.append(text[j:k])
                j = k
                continue
            buf.append(c)
            j += 1
            continue
        if c == quote:
            return "".join(buf), j + 1
        buf.append(c)
        j += 1
    return "".join(buf), j


_HEREDOC_RE = re.compile(r"<<([-~]?)((['\"])([A-Za-z_0-9]+)\3|([A-Za-z_0-9]+))")


def _scan_segment(text: str, pool: LiteralPool) -> str:
    """Scanner for heredoc-free Ruby: comments, strings, symbols, %literals."""
    out: list[str] = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c == "#":  # comment to end of line
            j = text.find("\n", i)
            i = n if j < 0 else j
            continue
        if c == ":" and i + 1 < n and text[i + 1] == '"':
            raw, i = _scan_quoted(text, i + 1, interp=True)
            out.append(pool.add("Y", raw))
            continue
        if c == "'" or c == '"':
            raw, i = _scan_quoted(text, i, interp=(c == '"'))
            out.append(pool.add("I" if c == '"' and "#{" in raw else "S", raw))
            continue
        m = re.match(r"%(w|i|W|I)([\[{(])", text[i:])
        if m:
            close = {"[": "]", "{": "}", "(": ")"}[m.group(2)]
            j = text.find(close, i)
            j = n if j < 0 else j
            out.append(pool.add("L", text[i + 3:j]))
            i = j + 1
            continue
        m = re.match(r"%(q|Q|s)?([\[{(])", text[i:])
        if m:
            open_c = m.group(2)
            close = {"[": "]", "{": "}", "(": ")"}[open_c]
            j, k = i + 2, 1
            while j < n and k:
                if text[j] == open_c:
                    k += 1
                elif text[j] == close:
                    k -= 1
                j += 1
            out.append(pool.add("S", text[i + 2:j - 1]))
            i = j
            continue
        out.append(c)
        i += 1
    return "".join(out)


def _strip_ruby(text: str, pool: LiteralPool) -> str:
    """Replace heredoc bodies first, then all other literals, with placeholders."""
    out: list[str] = []
    pos, n = 0, len(text)
    while True:
        hd = None
        for m in _HEREDOC_RE.finditer(text, pos):
            if m.start() > 0 and text[m.start() - 1] in "<=+-*/&|^0123456789":
                continue
            hd = m
            break
        if hd is None:
            out.append(_scan_segment(text[pos:], pool))
            break
        out.append(_scan_segment(text[pos:hd.start()], pool))
        body_start = text.find("\n", hd.start())
        if body_start < 0:
            out.append(_scan_segment(text[hd.start():], pool))
            break
        term = hd.group(4) or hd.group(5)
        j = body_start + 1
        while j < n:
            le = text.find("\n", j)
            le = n if le < 0 else le
            if text[j:le].strip() == term:
                out.append(pool.add("S", text[body_start + 1:j]))
                # keep the start-line suffix (e.g. `.squish`, closing `)`,
                # further args) scannable, then skip body + terminator
                out.append(_scan_segment(text[hd.end():body_start], pool))
                out.append("\n")
                pos = le
                break
            j = le + 1
        else:
            out.append(_scan_segment(text[hd.start():hd.end()], pool))
            pos = hd.end()
    cleaned = "".join(out)
    # -> { "expr" } lambdas (string bodies already replaced by placeholders)
    cleaned = re.sub(
        r"->\s*\{([^{}]*)\}",
        lambda m: pool.add("F", m.group(1).strip()),
        cleaned)
    return cleaned


# --------------------------------------------------------------------------
# Statement extraction
# --------------------------------------------------------------------------

_OPENER_RE = re.compile(
    r"^(?:def\s|class\s|module\s|if\s|unless\s|while\s|until\s|case\s|begin\b|"
    r"for\s|.*\bdo\s*(?:\|\s*[^|]*\s*\|)?\s*$)")
_BRACE_BLOCK_RE = re.compile(
    r"^\(?\s*(\d+)\s*\.\.\s*(\d+)\s*\)?\.each\s*\{\s*\|(\w+)\|\s*(\w+)\((\w+)\)\s*\}")
_ASSIGN_RE = re.compile(r"^(\w+)\s*=\s*(__R[SYLFI]\d+__)$")
_DEF_RE = re.compile(r"^def\s+(?:self\.)?(\w+)(?:\s*\(([^)]*)\))?")
_TBLOCK_RE = re.compile(r"\bdo\s*\|\s*t\s*\|$")
_LEAF_METHOD_RE = re.compile(r"^([A-Za-z_][\w?!]*)(?:\s*\((.*)\)|\s+(.*))?$", re.S)

def _extract_statements(cleaned: str) -> list[str]:
    """Join continuation lines (open brackets, trailing comma, trailing
    backslash) into statements."""
    stmts: list[str] = []
    buf: list[str] = []
    depth = 0
    for raw_line in cleaned.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        force_join = line.endswith("\\")
        if force_join:
            line = line[:-1].rstrip()
            if not line and buf:
                continue
        if buf:
            buf.append(line)
        else:
            buf = [line]
        joined = " ".join(buf)
        depth += sum(line.count(o) - line.count(c)
                     for o, c in (("(", ")"), ("[", "]"), ("{", "}")))
        if depth > 0 or joined.endswith(",") or force_join:
            continue
        stmts.append(joined)
        buf, depth = [], 0
    if buf:
        stmts.append(" ".join(buf))
    return stmts


# --------------------------------------------------------------------------
# Argument evaluation
# --------------------------------------------------------------------------


def _split_top(s: str, sep: str = ",") -> list[str]:
    parts, depth, cur = [], 0, []
    for ch in s:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == sep and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    if cur:
        parts.append("".join(cur))
    return [p.strip() for p in parts if p.strip()]


class ArgsParser:
    def __init__(self, pool: LiteralPool, notes: list):
        self.pool = pool
        self.notes = notes

    def value(self, tok: str):
        tok = tok.strip()
        m = _LITERAL_RE.fullmatch(tok)
        if m:
            kind, idx = m.group(1), int(m.group(2))
            raw = self.pool.get(kind, idx)
            if kind == "L":
                return raw.split()
            if kind == "F":
                return self.value(raw)
            if kind == "Y":
                return self.pool.resolve_string(raw, self.notes)
            if kind == "I" or "#{" in raw:
                return self.pool.resolve_string(raw, self.notes)
            return raw
        m = re.fullmatch(r"(__R[SYLFI]\d+__)((?:\.\w+)+)", tok)
        if m:  # placeholder.freeze / placeholder.squish / ...
            return self.value(m.group(1))
        if tok.startswith(":"):
            inner = tok[1:]
            if inner.startswith('"'):
                return self.pool.resolve_string(inner[1:-1], self.notes)
            return inner
        if tok in ("true", "false"):
            return tok == "true"
        if tok == "nil":
            return None
        if re.fullmatch(r"-?\d[\d_]*", tok):
            return int(tok.replace("_", ""))
        if re.fullmatch(r"-?\d[\d_]*\.\d+", tok):
            return float(tok.replace("_", ""))
        m = re.fullmatch(r"(\d[\d_]*)(\.)(kilobytes|megabytes|gigabytes|bytes)",
                         tok)
        if m:
            mult = {"bytes": 1, "kilobytes": 1024, "megabytes": 1024 ** 2,
                    "gigabytes": 1024 ** 3}[m.group(3)]
            return int(m.group(1).replace("_", "")) * mult
        if tok.startswith("[") and tok.endswith("]"):
            return [self.value(t) for t in _split_top(tok[1:-1])]
        if tok.startswith("{") and tok.endswith("}"):
            return self._hash(tok[1:-1])
        m = re.fullmatch(r"(\w+):\s*(.*)", tok, re.S)
        if m:
            return {m.group(1): self.value(m.group(2))}
        if re.fullmatch(r"[\w?&.:]+", tok) and not tok[0].isdigit():
            return tok  # bare identifier (e.g. a table name local) — kept as-is
        self.notes.append({"kind": "unresolved_arg", "token": tok[:120]})
        return tok

    def _hash(self, body: str) -> dict:
        out = {}
        for part in _split_top(body):
            m = re.fullmatch(r"(\"[^\"]*\"|:\w+|\w+)\s*:\s*(.*)", part, re.S)
            if not m:
                self.notes.append({"kind": "unresolved_hash_entry",
                                   "entry": part[:120]})
                continue
            key = m.group(1)
            if key.startswith('"'):
                key = key[1:-1]
            elif key.startswith(":"):
                key = key[1:]
            out[key] = self.value(m.group(2))
        return out

    def call(self, argstring: str | None):
        """Return (positional_args, kwargs) for a cleaned argument string."""
        args, kwargs = [], {}
        if not argstring:
            return args, kwargs
        for tok in _split_top(argstring):
            m = re.fullmatch(r"(\w+)\s*:\s*(.*)", tok, re.S)
            if m and not tok.startswith(":"):
                kwargs[m.group(1)] = self.value(m.group(2))
                continue
            v = self.value(tok)
            if isinstance(v, dict):
                kwargs.update(v)
            else:
                args.append(v)
        return args, kwargs


# --------------------------------------------------------------------------
# Rails naming helpers (7.1 algorithms)
# --------------------------------------------------------------------------

MAX_INDEX_NAME = 63


def default_index_name(table: str, columns) -> str:
    cols = [str(c) for c in columns]
    # Rails: plain column names join with "_and_"; expression columns
    # (anything containing non-word chars) are word-scanned and joined "_"
    if any(re.search(r"\W", c) for c in cols):
        joined = "_".join(re.findall(r"\w+", "_".join(cols)))
    else:
        joined = "_and_".join(cols)
    name = f"index_{table}_on_{joined}"
    if len(name.encode()) <= MAX_INDEX_NAME:
        return name
    digest = hashlib.sha256(name.encode()).hexdigest()[:10]
    short = f"idx_on_{'_'.join(cols)}"[: MAX_INDEX_NAME - 11]
    return f"{short}_{digest}"


def default_fk_name(from_table: str, column) -> str:
    cols = [str(c) for c in (column if isinstance(column, list) else [column])]
    ident = f"{from_table}_{'_and_'.join(cols)}_fk"
    return "fk_rails_" + hashlib.sha256(ident.encode()).hexdigest()[:10]


# --------------------------------------------------------------------------
# Schema model + replay
# --------------------------------------------------------------------------

_COLUMN_TYPES = {
    "string", "text", "integer", "bigint", "float", "decimal", "numeric",
    "datetime", "timestamp", "time", "date", "boolean", "json", "jsonb",
    "binary", "blob", "uuid", "xml", "inet", "cidr", "macaddr", "interval",
    "citext", "vector", "primary_key", "serial", "enum", "virtual",
}


class Table:
    def __init__(self, name: str):
        self.name = name
        self.columns: dict[str, dict] = {}
        self.indexes: dict[str, dict] = {}
        self.foreign_keys: list[dict] = []
        self.pk_cols: list[str] | None = None

    def add_column(self, name, type_, *, limit=None, precision=None,
                   scale=None, not_null=False, default=None):
        self.columns[str(name)] = {
            "type": type_, "limit": limit, "precision": precision,
            "scale": scale, "not_null": bool(not_null), "default": default,
        }

    def to_json(self) -> dict:
        return {
            "columns": self.columns,
            "indexes": self.indexes,
            "foreign_keys": self.foreign_keys,
        }


class Reference:
    def __init__(self):
        self.tables: dict[str, Table] = {}
        self.extensions: list[str] = []
        self.notes: list[dict] = []
        self.ignored: dict[str, int] = {}
        self.helper_templates: dict[str, tuple[str, list[str]]] = {}
        self._replica_identity_pending: dict[str, list] = {}

    def note(self, kind: str, **kw):
        self.notes.append({"kind": kind, **kw})

    def count_ignored(self, kind: str):
        self.ignored[kind] = self.ignored.get(kind, 0) + 1

    def _table(self, name) -> Table | None:
        t = self.tables.get(str(name))
        if t is None:
            self.note("op_on_unknown_table", table=str(name))
        return t

    def add_index(self, table: str, columns, opts: dict):
        t = self.tables.get(table)
        if t is None:
            self.note("op_on_unknown_table", table=table)
            return
        if columns is None:
            self.note("index_without_columns", table=table,
                      opts={k: str(v) for k, v in opts.items()})
            return
        cols = columns if isinstance(columns, list) else [columns]
        name = opts.get("name") or default_index_name(table, cols)
        t.indexes[str(name)] = {
            "columns": [str(c) for c in cols],
            "unique": bool(opts.get("unique", False)),
            "using": opts.get("using"),
            "where": opts.get("where"),
        }

    def remove_index(self, table: str, columns, opts: dict):
        t = self.tables.get(table)
        if t is None:
            self.note("op_on_unknown_table", table=table)
            return
        if opts.get("name"):
            t.indexes.pop(str(opts["name"]), None)
            return
        cols = columns if isinstance(columns, list) else ([columns] if columns else [])
        if not cols:
            self.note("remove_index_without_target", table=table)
            return
        t.indexes.pop(default_index_name(table, cols), None)

    def add_fk(self, from_table: str, to_table: str, opts: dict):
        column = opts.get("column") or f"{singularize(to_table)}_id"
        entry = {
            "column": column,
            "to_table": str(to_table),
            "to_column": opts.get("primary_key"),
            "name": opts.get("name") or default_fk_name(from_table, column),
            "on_delete": opts.get("on_delete"),
            "on_update": opts.get("on_update"),
        }
        t = self.tables.get(from_table)
        if t is None:
            self.note("fk_dropped_no_source_table", **entry)
            return
        t.foreign_keys.append(entry)


def _normalize_index_opts(opts: dict) -> dict:
    out = dict(opts)
    idx = out.pop("index", None)
    if isinstance(idx, dict):
        out.update(idx)
    return out


class Replayer:
    """Replays migration class bodies (up/change) against a Reference."""

    EXEC_DEFS = {"up", "change"}

    def __init__(self, ref: Reference, pool: LiteralPool, parser: ArgsParser):
        self.ref = ref
        self.pool = pool
        self.p = parser

    # ---- statement walk ------------------------------------------------

    def replay_statements(self, stmts: list[str]):
        stack: list[tuple[str, object]] = []
        for stmt in stmts:
            if stmt == "end":
                if stack:
                    stack.pop()
                continue
            dm = _DEF_RE.match(stmt)
            if dm:
                name = dm.group(1)
                if name in self.EXEC_DEFS:
                    stack.append(("def", "exec"))
                else:
                    param = (dm.group(2) or "").split(",")[0].strip()
                    self.ref.helper_templates[name] = (param, [])
                    stack.append(("def", ("capture", name)))
                continue
            if _OPENER_RE.match(stmt) and not stmt.rstrip().endswith("}"):
                if _TBLOCK_RE.search(stmt):
                    nearest = self._nearest_def(stack)
                    if nearest and nearest[0] == "capture":
                        self.ref.helper_templates[nearest[1]][1].append(stmt)
                        stack.append(("other", None))
                    else:
                        header = self._parse_block_header(stmt)
                        if header is None:
                            stack.append(("other", None))
                        else:
                            method, args, kwargs, table = header
                            if method == "create_table":
                                self._create_table(args, kwargs)
                            stack.append(("tblock", table))
                else:
                    stack.append(("other", None))
                continue
            # leaf statement
            nearest = self._nearest_def(stack)
            if nearest and nearest[0] == "capture":
                self.ref.helper_templates[nearest[1]][1].append(stmt)
            elif nearest == ("exec", None):
                tblock = next((v for k, v in reversed(stack) if k == "tblock"),
                              None)
                if tblock:
                    self._t_op(tblock, stmt)
                else:
                    self._dispatch(stmt)
            else:
                self.ref.count_ignored("class_level")
        return

    @staticmethod
    def _nearest_def(stack) -> tuple[str, str | None] | None:
        for kind, payload in reversed(stack):
            if kind == "def":
                if payload == "exec":
                    return ("exec", None)
                return ("capture", payload[1])
            # tblock / other (if, class, do-blocks): keep scanning outward
        return None

    def _parse_block_header(self, stmt: str):
        header = _TBLOCK_RE.sub("", stmt).strip()
        m = re.match(r"^(\w+)[\s(]+(.*)$", header, re.S)
        if not m:
            return None
        method, argstr = m.group(1), m.group(2)
        if header.endswith(")") and "(" in header:
            argstr = header[header.index("(") + 1:-1]
        args, kwargs = self.p.call(argstr)
        name = str(args[0]) if args else None
        if not name or name == "table_name":
            return None
        return (method, args, kwargs, name)

    # ---- top-level dispatch ---------------------------------------------

    def _dispatch(self, stmt: str):
        ref, p = self.ref, self.p
        if re.match(r"^\w+\s*(\+?=|<<)", stmt):
            ref.count_ignored("assignment")
            return
        lm = _BRACE_BLOCK_RE.match(stmt)
        if lm:
            lo, hi, _var, helper, _arg = lm.groups()
            tmpl = ref.helper_templates.get(helper)
            if tmpl is None:
                ref.note("loop_helper_missing", helper=helper)
                return
            param, body = tmpl
            for v in range(int(lo), int(hi) + 1):
                _instantiate_template(ref, body, {param: v}, self.pool, self.p)
            return

        m = _LEAF_METHOD_RE.match(stmt)
        if not m:
            ref.count_ignored("unparsed")
            return
        method, paren_args, bare_args = m.group(1), m.group(2), m.group(3)
        # statements whose args we never need: skip parsing entirely (their
        # SQL/heredoc bodies must not produce notes)
        if method in ("execute", "say", "say_with_time", "quietly",
                      "suppress_messages", "set_replica_identity",
                      "add_check_constraint", "remove_check_constraint",
                      "validate_check_constraint",
                      "remove_index_check_constraint") or method.startswith(
                "connection"):
            ref.count_ignored("noise_or_constraint")
            return
        argstr = paren_args if paren_args is not None else bare_args
        args, kwargs = p.call(argstr)

        if method in ("create_extension", "enable_extension"):
            if args:
                name = str(args[0])
                if name not in ref.extensions:
                    ref.extensions.append(name)
        elif method == "create_table":
            self._create_table(args, kwargs)
        elif method == "drop_table":
            name = str(args[0])
            if name in ref.tables:
                ref.tables.pop(name)
            else:
                ref.note("drop_unknown_table", table=name)
        elif method == "rename_table":
            old, new = str(args[0]), str(args[1])
            if old in ref.tables:
                t = ref.tables.pop(old)
                t.name = new
                ref.tables[new] = t
            else:
                ref.note("rename_unknown_table", table=old)
        elif method == "add_column":
            t = ref._table(args[0])
            if t is not None:
                t.add_column(args[1], _col_type(args[2]), **_col_opts(kwargs))
        elif method in ("add_reference", "add_belongs_to"):
            self._reference_op(ref.tables.get(str(args[0])), args[1], kwargs)
        elif method == "remove_column":
            t = ref._table(args[0])
            if t is not None:
                t.columns.pop(str(args[1]), None)
        elif method == "rename_column":
            t = ref._table(args[0])
            if t is not None and str(args[1]) in t.columns:
                t.columns[str(args[2])] = t.columns.pop(str(args[1]))
        elif method == "change_column":
            t = ref._table(args[0])
            if t is not None and str(args[1]) in t.columns:
                c = t.columns[str(args[1])]
                c.update(_col_opts(kwargs))
                c["type"] = _col_type(args[2])
        elif method == "change_column_default":
            t = ref._table(args[0])
            if t is not None and str(args[1]) in t.columns:
                rest = args[2:]
                default = (kwargs["to"] if "to" in kwargs
                           else (rest[0] if rest else kwargs.get("default")))
                t.columns[str(args[1])]["default"] = default
        elif method == "change_column_null":
            t = ref._table(args[0])
            if t is not None and str(args[1]) in t.columns:
                t.columns[str(args[1])]["not_null"] = not bool(args[2])
        elif method == "add_index":
            cols = args[1] if len(args) > 1 else kwargs.get("column")
            ref.add_index(str(args[0]), cols, kwargs)
        elif method == "remove_index":
            cols = kwargs.get("column") or (args[1] if len(args) > 1 else None)
            ref.remove_index(str(args[0]), cols, kwargs)
        elif method == "add_foreign_key":
            ref.add_fk(str(args[0]), str(args[1]), kwargs)
        elif method == "remove_foreign_key":
            self._remove_fk(str(args[0]), kwargs,
                            to_table=str(args[1]) if len(args) > 1 else None)
        else:
            if method.endswith("!"):
                ref.count_ignored("bang_macro")
            else:
                ref.count_ignored(f"method:{method}")
                if re.match(r"^(add_|remove_|change_|rename_|drop_|create_table)",
                            stmt):
                    ref.note("unknown_top_level_statement",
                             statement=stmt[:160])

    def _create_table(self, args, kwargs):
        ref = self.ref
        if not args:
            ref.note("create_table_without_name")
            return
        name = str(args[0])
        if name in ref.tables:
            ref.note("duplicate_create_table", table=name)
            ref.tables.pop(name)
        t = Table(name)
        ref.tables[name] = t
        pk = kwargs.get("primary_key")
        id_opt = kwargs.get("id", "primary_key")
        if id_opt is not False and not isinstance(pk, list):
            colname = str(pk) if isinstance(pk, str) else "id"
            coltype = "bigint" if id_opt in (True, None, "primary_key") else str(id_opt)
            t.add_column(colname, coltype, not_null=True)
        if isinstance(pk, list):
            t.pk_cols = [str(c) for c in pk]

    # ---- t.* block ops ---------------------------------------------------

    def _t_op(self, table_name: str, stmt: str):
        ref, p = self.ref, self.p
        m = re.match(r"^t\.(\w+)(?:\s*\((.*)\)|\s+(.*))?$", stmt, re.S)
        if not m:
            ref.count_ignored("unparsed_t")
            return
        method = m.group(1)
        args, kwargs = p.call(m.group(2) if m.group(2) is not None else m.group(3))
        t = ref.tables.get(table_name)

        if method in ("check_constraint", "polymorphic_check_constraint",
                      "replica_identity"):
            ref.count_ignored("t:constraint")
            return
        if t is None:
            ref.note("op_on_unknown_table", table=table_name)
            return

        if method == "column":
            t.add_column(args[0], _col_type(args[1]), **_col_opts(kwargs))
        elif method == "timestamps":
            precision = kwargs.get("precision", 6)
            default = kwargs.get("default", "now()")
            not_null = kwargs.get("null") is not True
            for col in ("created_at", "updated_at"):
                t.add_column(col, "datetime", precision=precision,
                             not_null=not_null, default=default)
        elif method in ("references", "belongs_to"):
            self._reference_op(t, args[0], kwargs)
        elif method == "index":
            cols = args[0] if args else kwargs.get("column")
            ref.add_index(table_name, cols,
                          {k: v for k, v in kwargs.items() if k != "column"})
        elif method == "remove_index":
            cols = args[0] if args else kwargs.get("column")
            ref.remove_index(table_name, cols, kwargs)
        elif method == "foreign_key":
            to_table = str(args[0])
            column = kwargs.get("column") or f"{singularize(to_table)}_id"
            t.foreign_keys.append({
                "column": column,
                "to_table": to_table,
                "to_column": kwargs.get("primary_key"),
                "name": kwargs.get("name") or default_fk_name(table_name, column),
                "on_delete": kwargs.get("on_delete"),
                "on_update": kwargs.get("on_update"),
            })
        elif method == "primary_keys":
            pk_cols = args[0] if isinstance(args[0], list) else [args[0]]
            t.pk_cols = [str(c) for c in pk_cols]
            for c in t.pk_cols:
                if c in t.columns:
                    t.columns[c]["not_null"] = True
        elif method == "replica_identity_index":
            first = str(args[0]) if args else "root_account_id"
            ref._replica_identity_pending.setdefault(table_name, []).append(
                (first, kwargs))
        elif method == "change_default":
            col = str(args[0])
            default = (kwargs["to"] if "to" in kwargs
                       else (args[1] if len(args) > 1 else None))
            if col in t.columns:
                t.columns[col]["default"] = default
        elif method == "remove":
            for name in args:
                t.columns.pop(str(name), None)
        elif method == "rename":
            t.columns[str(args[1])] = t.columns.pop(str(args[0]))
        elif method in _COLUMN_TYPES:
            for name in args:
                if not isinstance(name, dict):
                    t.add_column(name, method, **_col_opts(kwargs))
        else:
            ref.count_ignored(f"t:{method}")
            ref.note("unknown_t_statement", table=table_name,
                     statement=stmt[:160])

    # ---- shared pieces ----------------------------------------------------

    def _reference_op(self, t: Table | None, name: str, kwargs: dict):
        if t is None:
            return
        index_opt = kwargs.get("index")  # may be False / True / dict / None
        opts = _normalize_index_opts(kwargs)
        polymorphic = opts.get("polymorphic")
        not_null = opts.get("null") is False
        fk_opt = opts.get("foreign_key")

        if isinstance(polymorphic, list):
            # Canvas' polymorphic_associations extension: an ARRAY polymorphic
            # reference creates one plain <sub>_id column (+ index + FK) per
            # listed type — no <name>_type/<name>_id pair at all.
            for sub in polymorphic:
                sub = str(sub)
                t.add_column(f"{sub}_id", str(opts.get("type") or "bigint"),
                             limit=opts.get("limit"), not_null=not_null,
                             default=opts.get("default"))
                if index_opt is not False:
                    where = opts.get("where")
                    cond = f"{sub}_id IS NOT NULL"
                    where = f"{where} AND {cond}" if where else cond
                    cols = [f"{sub}_id"]
                    t.indexes[opts.get("name")
                              or default_index_name(t.name, cols)] = {
                        "columns": cols,
                        "unique": bool(opts.get("unique", False)),
                        "using": opts.get("using"),
                        "where": where,
                    }
                if fk_opt:
                    to_table = (str(fk_opt["to_table"])
                                if isinstance(fk_opt, dict)
                                and "to_table" in fk_opt else pluralize(sub))
                    extra = ({k: v for k, v in fk_opt.items()
                              if k != "to_table"}
                             if isinstance(fk_opt, dict) else {})
                    self.ref.add_fk(t.name, to_table,
                                    {"column": f"{sub}_id", **extra})
            return

        if polymorphic:
            # native Rails polymorphic: type + id pair, never a real FK
            t.add_column(f"{name}_type", "string", limit=opts.get("limit", 255),
                         not_null=not_null, default=opts.get("default"))
            t.add_column(f"{name}_id", str(opts.get("type") or "bigint"),
                         limit=opts.get("limit"), not_null=not_null,
                         default=opts.get("default"))
            if index_opt is not False:
                cols = [f"{name}_type", f"{name}_id"]
                entry = {
                    "columns": cols,
                    "unique": bool(opts.get("unique", False)),
                    "using": opts.get("using"),
                    "where": opts.get("where"),
                }
                t.indexes[opts.get("name")
                          or default_index_name(t.name, cols)] = entry
            return

        t.add_column(f"{name}_id", str(opts.get("type") or "bigint"),
                     limit=opts.get("limit"), not_null=not_null,
                     default=opts.get("default"))
        if index_opt is not False:
            cols = [f"{name}_id"]
            entry = {
                "columns": cols,
                "unique": bool(opts.get("unique", False)),
                "using": opts.get("using"),
                "where": opts.get("where"),
            }
            t.indexes[opts.get("name") or default_index_name(t.name, cols)] = entry
        if fk_opt:
            to_table = (str(fk_opt["to_table"]) if isinstance(fk_opt, dict)
                        and "to_table" in fk_opt else pluralize(name))
            extra = ({k: v for k, v in fk_opt.items() if k != "to_table"}
                     if isinstance(fk_opt, dict) else {})
            self.ref.add_fk(t.name, to_table, {"column": f"{name}_id", **extra})

    def _remove_fk(self, from_table: str, opts: dict, to_table=None):
        t = self.ref.tables.get(from_table)
        if t is None:
            return
        keep = []
        for fk in t.foreign_keys:
            match = ((opts.get("name") and fk["name"] == str(opts["name"]))
                     or (to_table and fk["to_table"] == to_table)
                     or (opts.get("column") and fk["column"] == str(opts["column"])))
            if not match:
                keep.append(fk)
        t.foreign_keys = keep


def _col_type(type_) -> str:
    s = str(type_)
    if s == "public.vector":
        return "vector"
    return s


def _col_opts(kwargs: dict) -> dict:
    return {
        "limit": kwargs.get("limit"),
        "precision": kwargs.get("precision"),
        "scale": kwargs.get("scale"),
        "not_null": kwargs.get("null") is False,
        "default": kwargs.get("default"),
    }


# --------------------------------------------------------------------------
# def-body templates (create_aua_log_partition-style helpers)
# --------------------------------------------------------------------------

def _instantiate_template(ref: Reference, tmpl: list[str], bindings: dict,
                          pool: LiteralPool, parser: ArgsParser):
    """Run a captured def body: `var = :"name_#{param}"` + create_table block."""
    var_values: dict[str, str] = {}
    pending_table = None
    replayer = Replayer(ref, pool, parser)
    for stmt in tmpl:
        m = _ASSIGN_RE.match(stmt)
        if m:
            kind, idx = m.group(2)[3], int(m.group(2)[4:-2])
            raw = pool.get(kind, idx)
            for var, v in bindings.items():
                raw = raw.replace(f"#{{{var}}}", str(v))
            var_values[m.group(1)] = pool.resolve_string(raw, ref.notes)
            continue
        if _TBLOCK_RE.search(stmt):
            header = _TBLOCK_RE.sub("", stmt).strip()
            argstr = re.sub(r"^\w+\s+", "", header)
            args, _ = parser.call(argstr)
            name = var_values.get(args[0]) if args else None
            if name is None:
                ref.note("loop_template_unresolved_table",
                         statement=stmt[:160])
                continue
            pending_table = name
            _dispatch_create_table_for_template(ref, name)
            continue
        if pending_table is not None and stmt.startswith("t."):
            replayer._t_op(pending_table, stmt)
            continue
        ref.count_ignored("loop_template_noise")


def _dispatch_create_table_for_template(ref: Reference, name: str):
    if name in ref.tables:
        ref.note("duplicate_create_table", table=name)
        ref.tables.pop(name)
    t = Table(name)
    ref.tables[name] = t
    t.add_column("id", "bigint", not_null=True)


def _attach_replica_identity_indexes(ref: Reference):
    for table_name, entries in ref._replica_identity_pending.items():
        t = ref.tables.get(table_name)
        if t is None:
            continue
        pk = t.pk_cols or ["id"]
        for first_col, kwargs in entries:
            cols = [first_col] + pk
            t.indexes[f"index_{table_name}_replica_identity"] = {
                "columns": cols, "unique": True,
                "using": kwargs.get("using"), "where": kwargs.get("where"),
            }
    # a PG primary key implies NOT NULL; t.primary_keys may be declared
    # before the columns themselves, so enforce it once at the end
    for t in ref.tables.values():
        for c in (t.pk_cols or []):
            if c in t.columns:
                t.columns[c]["not_null"] = True


# --------------------------------------------------------------------------
# FK resolution + output
# --------------------------------------------------------------------------

def resolve_foreign_keys(ref: Reference):
    """Drop FKs whose target table is absent from the reference (with a note)."""
    for table in ref.tables.values():
        keep = []
        for fk in table.foreign_keys:
            target = ref.tables.get(fk["to_table"])
            if target is None:
                ref.note("fk_dropped_target_table_absent", table=table.name,
                         column=fk["column"], to_table=fk["to_table"])
                continue
            to_col = fk.get("to_column")
            if to_col and str(to_col) not in target.columns:
                ref.note("fk_target_column_absent", table=table.name,
                         column=fk["column"], to_table=fk["to_table"],
                         to_column=str(to_col))
            keep.append(fk)
        table.foreign_keys = keep


def build_reference(sources: list[tuple[str, str]]) -> Reference:
    pool = LiteralPool()
    ref = Reference()
    parser = ArgsParser(pool, ref.notes)
    replayer = Replayer(ref, pool, parser)
    for fname, source in sources:
        pool.items.clear()
        cleaned = _strip_ruby(source, pool)
        stmts = _extract_statements(cleaned)
        replayer.replay_statements(stmts)
    _attach_replica_identity_indexes(ref)
    resolve_foreign_keys(ref)
    return ref


def reference_to_json(ref: Reference, canvas_dir: str, files: list[str]) -> dict:
    return {
        "_source": {
            "canvas_dir": canvas_dir,
            "migration_files": len(files),
            "tables": len(ref.tables),
            "rails_naming": "rails 7.1 (SHA256 fk_rails_ names, "
                            "index_<table>_on_<cols _and_>)",
        },
        "_extensions": sorted(ref.extensions),
        "_notes": ref.notes,
        "_ignored": ref.ignored,
        "tables": {name: t.to_json() for name, t in ref.tables.items()},
    }


# --------------------------------------------------------------------------
# Rails -> PG type mapping (authored-reference validation)
# --------------------------------------------------------------------------

def rails_to_pg(col: dict) -> str:
    t, limit = col["type"], col.get("limit")
    prec, scale = col.get("precision"), col.get("scale")
    if t == "string":
        return f"varchar({limit or 255})"
    if t == "text":
        return "text"
    if t == "bigint":
        return "bigint"
    if t in ("integer", "primary_key", "serial"):
        return "integer"
    if t == "boolean":
        return "boolean"
    if t in ("datetime", "timestamp"):
        return "timestamp"
    if t == "date":
        return "date"
    if t == "float":
        return "float8"
    if t == "decimal":
        if prec is not None and scale is not None:
            return f"numeric({prec},{scale})"
        return "numeric"
    if t == "json":
        return "json"
    if t == "jsonb":
        return "jsonb"
    if t == "uuid":
        return "uuid"
    if t == "binary":
        return "bytea"
    return t


def validate_authored(ref_json: dict, authored_path: Path) -> list[str]:
    authored = json.loads(authored_path.read_text())
    problems: list[str] = []
    for table, cols in authored.items():
        if table.startswith("_"):
            continue
        mine = ref_json["tables"].get(table)
        if mine is None:
            problems.append(f"table {table}: missing from generated reference")
            continue
        for col, pg_type in sorted(cols.items()):
            spec = mine["columns"].get(col)
            if spec is None:
                problems.append(
                    f"table {table}: column {col} ({pg_type}) missing from "
                    "generated reference")
                continue
            got = rails_to_pg(spec)
            if got != pg_type:
                problems.append(
                    f"table {table}.{col}: authored {pg_type} != generated "
                    f"{got} (rails {spec['type']}, not_null={spec['not_null']})")
    return problems


# --------------------------------------------------------------------------
# main
# --------------------------------------------------------------------------

def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--canvas-dir", required=True,
                    help="path to canvas-src (the Ruby checkout)")
    ap.add_argument("--out", required=True, help="output JSON path")
    ap.add_argument("--validate-authored", type=Path, default=None,
                    help="cross-check against canvas_authored_tables_reference.json")
    args = ap.parse_args(argv)

    migrate_dir = Path(args.canvas_dir) / "db" / "migrate"
    files = sorted(migrate_dir.glob("*.rb"), key=lambda p: p.name)
    if not files:
        print(f"error: no migrations under {migrate_dir}", file=sys.stderr)
        return 2

    sources = [(f.name, f.read_text(encoding="utf-8", errors="replace"))
               for f in files]
    ref = build_reference(sources)
    out = reference_to_json(ref, str(args.canvas_dir), [f.name for f in files])

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(out, indent=2, sort_keys=True,
                                   ensure_ascii=False) + "\n")

    print(f"tables: {len(ref.tables)}  extensions: {out['_extensions']}  "
          f"notes: {len(ref.notes)}")
    for n in ref.notes[:40]:
        print(f"  note: {n}")
    for kind, count in sorted(ref.ignored.items()):
        print(f"  ignored {kind}: {count}")

    if args.validate_authored:
        problems = validate_authored(out, args.validate_authored)
        if problems:
            print(f"\nauthored-reference validation: {len(problems)} problem(s)")
            for prob in problems:
                print(f"  {prob}")
            return 1
        n_tables = sum(1 for k in json.loads(args.validate_authored.read_text())
                       if not k.startswith("_"))
        print(f"\nauthored-reference validation: all {n_tables} tables match "
              "(column existence + Rails->PG type)")

    return 0


if __name__ == "__main__":
    sys.exit(main())
