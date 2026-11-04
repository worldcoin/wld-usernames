# World ID 4 username ownership

The v2 API binds a unique human and a persistent session in the same proof.
The action `world_usernames:v2:owner` never includes a wallet, username, epoch
or recovery counter. Registration consumes it once. Rename and recovery prove
the stored session; they do not disclose the uniqueness nullifier again.

Configure `USERNAME_V4_RP_ID` and `USERNAME_V4_RP_SIGNING_KEY` for a registered
RP in the matching environment. The feature stays unavailable without this
configuration. The existing Developer Portal endpoint must support
`proof_type: uniqueness` with `session_id`, verified with
`verifyProofAndSignals`. Do not enable against a verifier that drops the session.

1. The authenticated app backend sends `POST /api/v2/challenge` with
   `operation` (`register`, `migrate`, `rename`, `recover`), `username`,
   the authenticated user's `address`, and `new_username` for rename.
2. Pass the returned `proof_request` directly to WalletKit. It includes a
   five-minute signed nonce, the correct session mode and a signal committing
   to the operation, both names, destination address and previous address.
3. Submit `POST /api/v2/complete` with `challenge_id` and
   `proof: { session_id, response: { proof, expires_at_min, nullifier?,
   session_nullifier? } }`. Use the Orb response. Registration and migration
   require `nullifier`; rename and recovery require `session_nullifier`.
4. Migration additionally requires `legacy_proof: { merkle_root,
   nullifier_hash, proof }` for the existing v3 `username` action, with the
   returned `legacy_signal` as raw UTF-8 bytes.
5. After success, resolve the authoritative username record and synchronize
   the app user's profile. Retrying a completed challenge is safe, but returns
   an error if a subsequent operation has superseded it.

Migrate existing usernames while the old owner can still prove ownership.
A username that was never linked to v4 cannot be reassigned based only on its
name or a newly recovered wallet. Such an account needs an independently
authenticated migration; this API deliberately rejects guessing that link.

Ownership writes, namespace checks and challenge consumption share a database
transaction. Old v3 writes are blocked by a database trigger after migration.
The original legacy nullifier remains reserved. Cache tombstones are written
before commit so stale reads cannot restore the old wallet mapping; Redis
failure rolls back the change.

Run unit tests with `SQLX_OFFLINE=true cargo +1.86.0 test --locked`. Run the
ignored database tests with a disposable PostgreSQL `DATABASE_URL` and
`TEST_REDIS_URL`, then `cargo +1.86.0 test --locked routes::api::v2 -- --ignored`.
Never point these tests at production. The dedicated PR workflow provisions
disposable PostgreSQL and Redis services.

Operational retention: remove challenge rows only after their expiry plus the
desired retry window (at least 24 hours). Deploy the schema before enabling
clients. v4 profile-picture authorization and app-backend profile synchronization
are separate consumers of this ownership contract.
