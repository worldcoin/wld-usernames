# World Usernames

This is our open source implementation of ENS compatible Usernames

## Prerequisite

In order to use `sqlx` commands, you need to install `sqlx-cli`

```sh
cargo install sqlx-cli
```

## 🚀 Running Locally

```sh
cp .env.example .env
docker compose up --detach

cargo run

// go to localhost:8000
```

## Updating Queries

In order to update the queries, you need to run the following command:

```
cargo sqlx prepare
```

## 🛳️ Finding Deployments

[Production Deployment](https://usernames.worldcoin.org/docs)
[ENS Resolver](https://etherscan.io/address/0xB4E36A6C3403137d8fdaf4e91b91D1aBC2caF3Dd)

## 🧪 Dev/E2E Endpoints

The following endpoints are intended for local development and e2e tests
only. They are gated by the same flag as `x-e2e-skip-attestation`: the
service must run with `APP_ENV=development` or `APP_ENV=staging`, **and** the
caller must send `x-e2e-skip-attestation: true`. In production they always
respond with `403 Forbidden`.

- `DELETE /api/v1/internal/usernames/:address` — deletes the username record
  associated with a wallet address (idempotent). Wraps the same
  `UsernameDeletionService` used by the deletion worker, so it cleans up
  `names`, `old_names` and the related Redis cache entries. Returns
  `204 No Content` on success.

### Rust required installations

```bash
# For MacOS Core M
rustup target add aarch64-apple-darwin
# For Linux ARM64
rustup target add aarch64-unknown-linux-musl
# For Linux ARMv7
rustup target add armv7-unknown-linux-musl
# For Linux x86/64
rustup target add x86_64-unknown-linux-musl
```

### How to Build

To build for a specific target, specify it in the cargo build command:

```bash
cargo build --target aarch64-unknown-linux-musl
cargo build --target armv7-unknown-linux-musl
cargo build --target x86_64-unknown-linux-musl
```

This setup ensures that your binaries are compiled correctly for the specified architectures and configurations.
