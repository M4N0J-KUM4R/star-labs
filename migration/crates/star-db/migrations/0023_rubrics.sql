-- Migration 0023: rubrics, criteria, ratings, and rubric assessments.
-- Mirrors Canvas rubric structure:
--   rubrics      — one per course, has title + points_possible
--   rubric_criteria — ordered rows within a rubric
--   rubric_ratings  — ordered rows within a criterion
--   rubric_assessments — teacher assessment tied to a submission

CREATE TABLE IF NOT EXISTS rubrics (
    id               BIGSERIAL PRIMARY KEY,
    course_id        BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    title            TEXT NOT NULL,
    points_possible  FLOAT8 NOT NULL DEFAULT 0,
    workflow_state   TEXT NOT NULL DEFAULT 'active',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS rubric_criteria (
    id           BIGSERIAL PRIMARY KEY,
    rubric_id    BIGINT NOT NULL REFERENCES rubrics(id) ON DELETE CASCADE,
    description  TEXT NOT NULL,
    points       FLOAT8 NOT NULL,
    position     INT NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS rubric_ratings (
    id            BIGSERIAL PRIMARY KEY,
    criterion_id  BIGINT NOT NULL REFERENCES rubric_criteria(id) ON DELETE CASCADE,
    description   TEXT NOT NULL,
    points        FLOAT8 NOT NULL,
    position      INT NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS rubric_assessments (
    id             BIGSERIAL PRIMARY KEY,
    rubric_id      BIGINT NOT NULL REFERENCES rubrics(id) ON DELETE CASCADE,
    submission_id  BIGINT NOT NULL REFERENCES submissions(id) ON DELETE CASCADE,
    assessor_id    BIGINT NOT NULL REFERENCES users(id),
    score          FLOAT8,
    data           JSONB,  -- array of {criterion_id, rating_id, points, comments}
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_rubrics_course ON rubrics(course_id);
CREATE INDEX idx_rubrics_workflow ON rubrics(workflow_state);
CREATE INDEX idx_rubric_criteria_rubric ON rubric_criteria(rubric_id);
CREATE INDEX idx_rubric_ratings_criterion ON rubric_ratings(criterion_id);
CREATE INDEX idx_rubric_assessments_rubric ON rubric_assessments(rubric_id);
CREATE INDEX idx_rubric_assessments_submission ON rubric_assessments(submission_id);
