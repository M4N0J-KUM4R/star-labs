-- Migration 0104: Canvas peer reviews parity (assessment_requests).
--
-- Canvas models a peer review as an AssessmentRequest
-- (app/models/assessment_request.rb): a reviewer (assessor) is assigned to
-- review another student's submission (the asset, always a Submission in the
-- API). The API shape comes from lib/api/v1/assessment_request.rb:
--   { id, user_id, assessor_id, asset_id, asset_type, workflow_state }
-- where workflow_state is 'assigned' or 'completed'.
--
-- API scope (app/controllers/peer_reviews_api_controller.rb):
-- - GET    /courses/{courseId}/assignments/{assignmentId}/peer_reviews
-- - GET    /courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews
-- - POST   /courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews
-- - DELETE /courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews
--
-- Canvas also stores peer-review settings on the assignment
-- (Assignment#peer_reviews, Assignment#peer_review_count) and bumps
-- `peer_review_count` when reviews are assigned; those columns are added
-- here too so assignments can enable peer reviews.

CREATE TABLE IF NOT EXISTS assessment_requests (
    id              BIGSERIAL PRIMARY KEY,
    -- The student whose submission is being reviewed (AssessmentRequest#user).
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The assigned reviewer (AssessmentRequest#assessor).
    assessor_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The reviewed asset: always the assignment (the submission is resolved
    -- per (assignment_id, user_id) in star-lms). Canvas names this column
    -- `asset_id` with asset_type 'Submission'; keeping the FK on assignments
    -- matches star-lms' single-row submissions model.
    asset_id        BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    asset_type      TEXT NOT NULL DEFAULT 'Submission',
    workflow_state  TEXT NOT NULL DEFAULT 'assigned',  -- assigned | completed
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (asset_id, assessor_id)
);

CREATE INDEX IF NOT EXISTS idx_assessment_requests_asset
    ON assessment_requests(asset_id);
CREATE INDEX IF NOT EXISTS idx_assessment_requests_user
    ON assessment_requests(user_id);
CREATE INDEX IF NOT EXISTS idx_assessment_requests_assessor
    ON assessment_requests(assessor_id);

-- Canvas Assignment peer-review settings (app/models/assignment.rb):
-- peer_reviews enables the workflow, peer_review_count is how many reviews
-- each student is assigned automatically, and peer_reviews_assign_at is when
-- Canvas performs the automatic assignment.
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS peer_reviews BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS peer_review_count INT NOT NULL DEFAULT 0;
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS peer_reviews_assign_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_assignments_peer_reviews
    ON assignments(peer_reviews) WHERE peer_reviews;
