-- 0230: grading_period_sets API (parity gap).
-- Canvas GradingPeriodSetsController + GradingPeriodSetSerializer need
--   GET    /accounts/{accountId}/grading_period_sets          (index)
--   POST   /accounts/{accountId}/grading_period_sets          (create)
--   PATCH  /accounts/{accountId}/grading_period_sets/{id}     (update, 204)
--   DELETE /accounts/{accountId}/grading_period_sets/{id}     (destroy, 204)
-- grading_period_groups (0050) already carries id/title/weighted; add the
-- serializer's display_totals_for_all_grading_periods column.
-- Canvas also supports enrollment_term_ids on create/update; the workspace
-- has no enrollment_terms table yet, so that param is accepted-and-ignored
-- until the terms controller lands (no FK to store).
ALTER TABLE grading_period_groups
    ADD COLUMN IF NOT EXISTS display_totals_for_all_grading_periods
        BOOLEAN NOT NULL DEFAULT FALSE;
