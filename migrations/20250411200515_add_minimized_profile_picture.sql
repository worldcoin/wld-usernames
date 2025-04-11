-- Add migration script here

-- Add minimized profile picture support to the names table
ALTER TABLE names ADD COLUMN minimized_profile_picture_url VARCHAR(255);

-- Create a new index that includes both profile picture URLs
CREATE INDEX IF NOT EXISTS username_trgm_with_pictures_idx ON names USING GIST (username gist_trgm_ops(siglen=256)) INCLUDE (address, profile_picture_url, minimized_profile_picture_url);
