-- Canvas GraphQL root parity — the columns the new query-root object types
-- still owe Canvas.
--
-- Canvas references:
--   app/graphql/types/query_type.rb      (the 28 root fields)
--   app/graphql/types/assignment_type.rb (Assignment.type / AssignmentTypeEnum)
--   app/graphql/types/learning_outcome_type.rb
--   app/graphql/types/learning_outcome_group_type.rb
--   app/graphql/types/outcome_proficiency_type.rb
--   app/graphql/types/proficiency_rating_type.rb
--
-- Additive only: no DROP, no TRUNCATE, no renumbering, no data rewrite.

-- Canvas `AssignmentTypeEnum` has two members, ASSIGNMENT and
-- PEER_REVIEW_SUB_ASSIGNMENT. The port had no way to tell a plain
-- assignment from a peer-review sub-assignment, so
-- `Query.assignment(includeTypes:)` and `Query.peerReviewSubAssignment`
-- had nothing to filter on.
ALTER TABLE assignments
    ADD COLUMN IF NOT EXISTS assignment_type TEXT NOT NULL DEFAULT 'assignment';

CREATE INDEX IF NOT EXISTS index_assignments_assignment_type
    ON assignments (assignment_type);

-- Canvas `LearningOutcome` carries the context it belongs to, its
-- calculation method, the mastery/points scale and the vendor guid.
ALTER TABLE learning_outcomes
    ADD COLUMN IF NOT EXISTS context_id BIGINT,
    ADD COLUMN IF NOT EXISTS context_type TEXT,
    ADD COLUMN IF NOT EXISTS calculation_int INTEGER,
    ADD COLUMN IF NOT EXISTS calculation_method TEXT,
    ADD COLUMN IF NOT EXISTS mastery_points DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS points_possible DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS assessed BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS vendor_guid TEXT;

-- Canvas `LearningOutcomeGroup` carries a context, a description and a
-- vendor guid. The port stored the course context in `course_id`; keep that
-- column and back-fill the Canvas-shaped context from it.
ALTER TABLE learning_outcome_groups
    ADD COLUMN IF NOT EXISTS context_id BIGINT,
    ADD COLUMN IF NOT EXISTS context_type TEXT,
    ADD COLUMN IF NOT EXISTS description TEXT,
    ADD COLUMN IF NOT EXISTS vendor_guid TEXT;

UPDATE learning_outcome_groups
   SET context_id = course_id,
       context_type = 'Course'
 WHERE context_id IS NULL
   AND course_id IS NOT NULL;

-- Canvas `OutcomeProficiency.masteryPoints` is non-null.
ALTER TABLE outcome_proficiencies
    ADD COLUMN IF NOT EXISTS mastery_points DOUBLE PRECISION NOT NULL DEFAULT 0;

-- Canvas `ProficiencyRating` — the rating scale of a context, reached
-- through `OutcomeProficiency.proficiencyRatingsConnection`.
CREATE TABLE IF NOT EXISTS proficiency_ratings (
    id                  BIGSERIAL PRIMARY KEY,
    learning_outcome_id BIGINT NOT NULL REFERENCES learning_outcomes (id) ON DELETE CASCADE,
    context_id          BIGINT NOT NULL,
    context_type        TEXT NOT NULL,
    description         TEXT NOT NULL,
    points              DOUBLE PRECISION NOT NULL,
    mastery             BOOLEAN NOT NULL DEFAULT FALSE,
    workflow_state      TEXT NOT NULL DEFAULT 'active',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS index_proficiency_ratings_on_learning_outcome_id
    ON proficiency_ratings (learning_outcome_id);

CREATE INDEX IF NOT EXISTS index_proficiency_ratings_on_context
    ON proficiency_ratings (context_id, context_type);

-- Canvas `DiscussionTopic` carries the to-do date, the delayed post date,
-- the lock date and the anonymity state. The mutations
-- `createDiscussionTopic` / `updateDiscussionTopic` write them.
ALTER TABLE discussion_topics
    ADD COLUMN IF NOT EXISTS todo_date TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS delayed_post_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS lock_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS anonymous_state TEXT;

-- Canvas `Enrollment#uuid` identifies an invitation;
-- `acceptEnrollmentInvitation` / `rejectEnrollmentInvitation` look the
-- enrollment up by it.
ALTER TABLE enrollments
    ADD COLUMN IF NOT EXISTS uuid TEXT;

CREATE INDEX IF NOT EXISTS index_enrollments_on_uuid
    ON enrollments (uuid);

-- Canvas keeps the per-user preferences blob on `users`; the dismissed
-- account notifications live in `preferences.closed_notifications`.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS preferences JSONB NOT NULL DEFAULT '{}'::jsonb;
