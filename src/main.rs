#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

use anyhow::{bail, Result};
use blocklist::Blocklist;
use dotenvy::dotenv;
use sqlx::postgres::PgPoolOptions;
use std::{env, time::Duration};
use tracing_subscriber::{
    prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
};

mod blocklist;
mod routes;
mod server;
mod types;
mod utils;

static ENV_VARS: &[&str] = &[
    "ENS_DOMAIN",
    "DATABASE_URL",
    "WORLD_ID_APP_ID",
    "ENS_RESOLVER_SALT",
    "RESERVED_USERNAMES",
    "BLOCKED_SUBSTRINGS",
    "ENS_SIGNER_PRIVATE_KEY",
];

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "wld_usernames=info".into()),
        ))
        .init();

    for var in ENV_VARS {
        if env::var(var).is_err() {
            bail!("Missing environment variable: ${var}");
        }
    }

    let blocklist = Blocklist::new(
        &env::var("RESERVED_USERNAMES").unwrap(),
        &env::var("BLOCKED_SUBSTRINGS").unwrap(),
    );

    let postgres = PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(3))
        .connect(&env::var("DATABASE_URL").unwrap())
        .await
        .expect("failed to connect to database");

    server::start(postgres, blocklist).await
}
