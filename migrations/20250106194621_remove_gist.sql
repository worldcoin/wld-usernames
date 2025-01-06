DROP INDEX IF EXISTS names_username_idx;

CREATE INDEX idx_username_trgm_covering ON names USING GIN (username gin_trgm_ops) INCLUDE (address, profile_picture_url);