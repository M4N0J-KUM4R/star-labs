#!/usr/bin/env python3
"""CI guard for star-db sqlx migrations (issue #102).

The reservation-ledger rules (protocol issue #20; the ledger itself lives at
orchestration/migration-reservations.json) are: a migration file is named
NNNN_snake_case.sql, its 4-digit number is unique, existing numbers are never
renumbered, renamed away or deleted, and schema changes are additive-only.
sqlx checksums already make editing an applied migration fatal at runtime;
this guard enforces the rest mechanically at review time:

  [format]       filename must match NNNN_snake_case.sql (4 digits, lowercase
                 snake_case name, .sql suffix) — same pattern as the hygiene
                 step in production-build.yml.
  [duplicate]    no two files at head may share a migration number.
  [renumber]     with --base: every (number, filename) present in the base
                 snapshot must still be present at head with the same
                 filename — catches renumbering, renaming and deletion. CI
                 passes the merge-base with origin/main as the base, so a
                 base branch that moved ahead (new migrations landing on main
                 after the PR branched) never yields false failures. The
                 workflow checks out the pull_request merge ref, so the
                 guard's code and the tree it validates always come from the
                 same commit.
  [destructive]  no DROP or TRUNCATE statement in any migration at head
                 (additive-only). Allowed: the `DROP NOT NULL` / `DROP DEFAULT`
                 relaxations (main's 0048/0228 use the former), SQL comments,
                 and the pinned exceptions in migration_destructive_allowlist.txt
                 (each entry carries its justification; additions are
                 diff-visible PR decisions).

Known limitations, deliberate: string literals are not parsed (a literal
containing "DROP TABLE" would be flagged), and dollar-quoted function bodies
are scanned as plain SQL. Ledger<->disk consistency is NOT re-checked here —
that is orchestration/check-migration-ledger.py's job (issue #136).

Usage:
    python3 tools/ci/migration_guard.py             # structural checks only
    python3 tools/ci/migration_guard.py --base DIR  # + renumber detection

Exit code 0 = all rules hold; 1 = violations printed.
"""
from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

DEFAULT_HEAD = Path("migration/crates/star-db/migrations")
DEFAULT_ALLOWLIST = Path(__file__).resolve().parent / "migration_destructive_allowlist.txt"
DEFAULT_EDIT_ALLOWLIST = Path(__file__).resolve().parent / "migration_edit_allowlist.txt"
FILENAME_RE = re.compile(r"^(\d{4})_([a-z0-9_]+)\.sql$")
ALLOWLIST_RE = re.compile(r"^(\S+):(\d+)\s+(DROP|TRUNCATE)$")
# DROP followed by anything except the NOT NULL / DEFAULT relaxations, and
# TRUNCATE anywhere. Word boundaries plus the required following token keep
# identifiers and values like `dropped`, `dropped_at` or `dropout` from
# matching; `\s+` spans newlines, so a DROP split across lines is caught.
DESTRUCTIVE_RE = re.compile(
    r"\bDROP\s+(?!NOT\s+NULL\b)(?!DEFAULT\b)\S+|\bTRUNCATE\b",
    re.IGNORECASE,
)


def strip_sql_comments(text: str) -> str:
    """Remove `-- line` and `/* block */` comments, preserving newlines.

    Newlines are preserved so that line numbers computed on the stripped text
    match the original file.
    """
    out: list[str] = []
    i = 0
    n = len(text)
    line_comment = False
    block_comment = False
    while i < n:
        two = text[i : i + 2]
        if line_comment:
            if text[i] == "\n":
                line_comment = False
                out.append("\n")
            i += 1
        elif block_comment:
            if two == "*/":
                block_comment = False
                i += 2
            else:
                if text[i] == "\n":
                    out.append("\n")
                i += 1
        elif two == "--":
            line_comment = True
            i += 2
        elif two == "/*":
            block_comment = True
            i += 2
        else:
            out.append(text[i])
            i += 1
    return "".join(out)


def find_destructive(sql: str) -> list[tuple[int, str]]:
    """Return (line_number, keyword) for every destructive statement."""
    text = strip_sql_comments(sql)
    hits: list[tuple[int, str]] = []
    for match in DESTRUCTIVE_RE.finditer(text):
        keyword = match.group(0).split()[0].upper()
        line = text.count("\n", 0, match.start()) + 1
        hits.append((line, keyword))
    return hits


def collect_migrations(directory: Path) -> tuple[dict[str, list[str]], list[str]]:
    """Map migration number -> sorted filenames; plus non-conforming names.

    Files without a .sql suffix are ignored entirely (sqlx would not pick
    them up either); .sql files with a bad name are reported by the caller.
    """
    numbers: dict[str, list[str]] = {}
    bad_names: list[str] = []
    for entry in sorted(directory.iterdir()):
        if not entry.is_file() or entry.suffix != ".sql":
            continue
        match = FILENAME_RE.match(entry.name)
        if match:
            numbers.setdefault(match.group(1), []).append(entry.name)
        else:
            bad_names.append(entry.name)
    return numbers, bad_names


def load_allowlist(path: Path) -> tuple[set[str], list[str]]:
    """Parse `NNNN_name.sql:LINE KEYWORD` entries; return (set, errors)."""
    entries: set[str] = set()
    errors: list[str] = []
    if not path.is_file():
        return entries, errors
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        match = ALLOWLIST_RE.match(line)
        if match:
            entries.add(f"{match.group(1)}:{int(match.group(2))}:{match.group(3)}")
        else:
            errors.append(
                f"[allowlist] {path.name}: malformed entry {line!r} "
                "(expected NNNN_name.sql:LINE KEYWORD)"
            )
    return entries, errors


def introduced_content(git_root: Path, path: str) -> str | None:
    """The bytes of a migration file as the commit that introduced it wrote them.

    `None` when the file is not in git yet (a new migration) or the git
    history is unavailable — a file that git does not know cannot have been
    edited after it was applied.
    """
    added = subprocess.run(
        ["git", "-C", str(git_root), "log", "--follow", "--format=%H", "--", path],
        capture_output=True,
        text=True,
    )
    if added.returncode != 0:
        return None
    # `--follow` walks the rename chain, so the oldest commit may carry the
    # file under an earlier name; the first commit that has the file under
    # the name it has today is the one that introduced it.
    for commit in reversed(added.stdout.split()):
        show = subprocess.run(
            ["git", "-C", str(git_root), "show", f"{commit}:{path}"],
            capture_output=True,
            text=True,
        )
        if show.returncode == 0:
            return show.stdout
    return None


def load_edit_allowlist(path: Path) -> set[str]:
    """The migrations known to have been edited after they were added.

    Each entry is a filename and a reason. The entries are debt: every one
    of them breaks a database that applied the file, and the list shrinks as
    the files are restored.
    """
    entries: set[str] = set()
    if not path.is_file():
        return entries
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        entries.add(line.split()[0])
    return entries


def known_edit_debt(head_dir: Path, git_root: Path | None = None) -> list[str]:
    """The migrations that were edited after they were added, and are allowed.

    The list is debt: each entry is a file that breaks every database that
    applied it. `tools/ci/migration_edit_allowlist.txt` holds the entries.
    """
    if git_root is None or not (git_root / ".git").exists():
        return []
    allowlist = load_edit_allowlist(DEFAULT_EDIT_ALLOWLIST)
    debt: list[str] = []
    for _, names in sorted(collect_migrations(head_dir)[0].items()):
        for name in names:
            path = migration_path(head_dir, git_root, name)
            if path is None:
                continue
            original = introduced_content(git_root, path)
            if original is None or original == (head_dir / name).read_text():
                continue
            if name in allowlist:
                debt.append(f"[edited] {name}: known debt — restore the file and "
                            "put the change in a new migration")
    return debt


def migration_path(head_dir: Path, git_root: Path, name: str) -> str | None:
    """The repository path of a migration file, or None when it is outside."""
    try:
        return str((head_dir / name).relative_to(git_root))
    except ValueError:
        return None


def check(head_dir: Path, base_dir: Path | None = None,
          git_root: Path | None = None) -> tuple[list[str], int]:
    """Run all rules; return (violations, migration count at head)."""
    errors: list[str] = []
    if not head_dir.is_dir():
        return [f"[head] migration directory not found: {head_dir}"], 0
    if base_dir is not None and not base_dir.is_dir():
        return [f"[base] baseline migration directory not found: {base_dir}"], 0

    numbers, bad_names = collect_migrations(head_dir)
    count = sum(len(names) for names in numbers.values())
    if count == 0:
        return [f"[head] no *.sql migrations found in {head_dir}"], 0

    for name in bad_names:
        errors.append(
            f"[format] {name}: filename must match NNNN_snake_case.sql "
            "(4 digits, lowercase snake_case, .sql)"
        )
    for number, names in sorted(numbers.items()):
        if len(names) > 1:
            errors.append(f"[duplicate] number {number} used by {', '.join(names)}")

    allowed, allowlist_errors = load_allowlist(DEFAULT_ALLOWLIST)
    errors.extend(allowlist_errors)
    for number, names in sorted(numbers.items()):
        for name in names:
            for line, keyword in find_destructive((head_dir / name).read_text()):
                if f"{name}:{line}:{keyword}" in allowed:
                    continue
                errors.append(
                    f"[destructive] {name}:{line} {keyword} statement "
                    "(additive-only; see migration_destructive_allowlist.txt)"
                )

    if base_dir is not None:
        base_numbers, _ = collect_migrations(base_dir)
        for number, base_names in sorted(base_numbers.items()):
            head_names = numbers.get(number, [])
            if not head_names:
                errors.append(
                    f"[renumber] number {number} ({', '.join(base_names)}) "
                    "present in the base snapshot but missing at head"
                )
                continue
            renamed = [name for name in base_names if name not in head_names]
            if renamed:
                errors.append(
                    f"[renumber] number {number}: base has {', '.join(renamed)} "
                    f"but head has {', '.join(head_names)}"
                )

    # An edited migration is the failure mode that takes a whole database
    # down: sqlx records the SHA-384 of the file it applied
    # (sqlx-core/src/migrate/migration.rs) and refuses to run when the file
    # changed — `VersionMismatch`. The nightly's Supabase lane has been
    # failing exactly that way.
    if git_root is not None and (git_root / ".git").exists():
        edit_allowlist = load_edit_allowlist(DEFAULT_EDIT_ALLOWLIST)
        for number, names in sorted(numbers.items()):
            for name in names:
                path = migration_path(head_dir, git_root, name)
                if path is None:
                    continue
                original = introduced_content(git_root, path)
                if original is None:
                    continue
                if original == (head_dir / name).read_text():
                    continue
                message = (
                    f"[edited] {name}: the file changed after it was added — "
                    "sqlx records the file's checksum, so an edited migration "
                    "breaks every database that has applied it. Restore the "
                    "file and put the change in a new migration."
                )
                if name not in edit_allowlist:
                    errors.append(message)

    return errors, count


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--head",
        type=Path,
        default=DEFAULT_HEAD,
        help=f"head migrations directory (default: {DEFAULT_HEAD})",
    )
    parser.add_argument(
        "--base",
        type=Path,
        default=None,
        help="baseline migrations directory (merge-base snapshot) for "
        "renumber/rename/deletion detection",
    )
    parser.add_argument(
        "--git-root",
        type=Path,
        default=Path.cwd(),
        help="repository root, for the 'an added migration is never edited' "
             "rule (default: the current directory)",
    )
    args = parser.parse_args(argv)

    errors, count = check(args.head, args.base, args.git_root)
    debt = known_edit_debt(args.head, args.git_root)
    if debt:
        print(f"migration guard: {len(debt)} known debt item(s) (ratchet — see "
              f"{DEFAULT_EDIT_ALLOWLIST.name}):")
        for item in debt:
            print(f"  {item}")
    if errors:
        print(f"migration guard: {len(errors)} violation(s) in {args.head}")
        for error in errors:
            print(f"  {error}")
        print(
            "\nRules (protocol #20): numbers unique, never renumbered, "
            "additive-only — no DROP/TRUNCATE in migrations, and a migration "
            "file is never edited after it is added."
        )
        return 1
    print(f"migration guard: OK — {count} migrations pass all rules")
    return 0


if __name__ == "__main__":
    sys.exit(main())
