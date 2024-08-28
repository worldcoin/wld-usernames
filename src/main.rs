#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

use anyhow::Result;
use config::Config;
use dotenvy::dotenv;
use tracing_subscriber::{
	prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
};

mod blocklist;
mod config;
mod routes;
mod server;
mod types;
mod utils;

#[tokio::main]
async fn main() -> Result<()> {
	dotenv().ok();

	tracing_subscriber::registry()
		.with(tracing_subscriber::fmt::layer().with_filter(
			EnvFilter::try_from_default_env().unwrap_or_else(|_| "wld_usernames=info".into()),
		))
		.init();

	let config = Config::from_env().await?;
	config.migrate_database().await?;

	server::start(config).await
}
