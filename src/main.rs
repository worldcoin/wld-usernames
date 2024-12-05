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
	// Create a span for the entire startup process
	let _startup_span = tracing::info_span!("application_startup").entered();

	dotenv().ok();

	// Create a nested span for configuration
	{
		let _config_span = tracing::info_span!("configuration_loading", phase = "init").entered();

		// Initialize DataDog tracing
		// let (_guard, _tracer_shutdown) = datadog_tracing::init()?;

		let env_filter = EnvFilter::try_from_default_env()
			.unwrap_or_else(|_| "wld_usernames=info,tower_http=debug".into());

		if tracing_subscriber::registry()
			.with(tracing_subscriber::fmt::layer())
			.with(env_filter)
			.try_init()
			.is_err() {
			eprintln!("Tracing subscriber was already set");
		}

		tracing::info!("DD_SERVICE={}", std::env::var("DD_SERVICE").unwrap_or_default());	
	} // config_span ends here

	// Create a span for database initialization
	{
		let _db_span = tracing::info_span!("database_initialization", phase = "init").entered();
		let config = Config::from_env().await?;
		// config.migrate_database().await?;
		server::start(config).await
	} // db_span ends here

	// Server startup will be traced in the server::start function
}
