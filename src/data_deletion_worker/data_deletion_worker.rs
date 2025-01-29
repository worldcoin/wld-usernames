use anyhow::Result;
use sqlx::PgPool;
use tokio::{
	sync::broadcast,
	time::{sleep, Duration},
};
use tracing::{error, info};

use super::{
	deletion_completion_queue::DeletionCompletionQueue,
	deletion_request_queue::DeletionRequestQueue,
};

pub struct DataDeletionWorker {
	request_queue: Box<dyn DeletionRequestQueue>,
	completion_queue: Box<dyn DeletionCompletionQueue>,
	pg_pool: PgPool,
	sleep_interval: Duration,
}

impl DataDeletionWorker {
	pub fn new(
		request_queue: Box<dyn DeletionRequestQueue>,
		completion_queue: Box<dyn DeletionCompletionQueue>,
		pg_pool: PgPool,
	) -> Result<Self> {
		let sleep_interval_secs = std::env::var("DELETION_WORKER_SLEEP_INTERVAL_SECS")?
			.parse::<u64>()
			.map_err(|e| anyhow::anyhow!("Invalid sleep interval: {}", e))?;

		Ok(Self {
			request_queue,
			completion_queue,
			pg_pool,
			sleep_interval: Duration::from_secs(sleep_interval_secs),
		})
	}

	async fn process_deletion_requests(&self) -> Result<()> {
		info!("Processing deletion requests...");
		// TODO: Implement actual deletion logic here
		sleep(Duration::from_secs(1)).await;
		Ok(())
	}

	pub async fn run(&self, mut shutdown: broadcast::Receiver<()>) {
		info!(
			"Starting data deletion worker with {}s sleep interval...",
			self.sleep_interval.as_secs()
		);

		loop {
			/**
			 * Select between the shutdown signal and the sleep interval
			 * If the shutdown signal is received, break the loop
			 * If the sleep interval is reached, process the deletion requests
			 */
			tokio::select! {
				_ = shutdown.recv() => {
					info!("Shutdown signal received, stopping data deletion worker...");
					break;
				}
				_ = sleep(self.sleep_interval) => {
					if let Err(e) = self.process_deletion_requests().await {
						error!("Error processing deletion requests: {}", e);
					}
				}
			}
		}

		info!("Data deletion worker stopped.");
	}
}
