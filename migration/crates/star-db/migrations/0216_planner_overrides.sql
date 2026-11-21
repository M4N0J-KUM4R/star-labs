-- Migration 0216: planner overrides.
--
-- Canvas `PlannerOverridesController` (app/controllers/planner_overrides_controller.rb)
-- with `PlannerOverride` (app/models/planner_override.rb):
--
--   GET    /api/v1/planner/overrides
--   GET    /api/v1/planner/overrides/:id
--   PUT    /api/v1/planner/overrides/:id
--   POST   /api/v1/planner/overrides
--   DELETE /api/v1/planner/overrides/:id
--
-- An override records that a user marked a plannable item (assignment,
-- discussion topic, wiki page, ...) complete or dismissed it in the planner.
-- The plannable is a polymorphic reference, so `plannable_id`/`plannable_type`
-- are plain columns without a foreign key.
--
-- Canvas stores the model class name ("Assignment") and maps it back to the
-- API name in `planner_override_json`; the port stores the API name
-- ("assignment") directly.
--
-- Canvas validates the uniqueness of the plannable per user and plannable
-- type, which is the unique index below.

CREATE TABLE IF NOT EXISTS planner_overrides (
    id              BIGSERIAL PRIMARY KEY,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plannable_id    BIGINT NOT NULL,
    plannable_type  TEXT NOT NULL,
    workflow_state  TEXT NOT NULL DEFAULT 'active',
    marked_complete BOOLEAN NOT NULL DEFAULT false,
    dismissed       BOOLEAN NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, plannable_id, plannable_type)
);

CREATE INDEX IF NOT EXISTS idx_planner_overrides_user
    ON planner_overrides (user_id, workflow_state);
