-- Migration 0113: quiz submission take-flow + report generation state.
-- Slice G tail (#54): POST .../submissions/:id/complete requires the
-- validation_token issued at take time (Canvas QuizSubmission#generate_validation_token,
-- 20 hex chars); GET .../submissions/:id/time uses the stored end_at when
-- present. quiz_statistics gains Canvas' report generation lifecycle so
-- DELETE .../reports/:id (abort) can distinguish "not being generated"
-- (failed), "can not be aborted" (running) and destroyable states
-- (completed/queued). Existing rows default to 'completed' (they were
-- generated synchronously). Additive only.
ALTER TABLE quiz_submissions
    ADD COLUMN IF NOT EXISTS validation_token TEXT;

ALTER TABLE quiz_statistics
    ADD COLUMN IF NOT EXISTS generation_state TEXT NOT NULL DEFAULT 'completed';
