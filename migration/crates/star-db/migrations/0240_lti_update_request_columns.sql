-- Migration 0240: the LTI registration update-request columns Canvas
-- carries but the workspace table (0115) lacks.
--
-- Canvas `Lti::RegistrationUpdateRequest`
-- (app/models/lti/registration_update_request.rb) and the API JSON
-- (lib/api/v1/lti/registration_update_request.rb, JSON_ATTRS) expose
-- `reinstall`, `tool_initiated` and `comment`; the workspace table only had
-- the timestamps. The flags default to false like the Rails booleans, the
-- comment stays nullable.

ALTER TABLE lti_registration_update_requests
    ADD COLUMN IF NOT EXISTS reinstall boolean NOT NULL DEFAULT false;

ALTER TABLE lti_registration_update_requests
    ADD COLUMN IF NOT EXISTS tool_initiated boolean NOT NULL DEFAULT false;

ALTER TABLE lti_registration_update_requests
    ADD COLUMN IF NOT EXISTS comment text;
