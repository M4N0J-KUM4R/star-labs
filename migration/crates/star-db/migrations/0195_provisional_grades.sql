-- Migration 0195: submissions.extra_attempts for assignment extensions (#175).
--
-- Canvas AssignmentExtensionsController#create (app/controllers/
-- assignment_extensions_controller.rb) stores extra attempts directly on the
-- student's submission row (Submission#extra_attempts, validated >= 0);
-- there is no separate assignment_extensions table. All other moderated
-- grading tables this branch originally created (provisional_grades,
-- moderated_grading_selections, moderation_graders, the assignments
-- moderated-grading columns, anonymous_or_moderation_events) already exist
-- via 0108_moderated_grading.sql, and the moderated/provisional route
-- handlers are main's implementations — this migration only adds the one
-- column the retained POST .../extensions route writes.

ALTER TABLE submissions
    ADD COLUMN IF NOT EXISTS extra_attempts INTEGER;

-- Canvas validates extra_attempts >= 0 (Submission numericality).
ALTER TABLE submissions
    ADD CONSTRAINT submissions_extra_attempts_nonnegative
    CHECK (extra_attempts IS NULL OR extra_attempts >= 0)
    NOT VALID;
