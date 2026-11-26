"""pytest fixtures for tools/schema/canvas_reference.py.

Each test replays a small synthetic db/migrate snippet and asserts the exact
schema the Rails semantics dictate (explicit not_null boolean, Rails 7.1
default index/FK naming, Canvas' polymorphic-list references, the aua_logs
partition loop).
"""
import hashlib
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import canvas_reference as cr  # noqa: E402


def replay(*sources: str) -> dict:
    ref = cr.build_reference([("m.rb", s) for s in sources])
    return {name: tb.to_json() for name, tb in ref.tables.items()}


# ---- create_table -------------------------------------------------------

def test_create_table_options_and_default_id():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :widgets, id: false do |t|
          t.string :name, limit: 100
          t.primary_keys [:name]
        end
        create_table :gadgets do |t|
          t.integer :size, null: false, default: 0
        end
      end
    end
    """)
    assert "id" not in t["widgets"]["columns"]
    assert t["widgets"]["columns"]["name"]["not_null"] is True  # PK implies NOT NULL
    g = t["gadgets"]["columns"]
    assert g["id"] == {"type": "bigint", "limit": None, "precision": None,
                       "scale": None, "not_null": True, "default": None}
    assert g["size"]["default"] == 0 and g["size"]["not_null"] is True


def test_create_table_string_limit_and_decimal():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :things do |t|
          t.string :code, limit: 8
          t.decimal :rate, precision: 12, scale: 2
          t.text :notes, limit: 65_536
        end
      end
    end
    """)
    c = t["things"]["columns"]
    assert c["code"]["limit"] == 8
    assert (c["rate"]["precision"], c["rate"]["scale"]) == (12, 2)
    assert c["notes"]["limit"] == 65536


# ---- timestamps ---------------------------------------------------------

def test_timestamps_not_null_default_now_precision_nil():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :a do |t|
          t.timestamps
        end
        create_table :b do |t|
          t.timestamps precision: nil
        end
      end
    end
    """)
    for name, precision in (("a", 6), ("b", None)):
        for col in ("created_at", "updated_at"):
            c = t[name]["columns"][col]
            assert c["type"] == "datetime"
            assert c["not_null"] is True
            assert c["default"] == "now()"
            assert c["precision"] == precision


def test_timestamp_lambda_default():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :a do |t|
          t.timestamps null: false, default: -> { "now()" }
        end
      end
    end
    """)
    assert t["a"]["columns"]["created_at"]["default"] == "now()"


# ---- references / foreign keys -----------------------------------------

def test_references_without_foreign_key_gets_index_only():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :subs do |t|
          t.references :quiz
        end
      end
    end
    """)
    assert "quiz_id" in t["subs"]["columns"]
    assert t["subs"]["columns"]["quiz_id"]["type"] == "bigint"
    assert "index_subs_on_quiz_id" in t["subs"]["indexes"]
    assert t["subs"]["foreign_keys"] == []


def test_references_foreign_key_true_pluralizes_target():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :quizzes do |t|
        end
        create_table :sis_batches do |t|
        end
        create_table :discussion_entries do |t|
        end
        create_table :group_categories do |t|
        end
        create_table :progresses do |t|
        end
        create_table :custom_grade_statuses do |t|
        end
        create_table :user_merge_data do |t|
        end
        create_table :subs do |t|
          t.references :quiz, foreign_key: true
          t.references :sis_batch, foreign_key: true, index: false
          t.references :discussion_entry, foreign_key: true
          t.references :group_category, foreign_key: true
          t.references :progress, foreign_key: true
          t.references :custom_grade_status, foreign_key: true
          t.references :user_merge_data, foreign_key: true
        end
      end
    end
    """)
    targets = {fk["column"]: fk["to_table"] for fk in t["subs"]["foreign_keys"]}
    # the exact pluralizer bugs from the pr-200 receipt, now resolved
    assert targets == {
        "quiz_id": "quizzes",
        "sis_batch_id": "sis_batches",
        "discussion_entry_id": "discussion_entries",
        "group_category_id": "group_categories",
        "progress_id": "progresses",
        "custom_grade_status_id": "custom_grade_statuses",
        "user_merge_data_id": "user_merge_data",
    }
    assert "index_subs_on_quiz_id" in t["subs"]["indexes"]
    assert not any(i.startswith("index_subs_on_sis_batch") for i in t["subs"]["indexes"])


def test_references_foreign_key_to_table_hash():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :accounts do |t|
        end
        create_table :subs do |t|
          t.references :parent_account, foreign_key: { to_table: :accounts },
                       index: false
          t.references :root_account, null: false,
                       foreign_key: { to_table: :accounts, deferrable: :immediate },
                       index: false
        end
      end
    end
    """)
    fks = {fk["column"]: fk["to_table"] for fk in t["subs"]["foreign_keys"]}
    assert fks == {"parent_account_id": "accounts", "root_account_id": "accounts"}
    assert t["subs"]["columns"]["root_account_id"]["not_null"] is True
    assert t["subs"]["indexes"] == {}


def test_fk_rails_name_matches_rails_7_1_algorithm():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :authors do |t|
        end
        create_table :articles do |t|
          t.references :author, foreign_key: true
        end
      end
    end
    """)
    ident = "articles_author_id_fk"
    expected = "fk_rails_" + hashlib.sha256(ident.encode()).hexdigest()[:10]
    assert t["articles"]["foreign_keys"][0]["name"] == expected


def test_polymorphic_reference_boolean_form_no_fk():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :blackout_dates do |t|
          t.references :context, polymorphic: true, null: false
        end
      end
    end
    """)
    c = t["blackout_dates"]["columns"]
    assert c["context_type"]["type"] == "string"
    assert c["context_type"]["not_null"] is True
    assert c["context_id"]["not_null"] is True
    assert "index_blackout_dates_on_context_type_and_context_id" in \
        t["blackout_dates"]["indexes"]
    assert t["blackout_dates"]["foreign_keys"] == []


def test_polymorphic_reference_list_form_makes_sub_references():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :wiki_pages do |t|
        end
        create_table :assignments do |t|
        end
        create_table :attachments do |t|
        end
        create_table :accessibility_issues do |t|
          t.references :context,
                       polymorphic: %i[wiki_page assignment attachment],
                       foreign_key: true,
                       check_constraint: false
        end
      end
    end
    """)
    c = t["accessibility_issues"]["columns"]
    assert "context_id" not in c and "context_type" not in c
    for sub in ("wiki_page", "assignment", "attachment"):
        assert c[f"{sub}_id"]["type"] == "bigint"
        idx = t["accessibility_issues"]["indexes"][
            f"index_accessibility_issues_on_{sub}_id"]
        assert idx["where"] == f"{sub}_id IS NOT NULL"
    targets = {fk["column"]: fk["to_table"]
               for fk in t["accessibility_issues"]["foreign_keys"]}
    assert targets == {"wiki_page_id": "wiki_pages",
                       "assignment_id": "assignments",
                       "attachment_id": "attachments"}


# ---- column mutations ---------------------------------------------------

def test_add_change_rename_remove_column():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :things do |t|
          t.string :a, limit: 255
          t.float :p
        end
        add_column :things, :b, :boolean, default: false, null: false
        add_column :things, :temp, :string
        change_column :things, :p, :decimal, precision: 12, scale: 2
        change_column_null :things, :a, true
        rename_column :things, :a, :alpha
        remove_column :things, :temp
      end
    end
    """)
    c = t["things"]["columns"]
    assert "a" not in c and "temp" not in c and "alpha" in c
    assert c["b"]["default"] is False


def test_change_column_default_from_to_and_rename_table():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :old_name do |t|
          t.string :nav_type, default: "course"
        end
        change_column_default :old_name, :nav_type, from: "course", to: "user"
        rename_table :old_name, :new_name
      end
    end
    """)
    assert "old_name" not in t
    assert t["new_name"]["columns"]["nav_type"]["default"] == "user"


def test_drop_table():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :doomed do |t|
          t.string :x
        end
        drop_table :doomed
      end
    end
    """)
    assert "doomed" not in t


# ---- indexes ------------------------------------------------------------

def test_add_index_default_and_explicit_naming():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :things do |t|
          t.string :a
          t.string :b
        end
        add_index :things, :a
        add_index :things, [:a, :b], name: "custom_idx", unique: true
        add_index :things, :a,
                  name: "partial_idx",
                  where: "a IS NOT NULL",
                  using: :gin
      end
    end
    """)
    idx = t["things"]["indexes"]
    assert idx["index_things_on_a"] == {"columns": ["a"], "unique": False,
                                        "using": None, "where": None}
    assert idx["custom_idx"]["unique"] is True
    assert idx["partial_idx"]["using"] == "gin"
    assert idx["partial_idx"]["where"] == "a IS NOT NULL"


def test_add_index_long_name_falls_back_to_hash():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :things do |t|
          t.string :a_very_long_column_name_that_goes_on
          t.string :and_another_equally_long_column_name_here
          t.string :plus_one_more_for_good_measure_too
        end
        add_index :things, [:a_very_long_column_name_that_goes_on,
                           :and_another_equally_long_column_name_here,
                           :plus_one_more_for_good_measure_too]
      end
    end
    """)
    names = list(t["things"]["indexes"])
    assert len(names) == 1
    assert len(names[0].encode()) <= 63
    assert names[0].startswith("idx_on_")


def test_replica_identity_index_and_remove_index():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :things do |t|
          t.references :root_account, foreign_key: { to_table: :accounts },
                       index: false
          t.string :x
          t.replica_identity_index
          t.index :x, name: "index_things_on_x"
        end
        remove_index :things, name: "index_things_on_x"
      end
    end
    """)
    assert t["things"]["indexes"]["index_things_replica_identity"] == {
        "columns": ["root_account_id", "id"], "unique": True,
        "using": None, "where": None}
    assert "index_things_on_x" not in t["things"]["indexes"]


# ---- top-level foreign keys --------------------------------------------

def test_add_and_remove_foreign_key():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :submissions do |t|
        end
        create_table :subs do |t|
          t.bigint :submission_id
        end
        add_foreign_key :subs, :submissions, on_delete: :nullify
        remove_foreign_key :subs, :submissions, if_exists: true
        add_foreign_key :subs, :submissions, on_delete: :nullify,
                        name: "fk_custom_name"
      end
    end
    """)
    fks = t["subs"]["foreign_keys"]
    assert len(fks) == 1
    assert fks[0]["name"] == "fk_custom_name"
    assert fks[0]["on_delete"] == "nullify"
    assert fks[0]["to_table"] == "submissions"


def test_add_foreign_key_default_column_singularizes():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :submissions do |t|
        end
        create_table :lti_assets do |t|
          t.string :url
        end
        add_foreign_key :lti_assets, :submissions
      end
    end
    """)
    fk = t["lti_assets"]["foreign_keys"][0]
    assert fk["column"] == "submission_id"
    ident = "lti_assets_submission_id_fk"
    assert fk["name"] == "fk_rails_" + \
        hashlib.sha256(ident.encode()).hexdigest()[:10]


def test_t_foreign_key_with_primary_key():
    t = replay("""
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :brand_configs do |t|
        end
        create_table :accounts do |t|
          t.string :brand_config_md5, limit: 32
          t.foreign_key :brand_configs, column: :brand_config_md5,
                        primary_key: :md5
        end
      end
    end
    """)
    fk = t["accounts"]["foreign_keys"][0]
    assert fk["column"] == "brand_config_md5"
    assert fk["to_table"] == "brand_configs"
    assert fk["to_column"] == "md5"


# ---- the aua_logs partition loop ----------------------------------------

def test_aua_log_partition_loop():
    src = """
    class InitCanvasDb < ActiveRecord::Migration[7.0]
      def create_aua_log_partition(index)
        table_name = :"aua_logs_#{index}"
        create_table table_name do |t|
          t.references :asset_user_access, null: false, index: false
          t.timestamp :created_at, null: false
        end
      end

      def up
        (0..6).each { |i| create_aua_log_partition(i) }
      end
    end
    """
    t = replay(src)
    for i in range(7):
        name = f"aua_logs_{i}"
        assert name in t, name
        cols = t[name]["columns"]
        assert cols["asset_user_access_id"]["not_null"] is True
        assert cols["asset_user_access_id"]["type"] == "bigint"
        assert cols["created_at"]["not_null"] is True
        assert cols["created_at"]["type"] == "timestamp"
        assert t[name]["indexes"] == {}  # index: false
        assert t[name]["foreign_keys"] == []


# ---- inflection (receipt quirks) ----------------------------------------

@pytest.mark.parametrize("word,expected", [
    ("sis_batch", "sis_batches"),
    ("quiz", "quizzes"),
    ("discussion_entry", "discussion_entries"),
    ("group_category", "group_categories"),
    ("progress", "progresses"),
    ("custom_grade_status", "custom_grade_statuses"),
    ("user_merge_data", "user_merge_data"),
    ("user", "users"),
    ("status", "statuses"),
    ("criterion", "criteria"),
    ("money", "money"),
    ("canvas", "canvas"),
])
def test_pluralize(word, expected):
    assert cr.pluralize(word) == expected


def test_singularize_submissions():
    assert cr.singularize("submissions") == "submission"


# ---- dangling FK handling ------------------------------------------------

def test_fk_to_absent_table_is_dropped_and_noted():
    ref = cr.build_reference([("m.rb", """
    class M < ActiveRecord::Migration[7.0]
      def change
        create_table :things do |t|
          t.references :missing_thing, foreign_key: true, index: false
        end
      end
    end
    """)])
    assert ref.tables["things"].to_json()["foreign_keys"] == []
    kinds = [n["kind"] for n in ref.notes]
    assert "fk_dropped_target_table_absent" in kinds


def test_extensions_recorded():
    ref = cr.build_reference([("m.rb", """
    class M < ActiveRecord::Migration[7.0]
      def change
        create_extension(:pg_trgm, schema: "public", if_not_exists: true)
        enable_extension "citext"
      end
    end
    """)])
    assert sorted(ref.extensions) == ["citext", "pg_trgm"]
