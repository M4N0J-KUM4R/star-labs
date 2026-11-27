#!/usr/bin/env python3
"""Seed the replay database with the rows the recorded Canvas cases refer to.

The recorded corpus was captured against a seeded Canvas, so the ids in it
(account 6, user 1, course 1, group 1, folder 1, ...) only resolve if the
replay database has matching rows.

Canvas answered the recorded requests with an access token, and the
`x-canvas-meta` header carries the token's global id (`at`) and the global id
of its developer key (`dk`). The port resolves the caller from the
`access_tokens` row, so the seed also creates that row (access token 7,
developer key 1) and hashes the replay token the way Canvas hashes it.

The Rust binary runs `star_db::MIGRATOR` on start, so the schema comes from
the binary. This script seeds the data, using the ids the Canvas seed
produced.

Usage:
    seed_replay_db.py            # uses DATABASE_URL from the environment

Environment:
    DATABASE_URL      the replay database
    STAR_REPLAY_TOKEN the bearer token the replay injects
    STAR_JWT_SECRET   the secret the port keys the access-token hash with
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import pathlib
import sys

try:
    import psycopg
except ImportError:  # pragma: no cover
    psycopg = None


# The bearer token the replay injects, and the secret the port keys the
# access-token hash with. Canvas hashes its access tokens with
# `Canvas::Security.encryption_key` (app/models/access_token.rb:176-180); the
# port keys the same hash with its one app secret, so the seeded row is
# looked up with exactly the hash the port computes.
REPLAY_TOKEN = os.environ.get("STAR_REPLAY_TOKEN", "").strip()
TOKEN_SECRET = os.environ.get("STAR_JWT_SECRET", "")


def hmac_sha1(value: str, secret: str) -> str:
    """Canvas' `Canvas::Security.hmac_sha1` (gems/canvas_security)."""
    return hmac.new(
        secret.encode("utf-8"), value.encode("utf-8"), hashlib.sha1
    ).hexdigest()


# id -> the row the Canvas oracle had. The columns match star-db's schema,
# which is thinner than Canvas' own; the replay then shows which fields the
# port still owes Canvas.
ROWS = [
    (
        "accounts",
        "INSERT INTO accounts (id, name, parent_account_id, workflow_state) "
        "VALUES (%s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
        (6, "Parity Account", None, "registered"),
    ),
    (
        "users",
        "INSERT INTO users (id, name, sortable_name, email, password_hash, role) "
        "VALUES (%s, %s, %s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
        (1, "Parity User", "Parity User", "parity@example.com", "not-a-real-hash", "teacher"),
    ),
    (
        # Canvas' seeded course 1 hangs off the Parity Account
        # (`accounts.id = 6`, `canvas-boot/seed.rb`). The port's `courses.account_id`
        # defaults to 1, which puts the course in the wrong account: every
        # account-scoped rule that Canvas resolves through
        # `Account#courses` (`app/models/account.rb:1692` — a course admin
        # enrollment in one of the account's courses grants the account
        # `:read`/`:read_files`) then misses, and the account routes answer
        # 403 where Canvas answered 200.
        "courses",
        "INSERT INTO courses (id, name, course_code, account_id) "
        "VALUES (%s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
        (1, "Parity Course", "PARITY", 6),
    ),
    (
        "enrollments",
        "INSERT INTO enrollments (id, user_id, course_id, role) "
        "VALUES (%s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
        (1, 1, 1, "teacher"),
    ),
    (
        # Canvas' seeded course 1 has a course section, and the recorded
        # section-scoped answers (`/api/v1/sections/1/...`) only resolve when
        # the section exists. Without the row the port answers 404 where
        # Canvas answered 403 (the permission is checked before the section's
        # contents).
        "course_sections",
        "INSERT INTO course_sections (id, course_id, name) "
        "VALUES (%s, %s, %s) ON CONFLICT (id) DO NOTHING",
        (1, 1, "Parity Section"),
    ),
    (
        "group_categories",
        "INSERT INTO group_categories (id, course_id, account_id, name) "
        "VALUES (%s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
        (1, 1, 6, "Parity Category"),
    ),
    (
        "groups",
        "INSERT INTO groups (id, course_id, name, group_category_id) "
        "VALUES (%s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
        (1, 1, "Parity Group", 1),
    ),
    (
        "folders",
        "INSERT INTO folders (id, name, full_name, context_type, context_id) "
        "VALUES (%s, %s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
        (1, "Parity Folder", "course files/Parity Folder", "Course", 1),
    ),
    (
        "assignments",
        "INSERT INTO assignments (id, course_id, title, points_possible) "
        "VALUES (%s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
        (1, 1, "Parity Assignment", 100),
    ),
]


def oracle_rows() -> list[tuple[str, str, tuple]]:
    """The rows the Canvas oracle had, as `tools/parity/replay-seed.json`.

    The dump comes from the oracle itself (`dump_oracle_seed.py`), so the
    replay seeds the same rows the recording was made against. Without them
    the port answers 404 where Canvas answered 200 and the replay measures
    the seed instead of the port.
    """
    path = pathlib.Path(__file__).resolve().parent / "replay-seed.json"
    if not path.is_file():
        return []
    seed = json.loads(path.read_text())
    rows = []
    # The order the foreign keys need: the accounts and the users before the
    # rows that point at them.
    order = [
        "accounts",
        "users",
        "courses",
        "course_sections",
        "group_categories",
        "groups",
        "folders",
        "assignments",
        "enrollments",
    ]
    for table in order + [t for t in seed if t not in order]:
        spec = seed.get(table)
        if not spec:
            continue
        columns = spec["columns"]
        placeholders = ", ".join(["%s"] * len(columns))
        sql = (
            f"INSERT INTO {table} ({', '.join(columns)}) "
            f"VALUES ({placeholders}) ON CONFLICT (id) DO NOTHING"
        )
        for row in spec["rows"]:
            rows.append((table, sql, tuple(row.get(c) for c in columns)))
    return rows


def access_token_rows() -> list[tuple[str, str, tuple]]:
    """The rows the replay's access token needs.

    Canvas answers the recorded requests with an access token, and the
    `x-canvas-meta` header carries its global id (`at`) and the global id of
    its developer key (`dk`):

        at=10000000000007;dk=10000000000001;o=accounts;n=show;...

    A global id is the shard id followed by the row id, so the ids the
    recorded corpus carries are access token 7 and developer key 1. The token
    row is what the port resolves the caller from
    (lib/authentication_methods.rb:144-177), so the replay has to seed it.
    """
    if not REPLAY_TOKEN or not TOKEN_SECRET:
        return []

    return [
        (
            "developer_keys",
            "INSERT INTO developer_keys (id, name, workflow_state, api_key, scopes) "
            "VALUES (%s, %s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
            (1, "Parity Developer Key", "active", "parity-developer-key", []),
        ),
        (
            "access_tokens",
            "INSERT INTO access_tokens (id, user_id, developer_key_id, token, "
            "crypted_token, token_hint, purpose, workflow_state) "
            "VALUES (%s, %s, %s, %s, %s, %s, %s, %s) ON CONFLICT (id) DO NOTHING",
            (
                7,
                1,
                1,
                REPLAY_TOKEN,
                hmac_sha1(REPLAY_TOKEN, TOKEN_SECRET),
                REPLAY_TOKEN[:5],
                "Parity replay",
                "active",
            ),
        ),
    ]


def seed() -> int:
    url = os.environ.get("DATABASE_URL")
    if not url:
        print("seed_replay_db: DATABASE_URL is not set", file=sys.stderr)
        return 2

    if psycopg is None:
        print(
            "seed_replay_db: psycopg is not installed; the schema comes from "
            "star_db::MIGRATOR when the binary starts. Install psycopg (or "
            "psycopg[binary]) to seed the rows the recorded ids refer to.",
            file=sys.stderr,
        )
        return 3

    ok, failed = 0, []
    rows = oracle_rows() + access_token_rows()
    with psycopg.connect(url) as conn:
        with conn.cursor() as cur:
            for index, (table, sql, params) in enumerate(rows):
                # A savepoint per row: one row the port's schema does not
                # accept must not take the rest of the seed with it.
                savepoint = f"seed_row_{index}"
                try:
                    cur.execute(f"SAVEPOINT {savepoint}")
                    # The row this one replaces: a replay database seeded
                    # twice keeps the old row otherwise, and the enrollment's
                    # unique key is the pair, not the id.
                    try:
                        if table == "enrollments":
                            cur.execute(
                                "DELETE FROM enrollments "
                                "WHERE user_id = %s AND course_id = %s AND role = %s",
                                (params[1], params[2], params[3]),
                            )
                        elif table == "folders":
                            # The folder's root is unique per context.
                            cur.execute(
                                "DELETE FROM folders WHERE id = %s "
                                "OR (context_type = %s AND context_id = %s)",
                                (params[0], params[3], params[4]),
                            )
                        else:
                            cur.execute(f"DELETE FROM {table} WHERE id = %s", (params[0],))
                    except Exception:  # noqa: BLE001 - the row stays
                        # Something else points at the row (an enrollment term
                        # at the account, a course at the account). The insert
                        # below then keeps what is already there.
                        cur.execute(f"ROLLBACK TO SAVEPOINT {savepoint}")
                        cur.execute(f"SAVEPOINT {savepoint}")
                    cur.execute(sql, params)
                    cur.execute(f"RELEASE SAVEPOINT {savepoint}")
                    ok += 1
                except Exception as exc:  # noqa: BLE001 - report and continue
                    cur.execute(f"ROLLBACK TO SAVEPOINT {savepoint}")
                    failed.append(f"{table}: {exc}")
        conn.commit()

    print(f"seed_replay_db: seeded {ok}/{len(rows)} tables")
    for f in failed:
        print(f"  skipped {f}")
    return 0 if not failed else 1


if __name__ == "__main__":
    raise SystemExit(seed())
