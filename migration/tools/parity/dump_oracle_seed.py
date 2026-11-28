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
        # Canvas keeps the account settings in a YAML column; the port keeps
        # them as JSON. The recorded cases do not depend on them, and a
        # leftover from an earlier run would.
        # The parity account hangs under the site admin account: the port's
        # account-scoped user removal is a root-accounts-only route, and it
        # hard-deletes the user. Canvas refused that route with a 403 (the
        # parity user's pseudonym lives in the default account), so the
        # parity student has to survive it -- the oracle kept its dataset
        # intact with role overrides, which the port consults for the
        # assignment route only.
        "columns": ["id", "name", "parent_account_id", "workflow_state", "settings"],
        "canvas": "id, name, CASE WHEN id = 6 THEN 5 ELSE parent_account_id END "
                  "AS parent_account_id, workflow_state",
        "from": "accounts",
        "extra": {"settings": "{}"},
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
    "course_sections": {
        "columns": ["id", "course_id", "name", "workflow_state"],
        "canvas": "id, course_id, name, workflow_state",
        "from": "course_sections",
    },
    "enrollments": {
        # Canvas names the enrollment type; the port keeps the role. A preview
        # enrollment (StudentViewEnrollment) is not a student for Canvas's
        # submission summary, so it is not one in the port either.
        "columns": ["id", "user_id", "course_id", "role", "workflow_state", "type",
                    "course_section_id", "root_account_id"],
        "canvas": "id, user_id, course_id, type, workflow_state, "
                  "type AS enrollment_type, course_section_id, root_account_id",
        "from": "enrollments",
        "role_from_type": True,
    },
    "account_users": {
        # The parity user administers the parity account, which is what the
        # account-scoped routes answer with.
        "columns": ["id", "account_id", "user_id", "role", "workflow_state"],
        "canvas": "id, account_id, user_id, "
                  "(SELECT r.name FROM roles r WHERE r.id = role_id) AS role, "
                  "workflow_state",
        "from": "account_users",
    },
    "role_overrides": {
        # The oracle denied the destructive permissions to the account admin
        # role and to the teacher role, so that the corpus' own DELETE routes
        # could not take a seeded resource (and every read route that points
        # at it) with it. The port's role ids are its own built-in ones.
        "columns": ["id", "permission", "enabled", "locked", "context_type",
                    "context_id", "role_id"],
        "canvas": "id, permission, enabled, locked, context_type, context_id, "
                  "CASE role_id "
                  "WHEN (SELECT r.id FROM roles r WHERE r.name = 'AccountAdmin' "
                  "ORDER BY r.id LIMIT 1) THEN 6 "
                  "WHEN (SELECT r.id FROM roles r WHERE r.name = 'TeacherEnrollment' "
                  "ORDER BY r.id LIMIT 1) THEN 2 "
                  "ELSE role_id END AS role_id",
        "from": "role_overrides",
    },
    "group_categories": {
        "columns": ["id", "course_id", "account_id", "name", "role", "context_type",
                    "context_id", "workflow_state", "created_at"],
        "canvas": "id, "
                  "CASE WHEN context_type = 'Course' THEN context_id END AS course_id, "
                  "(SELECT a.root_account_id FROM accounts a "
                  "WHERE a.id = group_categories.context_id) AS account_id, "
                  "name, role, context_type, context_id, workflow_state, created_at",
        "from": "group_categories",
    },
    "groups": {
        "columns": ["id", "course_id", "name", "group_category_id", "description",
                    "workflow_state", "join_level", "is_public", "max_membership",
                    "members_count", "storage_quota_mb", "context_type",
                    "context_name", "role", "created_at"],
        "canvas": "id, "
                  "CASE WHEN context_type = 'Course' THEN context_id END AS course_id, "
                  "name, group_category_id, description, workflow_state, join_level, "
                  "is_public, max_membership, "
                  "(SELECT count(*) FROM group_memberships gm "
                  "WHERE gm.group_id = groups.id AND gm.workflow_state <> 'deleted') "
                  "AS members_count, "
                  "coalesce(storage_quota / 1048576, 0) AS storage_quota_mb, "
                  "context_type, "
                  "(SELECT c.name FROM courses c WHERE c.id = groups.context_id) "
                  "AS context_name, "
                  "(SELECT gc.role FROM group_categories gc "
                  "WHERE gc.id = groups.group_category_id) AS role, created_at",
        "from": "groups",
    },
    "group_memberships": {
        "columns": ["id", "group_id", "user_id", "workflow_state", "moderator",
                    "created_at"],
        "canvas": "id, group_id, user_id, workflow_state, moderator, created_at",
        "from": "group_memberships",
    },
    "folders": {
        "columns": ["id", "name", "full_name", "context_type", "context_id",
                    "parent_folder_id", "workflow_state", "position", "created_at"],
        "canvas": "id, name, full_name, context_type, context_id, parent_folder_id, "
                  "workflow_state, coalesce(position, 1) AS position, created_at",
        "from": "folders",
    },
    "assignments": {
        # Canvas keeps the course in the assignment's context.
        "columns": ["id", "course_id", "title", "points_possible", "workflow_state"],
        "canvas": "id, context_id AS course_id, title, points_possible, workflow_state",
        "from": "assignments",
    },
    "submissions": {
        "columns": ["id", "assignment_id", "user_id", "body", "score", "submitted_at",
                    "submission_type", "attempt", "workflow_state", "excused", "late",
                    "anonymous_id", "created_at", "updated_at"],
        "canvas": "id, assignment_id, user_id, body, score, submitted_at, "
                  "submission_type, attempt, workflow_state, "
                  "coalesce(excused, false) AS excused, "
                  "coalesce(late, false) AS late, "
                  "anonymous_id, created_at, updated_at",
        "from": "submissions",
    },
    "discussion_topics": {
        "columns": ["id", "course_id", "title", "message", "workflow_state",
                    "discussion_type", "user_id", "pinned", "locked", "created_at",
                    "updated_at"],
        "canvas": "id, context_id AS course_id, title, message, workflow_state, "
                  "discussion_type, user_id, coalesce(pinned, false) AS pinned, "
                  "coalesce(locked, false) AS locked, created_at, updated_at",
        "from": "discussion_topics",
    },
    "discussion_entries": {
        "columns": ["id", "topic_id", "parent_id", "user_id", "message",
                    "workflow_state", "created_at", "updated_at"],
        "canvas": "id, discussion_topic_id AS topic_id, parent_id, user_id, message, "
                  "workflow_state, created_at, updated_at",
        "from": "discussion_entries",
    },
    "discussion_topic_summaries": {
        "columns": ["id", "discussion_topic_id", "user_id", "llm_config_version",
                    "dynamic_content_hash", "summary", "parent_id", "locale",
                    "created_at", "updated_at"],
        "canvas": "id, discussion_topic_id, user_id, llm_config_version, "
                  "dynamic_content_hash, summary, parent_id, locale, "
                  "created_at, updated_at",
        "from": "discussion_topic_summaries",
    },
    "calendar_events": {
        "columns": ["id", "course_id", "user_id", "title", "description", "start_at",
                    "end_at", "workflow_state", "location_name", "created_at",
                    "updated_at"],
        "canvas": "id, "
                  "CASE WHEN context_type = 'Course' THEN context_id END AS course_id, "
                  "user_id, title, description, start_at, end_at, workflow_state, "
                  "location_name, created_at, updated_at",
        "from": "calendar_events",
    },
    "external_tools": {
        "columns": ["id", "name", "description", "url", "domain", "consumer_key",
                    "privacy_level", "workflow_state", "course_id", "account_id",
                    "context_type", "context_id", "tool_type", "lti_version",
                    "enabled", "visible", "created_at", "updated_at"],
        "canvas": "id, name, description, url, domain, consumer_key, privacy_level, "
                  "workflow_state, "
                  "CASE WHEN context_type = 'Course' THEN context_id END AS course_id, "
                  "(SELECT a.root_account_id FROM accounts a "
                  "WHERE a.id = external_tools.context_id) AS account_id, "
                  "context_type, context_id, tool_type, lti_version, "
                  "coalesce(enabled, true) AS enabled, "
                  "coalesce(visible, true) AS visible, created_at, updated_at",
        "from": "external_tools",
    },
    "files": {
        # Canvas calls them attachments and keeps the course in the folder the
        # attachment hangs in.
        "columns": ["id", "course_id", "uploader_id", "filename", "display_name",
                    "content_type", "size", "folder", "folder_id", "uuid",
                    "workflow_state", "created_at", "updated_at"],
        "canvas": "id, "
                  "(SELECT f.context_id FROM folders f WHERE f.id = attachments.context_id) "
                  "AS course_id, user_id AS uploader_id, filename, display_name, "
                  "content_type, size, NULL AS folder, folder_id, uuid, "
                  "workflow_state, created_at, updated_at",
        "from": "attachments",
        "extra": {"folder": "course files"},
    },
    "learning_outcomes": {
        "columns": ["id", "title", "short_description", "description", "display_name",
                    "workflow_state", "context_id", "context_type", "created_at",
                    "updated_at"],
        "canvas": "id, title, short_description, description, display_name, "
                  "workflow_state, context_id, context_type, created_at, updated_at",
        "from": "learning_outcomes",
    },
    "outcome_groups": {
        "columns": ["id", "course_id", "context_type", "context_id",
                    "parent_outcome_group_id", "title", "description",
                    "workflow_state", "created_at", "updated_at"],
        "canvas": "id, "
                  "CASE WHEN context_type = 'Course' THEN context_id END AS course_id, "
                  "context_type, context_id, "
                  "learning_outcome_group_id AS parent_outcome_group_id, "
                  "title, description, workflow_state, created_at, updated_at",
        "from": "learning_outcome_groups",
    },
    "outcome_links": {
        "columns": ["id", "outcome_id", "outcome_group_id", "context_type",
                    "context_id", "workflow_state"],
        "canvas": "id, outcome_id, learning_outcome_group_id AS outcome_group_id, "
                  "context_type, context_id, workflow_state",
        "from": "outcome_links",
    },
    "rubrics": {
        "columns": ["id", "course_id", "title", "points_possible", "workflow_state",
                    "created_at", "updated_at"],
        "canvas": "id, context_id AS course_id, title, points_possible, "
                  "workflow_state, created_at, updated_at",
        "from": "rubrics",
    },
    "rubric_associations": {
        "columns": ["id", "rubric_id", "association_id", "association_type",
                    "context_id", "context_type", "title", "purpose", "bookmarked",
                    "use_for_grading", "workflow_state"],
        "canvas": "id, rubric_id, association_id, association_type, context_id, "
                  "context_type, title, purpose, coalesce(bookmarked, true) AS bookmarked, "
                  "use_for_grading, workflow_state",
        "from": "rubric_associations",
    },
    "lti_registrations": {
        "columns": ["id", "account_id", "root_account_id", "name", "description",
                    "lti_version", "workflow_state", "configuration", "created_at",
                    "updated_at"],
        "canvas": "id, account_id, root_account_id, name, description, lti_version, "
                  "workflow_state, configuration, created_at, updated_at",
        "from": "lti_registrations",
    },
    "pages": {
        # Canvas calls them wiki pages and knows the front page through the
        # wiki it belongs to.
        "columns": ["id", "course_id", "title", "body", "url", "workflow_state",
                    "editing_roles", "front_page", "created_at", "updated_at"],
        "canvas": "id, context_id AS course_id, title, body, url, workflow_state, "
                  "editing_roles, "
                  "CASE WHEN (SELECT w.front_page_id FROM wikis w "
                  "WHERE w.id = wiki_pages.wiki_id) = wiki_pages.id THEN true "
                  "ELSE false END AS front_page, created_at, updated_at",
        "from": "wiki_pages",
    },
    "page_revisions": {
        "columns": ["id", "page_id", "revision_id", "title", "body", "url", "user_id",
                    "created_at", "updated_at"],
        "canvas": "id, page_id, revision_id, title, body, url, user_id, "
                  "created_at, updated_at",
        "from": "page_revisions",
    },
}

# The ids the recorded corpus refers to. The oracle has more rows (the
# bootstrap data), and those are not what the cases ask about.
KEEP_IDS = {
    "accounts": [1, 4, 5, 6, 7],
    "users": [1, 2, 3, 4, 5, 6, 7, 8, 9],
    "courses": [1, 2, 3, 4, 5],
    "course_sections": [1, 2, 3],
    "enrollments": [9, 10, 11, 12],
    "account_users": [1],
    "role_overrides": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
    "group_categories": [1],
    "groups": [1],
    "group_memberships": [1, 2],
    "folders": [1, 2, 4, 7],
    "assignments": [1],
    "submissions": [14],
    "discussion_topics": [1],
    "discussion_entries": [1],
    "discussion_topic_summaries": [1],
    "calendar_events": [1],
    "external_tools": [1],
    "files": [7],
    "learning_outcomes": [1, 2, 3, 4],
    "outcome_groups": [2, 4],
    "outcome_links": [1, 2, 3, 4, 5],
    "rubrics": [1],
    "rubric_associations": [1],
    "lti_registrations": [5],
    "pages": [5],
    "page_revisions": [1],
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
                # The Canvas enrollment type is the port's `type` column; the
                # copy under `enrollment_type` is what the role is mapped from.
                enrollment_type = record.pop("enrollment_type", "") or ""
                record["role"] = {
                    "TeacherEnrollment": "teacher",
                    "TaEnrollment": "ta",
                    "DesignerEnrollment": "designer",
                    # A preview enrollment is not a student enrollment.
                    "StudentViewEnrollment": "observer",
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
