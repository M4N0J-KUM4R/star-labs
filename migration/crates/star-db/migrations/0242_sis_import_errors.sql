-- 0242_sis_import_errors.sql — Canvas `sis_import_errors`
-- (SisImportsErrorsApiController#index). Canvas keeps one row per rejected
-- CSV row, linked to the sis batch that produced it.

CREATE TABLE IF NOT EXISTS sis_import_errors (
    id            BIGSERIAL PRIMARY KEY,
    account_id    BIGINT NOT NULL,
    sis_batch_id  BIGINT NOT NULL,
    file          VARCHAR(255),
    message       TEXT,
    row_info      TEXT,
    row           INTEGER,
    failure       BOOLEAN NOT NULL DEFAULT true,
    created_at    TIMESTAMP NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_sis_import_errors_account
    ON sis_import_errors (account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_sis_import_errors_batch
    ON sis_import_errors (sis_batch_id);
