-- Migration 0236: the columns of the polls and their choices.
--
-- The Canvas schema (`db/migrate/20101210192618_init_canvas_db.rb`) models
-- a poll with a `question` and a `description`, and a poll choice with a
-- `text`, an `is_correct` flag and a `position`:
--
--   create_table :polling_polls do |t|
--     t.text :question, null: false
--     t.text :description
--     ...
--
--   create_table :polling_poll_choices do |t|
--     t.text :text, null: false
--     t.boolean :is_correct, null: false, default: false
--     t.integer :position
--     ...
--
-- The workspace tables carry the choice text in `name` and lack the other
-- columns, so the poll description and the choice flags are added here. The
-- choice text stays in `name`; the routes read it from there and render it
-- as `text`, exactly like the Canvas serializer does.

ALTER TABLE polls
    ADD COLUMN IF NOT EXISTS description TEXT;

ALTER TABLE poll_choices
    ADD COLUMN IF NOT EXISTS is_correct BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS position INTEGER;

CREATE INDEX IF NOT EXISTS idx_poll_choices_poll_position
    ON poll_choices (poll_id, position);
