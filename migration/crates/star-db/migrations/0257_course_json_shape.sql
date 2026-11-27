-- 0257_course_json_shape.sql
--
-- Columns GET /api/v1/courses/:id serializes that star's `courses` table did
-- not carry, plus the backfills needed so those keys are never a fabricated
-- null. Canvas ground truth:
--   * canvas-src/db/migrate/20101210192618_init_canvas_db.rb:176-235 --
--     create_table :courses has uuid (181), locale (211), integration_id (218),
--     grade_passback_setting (226) and template (227, default false, not null).
--     `license` is the same table's line 207; star already keeps Canvas'
--     separate `licensing` column (usage-rights metadata), so both exist here
--     exactly as they do in Canvas.
--   * canvas-src/lib/api/v1/course_json.rb:22-35 -- BASE_ATTRIBUTES asks the
--     ORM for these five names, so a missing column is a missing key.
--   * canvas-src/app/models/course.rb:387 -- account_id, root_account_id and
--     enrollment_term_id are validated present, and :346 + :1556
--     (before_validation :assert_defaults) fill the term from
--     root_account.default_enrollment_term. The backfills below follow that
--     rule instead of emitting null.
--
-- Additive only: no applied migration is edited and nothing is dropped.

ALTER TABLE courses ADD COLUMN IF NOT EXISTS uuid varchar(255);
ALTER TABLE courses ADD COLUMN IF NOT EXISTS license varchar(255);
ALTER TABLE courses ADD COLUMN IF NOT EXISTS integration_id varchar(255);
ALTER TABLE courses ADD COLUMN IF NOT EXISTS grade_passback_setting varchar(255);
ALTER TABLE courses ADD COLUMN IF NOT EXISTS locale varchar(255);
ALTER TABLE courses ADD COLUMN IF NOT EXISTS template boolean NOT NULL DEFAULT false;

-- Canvas assigns uuid on save (canvas-src/app/models/course.rb:345 and
-- :2077-2080 `assign_uuid`), so it is never null in a course response. Canvas
-- generates a 40-character securish slug; we generate 40 hex characters from
-- the Postgres-native uuid plus an md5 tail. Same key, same type, same length,
-- different alphabet -- recorded as a value-level deviation, not a shape one.
ALTER TABLE courses
    ALTER COLUMN uuid SET DEFAULT substr(replace(gen_random_uuid()::text, '-', ''), 1, 32)
                              || substr(md5(random()::text), 1, 8);

UPDATE courses
SET uuid = substr(replace(gen_random_uuid()::text, '-', ''), 1, 32)
         || substr(md5(random()::text), 1, 8)
WHERE uuid IS NULL;

-- root_account_id is the top of the account chain, not a caller-supplied value
-- (Canvas: belongs_to :root_account, null: false). Walk each account to its
-- ancestor and use it for the rows written before the column existed.
WITH RECURSIVE account_root AS (
    SELECT id AS course_account_id, id AS root_id, parent_account_id
    FROM accounts
    UNION ALL
    SELECT ar.course_account_id, a.id, a.parent_account_id
    FROM account_root ar
    JOIN accounts a ON a.id = ar.parent_account_id
)
UPDATE courses c
SET root_account_id = r.root_id
FROM (
    SELECT course_account_id, root_id
    FROM account_root
    WHERE parent_account_id IS NULL
) r
WHERE c.root_account_id IS NULL
  AND c.account_id = r.course_account_id;

-- One Default Term per root account, mirroring Account#default_enrollment_term
-- (canvas-src/app/models/account.rb:1807-1814:
-- enrollment_terms.active.where(name: EnrollmentTerm::DEFAULT_TERM_NAME)
-- .first_or_create, with the name constant at
-- canvas-src/app/models/enrollment_term.rb:22 and `active` meaning
-- workflow_state<>'deleted' at :209). Canvas reaches it from
-- before_validation :assert_defaults (canvas-src/app/models/course.rb:346,
-- body at :1556), which is what lets enrollment_term_id be a non-null integer
-- key in course_json.
INSERT INTO enrollment_terms (root_account_id, name, workflow_state)
SELECT a.id, 'Default Term', 'active'
FROM accounts a
WHERE a.parent_account_id IS NULL
  AND NOT EXISTS (
      SELECT 1 FROM enrollment_terms et
      WHERE et.root_account_id = a.id
        AND et.name = 'Default Term'
        AND et.workflow_state IS DISTINCT FROM 'deleted'
  );

-- Same order Canvas uses: the course's root account Default Term, then any
-- live term of that root, then the site's first live term.
UPDATE courses c
SET enrollment_term_id = COALESCE(
    (SELECT et.id FROM enrollment_terms et
      WHERE et.root_account_id = c.root_account_id
        AND et.name = 'Default Term'
        AND et.workflow_state IS DISTINCT FROM 'deleted'
      ORDER BY et.id LIMIT 1),
    (SELECT et.id FROM enrollment_terms et
      WHERE et.root_account_id = c.root_account_id
        AND et.workflow_state IS DISTINCT FROM 'deleted'
      ORDER BY et.id LIMIT 1),
    (SELECT et.id FROM enrollment_terms et
      WHERE et.workflow_state IS DISTINCT FROM 'deleted'
      ORDER BY et.id LIMIT 1)
)
WHERE c.enrollment_term_id IS NULL;
