ALTER TABLE pages ADD COLUMN url TEXT;
ALTER TABLE pages ADD COLUMN workflow_state TEXT NOT NULL DEFAULT 'published';
ALTER TABLE pages ADD COLUMN editing_roles TEXT NOT NULL DEFAULT 'teachers';
