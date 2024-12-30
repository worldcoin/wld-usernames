-- Add migration script here
CREATE UNIQUE INDEX names_username_lower_idx ON names (LOWER(username));

CREATE UNIQUE INDEX old_names_username_lower_idx ON old_names (LOWER(old_username));