-- 0249: gradebooks controller parity gap (update_final_grade_overrides,
-- apply_score_to_ungraded_submissions). Canvas stores per-student final
-- grade override scores in score_overrides (current + grading-period
-- scoped rows); the workspace gets the same shape.
CREATE TABLE IF NOT EXISTS score_overrides (
    id                BIGSERIAL PRIMARY KEY,
    course_id         BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    student_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    grading_period_id BIGINT REFERENCES grading_periods(id) ON DELETE CASCADE,
    override_score    DOUBLE PRECISION,
    override_status_id BIGINT,
    workflow_state    TEXT NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_score_override UNIQUE (course_id, student_id, grading_period_id)
);

CREATE INDEX IF NOT EXISTS idx_score_overrides_course_student
    ON score_overrides (course_id, student_id);
