-- Migration 0044: outcome groups, links, and alignments (Canvas API v1 parity).
--
-- Routes served by star-outcomes against these tables:
--   GET/POST       /courses/{courseId}/outcome_groups
--   GET/PUT/DELETE /outcome_groups/{id}
--   GET/POST       /outcome_groups/{id}/outcomes
--   GET/POST       /courses/{courseId}/outcome_groups/{groupId}/outcomes
--
-- `outcome_groups` / `outcome_links` supersede the simplified
-- `learning_outcome_groups` / `learning_outcome_links` tables created by
-- migration 0022. The old tables are left untouched (never dropped) and their
-- rows are copied below so existing installations keep their data.

CREATE TABLE IF NOT EXISTS outcome_groups (
    id                      BIGSERIAL PRIMARY KEY,
    -- Course context (NULL for account/global groups). Kept as a first-class
    -- column because every existing route is course scoped.
    course_id               BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    context_type            TEXT NOT NULL DEFAULT 'Course', -- Course | Account
    context_id              BIGINT,
    parent_outcome_group_id BIGINT REFERENCES outcome_groups(id) ON DELETE SET NULL,
    title                   TEXT NOT NULL,
    description             TEXT,
    vendor_guid             TEXT,
    workflow_state          TEXT NOT NULL DEFAULT 'active', -- active | deleted
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_outcome_groups_course
    ON outcome_groups(course_id, workflow_state);
CREATE INDEX IF NOT EXISTS idx_outcome_groups_context
    ON outcome_groups(context_type, context_id);
CREATE INDEX IF NOT EXISTS idx_outcome_groups_parent
    ON outcome_groups(parent_outcome_group_id);

-- A link organizes an outcome inside a group. An outcome may be linked into
-- many groups but only once into the same group (Canvas OutcomeLink).
CREATE TABLE IF NOT EXISTS outcome_links (
    id               BIGSERIAL PRIMARY KEY,
    outcome_id       BIGINT NOT NULL REFERENCES learning_outcomes(id) ON DELETE CASCADE,
    outcome_group_id BIGINT NOT NULL REFERENCES outcome_groups(id) ON DELETE CASCADE,
    context_type     TEXT NOT NULL DEFAULT 'Course',
    context_id       BIGINT,
    workflow_state   TEXT NOT NULL DEFAULT 'active', -- active | deleted
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (outcome_id, outcome_group_id)
);

CREATE INDEX IF NOT EXISTS idx_outcome_links_outcome
    ON outcome_links(outcome_id);
CREATE INDEX IF NOT EXISTS idx_outcome_links_group
    ON outcome_links(outcome_group_id, workflow_state);

-- Alignment of an outcome to Canvas content (assignment, quiz, rubric, ...).
-- Canvas models this with polymorphic content tags; the simplified shape below
-- keeps the owning context plus the aligned content addressable.
CREATE TABLE IF NOT EXISTS outcome_alignments (
    id                    BIGSERIAL PRIMARY KEY,
    outcome_id            BIGINT NOT NULL REFERENCES learning_outcomes(id) ON DELETE CASCADE,
    context_type          TEXT NOT NULL DEFAULT 'Course',
    context_id            BIGINT,
    content_type          TEXT,
    content_id            BIGINT,
    associated_asset_type TEXT,
    associated_asset_id   BIGINT,
    workflow_state        TEXT NOT NULL DEFAULT 'active', -- active | deleted
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_outcome_alignments_outcome
    ON outcome_alignments(outcome_id, workflow_state);
CREATE INDEX IF NOT EXISTS idx_outcome_alignments_content
    ON outcome_alignments(content_type, content_id);
CREATE INDEX IF NOT EXISTS idx_outcome_alignments_context
    ON outcome_alignments(context_type, context_id);

-- Backfill from the 0022 tables (idempotent; fresh databases are a no-op).
INSERT INTO outcome_groups
    (id, course_id, context_type, context_id, title, workflow_state, created_at, updated_at)
SELECT g.id,
       g.course_id,
       CASE WHEN g.course_id IS NULL THEN 'Account' ELSE 'Course' END,
       g.course_id,
       g.title,
       g.workflow_state,
       g.created_at,
       g.updated_at
FROM learning_outcome_groups g
ON CONFLICT DO NOTHING;

SELECT setval(
    pg_get_serial_sequence('outcome_groups', 'id'),
    COALESCE((SELECT max(id) FROM outcome_groups), 1),
    (SELECT EXISTS (SELECT 1 FROM outcome_groups))
);

INSERT INTO outcome_links
    (id, outcome_id, outcome_group_id, context_type, context_id, workflow_state)
SELECT l.id,
       l.outcome_id,
       l.outcome_group_id,
       g.context_type,
       g.context_id,
       l.workflow_state
FROM learning_outcome_links l
JOIN outcome_groups g ON g.id = l.outcome_group_id
ON CONFLICT DO NOTHING;

SELECT setval(
    pg_get_serial_sequence('outcome_links', 'id'),
    COALESCE((SELECT max(id) FROM outcome_links), 1),
    (SELECT EXISTS (SELECT 1 FROM outcome_links))
);
