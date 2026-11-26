#!/usr/bin/env python3
"""Tests for the schema-row inputs of parity.py (schema like-for-like).

Each test pins one counting rule the scoreboard's schema and GraphQL-root rows
depend on:

- star-db counts unique REAL `CREATE TABLE` statements only: `--` comment prose
  ("CREATE TABLE would run before ...", a backticked `CREATE TABLE
  collaborations`) is stripped/needs no `(`, so it cannot pollute the count,
  and the same name in two migrations dedupes.
- the Canvas replay walks db/migrate/*.rb for `create_table :sym` /
  `create_table "str"` / `create_table(:sym)` forms (commented-out calls do not
  count) AND replays the Ruby range-loop idiom (`(0..6).each { |i|
  helper(i) }` with a same-file `table_name = :"prefix_#{index}"` helper), so
  the aua_logs_0..6-style partitions are emitted too.
- star-graphql Query root fields are the `async fn`s inside the `impl Query`
  block, whose bounds are located by regex markers (`impl Query {` /
  `#[derive(MergedObject`), never line numbers; a missing marker is a hard
  error rather than a silently-wrong 0.
"""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

import parity  # noqa: E402

# One real statement, one IF NOT EXISTS statement, and the two comment shapes
# the real migrations carry (bare prose with the word `would`, and a backticked
# real-looking table name). Only the first two may count.
COMMENT_BAIT_SQL = '''
-- numbered-0129 CREATE TABLE would run before 0225 on a fresh CI database
-- `CREATE TABLE collaborations` on every fresh database and abort the whole
CREATE TABLE real_one (
  id bigint PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS guarded_two (
  id bigint PRIMARY KEY
);
'''

DUP_A_SQL = '''
CREATE TABLE dup_table (
  id bigint
);
'''

DUP_B_SQL = '''
-- next lane re-creates the same name; the NAME set must not grow
CREATE TABLE dup_table (
  id bigint
);
'''

CANVAS_INIT_RB = '''\
class InitCanvasDb < ActiveRecord::Migration[7.0]
  def create_log_partition(index)
    table_name = :"logs_#{index}"
    create_table table_name do |t|
      t.references :asset_user_access, null: false, index: false
      t.timestamp :created_at, null: false
    end
    # Intentionally not adding FK on asset_user_access_id.
  end

  def up
    create_table :alpha
    create_table "beta", id: false
    create_table(:gamma) do |t|
      t.string :name
    end
    # create_table :ghost is commented out and must not count.
    (0..1).each { |i| create_log_partition(i) }
  end
end
'''

QUERY_LIB_RS = '''\
use async_graphql::{Context, MergedObject, SimpleObject, ID};

async fn outside(&self) -> ID { ID("no") }

impl Query {
    async fn course(&self, ctx: &Context<'_>, id: ID) -> Option<&str> {
        None
    }

    async fn user(&self, ctx: &Context<'_>, id: ID) -> Result<Option<&str>,
                                                                  ()> {
        Ok(None)
    }
}

#[derive(MergedObject, Default)]
pub struct MutationRoot(Query, Mutation);
'''


def write_sql_tree(root: Path, files):
    for rel, body in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)


def test_comment_prose_and_backticks_never_count(tmp_path):
    write_sql_tree(tmp_path, {
        "crates/star-db/migrations/0001_comment_bait.sql": COMMENT_BAIT_SQL,
    })
    impl = parity.count_star_sources(tmp_path)
    assert impl["tables"] == 2
    assert impl["table_names"] == ["guarded_two", "real_one"]


def test_duplicate_create_table_dedupes_by_name(tmp_path):
    write_sql_tree(tmp_path, {
        "crates/star-db/migrations/0001_a.sql": DUP_A_SQL,
        "crates/star-db/migrations/0002_b.sql": DUP_B_SQL,
    })
    impl = parity.count_star_sources(tmp_path)
    assert impl["tables"] == 1
    assert impl["table_names"] == ["dup_table"]


def test_canvas_replay_covers_named_forms_loop_and_comments(tmp_path):
    (tmp_path / "db" / "migrate").mkdir(parents=True)
    (tmp_path / "db" / "migrate" / "20101210192618_init_canvas_db.rb") \
        .write_text(CANVAS_INIT_RB)
    loop_tables, named = parity.canvas_schema_tables(tmp_path)
    # The three named spellings (:sym, "str", (:sym)) count once each; the
    # commented-out ghost does not; the loop emits logs_0..logs_1.
    assert named == ["alpha", "beta", "gamma"]
    assert loop_tables == ["logs_0", "logs_1"]


def test_query_root_fields_count_only_inside_impl_query(tmp_path):
    lib = tmp_path / "crates" / "star-graphql" / "src" / "lib.rs"
    lib.parent.mkdir(parents=True)
    lib.write_text(QUERY_LIB_RS)
    # `outside` sits before the impl block; only the two fns inside count.
    assert parity.count_star_query_root_fields(tmp_path) == 2


def test_query_root_count_fails_loudly_without_markers(tmp_path):
    lib = tmp_path / "crates" / "star-graphql" / "src" / "lib.rs"
    lib.parent.mkdir(parents=True)
    lib.write_text("impl Query {\n    async fn course() {}\n}\n")
    with pytest.raises(SystemExit):
        parity.count_star_query_root_fields(tmp_path)
