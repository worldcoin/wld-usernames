-- Create verifying_keys table for storing DF signing keys
-- Single row table storing comma-separated hex public keys (max 5)
CREATE TABLE
    verifying_keys (
        id INTEGER PRIMARY KEY DEFAULT 1,
        keys VARCHAR(1000) NOT NULL DEFAULT '',
        updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
        CONSTRAINT single_row CHECK (id = 1)
    );

-- Insert the initial row
INSERT INTO
    verifying_keys (id, keys)
VALUES
    (1, '');