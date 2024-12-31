-- Add migration script here
CREATE INDEX names_username_lower_idx ON names (LOWER(username));

CREATE INDEX old_names_username_lower_idx ON old_names (LOWER(old_username));