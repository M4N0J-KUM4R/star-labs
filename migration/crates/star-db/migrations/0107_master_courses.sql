-- Migration 0107: master courses (blueprint courses parity)
--
-- Canvas stores blueprint ("master") course configuration in
-- master_courses/master_templates, sync runs in master_migrations and the
-- blueprint->child associations in master_course_subscriptions. The split
-- master_courses vs master_templates tables reflect the Canvas naming; here
-- both collapse onto one row per course (course_id UNIQUE).
--
-- Backs the Phase 5 routes:
--   GET    /courses/:course_id/blueprint_templates/:template_id
--   GET    /courses/:course_id/blueprint_templates/:template_id/associated_courses
--   PUT    /courses/:course_id/blueprint_templates/:template_id/update_associations
--   GET    /courses/:course_id/blueprint_templates/:template_id/unsynced_changes
--   POST+GET /courses/:course_id/blueprint_templates/:template_id/migrations
--   GET    /courses/:course_id/blueprint_templates/:template_id/migrations/:id(/details)
--   PUT    /courses/:course_id/blueprint_templates/:template_id/restrict_item
--   GET    /courses/:course_id/blueprint_subscriptions (+ :sub_id/migrations...)

-- One row per blueprint course (Canvas MasterCourses::MasterTemplate).
CREATE TABLE IF NOT EXISTS master_courses (
    id             BIGSERIAL PRIMARY KEY,
    course_id      BIGINT NOT NULL UNIQUE REFERENCES courses(id) ON DELETE CASCADE,
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Template side of a blueprint course: sync bookkeeping columns live here
-- (Canvas MasterCourses::MasterTemplate). Kept as a separate table so the
-- migration history joins against it rather than the course settings row.
CREATE TABLE IF NOT EXISTS master_templates (
    id                      BIGSERIAL PRIMARY KEY,
    course_id               BIGINT NOT NULL UNIQUE REFERENCES courses(id),
    workflow_state          TEXT NOT NULL DEFAULT 'active',
    last_export_started_at  TIMESTAMPTZ,
    last_export_completed_at TIMESTAMPTZ,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One sync run from a blueprint to its associated courses
-- (Canvas MasterCourses::MasterMigration). workflow_state values:
-- queued | exporting | imports_queued | completed | exports_failed | imports_failed.
CREATE TABLE IF NOT EXISTS master_migrations (
    id                  BIGSERIAL PRIMARY KEY,
    master_template_id  BIGINT NOT NULL REFERENCES master_templates(id) ON DELETE CASCADE,
    user_id             BIGINT NOT NULL REFERENCES users(id),
    workflow_state      TEXT NOT NULL,
    comment             TEXT,
    exports_started_at  TIMESTAMPTZ,
    imports_queued_at   TIMESTAMPTZ,
    imports_completed_at TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_master_migrations_template
    ON master_migrations (master_template_id);

-- Blueprint association: which child course receives updates from which
-- template (Canvas MasterCourses::ChildSubscription).
CREATE TABLE IF NOT EXISTS master_course_subscriptions (
    id                 BIGSERIAL PRIMARY KEY,
    master_template_id BIGINT NOT NULL REFERENCES master_templates(id) ON DELETE CASCADE,
    child_course_id    BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    workflow_state     TEXT NOT NULL DEFAULT 'active',
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (master_template_id, child_course_id)
);

CREATE INDEX IF NOT EXISTS idx_master_course_subscriptions_child
    ON master_course_subscriptions (child_course_id);
