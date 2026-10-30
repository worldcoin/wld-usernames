-- SQLx runs this migration in one transaction. Preserve the original wire value
-- for existing clients, but enforce ownership using the verified numeric value.
-- Keep this function immutable: changing it requires rebuilding its index.
CREATE FUNCTION canonical_world_id_nullifier(value TEXT) RETURNS TEXT
LANGUAGE plpgsql IMMUTABLE STRICT PARALLEL SAFE
SET search_path = pg_catalog
AS $$
DECLARE
    digits TEXT := value;
    canonical_digits TEXT;
BEGIN
    IF left(digits, 2) IN ('0x', '0X') THEN
        digits := substr(digits, 3);
    END IF;
    IF length(digits) NOT BETWEEN 1 AND 64
       OR translate(digits, '0123456789abcdefABCDEF', '') <> '' THEN
        RAISE EXCEPTION 'Invalid World ID nullifier encoding'
            USING ERRCODE = '22023',
                  HINT = 'Expected 1-64 ASCII hex digits with an optional 0x prefix.';
    END IF;
    canonical_digits := lpad(lower(digits), 64, '0');
    -- BN254 scalar field modulus. Compare equal-width ASCII strings, not floats
    -- or a reduction modulo the field (which would alias invalid inputs).
    IF canonical_digits COLLATE "C" >=
       '30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000001' COLLATE "C" THEN
        RAISE EXCEPTION 'World ID nullifier is outside the scalar field'
            USING ERRCODE = '22023';
    END IF;
    RETURN '0x' || canonical_digits;
END;
$$;

-- Prevent legacy writers from racing the preflight and index installation.
LOCK TABLE names IN SHARE ROW EXCLUSIVE MODE;
DO $$
BEGIN
    -- This also validates every pre-existing value; malformed data aborts the
    -- migration without deleting or rewriting any username or alias.
    IF EXISTS (
        SELECT canonical_world_id_nullifier(nullifier_hash)
        FROM names
        GROUP BY canonical_world_id_nullifier(nullifier_hash)
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'Multiple usernames share a canonical World ID nullifier'
            USING ERRCODE = '23505',
                  HINT = 'Review ownership collisions before retrying; see docs/nullifier-uniqueness-rollout.md. No records were changed.';
    END IF;
END;
$$;

CREATE UNIQUE INDEX names_nullifier_canonical_unique
    ON names (canonical_world_id_nullifier(nullifier_hash));
