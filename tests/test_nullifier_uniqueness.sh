#!/usr/bin/env bash
# Requires psql and a DISPOSABLE PostgreSQL database; never use production.
# Creates and drops only its own randomly named schema, not the database.
set -euo pipefail
: "${TEST_DATABASE_URL:?Set TEST_DATABASE_URL to a disposable PostgreSQL database}"
: "${ALLOW_NULLIFIER_DB_TESTS:?Set ALLOW_NULLIFIER_DB_TESTS=1 to acknowledge the disposable database}"
[[ "$ALLOW_NULLIFIER_DB_TESTS" == 1 ]] || exit 1
root="$(cd "$(dirname "$0")/.." && pwd)"
schema="nullifier_test_${$}_${RANDOM}"
logs="$(mktemp -d)"
export PGOPTIONS="-c search_path=${schema},pg_catalog"
psql_test() { psql -X --set ON_ERROR_STOP=1 --dbname "$TEST_DATABASE_URL" "$@"; }
cleanup() {
  psql_test -c "DROP SCHEMA IF EXISTS ${schema} CASCADE" >/dev/null 2>&1 || true
  rm -rf "$logs"
}
trap cleanup EXIT
reset_schema() {
  psql_test -c "DROP SCHEMA IF EXISTS ${schema} CASCADE; CREATE SCHEMA ${schema}" >/dev/null
  psql_test --file "$root/migrations/20240827152401_create_names_table.sql" >/dev/null
}
apply_migration() {
  psql_test --single-transaction --file "$root/migrations/20261030210000_unique_canonical_nullifier.sql"
}
reset_schema

# Demonstrate the original DB invariant is missing: distinct wire encodings pass.
psql_test <<'SQL'
INSERT INTO names(username, address, nullifier_hash, verification_level)
VALUES ('first', 'wallet', '0x0a', 'orb'), ('second', 'wallet', '0X000A', 'orb');
DO $$ BEGIN
    IF (SELECT count(*) FROM names) <> 2 THEN RAISE EXCEPTION 'Baseline did not reproduce'; END IF;
END $$;
SQL
if apply_migration >"$logs/duplicate_migration.log" 2>&1; then
  echo "FAIL: migration accepted duplicate existing owners" >&2; exit 1
fi
grep -q 'Multiple usernames share a canonical World ID nullifier' "$logs/duplicate_migration.log"
psql_test <<'SQL'
DO $$ BEGIN
    IF (SELECT count(*) FROM names) <> 2 THEN RAISE EXCEPTION 'Migration changed existing names'; END IF;
    IF to_regprocedure('canonical_world_id_nullifier(text)') IS NOT NULL THEN
        RAISE EXCEPTION 'Failed migration was not atomic';
    END IF;
END $$;
SQL
reset_schema
psql_test -c "INSERT INTO names(username,address,nullifier_hash,verification_level) VALUES ('bad','wallet','0xg','orb')" >/dev/null
if apply_migration >"$logs/invalid_migration.log" 2>&1; then
  echo "FAIL: migration accepted malformed existing owner" >&2; exit 1
fi
grep -q 'Invalid World ID nullifier encoding' "$logs/invalid_migration.log"
psql_test <<'SQL'
DO $$ BEGIN
    IF (SELECT nullifier_hash FROM names WHERE username='bad') <> '0xg' THEN RAISE EXCEPTION 'Malformed record changed'; END IF;
    IF to_regprocedure('canonical_world_id_nullifier(text)') IS NOT NULL THEN RAISE EXCEPTION 'Failed migration was not atomic'; END IF;
END $$;
SQL
reset_schema
psql_test -c "INSERT INTO names(username,address,nullifier_hash,verification_level) VALUES ('legacy','wallet','0X000A','orb')" >/dev/null
apply_migration >/dev/null
psql_test <<'SQL'
DO $$
DECLARE encoding TEXT; found_constraint TEXT;
BEGIN
    IF (SELECT nullifier_hash FROM names WHERE username='legacy') <> '0X000A' THEN
        RAISE EXCEPTION 'Migration rewrote legacy owner';
    END IF;
    IF pg_typeof(canonical_world_id_nullifier('0')) <> 'bytea'::regtype
       OR octet_length(canonical_world_id_nullifier('0')) <> 32
       OR octet_length(canonical_world_id_nullifier('a')) <> 32 THEN
        RAISE EXCEPTION 'Identity is not a fixed 32-byte blob';
    END IF;
    IF canonical_world_id_nullifier('100') <> decode(repeat('0', 60) || '0100', 'hex') THEN
        RAISE EXCEPTION 'Identity is not big endian';
    END IF;
    FOREACH encoding IN ARRAY ARRAY['0xa','0x0a','0x0A','A','0X000A', '000000000000000a'] LOOP
        IF canonical_world_id_nullifier(encoding) <> decode(repeat('0', 63) || 'a', 'hex') THEN
            RAISE EXCEPTION 'Canonical mismatch';
        END IF;
        BEGIN
            INSERT INTO names(username,address,nullifier_hash,verification_level)
            VALUES ('duplicate','wallet',encoding,'orb');
            RAISE EXCEPTION 'Accepted equivalent owner %', encoding;
        EXCEPTION WHEN unique_violation THEN
            GET STACKED DIAGNOSTICS found_constraint = CONSTRAINT_NAME;
            IF found_constraint <> 'names_nullifier_canonical_unique' THEN
                RAISE EXCEPTION 'Wrong uniqueness constraint %', found_constraint;
            END IF;
        END;
    END LOOP;
    FOREACH encoding IN ARRAY ARRAY['','0x','-1','+1',' 1','1 ',E'1\n',E'\t1','0xg','0x0x1',repeat('0',65),'１',
        '30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000001',repeat('f',64),'v4:0xa'] LOOP
        BEGIN
            INSERT INTO names(username,address,nullifier_hash,verification_level)
            VALUES ('invalid','wallet',encoding,'orb');
            RAISE EXCEPTION 'Accepted invalid encoding %', encoding;
        EXCEPTION WHEN invalid_parameter_value THEN NULL;
        END;
    END LOOP;
    IF canonical_world_id_nullifier('0') <> decode(repeat('0',64), 'hex') THEN RAISE EXCEPTION 'Zero mismatch'; END IF;
    IF canonical_world_id_nullifier('30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000000') <>
       decode('30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000000', 'hex') THEN
        RAISE EXCEPTION 'Field maximum rejected';
    END IF;
END $$;
INSERT INTO names(username,address,nullifier_hash,verification_level)
VALUES ('different','wallet','0xc','orb');
DO $$ BEGIN
    BEGIN
        UPDATE names SET nullifier_hash = '0X000A' WHERE username = 'different';
        RAISE EXCEPTION 'An update bypassed canonical uniqueness';
    EXCEPTION WHEN unique_violation THEN NULL;
    END;
END $$;
SQL

# Separate transactions contend for one numeric owner. The unique index waits
# for the other transaction, then allows exactly one commit, even for old writers.
for pair in '0xb 0X000B' '0xd 0xd'; do
  read -r first_encoding second_encoding <<< "$pair"
  psql_test >"$logs/concurrent_first.log" 2>&1 <<SQL &
BEGIN;
INSERT INTO names(username,address,nullifier_hash,verification_level)
VALUES ('first_${first_encoding}','wallet','${first_encoding}','orb');
SELECT pg_sleep(1);
COMMIT;
SQL
  first=$!
  psql_test >"$logs/concurrent_second.log" 2>&1 <<SQL &
INSERT INTO names(username,address,nullifier_hash,verification_level)
VALUES ('second_${first_encoding}','wallet','${second_encoding}','orb');
SQL
  second=$!
  set +e
  wait "$first"; first_status=$?
  wait "$second"; second_status=$?
  set -e
  if [[ "$first_status" == 0 && "$second_status" == 0 ]] || [[ "$first_status" != 0 && "$second_status" != 0 ]]; then
    cat "$logs"/concurrent_*.log >&2
    echo 'FAIL: concurrent claims did not produce exactly one successful transaction' >&2; exit 1
  fi
  grep -q 'names_nullifier_canonical_unique' "$logs"/concurrent_*.log
  count="$(psql_test --tuples-only --no-align -c "SELECT count(*) FROM names WHERE canonical_world_id_nullifier(nullifier_hash) = canonical_world_id_nullifier('${first_encoding}')")"
  [[ "$count" == 1 ]] || { echo 'FAIL: concurrent uniqueness violated' >&2; exit 1; }
done
echo 'PASS: original duplicate reproduced; migration atomicity, legacy preservation, canonical uniqueness, invalid inputs and concurrent claims verified.'
