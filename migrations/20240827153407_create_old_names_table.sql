-- Create old_names table
CREATE TABLE old_names (
    id INT PRIMARY KEY,
    old_username VARCHAR NOT NULL UNIQUE,
    new_username VARCHAR NOT NULL,
    FOREIGN KEY (new_username) REFERENCES names(username)
);
