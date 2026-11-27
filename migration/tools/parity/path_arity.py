#!/usr/bin/env python3
"""Flag star routes whose `Path<T>` extractor arity cannot match the path.

Axum rejects a request when the number of `{param}` segments in a route
template differs from the number of values `Path<T>` expects, so a mismatch is
a runtime 500 on that route -- and the parity scoreboard never sees it, because
parity.py compares route templates against Canvas' route table, not handler
signatures.

    python3 tools/parity/path_arity.py [path/to/migration/crates]

Prints one line per mismatch; exit 1 if any. Runs from anywhere: with no
argument it scans the `crates` directory next to this file's workspace.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROUTE_HEAD = re.compile(r'\.route\(\s*\n?\s*"([^"]+)"\s*,')
VERB_CALL = re.compile(r"\b(?:get|post|put|patch|delete|head|options|trace)\((\w+(?:::\w+)*)\)")
PATH_TYPE = re.compile(r"Path<\s*(?:\(([^)]*)\)|([A-Za-z_][\w:]*))\s*>")
PRIMITIVES = {"i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64",
              "u128", "usize", "f32", "f64", "bool", "String", "str", "Uuid"}


def params(template: str) -> int:
    return len(re.findall(r"\{[^}]+\}", template))


def top_level_entries(body: str) -> int:
    """Comma-separated entries at bracket depth 0, ignoring // comments."""
    body = re.sub(r"//[^\n]*", "", body)
    opens = {"(": ")", "{": "}", "[": "]", "<": ">"}
    closers = set(opens.values())
    count = 0
    token = ""
    depth = 0
    for c in body:
        if c in opens:
            depth += 1
        elif c in closers:
            depth -= 1
        if c == "," and depth == 0:
            if token.strip():
                count += 1
            token = ""
        else:
            token += c
    if token.strip():
        count += 1
    return count


def struct_arity(text: str, name: str) -> int | None:
    """Field/element count of a local `struct NAME { .. }` or `struct NAME(..)`."""
    m = re.search(rf"\bstruct\s+{re.escape(name)}\s*([{{(])", text)
    if not m:
        return None
    open_at = m.start(1)
    return top_level_entries(text[open_at + 1 : close_bracket(text, open_at)])


def close_bracket(text: str, open_at: int) -> int:
    opener = text[open_at]
    closer = {"(": ")", "{": "}", "[": "]", "<": ">"}[opener]
    depth = 0
    for i in range(open_at, len(text)):
        if text[i] == opener:
            depth += 1
        elif text[i] == closer:
            depth -= 1
            if depth == 0:
                return i
    return len(text) - 1


def path_arity(sig: str, crate_text: str, skipped: set[str]) -> int | None:
    """Values the signature's `Path<T>` expects: 1 for `Path<i64>`, N for an
    N-tuple or N-field local struct, None when there is no Path extractor or
    the type is not resolvable from this crate."""
    m = PATH_TYPE.search(re.sub(r"\s+", "", sig))
    if not m:
        return None
    if m.group(1) is not None:
        return len([p for p in m.group(1).split(",") if p])
    ty = m.group(2).split("::")[-1]
    if ty in PRIMITIVES:
        return 1
    arity = struct_arity(crate_text, ty)
    if arity is None:
        skipped.add(ty)
    return arity


def close_index(text: str, open_at: int) -> int:
    depth = 0
    for i in range(open_at, len(text)):
        if text[i] == "(":
            depth += 1
        elif text[i] == ")":
            depth -= 1
            if depth == 0:
                return i
    return len(text) - 1


def signature(text: str, handler: str) -> str | None:
    for m in re.finditer(rf"(?:pub )?(?:async )?fn {re.escape(handler)}\s*\(", text):
        return text[m.end() - 1 : close_index(text, m.end() - 1)]
    return None


def scan(path: Path, crate_dir: Path, crate_text: str,
         skipped: set[str]) -> list[str]:
    text = path.read_text(encoding="utf-8", errors="replace")
    cut = text.find("#[cfg(test)]")
    if cut != -1:
        text = text[:cut]
    out = []
    for m in ROUTE_HEAD.finditer(text):
        template = m.group(1)
        route_open = text.index("(", m.start())
        route_close = close_index(text, route_open)
        body = text[m.end() : route_close]
        want = params(template)
        for handler in VERB_CALL.findall(body):
            sig = signature(text, handler) or signature(crate_text, handler)
            if sig is None:
                continue
            have = path_arity(sig, crate_text, skipped)
            if have is not None and have != want:
                line = text[: m.start()].count("\n") + 1
                shape = "a 1-value Path" if have == 1 else f"a {have}-value Path"
                out.append(f"{crate_dir.name}/{path.relative_to(crate_dir)}:{line}: "
                           f"{template} ({want} param) -> {handler} expects {shape}")
    return out


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    root = Path(args[0]) if args else Path(__file__).resolve().parents[2] / "crates"
    if not root.is_absolute():
        root = Path.cwd() / root
    rows: list[str] = []
    skipped: set[str] = set()
    for crate_dir in sorted(p for p in root.iterdir() if (p / "src").is_dir()):
        files = sorted((crate_dir / "src").rglob("*.rs"))
        crate_text = "\n".join(f.read_text(encoding="utf-8", errors="replace")
                               for f in files)
        for path in files:
            rows.extend(scan(path, crate_dir, crate_text, skipped))
    for row in dict.fromkeys(rows):
        print(row)
    if "--list-skipped" in sys.argv and skipped:
        print("unresolved Path types (review by hand): "
              + ", ".join(sorted(skipped)))
    print(f"path-arity: {len(dict.fromkeys(rows))} mismatch(es)")
    return 1 if rows else 0


if __name__ == "__main__":
    sys.exit(main())
