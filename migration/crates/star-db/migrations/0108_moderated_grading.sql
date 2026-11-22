-- Migration 0108: Canvas moderated / provisional grading (issue #31).
--
-- Canvas models (db/migrate/20101210192618_init_canvas_db.rb and
-- app/models/moderated_grading/*.rb):
--
--   assignments additions (issue #31 slice F):
--     moderated_grading, grades_published_at, final_grader_id, grader_count,
--     grader_names_visible_to_final_grader, grader_comments_visible_to_graders,
--     graders_anonymous_to_graders
--
--   provisional_grades (Canvas moderated_grading_provisional_grades):
--     one non-final provisional grade per (submission, scorer), one final
--     provisional grade per submission (partial unique indexes, like Canvas).
--     grade_matches_current_submission is computed at serialization time in
--     Canvas (ModeratedGrading::ProvisionalGrade#grade_matches_current_submission),
--     so it is not stored here either.
--
--   moderated_grading_selections (Canvas ModeratedGrading::Selection):
--     the "moderation set"; unique per (assignment, student).
--
--   moderation_graders (Canvas ModerationGrader):
--     grader slots for limited moderated grading; 5-char anonymous ids.
--
--   anonymous_or_moderation_events (Canvas AnonymousOrModerationEvent):
--     the moderation audit trail (provisional_grade_created/updated/selected
--     events written when provisional grades change).
--
--   submission_comments.provisional_grade_id:
--     Canvas comments created with submission[provisional]=true are attached
--     to the provisional grade and only published on publish.

-- ---------------------------------------------------------------------------
-- Assignments additions
-- ---------------------------------------------------------------------------

ALTER TABLE assignments ADD COLUMN IF NOT EXISTS moderated_grading
    BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grades_published_at TIMESTAMPTZ;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS final_grader_id
    BIGINT REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grader_count
    INTEGER NOT NULL DEFAULT 0;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grader_names_visible_to_final_grader
    BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS grader_comments_visible_to_graders
    BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS graders_anonymous_to_graders
    BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_assignments_final_grader
    ON assignments(final_grader_id) WHERE final_grader_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- Provisional grades
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS provisional_grades (
    id                          BIGSERIAL PRIMARY KEY,
    submission_id               BIGINT NOT NULL REFERENCES submissions(id) ON DELETE CASCADE,
    scorer_id                   BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    score                       NUMERIC(10,2),
    grade                       TEXT,
    graded_at                   TIMESTAMPTZ,
    final                       BOOLEAN NOT NULL DEFAULT FALSE,
    source_provisional_grade_id BIGINT REFERENCES provisional_grades(id) ON DELETE SET NULL,
    graded_anonymously          BOOLEAN,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One final provisional grade per submission (Canvas
-- idx_mg_provisional_grades_unique_submission_when_final).
CREATE UNIQUE INDEX IF NOT EXISTS idx_provisional_grades_unique_final
    ON provisional_grades(submission_id) WHERE final = TRUE;

-- One non-final provisional grade per (submission, scorer) (Canvas
-- idx_mg_provisional_grades_unique_sub_scorer_when_not_final).
CREATE UNIQUE INDEX IF NOT EXISTS idx_provisional_grades_unique_sub_scorer
    ON provisional_grades(submission_id, scorer_id) WHERE final = FALSE;

CREATE INDEX IF NOT EXISTS idx_provisional_grades_submission
    ON provisional_grades(submission_id);
CREATE INDEX IF NOT EXISTS idx_provisional_grades_scorer
    ON provisional_grades(scorer_id);
CREATE INDEX IF NOT EXISTS idx_provisional_grades_source
    ON provisional_grades(source_provisional_grade_id)
    WHERE source_provisional_grade_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- Moderation set (Canvas ModeratedGrading::Selection)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS moderated_grading_selections (
    id                           BIGSERIAL PRIMARY KEY,
    assignment_id                BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    student_id                   BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    selected_provisional_grade_id BIGINT REFERENCES provisional_grades(id) ON DELETE SET NULL,
    created_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Canvas idx_mg_selections_unique_on_assignment_and_student.
CREATE UNIQUE INDEX IF NOT EXISTS idx_mg_selections_assignment_student
    ON moderated_grading_selections(assignment_id, student_id);

CREATE INDEX IF NOT EXISTS idx_mg_selections_selected_grade
    ON moderated_grading_selections(selected_provisional_grade_id)
    WHERE selected_provisional_grade_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- Moderation graders (Canvas ModerationGrader)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS moderation_graders (
    id           BIGSERIAL PRIMARY KEY,
    assignment_id BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    anonymous_id TEXT NOT NULL,
    slot_taken   BOOLEAN NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Canvas: unique (assignment_id, anonymous_id) and (user_id, assignment_id).
CREATE UNIQUE INDEX IF NOT EXISTS idx_moderation_graders_assignment_anonymous
    ON moderation_graders(assignment_id, anonymous_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_moderation_graders_assignment_user
    ON moderation_graders(assignment_id, user_id);

-- ---------------------------------------------------------------------------
-- Moderation audit events (Canvas AnonymousOrModerationEvent)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS anonymous_or_moderation_events (
    id            BIGSERIAL PRIMARY KEY,
    assignment_id BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    user_id       BIGINT REFERENCES users(id) ON DELETE SET NULL,
    submission_id BIGINT REFERENCES submissions(id) ON DELETE SET NULL,
    event_type    TEXT NOT NULL,
    payload       JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_aom_events_assignment
    ON anonymous_or_moderation_events(assignment_id, created_at);
CREATE INDEX IF NOT EXISTS idx_aom_events_submission
    ON anonymous_or_moderation_events(submission_id);

-- ---------------------------------------------------------------------------
-- Provisional submission comments
-- ---------------------------------------------------------------------------

ALTER TABLE submission_comments ADD COLUMN IF NOT EXISTS provisional_grade_id
    BIGINT REFERENCES provisional_grades(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_submission_comments_provisional_grade
    ON submission_comments(provisional_grade_id)
    WHERE provisional_grade_id IS NOT NULL;
