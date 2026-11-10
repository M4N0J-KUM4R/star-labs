-- Canvas parity: course sections and section-scoped enrollments.
--
-- Canvas `Enrollment#course_section_id` links an enrollment to one course
-- section (NULL means the course default/all sections). Section-scoped
-- enrollment endpoints live at /api/v1/sections/:section_id/enrollments.
--
-- `course_sections` is also owned by the courses/terms workstream; the
-- IF NOT EXISTS guards keep this migration safe in either order.

CREATE TABLE IF NOT EXISTS course_sections (
    id             BIGSERIAL PRIMARY KEY,
    course_id      BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    start_at       TIMESTAMPTZ,
    end_at         TIMESTAMPTZ,
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_course_sections_course ON course_sections(course_id);

-- Canvas enrollment shape: course_section_id (nullable bigint).
ALTER TABLE enrollments
    ADD COLUMN IF NOT EXISTS course_section_id BIGINT REFERENCES course_sections(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_enrollments_course_section ON enrollments(course_section_id);
