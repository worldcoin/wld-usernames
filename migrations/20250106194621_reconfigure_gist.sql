DROP INDEX IF EXISTS names_username_idx;
DROP INDEX IF EXISTS idx_username_trgm;

CREATE INDEX IF NOT EXISTS username_trgm_idx ON names USING GIST (username gist_trgm_ops(siglen=256)) INCLUDE (address, profile_picture_url);