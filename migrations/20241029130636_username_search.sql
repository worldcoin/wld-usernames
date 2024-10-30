CREATE EXTENSION pg_trgm;

SET pg_trgm.similarity_threshold = 0.3;

CREATE INDEX idx_username_trgm ON names USING GIN (username gin_trgm_ops);
