-- Create names table
CREATE TABLE names (
    username VARCHAR PRIMARY KEY,
    address VARCHAR NOT NULL,
    nullifier_hash VARCHAR NOT NULL,
    verification_level VARCHAR NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Add an index on the address column for faster queries
CREATE INDEX idx_address ON names (address);
