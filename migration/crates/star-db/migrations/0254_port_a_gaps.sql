-- 0254: PORT-A parity gaps. The Academic Benchmarks outcome import family
-- (OutcomesAcademicBenchmarkImportApiController) tracks its migrations in
-- outcome_ab_migrations; the sync stance of the ported routes records one
-- completed row per POST /api/v1/global/outcomes_import and reports it
-- through GET .../migration_status/:migration_id. No other PORT-A surface
-- needs a table: the group wiki revision routes decline in this schema
-- (pages are course-scoped), the calendar gap routes reuse
-- calendar_events/appointment_group_* (0019/0063), developer_keys (0032) is
-- account-scoped through developer_key_account_bindings (0224),
-- release_notes (0097) and authentication_providers (0103) already exist.
CREATE TABLE IF NOT EXISTS outcome_ab_migrations (
    id              BIGSERIAL PRIMARY KEY,
    workflow_state  TEXT NOT NULL DEFAULT 'completed',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
