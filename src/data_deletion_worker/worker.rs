use anyhow::Result;
use tokio::{
	sync::broadcast,
	time::{sleep, Duration},
};
use tracing::{error, info};

use super::{
	deletion_completion_queue::DeletionCompletionQueue,
	deletion_request_queue::DeletionRequestQueue,
	username_deletion_service::UsernameDeletionService,
};

pub struct DataDeletionWorker {
	request_queue: Box<dyn DeletionRequestQueue>,
	completion_queue: Box<dyn DeletionCompletionQueue>,
	deletion_service: Box<dyn UsernameDeletionService>,
	sleep_interval: Duration,
}

impl DataDeletionWorker {
	pub fn new(
		request_queue: Box<dyn DeletionRequestQueue>,
		completion_queue: Box<dyn DeletionCompletionQueue>,
		deletion_service: Box<dyn UsernameDeletionService>,
	) -> Result<Self> {
		let sleep_interval_secs = std::env::var("DELETION_WORKER_SLEEP_INTERVAL_SECS")?
			.parse::<u64>()
			.map_err(|e| anyhow::anyhow!("Invalid sleep interval: {}", e))?;

		Ok(Self {
			request_queue,
			completion_queue,
			deletion_service,
			sleep_interval: Duration::from_secs(sleep_interval_secs),
		})
	}

	async fn process_deletion_requests(&self) -> Result<()> {
		info!("Processing deletion requests...");

		let messages = self.request_queue.poll_messages().await?;

		for message in messages {
			match self
				.deletion_service
				.delete_username(
					&message.request.user.wallet_address,
					message.request.correlation_id,
				)
				.await
			{
				Ok(_) => {
					if let Err(e) = self
						.request_queue
						.acknowledge(&message.receipt_handle)
						.await
					{
						error!("Failed to acknowledge message: {}", e);
					}
				},
				Err(e) => {
					error!("Failed to delete username: {}", e);
				},
			}
		}

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
