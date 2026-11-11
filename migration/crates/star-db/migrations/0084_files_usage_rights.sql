-- Migration 0084: Canvas usage rights for files (UsageRightsController).
--
-- Canvas models copyright/license info per context (Course) and links
-- attachments to it via files.usage_rights_id.
--
--   PUT /courses/{courseId}/usage_rights
--     body: file_ids[], folder_ids[], publish, usage_rights{
--       use_justification, legal_copyright, license }
--     response: usage_rights_json + message + file_ids
--
-- use_justification: own_copyright | used_by_permission | fair_use |
--                    public_domain | creative_commons
-- license: cc_by | cc_by_sa | cc_by_nd | cc_by_nc | cc_by_nc_sa |
--          cc_by_nc_nd | cc0 | public_domain | private

CREATE TABLE IF NOT EXISTS usage_rights (
    id                BIGSERIAL PRIMARY KEY,
    context_type      TEXT NOT NULL DEFAULT 'Course',
    context_id        BIGINT NOT NULL,
    use_justification TEXT NOT NULL,
    legal_copyright   TEXT,
    license           TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_usage_rights_context
    ON usage_rights(context_type, context_id);

ALTER TABLE files
    ADD COLUMN IF NOT EXISTS usage_rights_id BIGINT
        REFERENCES usage_rights(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_files_usage_rights_id ON files(usage_rights_id);
