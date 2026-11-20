-- Migration 0195: provisional_grades, moderated_grading_selections, moderation_graders,
-- assignment_extensions, and assignments columns for moderated grading.
--
-- Canvas models covered (from specs/p5-rest-assessments.md §5):
--   provisional_grades      — ModeratedGrading::ProvisionalGrade
--   moderated_grading_selections — ModeratedGrading::Selection ("moderation set")
--   moderation_graders      — ModerationGrader (grader slots)
--   assignment_extensions   — AssignmentExtension (per-student late policy override)
--   assignments additions    — moderated_grading, final_grader_id, grader_names_visible_to_...,
--                              grader_comments_visible_to_graders, graders_anonymous_to_graders,
--                              grades_published_at
--
-- Route scope (slice F, assignments-t1):
--   POST   .../courses/:course_id/assignments/:assignment_id/extensions
--   GET    .../courses/:course_id/assignments/:assignment_id/moderated_students
--   POST   .../courses/:course_id/assignments/:assignment_id/moderated_students
--   GET    .../courses/:course_id/assignments/:assignment_id/provisional_grades/status
--   PUT    .../courses/:course_id/assignments/:assignment_id/provisional_grades/:provisional_grade_id/select
--   PUT    .../courses/:course_id/assignments/:assignment_id/provisional_grades/bulk_select
--   POST   .../courses/:course_id/assignments/:assignment_id/provisional_grades/publish
--   GET    .../courses/:course_id/assignments/:assignment_id/anonymous_provisional_grades/status
--   POST   .../sections/:section_id/assignments/:assignment_id/submissions
--                                                     (section twin of submissions#create)

BEGIN;

-- ---------------------------------------------------------------------------
-- provisional_grades — one row per scorer per submission (Canvas ProvisionalGrade)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS provisional_grades (
    id                           BIGSERIAL PRIMARY KEY,
    submission_id                BIGINT NOT NULL REFERENCES submissions(id) ON DELETE CASCADE,
    scorer_id                    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    score                        NUMERIC(10,2),
    grade                        TEXT,
    graded_at                    TIMESTAMPTZ,  -- null when comment-only
    final                        BOOLEAN NOT NULL DEFAULT FALSE,
    source_provisional_grade_id  BIGINT REFERENCES provisional_grades(id) ON DELETE SET NULL,
    grade_matches_current_submission BOOLEAN,
    created_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_provisional_grades_submission_id ON provisional_grades(submission_id);
CREATE INDEX IF NOT EXISTS idx_provisional_grades_scorer_id ON provisional_grades(scorer_id);

-- ---------------------------------------------------------------------------
-- moderated_grading_selections — the "moderation set" (Canvas Selection)
-- uniqueness: (assignment_id, student_id)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS moderated_grading_selections (
    id                          BIGSERIAL PRIMARY KEY,
    assignment_id               BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    student_id                  BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    selected_provisional_grade_id BIGINT REFERENCES provisional_grades(id) ON DELETE SET NULL,
    UNIQUE (assignment_id, student_id)
);
CREATE INDEX IF NOT EXISTS idx_moderated_grading_selections_selected_pg
    ON moderated_grading_selections(selected_provisional_grade_id)
    WHERE selected_provisional_grade_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- moderation_graders — grader slots for limited moderated grading
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS moderation_graders (
    id             BIGSERIAL PRIMARY KEY,
    assignment_id  BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    anonymous_id   TEXT,
    slot_taken     BOOLEAN NOT NULL DEFAULT FALSE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (assignment_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_moderation_graders_anonymous_id
    ON moderation_graders(anonymous_id) WHERE anonymous_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- assignment_extensions — per-student late-policy extension (Canvas AssignmentExtension)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS assignment_extensions (
    id                   BIGSERIAL PRIMARY KEY,
    assignment_id        BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    user_id              BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    seconds_late_extension INT NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (assignment_id, user_id)
);

-- ---------------------------------------------------------------------------
-- assignments additions for moderated grading
-- ---------------------------------------------------------------------------
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS final_grader_id BIGINT REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grader_names_visible_to_final_grader BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grader_comments_visible_to_graders BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS graders_anonymous_to_graders BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS moderated_grading BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grades_published_at TIMESTAMPTZ;

COMMIT;
