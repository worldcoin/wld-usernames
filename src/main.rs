#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod blocklist;
mod config;
mod routes;
mod server;
mod types;
mod utils;
mod verify;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	dotenvy::dotenv().ok();

	tracing_subscriber::fmt()
		.json()
		.with_target(false)
		.flatten_event(true)
		.init();

	tracing::info!("👩 Server started");

	let config = config::Config::from_env().await?;
	config.migrate_database().await?;

	tracing::info!("👩‍🌾 Migrations run");

	server::start(config).await
}
