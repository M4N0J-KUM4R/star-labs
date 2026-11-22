"""Unit tests for tools/ci/migration_guard.py (issue #102).

Acceptance criteria pinned here: a deliberately-bad migration fails the check
(every violation class), and the existing repo migrations pass. The
merge-base semantics — every base-snapshot migration must survive at head,
base additions beyond the fork point are fine — are pinned by the renumber
tests.
"""
from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from migration_guard import (  # noqa: E402
    DEFAULT_ALLOWLIST,
    check,
    find_destructive,
    main,
    strip_sql_comments,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
REPO_MIGRATIONS = REPO_ROOT / "migration/crates/star-db/migrations"

GOOD_SQL = """
-- enrollment workflow states: active, invited, dropped
CREATE TABLE enrollments (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL,
    course_id BIGINT NOT NULL,
    workflow_state TEXT NOT NULL DEFAULT 'active'
        CHECK (workflow_state IN ('active', 'invited', 'dropped')),
    dropped_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX enrollments_user_fk ON enrollments (user_id);
ALTER TABLE enrollments
    ADD CONSTRAINT enrollments_course_fk FOREIGN KEY (course_id)
    REFERENCES courses (id) ON DELETE CASCADE;
ALTER TABLE enrollments ALTER COLUMN user_id DROP NOT NULL;
ALTER TABLE enrollments ALTER COLUMN course_id DROP DEFAULT;
INSERT INTO enrollments (user_id, course_id) VALUES (1, 2);
"""


class MigrationGuardTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        root = Path(self._tmp.name)
        self.head = root / "head"
        self.base = root / "base"
        self.head.mkdir()
        self.base.mkdir()

    def tearDown(self) -> None:
        self._tmp.cleanup()

    def write(self, directory: Path, name: str, sql: str) -> None:
        (directory / name).write_text(sql)

    def errors(self, with_base: bool = False) -> list[str]:
        return check(self.head, self.base if with_base else None)[0]

    # -- happy path ----------------------------------------------------------

    def test_valid_migrations_pass(self) -> None:
        self.write(self.head, "0001_core.sql", GOOD_SQL)
        self.write(
            self.head,
            "0002_commented_drop.sql",
            "-- we deliberately drop nothing here\n"
            "/* DROP TABLE secrets */\n"
            "CREATE TABLE fine (id BIGINT PRIMARY KEY);\n",
        )
        self.assertEqual(self.errors(), [])

    def test_real_repo_migrations_pass(self) -> None:
        errors, count = check(REPO_MIGRATIONS)
        self.assertEqual(errors, [])
        self.assertGreater(count, 100)  # non-vacuous: the real dir is scanned

    def test_guard_main_returns_zero_on_valid_tree(self) -> None:
        self.write(self.head, "0001_core.sql", GOOD_SQL)
        self.assertEqual(main(["--head", str(self.head)]), 0)

    # -- [format] ------------------------------------------------------------

    def test_bad_filenames_fail(self) -> None:
        for name in ("0100-kebab-case.sql", "0100_Upper.sql", "0100short.sql"):
            self.write(self.head, name, "CREATE TABLE t (id BIGINT);\n")
        self.write(self.head, "0001_core.sql", GOOD_SQL)
        errors = self.errors()
        self.assertEqual(len([e for e in errors if e.startswith("[format]")]), 3)

    def test_non_sql_files_are_ignored(self) -> None:
        self.write(self.head, "0001_core.sql", GOOD_SQL)
        self.write(self.head, "README.md", "not a migration")
        self.assertEqual(self.errors(), [])

    # -- [duplicate] ---------------------------------------------------------

    def test_duplicate_numbers_fail(self) -> None:
        self.write(self.head, "0100_a.sql", GOOD_SQL)
        self.write(self.head, "0100_b.sql", GOOD_SQL)
        errors = self.errors()
        self.assertTrue(any(e.startswith("[duplicate] number 0100") for e in errors))

    # -- [renumber] ------------------------------------------------------------

    def test_renamed_migration_fails(self) -> None:
        self.write(self.base, "0005_files.sql", GOOD_SQL)
        self.write(self.head, "0005_files_v2.sql", GOOD_SQL)
        errors = self.errors(with_base=True)
        self.assertTrue(any(e.startswith("[renumber] number 0005") for e in errors))

    def test_deleted_migration_fails(self) -> None:
        self.write(self.base, "0005_files.sql", GOOD_SQL)
        self.write(self.base, "0006_next.sql", GOOD_SQL)
        self.write(self.head, "0006_next.sql", GOOD_SQL)
        errors = self.errors(with_base=True)
        self.assertTrue(any("0005" in e and e.startswith("[renumber]") for e in errors))

    def test_base_additions_are_fine(self) -> None:
        # Merge-base semantics: the baseline is a subset of head. Migrations
        # the base gained beyond the fork point are absent from the baseline,
        # so they cannot trigger the missing rule.
        self.write(self.base, "0005_files.sql", GOOD_SQL)
        self.write(self.head, "0005_files.sql", GOOD_SQL)
        self.write(self.head, "0006_new_on_branch.sql", GOOD_SQL)
        self.assertEqual(self.errors(with_base=True), [])

    def test_missing_base_directory_is_an_error(self) -> None:
        self.write(self.head, "0001_core.sql", GOOD_SQL)
        errors = check(self.head, self.base / "nope")[0]
        self.assertTrue(any(e.startswith("[base]") for e in errors))

    def test_empty_head_directory_is_an_error(self) -> None:
        empty = self.head / "empty"
        empty.mkdir()
        errors = check(empty)[0]
        self.assertTrue(any(e.startswith("[head]") for e in errors))

    # -- [destructive] ----------------------------------------------------------

    def test_drop_table_fails(self) -> None:
        self.write(self.head, "0001_core.sql", "DROP TABLE enrollments;\n")
        errors = self.errors()
        self.assertTrue(any(e.startswith("[destructive] 0001_core.sql:1 DROP") for e in errors))

    def test_truncate_fails(self) -> None:
        self.write(self.head, "0001_core.sql", "TRUNCATE TABLE enrollments;\n")
        errors = self.errors()
        self.assertTrue(any(e.startswith("[destructive] 0001_core.sql:1 TRUNCATE") for e in errors))

    def test_drop_column_and_constraint_fail(self) -> None:
        self.write(
            self.head,
            "0001_core.sql",
            "ALTER TABLE enrollments DROP COLUMN dropped_at;\n"
            "ALTER TABLE enrollments DROP CONSTRAINT enrollments_course_fk;\n",
        )
        errors = self.errors()
        self.assertEqual(len([e for e in errors if e.startswith("[destructive]")]), 2)

    def test_drop_split_across_lines_fails(self) -> None:
        self.write(self.head, "0001_core.sql", "ALTER TABLE enrollments\n  DROP\n  COLUMN x;\n")
        errors = self.errors()
        self.assertTrue(any(e.startswith("[destructive] 0001_core.sql:2 DROP") for e in errors))

    def test_commented_drop_passes(self) -> None:
        sql = "-- DROP TABLE enrollments\n/* TRUNCATE TABLE x */\nSELECT 1;\n"
        self.assertEqual(find_destructive(sql), [])
        self.assertEqual(strip_sql_comments(sql).count("DROP"), 0)

    # -- allowlist ---------------------------------------------------------------

    def _sql_with_drop_at_line(self, line: int) -> str:
        return "\n".join(["-- pad"] * (line - 2) + ["ALTER TABLE t", "  DROP CONSTRAINT broken_fk;"]) + "\n"

    def test_shipped_allowlist_exception_passes(self) -> None:
        # The shipped allowlist pins 0228_...:35 DROP; a hit at exactly that
        # file:line:keyword is allowed.
        self.write(self.head, "0228_lti_registrations_deep.sql", self._sql_with_drop_at_line(35))
        self.assertEqual(self.errors(), [])

    def test_allowlist_is_line_precise(self) -> None:
        # Same file, hit one line off the pinned entry: not covered.
        self.write(self.head, "0228_lti_registrations_deep.sql", self._sql_with_drop_at_line(34))
        errors = self.errors()
        self.assertTrue(any(e.startswith("[destructive] 0228") for e in errors))

    def test_shipped_allowlist_entries_parse(self) -> None:
        from migration_guard import ALLOWLIST_RE, load_allowlist

        entries, parse_errors = load_allowlist(DEFAULT_ALLOWLIST)
        self.assertEqual(parse_errors, [])
        self.assertTrue(entries)  # 0228's pinned exception is present
        for raw in DEFAULT_ALLOWLIST.read_text().splitlines():
            line = raw.strip()
            if line and not line.startswith("#"):
                self.assertTrue(ALLOWLIST_RE.match(line), line)

    def test_malformed_allowlist_line_fails(self) -> None:
        import migration_guard

        original = migration_guard.DEFAULT_ALLOWLIST
        try:
            bad = self.head / "bad_allowlist.txt"
            bad.write_text("this is not an entry\n")
            migration_guard.DEFAULT_ALLOWLIST = bad
            self.write(self.head, "0001_core.sql", GOOD_SQL)
            errors = self.errors()
            self.assertTrue(any(e.startswith("[allowlist]") for e in errors))
        finally:
            migration_guard.DEFAULT_ALLOWLIST = original

    # -- main() wiring ----------------------------------------------------------

    def test_guard_main_returns_one_on_violations(self) -> None:
        self.write(self.head, "0100_a.sql", "DROP TABLE x;\n")
        self.write(self.head, "0100_b.sql", "TRUNCATE TABLE y;\n")
        self.assertEqual(main(["--head", str(self.head)]), 1)


if __name__ == "__main__":
    unittest.main()
