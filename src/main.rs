#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod blocklist;
mod config;
mod routes;
mod server;
mod types;
mod utils;
mod verify;

#[tokio::main]
#[tracing::instrument]
async fn main() -> anyhow::Result<()> {
	dotenvy::dotenv().ok();

	// Initialize Datadog tracing
	let (_guard, _tracer_shutdown) = datadog_tracing::init()?;

	tracing::info!("👩 Server started");

	// required for tls support
	init_crypto();

	let config = config::Config::from_env().await?;
	config.migrate_database().await?;
	tracing::info!("👩‍🌾 Migrations run");

	server::start(config).await
}
fn init_crypto() {
	rustls::crypto::ring::default_provider()
		.install_default()
		.expect("Error initializing crypto provider");
}
