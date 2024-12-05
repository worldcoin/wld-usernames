#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

use anyhow::Result;
use config::Config;
use dotenvy::dotenv;
use tracing_subscriber::{
	prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
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

	// Initialize DataDog tracing
	let (_guard, _tracer_shutdown) = datadog_tracing::init()?;

	// Set up the tracing subscriber with DataDog integration
	let env_filter = EnvFilter::try_from_default_env()
		.unwrap_or_else(|_| "wld_usernames=info,tower_http=debug".into());

	tracing_subscriber::registry()
		.with(tracing_subscriber::fmt::layer())
		.with(env_filter)
		.init();

	let config = Config::from_env().await?;
	config.migrate_database().await?;

	server::start(config).await
}
