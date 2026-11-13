-- Migration 0106: stream_items (Canvas users_controller activity stream parity)
--
-- API scope (app/controllers/users_controller.rb):
-- - GET    /users/self/activity_stream          (activity_stream)
-- - GET    /users/self/activity_stream/summary  (activity_stream_summary)
-- - DELETE /users/self/activity_stream/{id}     (ignore_stream_item)
-- - DELETE /users/self/activity_stream          (ignore_all_stream_items)
--
-- Canvas stores one StreamItem per asset plus per-user StreamItemInstances
-- whose `hidden` flag the DELETE routes set. star-lms collapses that into a
-- single per-user table: each row is one stream event for one user, and the
-- hide semantics ("DELETE sets hidden=true per Canvas") map directly onto
-- it. Rows are appended by the API; there is no background dispatcher.
--
-- item_type holds the Canvas StreamItem type ("Submission",
-- "DiscussionTopic", "Conversation", ...); data carries the arbitrary
-- payload the type needs.

CREATE TABLE IF NOT EXISTS stream_items (
    id           BIGSERIAL PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    asset_string TEXT NOT NULL,
    item_type    TEXT NOT NULL,
    item_id      BIGINT,
    context_code TEXT,
    title        TEXT,
    data         JSONB,
    hidden       BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_stream_items_user
    ON stream_items (user_id, created_at DESC)
    WHERE NOT hidden;

CREATE INDEX IF NOT EXISTS idx_stream_items_user_type
    ON stream_items (user_id, item_type)
    WHERE NOT hidden;
