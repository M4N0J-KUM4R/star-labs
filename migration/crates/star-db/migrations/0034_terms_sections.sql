-- Migration 0034: enrollment terms + course sections (Canvas API v1 parity).
--   GET/POST      /accounts/{accountId}/terms
--   GET/PUT/DELETE /terms/{id} (+ Canvas account-scoped aliases)
--   GET/POST      /courses/{courseId}/sections
--   GET/PUT/DELETE /sections/{id} (+ Canvas course-scoped alias)

-- terms table: account-scoped enrollment terms (Canvas EnrollmentTerm).
CREATE TABLE IF NOT EXISTS terms (
    id             BIGSERIAL PRIMARY KEY,
    account_id     BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    start_at       TIMESTAMPTZ,
    end_at         TIMESTAMPTZ,
    workflow_state TEXT NOT NULL DEFAULT 'active',  -- active | deleted
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_terms_account
    ON terms(account_id, workflow_state);
CREATE INDEX IF NOT EXISTS idx_terms_account_start
    ON terms(account_id, start_at DESC);

-- course_sections table: sections of a course (Canvas Section).
CREATE TABLE IF NOT EXISTS course_sections (
    id             BIGSERIAL PRIMARY KEY,
    course_id      BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    start_at       TIMESTAMPTZ,
    end_at         TIMESTAMPTZ,
    workflow_state TEXT NOT NULL DEFAULT 'active',  -- active | deleted
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_course_sections_course
    ON course_sections(course_id, workflow_state);
