-- The Canvas `OriginalityReport` owns an `lti_link` whose
-- `resource_type_code` and `resource_url` are reported as the `tool_setting`
-- of the report. The link is reported as a plain JSON column so that the
-- originality-report routes can round-trip the `tool_setting` of a request.
ALTER TABLE originality_reports
    ADD COLUMN IF NOT EXISTS tool_setting JSONB;
