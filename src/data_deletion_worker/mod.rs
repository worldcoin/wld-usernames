mod deletion_completion_queue;
mod deletion_request_queue;
mod error;
mod username_deletion_service;
mod worker;

use anyhow::{Context, Result};
use redis::aio::ConnectionManager;
use sqlx::postgres::PgPoolOptions;
use std::{env, time::Duration};

use self::{
	deletion_completion_queue::DeletionCompletionQueueImpl,
	deletion_request_queue::DeletionRequestQueueImpl,
	username_deletion_service::UsernameDeletionServiceImpl, worker::DataDeletionWorker,
};

async fn build_redis_pool(mut redis_url: String) -> redis::RedisResult<ConnectionManager> {
	if !redis_url.starts_with("redis://") && !redis_url.starts_with("rediss://") {
		redis_url = format!("redis://{redis_url}");
	}

	let client = redis::Client::open(redis_url)?;

	ConnectionManager::new(client).await
}

pub async fn init_deletion_worker() -> Result<DataDeletionWorker> {
	// Initialize a dedicated DB pool for the worker
	let db_pool = PgPoolOptions::new()
        .max_connections(5) // Lower connection count since it's dedicated
        .acquire_timeout(Duration::from_secs(4))
        .connect(&env::var("DATABASE_URL")?)
        .await?;

	// Initialize Redis connection for cache invalidation - now mandatory
	let redis_url = env::var("REDIS_URL")
		.context("REDIS_URL environment variable is required for data deletion worker")?;

	let redis_manager = build_redis_pool(redis_url)
		.await
		.context("Failed to connect to Redis for data deletion worker")?;

	tracing::info!("✅ Connection to Redis established for deletion worker.");

	// Initialize worker components
	let request_queue = DeletionRequestQueueImpl::new().await?;
	let completion_queue = DeletionCompletionQueueImpl::new().await?;

	// Create deletion service with Redis
	let deletion_service = UsernameDeletionServiceImpl::new(db_pool, redis_manager);

	// Initialize the worker
	let worker = DataDeletionWorker::new(
		Box::new(request_queue),
		Box::new(completion_queue),
		Box::new(deletion_service),
	)?;

	Ok(worker)
}
