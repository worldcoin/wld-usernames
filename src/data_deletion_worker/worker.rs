use anyhow::Result;
use tokio::{
	sync::broadcast,
	time::{sleep, Duration},
};
use tracing::{error, info, instrument};

use super::{
	deletion_completion_queue::{DataDeletionCompletion, DeletionCompletionQueue},
	deletion_request_queue::{DeletionRequestQueue, QueueMessage},
	username_deletion_service::UsernameDeletionService,
};

#[allow(clippy::module_name_repetitions)]
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

	#[instrument(skip(self), err)]
	async fn handle_single_deletion(&self, deletion_request: QueueMessage) -> Result<()> {
		let message = deletion_request.request;

		info!(correlation_id = %message.correlation_id, "Deleting username");

		self.deletion_service
			.delete_username(&message.user.wallet_address)
			.await?;

		info!(correlation_id = %message.correlation_id, "Deleted username");

		let completion_message = DataDeletionCompletion::new(message.correlation_id);
		self.completion_queue
			.send_message(completion_message)
			.await?;

		info!(correlation_id = %message.correlation_id, "Sent completion message");

		self.request_queue
			.acknowledge(&deletion_request.receipt_handle)
			.await?;

		info!(correlation_id = %message.correlation_id, "Acknowledged deletion request");

		Ok(())
	}

	async fn poll_and_process_batch(&self) -> Result<()> {
		info!("Processing deletion requests...");

		let deletion_requests = self.request_queue.poll_messages().await?;

		for deletion_request in deletion_requests {
			let correlation_id = deletion_request.request.correlation_id;
			match self.handle_single_deletion(deletion_request).await {
				Ok(()) => {
					info!(correlation_id = %correlation_id, "Deleted username successfully for {correlation_id}");
				},
				Err(e) => {
					error!(
						correlation_id = %correlation_id,
						error = %e,
						error.kind = "username_deletion_failed",
						"Failed to delete username for {correlation_id}"
					);
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
			sleep(self.sleep_interval).await;

			tokio::select! {
				_ = shutdown.recv() => {
					info!("Shutdown signal received, stopping data deletion worker...");
					break;
				}
				() = sleep(self.sleep_interval) => {
					if let Err(e) = self.poll_and_process_batch().await {
						error!("Error processing deletion requests: {}", e);
					}
				}
			}
		}

		info!("Data deletion worker stopped.");
	}
}
