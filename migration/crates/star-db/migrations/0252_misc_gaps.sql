-- 0245: misc parity gaps (course_tag_conversion job progress, planner
-- items, plugins). Canvas CourseTagConversion tracks the conversion worker
-- via progresses rows tagged differentiation_tag_conversion; planner_items
-- is the Canvas planner note/shortcut store; plugins holds the site-admin
-- plugin id/value pairs (Canvas plugins are config records).
CREATE TABLE IF NOT EXISTS planner_items (
    id            BIGSERIAL PRIMARY KEY,
    user_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    course_id     BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    title         TEXT NOT NULL,
    details       TEXT,
    todo_date     TIMESTAMPTZ,
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_planner_items_user ON planner_items (user_id, todo_date);

CREATE TABLE IF NOT EXISTS plugins (
    id         BIGSERIAL PRIMARY KEY,
    name       TEXT NOT NULL,
    value      JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_plugin UNIQUE (name)
);

ALTER TABLE progresses
    ADD COLUMN IF NOT EXISTS completion_source TEXT;

-- Course-level settings store (Canvas courses carry a serialized settings
-- hash; learning_mastery_gradebook_settings lives there).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS settings JSONB NOT NULL DEFAULT '{}'::jsonb;
