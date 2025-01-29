mod deletion_completion_queue;
mod deletion_request_queue;
mod error;
mod username_deletion_service;
mod worker;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use std::{env, time::Duration};

use self::{
	deletion_completion_queue::DeletionCompletionQueueImpl,
	deletion_request_queue::DeletionRequestQueueImpl,
	username_deletion_service::UsernameDeletionService, worker::DataDeletionWorker,
};

pub async fn init_deletion_worker() -> Result<DataDeletionWorker> {
	// Initialize a dedicated DB pool for the worker
	let db_pool = PgPoolOptions::new()
        .max_connections(5) // Lower connection count since it's dedicated
        .acquire_timeout(Duration::from_secs(4))
        .connect(&env::var("DATABASE_URL")?)
        .await?;

	// Initialize worker components
	let request_queue = DeletionRequestQueueImpl::new().await?;
	let completion_queue = DeletionCompletionQueueImpl::new().await?;
	let deletion_service = username_deletion_service::PgUsernameDeletionService::new(db_pool);

	// Initialize the worker
	let worker = DataDeletionWorker::new(
		Box::new(request_queue),
		Box::new(completion_queue),
		Box::new(deletion_service),
	)?;

	Ok(worker)
}
