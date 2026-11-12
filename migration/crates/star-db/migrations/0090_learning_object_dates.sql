-- Migration 0090: Canvas learning object dates (learning_object_dates_api).
-- One row per (learning_object_type, learning_object_id) holding the base
-- due/unlock/lock dates plus `only_visible_to_overrides`
-- (Canvas Api::V1::LearningObjectDates#learning_object_dates_json), and one
-- row per assignment override (Canvas AssignmentOverride).
-- learning_object_type: 'Assignment' | 'Quiz' | 'DiscussionTopic' | 'WikiPage'.

CREATE TABLE IF NOT EXISTS learning_object_dates (
    id                       BIGSERIAL PRIMARY KEY,
    course_id                BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    learning_object_type     TEXT NOT NULL,
    learning_object_id       BIGINT NOT NULL,
    due_at                   TIMESTAMPTZ,
    unlock_at                TIMESTAMPTZ,
    lock_at                  TIMESTAMPTZ,
    only_visible_to_overrides BOOLEAN NOT NULL DEFAULT false,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (learning_object_type, learning_object_id)
);

CREATE INDEX IF NOT EXISTS idx_learning_object_dates_course_id
    ON learning_object_dates(course_id);

CREATE TABLE IF NOT EXISTS learning_object_date_overrides (
    id                     BIGSERIAL PRIMARY KEY,
    learning_object_date_id BIGINT NOT NULL
        REFERENCES learning_object_dates(id) ON DELETE CASCADE,
    title                  TEXT,
    set_type               TEXT,
    set_id                 BIGINT,
    student_ids            BIGINT[] NOT NULL DEFAULT '{}',
    due_at                 TIMESTAMPTZ,
    unlock_at              TIMESTAMPTZ,
    lock_at                TIMESTAMPTZ,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_learning_object_date_overrides_date_id
    ON learning_object_date_overrides(learning_object_date_id);
