-- Keep v4 ownership separate so legacy SELECT * decoders remain compatible.
CREATE TABLE username_v4_owners (
    owner_nullifier VARCHAR(66) PRIMARY KEY,
    username VARCHAR NOT NULL UNIQUE REFERENCES names(username) ON UPDATE CASCADE ON DELETE CASCADE,
    session_id VARCHAR(136) NOT NULL UNIQUE,
    migrated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE username_v4_challenges (
    id VARCHAR(36) PRIMARY KEY,
    operation VARCHAR NOT NULL CHECK (operation IN ('register', 'migrate', 'rename', 'recover')),
    username VARCHAR NOT NULL,
    new_username VARCHAR,
    address VARCHAR(42) NOT NULL,
    previous_address VARCHAR(42),
    nonce VARCHAR(66) NOT NULL UNIQUE,
    session_id VARCHAR(136),
    signal TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    consumed BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX username_v4_challenges_expiry ON username_v4_challenges (expires_at);

-- This runs under the same row lock as the write. A legacy request that started
-- before migration cannot pass an earlier application check and overwrite v4
-- ownership after migration commits. Only the v4 transaction opts into writes.
CREATE FUNCTION protect_v4_username() RETURNS TRIGGER AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM username_v4_owners WHERE username = OLD.username)
       AND current_setting('world_usernames.v4_write', true) IS DISTINCT FROM 'on' THEN
        RAISE EXCEPTION 'This username requires World ID 4 ownership'
            USING ERRCODE = '42501';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER protect_v4_username BEFORE UPDATE ON names
    FOR EACH ROW EXECUTE FUNCTION protect_v4_username();

-- Legacy routes checked availability before their transaction. Protect the
-- shared case-insensitive namespace at the write boundary, including aliases.
CREATE FUNCTION enforce_username_namespace() RETURNS TRIGGER AS $$
DECLARE candidate TEXT; previous TEXT;
BEGIN
    IF TG_TABLE_NAME = 'names' THEN
        candidate := NEW.username;
        IF TG_OP = 'UPDATE' THEN previous := OLD.username; END IF;
    ELSE
        candidate := NEW.old_username;
        IF TG_OP = 'UPDATE' THEN previous := OLD.old_username; END IF;
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended(lower(candidate), 7142));
    IF TG_TABLE_NAME = 'names' THEN
        IF EXISTS (SELECT 1 FROM names WHERE lower(username) = lower(candidate) AND username IS DISTINCT FROM previous)
           OR EXISTS (SELECT 1 FROM old_names WHERE lower(old_username) = lower(candidate)) THEN
            RAISE EXCEPTION 'Username is taken' USING ERRCODE = '23505';
        END IF;
    ELSE
        IF EXISTS (SELECT 1 FROM names WHERE lower(username) = lower(candidate))
           OR EXISTS (SELECT 1 FROM old_names WHERE lower(old_username) = lower(candidate) AND old_username IS DISTINCT FROM previous) THEN
            RAISE EXCEPTION 'Username alias is taken' USING ERRCODE = '23505';
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER enforce_names_namespace BEFORE INSERT OR UPDATE OF username ON names
    FOR EACH ROW EXECUTE FUNCTION enforce_username_namespace();
CREATE TRIGGER enforce_alias_namespace BEFORE INSERT OR UPDATE OF old_username ON old_names
    FOR EACH ROW EXECUTE FUNCTION enforce_username_namespace();
