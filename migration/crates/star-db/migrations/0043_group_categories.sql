-- Migration 0043: Canvas Group Categories
--
-- Canvas API v1 parity:
--   GET    /courses/{courseId}/group_categories        -> list categories (teacher/admin)
--   POST   /courses/{courseId}/group_categories        -> create category (teacher/admin)
--   GET    /group_categories/{id}                      -> get category (enrolled/admin)
--   PUT    /group_categories/{id}                      -> update category (teacher/admin)
--   DELETE /group_categories/{id}                      -> soft-delete category + its groups
--   GET    /group_categories/{id}/groups               -> list groups in category (enrolled/admin)
--
-- Special category roles (Canvas built-ins): "communities", "student_organized",
-- "imported". A category with any other non-null role is protected and cannot be
-- deleted (Canvas returns 401 "unauthorized").
--
-- Category workflow_state: available | deleted

CREATE TABLE IF NOT EXISTS group_categories (
    id                 BIGSERIAL PRIMARY KEY,
    -- Canvas GroupCategory has a polymorphic course/account context; this phase
    -- only exposes course-scoped routes, account_id is kept for schema parity.
    course_id          BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    account_id         BIGINT REFERENCES accounts(id) ON DELETE CASCADE,
    name               TEXT NOT NULL,
    role               TEXT,
    self_signup        TEXT,
    self_signup_end_at TIMESTAMPTZ,
    group_limit        INTEGER,
    auto_leader        TEXT,
    non_collaborative  BOOLEAN NOT NULL DEFAULT FALSE,
    workflow_state     TEXT NOT NULL DEFAULT 'available',
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_group_categories_course_id
    ON group_categories(course_id);
CREATE INDEX IF NOT EXISTS idx_group_categories_account_id
    ON group_categories(account_id);
CREATE INDEX IF NOT EXISTS idx_group_categories_workflow_state
    ON group_categories(workflow_state);

-- groups.group_category_id already exists (migration 0018); index the lookups
-- used by GET /group_categories/{id}/groups and category deletion.
CREATE INDEX IF NOT EXISTS idx_groups_group_category_id
    ON groups(group_category_id);
