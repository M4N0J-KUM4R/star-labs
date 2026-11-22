-- Migration 0141: user_observees (observer enrollments)
--
-- Canvas user_observees controller (app/controllers/user_observees_controller.rb):
-- links an "observer" user to a target "observee" user, so the observer can
-- view the observee's progress.  The observer-enrollment relationship is
-- scoped to a particular course_id (NULL = all courses).
--
-- Routes in scope (14 routes total — 7 user_observees + 7 users):
--   GET    /api/v1/users/:user_id/observers          → observers()
--   GET    /api/v1/users/:user_id/observees          → index()
--   POST   /api/v1/users/:user_id/observees          → create()
--   GET    /api/v1/users/:user_id/observees/:id      → show()
--   GET    /api/v1/users/:user_id/observers/:id     → show_observer()
--   PUT    /api/v1/users/:user_id/observees/:id      → update()
--   DELETE /api/v1/users/:user_id/observees/:id      → destroy()

CREATE TABLE IF NOT EXISTS observer_enrollments (
    id              BIGSERIAL PRIMARY KEY,
    observer_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    course_id       BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- unique: one observer can link to the same observee per course (or globally)
    CONSTRAINT uq_observer_enrollment UNIQUE (observer_id, user_id, course_id)
);

CREATE INDEX IF NOT EXISTS idx_observer_enrollments_observer_id ON observer_enrollments(observer_id);
CREATE INDEX IF NOT EXISTS idx_observer_enrollments_user_id      ON observer_enrollments(user_id);
CREATE INDEX IF NOT EXISTS idx_observer_enrollments_course_id    ON observer_enrollments(course_id);
