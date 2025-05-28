#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod blocklist;
mod config;
mod data_deletion_worker;
mod routes;
mod search;
mod server;
mod types;
mod utils;
mod verify;

use datadog_tracing::axum::shutdown_signal;
use std::env;
use tokio::sync::broadcast;

#[tokio::main]
#[tracing::instrument]
async fn main() -> anyhow::Result<()> {
	dotenvy::dotenv().ok();

	// Initialize Datadog tracing
	let (_guard, _tracer_shutdown) = datadog_tracing::init()?;

	log_panics::init();

	tracing::info!("👩 Server started");

	// required for tls support
	init_crypto();

	let config = config::Config::from_env().await?;
	config.migrate_database().await?;
	tracing::info!("👩‍🌾 Migrations run");

	// Create shutdown channel
	let (shutdown_tx, _) = broadcast::channel(1);

	// Initialize worker only in staging environment
	let worker_handle = if env::var("ENABLE_DATA_DELETION_WORKER").unwrap_or_default() == "true" {
		tracing::info!("👩‍🌾 Initializing data deletion worker...");

		// Get Redis connection from config
		let redis_connection = config.get_redis_connection();
		tracing::info!("✅ Got Redis connection for deletion worker from config");

		// Initialize worker with the Redis connection
		match data_deletion_worker::init_deletion_worker(redis_connection).await {
			Ok(worker) => {
				tracing::info!("✅ Data deletion worker initialized successfully");
				let worker_shutdown_rx = shutdown_tx.subscribe();
				Some(tokio::spawn(async move {
					worker.run(worker_shutdown_rx).await;
				}))
			},
			Err(e) => {
				tracing::error!("❌ Error initializing worker: {}", e);
				None
			},
		}
	} else {
		tracing::info!("👩‍🌾 Data deletion worker not enabled");
		None
	};

	// Spawn shutdown signal task
	let _shutdown_handle = {
		let shutdown_tx = shutdown_tx.clone();
		tokio::spawn(async move {
			shutdown_signal().await;
			let _ = shutdown_tx.send(());
		})
	};

	// Run server in main thread with shutdown receiver
	let server_result = server::start(config, shutdown_tx.subscribe()).await;

	// Wait for worker to finish if it was spawned
	if let Some(handle) = worker_handle {
		if let Err(e) = handle.await {
			tracing::error!("Error waiting for worker to shutdown: {}", e);
		}
	}

	// Check server result
	if let Err(e) = server_result {
		tracing::error!("Server error: {}", e);
	}

	Ok(())
}

fn init_crypto() {
	rustls::crypto::ring::default_provider()
		.install_default()
		.expect("Error initializing crypto provider");
}
