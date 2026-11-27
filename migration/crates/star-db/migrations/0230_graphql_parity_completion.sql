-- Migration 0230: GraphQL parity completion backing tables.
--
-- Additive-only tables backing the remaining Canvas GraphQL surface
-- (peer review sub-assignments, audit events, internal settings,
-- speedgrader settings, checkpoints, gradebook filters, widget
-- dashboards, inbox labels, per-user dashboard tab selections,
-- account notification dismissals, submission history snapshots,
-- discussion entry reports). Shapes follow Canvas schema columns for
-- the fields the resolvers surface; anything not needed by a
-- GraphQL field is intentionally omitted.

CREATE TABLE IF NOT EXISTS sub_assignments (
    id                BIGSERIAL PRIMARY KEY,
    assignment_id     BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    -- Canvas SubAssignment#sub_assignment_tag: leader (group leader) or
    -- peer_review (anonymous peer reviews).
    sub_assignment_tag TEXT NOT NULL,
    title             TEXT,
    workflow_state    TEXT NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_sub_assignments_assignment
    ON sub_assignments (assignment_id);

CREATE TABLE IF NOT EXISTS peer_reviews (
    id                BIGSERIAL PRIMARY KEY,
    assignment_id     BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Canvas rubric_assessment_id / submission of the review target.
    assessor_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workflow_state    TEXT NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (assignment_id, user_id, assessor_id)
);

CREATE INDEX IF NOT EXISTS idx_peer_reviews_assignment
    ON peer_reviews (assignment_id);

CREATE TABLE IF NOT EXISTS audit_events (
    id                BIGSERIAL PRIMARY KEY,
    -- Canvas auditable_context: Account / Course / Enrollment / User.
    context_type      TEXT NOT NULL,
    context_id        BIGINT,
    uuid              TEXT NOT NULL UNIQUE,
    event_type        TEXT NOT NULL,
    payload           JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_audit_events_context
    ON audit_events (context_type, context_id, created_at);

CREATE TABLE IF NOT EXISTS internal_settings (
    id                BIGSERIAL PRIMARY KEY,
    name              TEXT NOT NULL UNIQUE,
    value             TEXT,
    description       TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS speed_grader_settings (
    id                BIGSERIAL PRIMARY KEY,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    course_id         BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    -- Canvas SpeedGraderSettings: options_help_enabled and related toggles
    -- stored as a JSONB map.
    settings          JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, course_id)
);

CREATE TABLE IF NOT EXISTS checkpoints (
    id                BIGSERIAL PRIMARY KEY,
    assignment_id     BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    -- Canvas Checkpoint#checkpoint_label (sequential_1/2/3) and
    -- Checkpoint#sub_assignments_tag (seq_replies_1 … / replies / …).
    checkpoint_label  TEXT NOT NULL,
    points_possible   FLOAT8,
    position          INT NOT NULL DEFAULT 0,
    workflow_state    TEXT NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_checkpoints_assignment
    ON checkpoints (assignment_id, position);

CREATE TABLE IF NOT EXISTS gradebook_user_filters (
    id                BIGSERIAL PRIMARY KEY,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    course_id         BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    -- Canvas GradingPeriodGroup / gradebook filter: stored as JSONB
    -- {type, value} pairs like Canvas's GradebookFilter.
    filter            JSONB NOT NULL DEFAULT '{}'::jsonb,
    position          INT NOT NULL DEFAULT 0,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_gradebook_user_filters_user_course
    ON gradebook_user_filters (user_id, course_id);

CREATE TABLE IF NOT EXISTS widget_dashboards (
    id                BIGSERIAL PRIMARY KEY,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    context_type      TEXT NOT NULL DEFAULT 'Course',
    context_id        BIGINT,
    -- Canvas WidgetDashboard layout: JSONB array of {widget, layout,
    -- settings} — the default_state / layout split Canvas ships.
    layout            JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, context_type, context_id)
);

CREATE TABLE IF NOT EXISTS user_inbox_labels (
    id                BIGSERIAL PRIMARY KEY,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    color             TEXT,
    workflow_state    TEXT NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_user_inbox_labels_user
    ON user_inbox_labels (user_id);

CREATE TABLE IF NOT EXISTS learner_dashboard_tab_selections (
    id                BIGSERIAL PRIMARY KEY,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Canvas LearnerDashboardTabSelection#tab_handle (resources|grades|
    -- homeroom|...
    tab_handle        TEXT NOT NULL,
    context_type      TEXT,
    context_id        BIGINT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, tab_handle, context_type, context_id)
);

CREATE TABLE IF NOT EXISTS account_user_dismissals (
    id                BIGSERIAL PRIMARY KEY,
    account_notification_id BIGINT NOT NULL REFERENCES account_notifications(id) ON DELETE CASCADE,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_notification_id, user_id)
);

CREATE TABLE IF NOT EXISTS submission_histories (
    id                BIGSERIAL PRIMARY KEY,
    submission_id     BIGINT NOT NULL REFERENCES submissions(id) ON DELETE CASCADE,
    attempt           INT NOT NULL DEFAULT 0,
    body              TEXT,
    score             FLOAT8,
    submitted_at      TIMESTAMPTZ,
    snapshot          JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (submission_id, attempt)
);

CREATE TABLE IF NOT EXISTS discussion_entry_reports (
    id                BIGSERIAL PRIMARY KEY,
    discussion_entry_id BIGINT NOT NULL REFERENCES discussion_entries(id) ON DELETE CASCADE,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Canvas DiscussionEntryReport#resolution_state.
    resolution_state  TEXT NOT NULL DEFAULT 'unread',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_discussion_entry_reports_entry
    ON discussion_entry_reports (discussion_entry_id);
