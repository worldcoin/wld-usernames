#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod blocklist;
mod config;
mod data_deletion_worker;
mod routes;
mod server;
mod types;
mod utils;
mod verify;

use data_deletion_worker::{
	data_deletion_worker::DataDeletionWorker,
	deletion_completion_queue::DeletionCompletionQueueImpl,
	deletion_request_queue::DeletionRequestQueueImpl,
};
use sqlx::PgPool;
use tokio::sync::broadcast;

#[tokio::main]
#[tracing::instrument]
async fn main() -> anyhow::Result<()> {
	dotenvy::dotenv().ok();

	// Initialize Datadog tracing
	let (_guard, _tracer_shutdown) = datadog_tracing::init()?;

	tracing::info!("👩 Server started");

	// required for tls support
	init_crypto();

	let mut config = config::Config::from_env().await?;
	config.migrate_database().await?;
	tracing::info!("👩‍🌾 Migrations run");

	// Initialize worker with write pool
	let (worker, shutdown_tx) = init_worker(config.db_client()).await?;

	// Spawn worker task
	let worker_handle = {
		let worker_shutdown_rx = shutdown_tx.subscribe();
		tokio::spawn(async move {
			worker.run(worker_shutdown_rx).await;
		})
	};

	// Start server with shutdown sender
	server::start(config, shutdown_tx).await?;

	// Wait for worker to finish after server shutdown
	if let Err(e) = worker_handle.await {
		tracing::error!("Error waiting for worker to shutdown: {}", e);
	}

	Ok(())
}

fn init_crypto() {
	rustls::crypto::ring::default_provider()
		.install_default()
		.expect("Error initializing crypto provider");
}

async fn init_worker(
	db_pool: PgPool,
) -> anyhow::Result<(DataDeletionWorker, broadcast::Sender<()>)> {
	// Create shutdown channel
	let (shutdown_tx, _) = broadcast::channel(1);

	// Initialize worker components
	let request_queue = DeletionRequestQueueImpl::new().await?;
	let completion_queue = DeletionCompletionQueueImpl::new().await?;

	// Initialize the worker
	let worker =
		DataDeletionWorker::new(Box::new(request_queue), Box::new(completion_queue), db_pool)?;

	Ok((worker, shutdown_tx))
}
