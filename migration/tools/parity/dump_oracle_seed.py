#!/usr/bin/env python3
"""Dump the Canvas oracle's seeded rows as the replay seed.

The recorded corpus was captured against the seeded Canvas in
`canvas-boot/seed.rb`. The replay needs the same rows in the port's schema:
without them the port answers 404 where Canvas answered 200, and the replay
measures the seed instead of the port.

This script asks the oracle for the rows and writes them to
`tools/parity/replay-seed.json`, which `seed_replay_db.py` inserts. The
JSON is committed, so CI seeds the same rows without a Canvas instance.

Usage:
    dump_oracle_seed.py            # writes tools/parity/replay-seed.json
"""
from __future__ import annotations

import json
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE / "replay-seed.json"

COMPOSE = pathlib.Path(__file__).resolve().parents[4] / "canvas-boot" / "docker-compose.yml"

# table -> (the columns the port's schema has, the Canvas columns behind
# them, the extra constant columns the port needs)
TABLES: dict[str, dict] = {
    "accounts": {
        "columns": ["id", "name", "parent_account_id", "workflow_state"],
        "canvas": "id, name, parent_account_id, workflow_state",
        "from": "accounts",
    },
    "users": {
        # Canvas keeps the email in the pseudonyms table; the port's schema
        # has it on the user row.
        "columns": ["id", "name", "sortable_name", "email", "password_hash", "role"],
        "canvas": "u.id, u.name, u.sortable_name, "
                  "coalesce((SELECT p.unique_id FROM pseudonyms p "
                  "WHERE p.user_id = u.id AND p.workflow_state <> 'deleted' "
                  "ORDER BY p.id LIMIT 1), 'user' || u.id || '@example.com') AS email",
        "from": "users u",
        "extra": {"password_hash": "not-a-real-hash", "role": "teacher"},
    },
    "courses": {
        "columns": ["id", "name", "course_code", "account_id"],
        "canvas": "id, name, course_code, account_id",
        "from": "courses",
        "extra": {"course_code": "PARITY"},
    },
    "enrollments": {
        # Canvas names the enrollment type; the port keeps the role.
        "columns": ["id", "user_id", "course_id", "role", "workflow_state"],
        "canvas": "id, user_id, course_id, type, workflow_state",
        "from": "enrollments",
        "role_from_type": True,
    },
    "course_sections": {
        "columns": ["id", "course_id", "name"],
        "canvas": "id, course_id, name",
        "from": "course_sections",
    },
    "group_categories": {
        "columns": ["id", "course_id", "account_id", "name"],
        "canvas": "id, name",
        "from": "group_categories",
        "extra": {"course_id": 1, "account_id": 6},
    },
    "groups": {
        "columns": ["id", "course_id", "name", "group_category_id"],
        "canvas": "id, name, group_category_id",
        "from": "groups",
        "extra": {"course_id": 1},
    },
    "folders": {
        "columns": ["id", "name", "full_name", "context_type", "context_id"],
        "canvas": "id, name, full_name, context_type, context_id",
        "from": "folders",
    },
    "assignments": {
        # Canvas keeps the course in the assignment's context.
        "columns": ["id", "course_id", "title", "points_possible"],
        "canvas": "id, context_id AS course_id, title, points_possible",
        "from": "assignments",
    },
}

# The ids the recorded corpus refers to. The oracle has more rows (the
# bootstrap data), and those are not what the cases ask about.
KEEP_IDS = {
    "accounts": [1, 4, 5, 6, 7],
    "users": [1, 2, 3, 4, 5, 6, 7, 8, 10],
    "courses": [1, 2, 3, 4, 5],
    "enrollments": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    "course_sections": [1, 2, 3],
    "group_categories": [1],
    "groups": [1],
    "folders": [1, 2],
    "assignments": [1],
}


def psql(sql: str) -> str:
    result = subprocess.run(
        ["docker", "compose", "-f", str(COMPOSE), "exec", "-T", "postgres",
         "psql", "-U", "canvas", "-d", "canvas_production", "-At", "-c", sql],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise SystemExit(f"psql failed: {result.stderr.strip()}")
    return result.stdout.strip()


def main() -> int:
    seed: dict[str, list[dict]] = {}
    for table, spec in TABLES.items():
        ids = KEEP_IDS.get(table, [])
        if not ids:
            continue
        id_list = ",".join(str(i) for i in ids)
        rows = psql(
            f"SELECT coalesce(json_agg(t), '[]'::json) FROM "
            f"(SELECT {spec['canvas']} FROM {spec['from']} WHERE id IN ({id_list}) ORDER BY id) t"
        )
        parsed = json.loads(rows)
        extra = spec.get("extra", {})
        out = []
        for row in parsed:
            record = dict(row)
            if spec.get("role_from_type"):
                enrollment_type = record.pop("type", "") or ""
                record["role"] = {
                    "TeacherEnrollment": "teacher",
                    "TaEnrollment": "ta",
                    "DesignerEnrollment": "designer",
                    "StudentViewEnrollment": "student",
                    "StudentEnrollment": "student",
                }.get(enrollment_type, "student")
            for key, value in extra.items():
                if record.get(key) is None:
                    record[key] = value
            out.append(record)
        seed[table] = {"columns": spec["columns"], "rows": out}
        print(f"{table}: {len(out)} row(s)")

    OUT.write_text(json.dumps(seed, indent=1, sort_keys=True) + "\n")
    print(f"wrote {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
