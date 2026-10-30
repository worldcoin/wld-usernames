# Canonical username ownership rollout

Username ownership is keyed by a numeric World ID nullifier, not its input text.
The migration installs an immutable parser and a unique expression index. It does
not change stored nullifier strings, add columns, delete names or choose a winner
among duplicates. Existing `SELECT *` consumers remain compatible.

## Before deployment

1. Rehearse the migration against a disposable copy of the schema and representative
   data. Run `tests/test_nullifier_uniqueness.sh` against a disposable PostgreSQL
   database. Do not point this test at production.
2. Audit legacy nullifiers for malformed/out-of-field values and multiple names
   with the same canonical numeric value. The parser accepts 1-64 ASCII hex digits,
   optionally prefixed by `0x`/`0X`, below the BN254 scalar-field modulus. Whitespace,
   signs and modular aliases are invalid. Do not use a floating-point cast.
3. If any conflicts exist, stop rollout and obtain an explicit ownership-resolution
   decision. Preserve names, aliases and related profile/cache data. Do not silently
   delete a row, select an arbitrary winner or merge ownership. Remediation is a
   separate reviewed operation; this migration intentionally refuses such data.
4. Schedule a deployment window for the index build. This transactional migration
   locks `names` against concurrent writes and scans the table. It is deliberately
   not `CREATE INDEX CONCURRENTLY`: validation and enforcement must become visible
   together. Estimate the lock duration on representative data first.

## Deployment and rollback

The new application runs migrations on startup before serving requests. Install
this migration before routing traffic to code that uses the canonical lookup.
During a rolling deployment, the expression index also protects writes from old
instances, including alternative encodings and simultaneous registrations. Old
instances may return a generic error for a rejected duplicate; new instances map
that constraint violation to the existing duplicate-World-ID validation error.
The pre-insert availability query is only a friendly early check; the unique index
is the final authority.

Existing nullifier strings are left untouched, so old instances do not lose their
original exact-string ownership checks through a backfill. New instances compare
canonical numeric values for ownership operations and store canonical new values.
Complete the application rollout before relying on equivalent encodings in those
operations. No cache invalidation is needed solely for this migration because no
stored record changes. Any separate collision remediation must invalidate affected
cache entries through the approved operational procedure.

If the migration encounters malformed values or collisions, its transaction rolls
back completely and application startup fails. Fix the data only after review and
retry the migration. Do not mark it applied manually or bypass the uniqueness check.
An application rollback can retain the index, preserving the security invariant;
do not drop the index as part of a routine rollback. Never change the immutable
parser's meaning without validating existing data and rebuilding its dependent
index in a new migration.

## World ID 4 recovery integration

Draft recovery work uses a separate ownership domain and may place `v4:<owner>`
sentinels in `names.nullifier_hash`. Those are not legacy nullifiers and this strict
legacy parser deliberately rejects them. Before that work is merged/deployed,
coordinate its schema with this invariant (for example, an explicit typed ownership
column/table with separate unique keys). Do not strip `v4:` and accidentally equate
v4 and legacy owners, or exempt arbitrary malformed legacy inputs from uniqueness.
The draft recovery work is not required to fix the current legacy registration API.

## Regression checks

With disposable PostgreSQL and Redis running locally, use the pinned toolchain:

```sh
SQLX_OFFLINE=true cargo test --locked nullifier::tests
SQLX_OFFLINE=true DATABASE_URL=postgres://postgres:test@localhost:5432/usernames_test \
  TEST_REDIS_URL=redis://localhost:6379 \
  cargo test --locked routes::api::v1::nullifier_tests -- --ignored
TEST_DATABASE_URL=postgres://postgres:test@localhost:5432/usernames_test \
  ALLOW_NULLIFIER_DB_TESTS=1 bash tests/test_nullifier_uniqueness.sh
```

The API tests mock the proof verifier locally; they do not generate cryptographic
proofs or contact a deployed service. They check canonical forwarding/persistence,
rejection of duplicate owners and invalid proofs, concurrent requests, and legacy
ownership through update, rename and profile-picture deletion. The SQL tests
reproduce the old encoding bypass and verify enforcement at the database boundary,
including writers that do not normalize their input.
