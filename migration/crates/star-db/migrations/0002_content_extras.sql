-- Star LMS schema v2: module_items for Canvas-style module workflows.
-- Maps modules to their contained items (pages, assignments, etc.)
-- so modules can be ordered and nested as in Canvas.

CREATE TABLE IF NOT EXISTS module_items (
    id          BIGSERIAL PRIMARY KEY,
    module_id   BIGINT NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    item_type   TEXT NOT NULL,  -- 'page' | 'assignment'
    item_id     BIGINT NOT NULL,
    position    INTEGER NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_module_items_module ON module_items(module_id);
CREATE INDEX idx_module_items_position ON module_items(module_id, position);
