-- 0128_collaborations.sql — collaborations domain tail (issue #91, spec
-- specs/p5-rest-collaborations.md §5).
--
-- TABLE OWNERSHIP (verified against this tree): the canvas-schema snapshot
-- migrations 0223_canvas_schema_tables_a.sql (tables), 0226 (indexes) and
-- 0227 (foreign keys) already create `collaborations` and `collaborators`
-- with Canvas' exact column layout:
--   collaborations(id, collaboration_type, document_id, user_id, context_id,
--     context_type, url, uuid, data, created_at, updated_at, description,
--     title NOT NULL, workflow_state 'active', deleted_at, context_code,
--     type, resource_link_lookup_uuid, root_account_id)
--   collaborators(id, user_id, collaboration_id, created_at, updated_at,
--     authorized_service_user_id, group_id)
-- and star-lti (resource_links.rs `collaboration_lookup_uuids`) already
-- queries the snapshot `collaborations` table. This migration must NOT
-- recreate them: sqlx applies migrations in filename order, so a
-- 0128-numbered CREATE TABLE would run BEFORE the snapshot's unguarded
-- `CREATE TABLE collaborations` on every fresh database and abort the whole
-- MIGRATOR chain (editing 0223 is also forbidden — checksums are recorded).
--
-- What this migration adds instead is the spec §5 tail, made order-safe:
-- fresh databases run 0128 BEFORE 0223 (tables absent → guarded no-op; the
-- snapshot's own 0226 indexes
-- index_collaborations_on_context_id_and_context_type and
-- index_collaborators_on_collaboration_id serve every API query), while
-- databases that already applied the snapshot run 0128 afterwards and get
-- the additions. No API route reads lti_id/lti_settings, so their absence
-- under fresh-database ordering is unobservable through the API.
--
-- Shape pins (spec §5 deltas vs the snapshot, deliberate):
-- - collaborators reference users and groups through Canvas' nullable
--   user_id/group_id columns (not a polymorphic context_type/context_id
--   pair): a row is a group collaborator iff group_id IS NOT NULL, a user
--   collaborator iff user_id IS NOT NULL (mirrors Canvas Collaborator and
--   the members endpoint's `preload(:group, :user)`).
-- - workflow_state keeps Canvas' 'active'/'deleted' states (default
--   'active'); soft delete also stamps deleted_at (Canvas
--   Collaboration#destroy). The spec's CHECK constraint is intentionally
--   NOT added: the snapshot column is a plain varchar(255) DEFAULT 'active'
--   shared with the whole canvas schema, and a CHECK here would only exist
--   on one of the two orderings.
-- - context_type stores Canvas' capitalized 'Course'/'Group'; lookups match
--   case-insensitively, tolerating the lowercase values other star-lms
--   polymorphic columns accept.

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.tables
               WHERE table_schema = current_schema()
                 AND table_name = 'collaborations') THEN
        ALTER TABLE collaborations
            ADD COLUMN IF NOT EXISTS lti_id TEXT,
            ADD COLUMN IF NOT EXISTS lti_settings JSONB;
        -- Spec §5: (context_type, context_id) index filtered to the active
        -- scope (Canvas `scope :active` → workflow_state <> 'deleted').
        CREATE INDEX IF NOT EXISTS idx_collaborations_context_active
            ON collaborations (context_type, context_id)
            WHERE workflow_state <> 'deleted';
    END IF;
    IF EXISTS (SELECT 1 FROM information_schema.tables
               WHERE table_schema = current_schema()
                 AND table_name = 'collaborators') THEN
        -- Spec §5 uniqueness (one collaborator identity per collaboration)
        -- expressed on Canvas' user_id/group_id shape: at most one row per
        -- (collaboration, user) and per (collaboration, group).
        CREATE UNIQUE INDEX IF NOT EXISTS uq_collaborators_identity
            ON collaborators (collaboration_id,
                              COALESCE(user_id, 0), COALESCE(group_id, 0));
    END IF;
END $$;
