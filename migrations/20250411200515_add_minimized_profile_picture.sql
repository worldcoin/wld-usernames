-- Add migration script here

-- Add minimized profile picture support to the names table
ALTER TABLE names ADD COLUMN minimized_profile_picture_url VARCHAR(255);

DROP INDEX IF EXISTS username_trgm_idx;