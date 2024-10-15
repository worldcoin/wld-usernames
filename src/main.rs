#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

use anyhow::Result;
use config::Config;
use dotenvy::dotenv;

mod blocklist;
mod config;
mod routes;
mod server;
mod types;
mod utils;
mod verify;

#[tokio::main]
async fn main() -> Result<()> {
	dotenv().ok();

	tracing_subscriber::fmt()
		.json()
		.with_target(false)
		.flatten_event(true)
		.init();

	tracing::info!("👩 Server started");

	let config = Config::from_env().await?;
	config.migrate_database().await?;

	tracing::info!("👩‍🌾 Migrations run");

	server::start(config).await
}
