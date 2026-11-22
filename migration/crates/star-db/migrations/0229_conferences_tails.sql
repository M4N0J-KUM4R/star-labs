-- Migration 0229: web conference tails for the star-conferences crate.
--
-- Numbering note: the coordinator ledger had reserved a 012x number for the
-- conferences crate (spec p5-rest-conferences.md §5 says "010x,
-- coordinator-assigned"), but sqlx applies migrations in FILENAME order and
-- the canvas-schema snapshot migration 0225_canvas_schema_tables_c.sql
-- already creates `web_conferences` / `web_conference_participants`. A
-- numbered-0129 CREATE TABLE would run before 0225 on a fresh CI database
-- and abort the chain. Following the precedent set by the polling crate
-- (PR #212, migration 0211 substitution), the next free hole after the
-- snapshot is used instead: 0229.
--
-- Canvas ground truth (canvas-lms, read 2026-11):
--   - `web_conferences.recordings` does NOT exist upstream; Canvas fetches
--     recordings live from the provider (BigBlueButtonConference#recordings,
--     preload_recordings). star-lms has no provider, so the parity stub
--     stores the recording metadata the recording_ready webhook produces in
--     a JSONB column; the API serializes it as the `recordings` array
--     (Canvas WebConference#as_json methods include :recordings, and the
--     ConferenceRecording API model documents duration_minutes / title /
--     created_at / updated_at / playback_url).
--   - `calendar_events.web_conference_id` DOES exist upstream (CalendarEvent
--     belongs_to web_conference; conferences_controller#create builds a
--     linked event via CalendarConferencesHelper, WebConference's has_one
--     :calendar_event is dependent: :nullify). The 0019 calendar migration
--     predates conferences, so the column is added here, guarded and
--     additive.
--   - `sync_attendees`, `user_settings` and `lti_settings` need NO columns:
--     conference_params permits :sync_attendees but no such column or
--     reader exists on WebConference (virtual attribute, ignored), and
--     user_settings/lti_settings are kept inside the serialized `settings`
--     text column (WebConference#merge_user_settings, #lti_settings=).
--   - Canvas indexes web_conferences on (context_id, context_type)
--     (20101210192618_init_canvas_db.rb:5325).

ALTER TABLE web_conferences ADD COLUMN IF NOT EXISTS recordings JSONB;

ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS web_conference_id BIGINT;

-- Canvas: dependent: :nullify on WebConference#calendar_event.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'fk_calendar_events_web_conference_id'
    ) THEN
        ALTER TABLE calendar_events
            ADD CONSTRAINT fk_calendar_events_web_conference_id
            FOREIGN KEY (web_conference_id) REFERENCES web_conferences (id)
            ON DELETE SET NULL;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_web_conferences_context
    ON web_conferences (context_id, context_type);

CREATE INDEX IF NOT EXISTS idx_calendar_events_web_conference_id
    ON calendar_events (web_conference_id);
