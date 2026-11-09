-- Migration 0029: unique identity for module_items.
-- POST /modules/{id}/items upserts with
-- ON CONFLICT (module_id, item_type, item_id); without this unique index
-- Postgres rejects every insert with SQLSTATE 42P10.

CREATE UNIQUE INDEX IF NOT EXISTS idx_module_items_unique
    ON module_items(module_id, item_type, item_id);
