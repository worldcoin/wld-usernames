-- Add migration script here
-- 1. Drop the primary key on id and remove the id column
ALTER TABLE old_names
DROP CONSTRAINT old_names_pkey, -- Drop the current primary key on id
DROP COLUMN id;

-- Remove the id column
-- 2. Add a new primary key on old_username
ALTER TABLE old_names ADD CONSTRAINT old_names_pkey PRIMARY KEY (old_username);