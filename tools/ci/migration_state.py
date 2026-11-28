#!/usr/bin/env python3
"""Compare the tree's sqlx migrations with the migrations a database has applied.

sqlx records `(version, checksum)` for every migration it applies, and the
checksum is the SHA-384 of the migration file's bytes
(`sqlx-core/src/migrate/migration.rs`). A migration whose file changed after
it was applied is a `VersionMismatch` at the next `MIGRATOR.run()` — the
port cannot start against that database at all.

The nightly's Supabase lane runs the whole suite against one long-lived test
schema, so a migration edited after that schema applied it takes the whole
lane down. This tool says exactly which versions disagree, and can rebuild
the schema so the lane measures the tree instead of a stale database.

Usage:
    DATABASE_URL=postgres://... python3 tools/ci/migration_state.py \
        --migrations migration/crates/star-db/migrations

    ... --reset      # drop and recreate the public schema (a test schema only)

Exit codes:
    0  the database agrees with the tree (or it was reset)
    1  the database disagrees with the tree and --reset was not given
    2  the check could not run
"""

from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
import sys
from pathlib import Path


def checksum(path: Path) -> str:
    """The checksum sqlx records for a file migration: SHA-384 of the bytes."""
    return hashlib.sha384(path.read_bytes()).hexdigest()


def tree_migrations(directory: Path) -> dict[int, tuple[str, str]]:
    """version -> (filename, checksum) for every migration in the tree."""
    out: dict[int, tuple[str, str]] = {}
    for path in sorted(directory.glob("*.sql")):
        number = path.name.split("_", 1)[0]
        if not number.isdigit():
            continue
        out[int(number)] = (path.name, checksum(path))
    return out


def psql(url: str, sql: str) -> str:
    result = subprocess.run(
        ["psql", url, "-v", "ON_ERROR_STOP=1", "-At", "-c", sql],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "psql failed")
    return result.stdout.strip()


def applied_migrations(url: str) -> dict[int, str]:
    """version -> checksum hex for the migrations the database has applied."""
    try:
        rows = psql(
            url,
            "SELECT version, encode(checksum, 'hex') FROM _sqlx_migrations ORDER BY version",
        )
    except RuntimeError as error:
        # A database that has never been migrated has no table; that is an
        # empty history, not a failure.
        if "_sqlx_migrations" in str(error):
            return {}
        raise
    out: dict[int, str] = {}
    for line in rows.splitlines():
        if not line.strip():
            continue
        version, _, digest = line.partition("|")
        out[int(version)] = digest.strip()
    return out


def tables_schema(url: str) -> str:
    """The schema the port's tables live in.

    sqlx resolves an unqualified `CREATE TABLE` against the connection's
    search_path, and Postgres' default search_path is `"$user", public` —
    so the migrations table and the port's tables land in the schema named
    after the connecting role when that schema exists, and in `public`
    otherwise. Dropping `public` alone would leave them all in place, which
    is why the first reset did not take.
    """
    schema = psql(
        url,
        "SELECT n.nspname FROM pg_class c "
        "JOIN pg_namespace n ON n.oid = c.relnamespace "
        "WHERE c.relname = '_sqlx_migrations' AND c.relkind = 'r' LIMIT 1",
    )
    if schema:
        return schema
    schema = psql(
        url,
        "SELECT n.nspname FROM pg_class c "
        "JOIN pg_namespace n ON n.oid = c.relnamespace "
        "WHERE c.relname = 'courses' AND c.relkind = 'r' LIMIT 1",
    )
    return schema or "public"


def apply_migrations(url: str, directory: Path) -> int:
    """Apply the tree's migrations and record them the way sqlx does.

    The lane's four legs share one database, and the Supabase session
    pooler refuses connections beyond its client ceiling (15). If every leg
    migrated, four legs would hold a connection each while they wait for
    sqlx's advisory lock — and the pooler drops them ("expected to read 5
    bytes, got 0 bytes at EOF"). So the migration happens once, here, and
    the legs find a ready schema.

    The `_sqlx_migrations` rows are written with the checksum sqlx computes
    (SHA-384 of the file), so the legs' `MIGRATOR.run()` validates them
    and has nothing to do.
    """
    psql(url, "DROP TABLE IF EXISTS _sqlx_migrations")
    for path in sorted(directory.glob("*.sql")):
        number = path.name.split("_", 1)[0]
        if not number.isdigit():
            continue
        result = subprocess.run(
            ["psql", url, "-v", "ON_ERROR_STOP=1", "-q", "-f", str(path)],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            raise RuntimeError(f"{path.name}: {result.stderr.strip()[:400]}")
    psql(
        url,
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations ("
        "version BIGINT PRIMARY KEY,"
        "description TEXT NOT NULL,"
        "installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),"
        "success BOOLEAN NOT NULL,"
        "checksum BYTEA NOT NULL,"
        "execution_time BIGINT NOT NULL)",
    )
    tree = tree_migrations(directory)
    for version, (name, digest) in tree.items():
        description = name.split("_", 1)[1].replace(".sql", "")
        psql(
            url,
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) "
            f"VALUES ({version}, '{description}', true, "
            f"decode('{digest}', 'hex'), 0) ON CONFLICT (version) DO NOTHING",
        )
    return len(tree)


def reset_schema(url: str) -> str:
    """Drop and recreate the schema the port's tables live in.

    It is a test schema: the nightly runs the suite against it and every
    test seeds and tears down its own rows. Supabase's `auth` and `storage`
    schemas are left alone.
    """
    schema = tables_schema(url)
    # Both the schema that holds the migrations table and `public` are
    # cleared: the tables were created over several runs, and the ones from
    # a run whose search_path resolved differently sit in the other schema —
    # the apply then fails with "relation users already exists".
    schemas = [schema] if schema == "public" else [schema, "public"]
    for name in schemas:
        psql(url, f'DROP SCHEMA IF EXISTS "{name}" CASCADE')
        psql(url, f'CREATE SCHEMA "{name}"')
    return ", ".join(schemas)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--migrations", type=Path, required=True)
    parser.add_argument("--reset", action="store_true",
                        help="rebuild the schema when the database disagrees")
    parser.add_argument("--apply", action="store_true",
                        help="apply the tree's migrations and record them the way "
                             "sqlx does, so the lane's legs find a ready schema "
                             "instead of all migrating at once")
    args = parser.parse_args()

    url = os.environ.get("DATABASE_URL", "").strip()
    if not url:
        print("migration_state: DATABASE_URL is not set", file=sys.stderr)
        return 2
    if not args.migrations.is_dir():
        print(f"migration_state: {args.migrations} is not a directory", file=sys.stderr)
        return 2

    tree = tree_migrations(args.migrations)
    if not tree:
        print(f"migration_state: no migrations in {args.migrations}", file=sys.stderr)
        return 2

    try:
        applied = applied_migrations(url)
    except Exception as error:  # noqa: BLE001
        print(f"migration_state: cannot read the applied migrations: {error}", file=sys.stderr)
        return 2

    mismatched = sorted(
        version for version, digest in applied.items()
        if version in tree and tree[version][1] != digest
    )
    missing = sorted(set(tree) - set(applied))
    unknown = sorted(set(applied) - set(tree))

    print(f"tree: {len(tree)} migration(s); database: {len(applied)} applied")
    if applied:
        print(f"  the database's tables live in the schema {tables_schema(url)!r}")
    for version in mismatched:
        name, digest = tree[version]
        print(f"  MISMATCH {version} {name}: the file's checksum is {digest[:16]}…, "
              f"the database recorded {applied[version][:16]}…")
    for version in unknown:
        print(f"  UNKNOWN {version}: applied in the database, absent from the tree")
    if missing:
        print(f"  pending: {len(missing)} migration(s) the database has not applied yet "
              f"(first: {missing[0]})")

    if mismatched or unknown:
        if not args.reset:
            print("migration_state: the database disagrees with the tree", file=sys.stderr)
            return 1
        print("migration_state: rebuilding the schema so the lane measures the tree")
        try:
            schema = reset_schema(url)
        except Exception as error:  # noqa: BLE001
            print(f"migration_state: the schema could not be rebuilt: {error}", file=sys.stderr)
            return 2
        print(f"migration_state: the schema {schema!r} was rebuilt; "
              "the migrations run from scratch")
    if args.apply:
        print("migration_state: applying the tree's migrations")
        try:
            count = apply_migrations(url, args.migrations)
        except Exception as error:  # noqa: BLE001
            print(f"migration_state: the migrations could not be applied: {error}",
                  file=sys.stderr)
            return 2
        print(f"migration_state: {count} migration(s) applied and recorded")
    return 0


if __name__ == "__main__":
    sys.exit(main())
