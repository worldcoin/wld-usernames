-- Repoint username reservations on rename instead of orphaning them.
--
-- `old_names.new_username` references `names.username`. With the default
-- NO ACTION foreign key, renaming a user (`UPDATE names SET username = ...`)
-- fails whenever an `old_names` row still points at the old username, so the
-- rename handler had to DELETE those reservations before the UPDATE. In a
-- multi-rename chain (A -> B -> C) that deletion freed the *original* name (A),
-- letting any other World ID re-register it (HackerOne #3692075).
--
-- ON UPDATE CASCADE makes a rename automatically repoint existing reservations
-- to the new username, so earlier names in a chain stay reserved.
--
-- Backwards compatible: the previous handler deletes referencing rows before
-- the UPDATE, so the cascade is a no-op for it. Deploy this migration before
-- the new handler; to roll back the schema, roll back the handler first.
ALTER TABLE old_names DROP CONSTRAINT old_names_new_username_fkey;

ALTER TABLE old_names
	ADD CONSTRAINT old_names_new_username_fkey
	FOREIGN KEY (new_username) REFERENCES names (username) ON UPDATE CASCADE;
