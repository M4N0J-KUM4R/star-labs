-- Migration 0065: Canvas module item update/duplicate parity.
--
-- Backs the context_module_items_api_controller endpoints that were missing
-- from the Phase 2 module slice:
-- - GET    /api/v1/courses/{courseId}/modules/items/{itemId}
-- - PUT    /api/v1/courses/{courseId}/modules/items/{itemId}
-- - DELETE /api/v1/courses/{courseId}/modules/items/{itemId}
-- - POST   /api/v1/courses/{courseId}/modules/items/{itemId}/duplicate
-- - PUT    /api/v1/courses/{courseId}/modules/{moduleId}/items/{itemId}
--
-- Canvas stores these on ContentTag:
--   - title          renamed label; NULL falls back to the linked content title
--   - indent         0-based hierarchy level (module_item_json always emits it)
--   - workflow_state active | unpublished (module item publish/unpublish);
--                    'deleted' is reserved for the soft-delete transition
--
-- Existing columns (item_type, item_id, position) are shared with the
-- Phase 2 module routes and are left untouched.

ALTER TABLE module_items
    ADD COLUMN IF NOT EXISTS title TEXT;

ALTER TABLE module_items
    ADD COLUMN IF NOT EXISTS indent INTEGER NOT NULL DEFAULT 0;

ALTER TABLE module_items
    ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'active'
        CHECK (workflow_state IN ('active', 'unpublished', 'deleted'));
