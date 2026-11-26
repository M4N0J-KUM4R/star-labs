"""Pure-function tests for tools/schema/verify_schema_parity.py.

The apply/introspect path needs a live PostgreSQL (CI-only, pgvector/pg16
service container) and is deliberately NOT exercised here; everything below
runs without a database.
"""
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import verify_schema_parity as vp  # noqa: E402


def test_parse_indexdef_plain_unique_and_expression():
    d = vp.parse_indexdef(
        "CREATE UNIQUE INDEX index_things_on_a ON public.things USING btree (a)")
    assert d == {"columns": ["a"], "unique": True, "using": "btree",
                 "where": None}
    d2 = vp.parse_indexdef(
        "CREATE INDEX index_gin ON public.things USING gin "
        "(coalesce(lower(name), '') || ' ' || coalesce(lower(short_name), '') "
        "public.gin_trgm_ops) WHERE workflow_state IN ('registered')")
    assert d2["unique"] is False
    assert d2["using"] == "gin"
    assert "coalesce" in d2["columns"][0]
    assert d2["where"] == "workflow_state IN ('registered')"
    d3 = vp.parse_indexdef(
        "CREATE INDEX idx ON public.things USING btree (a, b, c)")
    assert d3["columns"] == ["a", "b", "c"]


def test_normalize_pg_type_strips_default_precision():
    assert vp.normalize_pg_type("timestamp(6) without time zone") == \
        "timestamp without time zone"
    assert vp.normalize_pg_type("timestamp without time zone") == \
        "timestamp without time zone"
    assert vp.normalize_pg_type("timestamp(3) without time zone") == \
        "timestamp(3) without time zone"  # non-default precision kept


def test_rails_type_to_pg_map():
    cases = [
        ({"type": "string", "limit": 255}, "character varying(255)"),
        ({"type": "string", "limit": None}, "character varying(255)"),
        ({"type": "string", "limit": 8}, "character varying(8)"),
        ({"type": "text", "limit": None}, "text"),
        ({"type": "bigint"}, "bigint"),
        ({"type": "boolean"}, "boolean"),
        ({"type": "datetime", "precision": 6}, "timestamp without time zone"),
        ({"type": "datetime", "precision": None},
         "timestamp without time zone"),
        ({"type": "datetime", "precision": 3}, "timestamp(3) without time zone"),
        ({"type": "timestamp", "precision": None},
         "timestamp without time zone"),
        ({"type": "decimal", "precision": 12, "scale": 2}, "numeric(12,2)"),
        ({"type": "float"}, "double precision"),
        ({"type": "jsonb"}, "jsonb"),
        ({"type": "uuid"}, "uuid"),
        ({"type": "binary"}, "bytea"),
        ({"type": "vector"}, "vector"),
    ]
    for col, expected in cases:
        assert vp.rails_type_to_pg(col) == expected, col


def test_normalize_default():
    assert vp.normalize_default("now()") == vp.normalize_default("NOW()")
    assert vp.normalize_default("CURRENT_TIMESTAMP") == "now()"
    assert vp.normalize_default("'{}'::jsonb") == "{}"
    assert vp.normalize_default(0) == "0"
    assert vp.normalize_default(None) == ""


def _reference():
    return {"tables": {
        "kept": {"columns": {
            "id": {"type": "bigint", "limit": None, "precision": None,
                   "scale": None, "not_null": True, "default": None},
            "name": {"type": "string", "limit": 255, "precision": None,
                     "scale": None, "not_null": False, "default": None}},
            "indexes": {"index_kept_on_name": {"columns": ["name"],
                                               "unique": False, "using": None,
                                               "where": None}},
            "foreign_keys": [{"column": "other_id", "to_table": "others",
                              "to_column": None, "name": "fk_x",
                              "on_delete": None, "on_update": None}]},
        "assignment_embeddings": {"columns": {}, "indexes": {},
                                  "foreign_keys": []},
    }}


def _live():
    return {
        "tables": {"kept": True, "star_only_thing": True},
        "columns": {"kept": {
            "id": {"type": "bigint", "not_null": True, "default": ""},
            "name": {"type": "character varying(255)", "not_null": True,
                     "default": "'x'::character varying"},
            "extra": {"type": "text", "not_null": False, "default": ""}}},
        "indexes": {"public.index_kept_on_name":
                    {"def": "CREATE INDEX index_kept_on_name ON public.kept "
                            "USING btree (name)", "table": "kept"}},
        "fks": [{"name": "fk_x", "table": "kept", "ref": "others",
                 "cols": ["other_id"], "ref_cols": ["id"]}],
    }


ALLOW = {"tables": {"assignment_embeddings": {"reason": "pgvector extension "
                                                      "pending (plan slice 1.3)"}}}


def test_diff_allows_allowlisted_missing_table_and_reports_rest():
    gap = vp.diff(_reference(), _live(), ALLOW)
    assert gap["missing_tables"]["assignment_embeddings"]["gating"] is False
    assert gap["gating_missing_table_count"] == 0
    assert "star_only_thing" in gap["star_only_tables"]
    entry = gap["tables"]["kept"]
    assert entry["extra_columns"] == ["extra"]
    # NOT NULL mismatch (reference nullable, live NOT NULL) is non-gating
    assert entry["not_null_mismatch"] == [{"column": "name",
                                           "reference_not_null": False,
                                           "live_not_null": True}]
    assert "missing_indexes" not in gap["tables"]["kept"]
    assert "missing_foreign_keys" not in gap["tables"]["kept"]


def test_diff_reports_missing_columns_and_gates_missing_table():
    ref = _reference()
    ref["tables"]["kept"]["columns"]["brand_new"] = {
        "type": "boolean", "limit": None, "precision": None, "scale": None,
        "not_null": False, "default": None}
    ref["tables"]["nonexistent_table"] = {"columns": {}, "indexes": {},
                                          "foreign_keys": []}
    gap = vp.diff(ref, _live(), ALLOW)
    assert gap["missing_tables"]["nonexistent_table"]["gating"] is True
    # Columns are reported, never gating (day one) — Wave 1.2 backlog.
    assert gap["tables"]["kept"]["missing_columns"]["columns"] == ["brand_new"]
    assert gap["gating_missing_table_count"] == 1
    assert gap["missing_column_count"] == 1
    assert gap["missing_column_tables"] == 1


def test_diff_missing_columns_ignore_allowlist():
    ref = _reference()
    ref["tables"]["kept"]["columns"]["brand_new"] = {
        "type": "boolean", "limit": None, "precision": None, "scale": None,
        "not_null": False, "default": None}
    allow = {"tables": ALLOW["tables"],
             "columns": {"kept": {"brand_new": {"reason": "known"}}}}
    gap = vp.diff(ref, _live(), allow)
    # Column allowlist entries are inert: columns report, tables gate.
    assert gap["gating_missing_table_count"] == 0
    assert gap["missing_column_count"] == 1


def test_diff_detects_missing_index_and_fk():
    live = _live()
    del live["indexes"]["public.index_kept_on_name"]
    live["fks"] = []
    gap = vp.diff(_reference(), live, ALLOW)
    entry = gap["tables"]["kept"]
    assert entry["missing_indexes"][0]["name"] == "index_kept_on_name"
    assert entry["missing_foreign_keys"] == [
        {"column": ["other_id"], "to_table": "others"}]
    assert gap["gating_missing_table_count"] == 0  # non-gating day one


def test_psql_cmd_snapshot_flags():
    # The snapshot query MUST run tuples-only + unaligned, or psql's aligned
    # framing (header + wrapped body + "(1 row)" footer) corrupts json.loads.
    cmd = vp._psql_cmd("postgres://x", sql="SELECT 1", tuples_only=True)
    assert cmd[:8] == ["psql", "postgres://x", "-v", "ON_ERROR_STOP=1",
                       "-q", "-X", "-t", "-A"]
    assert cmd[-2:] == ["-c", "SELECT 1"]
    plain = vp._psql_cmd("postgres://x", sql="SELECT 1")
    assert "-t" not in plain and "-A" not in plain
    file_mode = vp._psql_cmd("postgres://x", file="/tmp/m.sql", single_txn=True)
    assert file_mode[-2:] == ["-f", "/tmp/m.sql"] and "-1" in file_mode
