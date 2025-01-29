use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;

use super::error::QueueError;

#[async_trait]
pub trait UsernameDeletionService: Send + Sync {
	async fn delete_username(&self, username: &str, correlation_id: Uuid)
		-> Result<(), QueueError>;
}

pub struct PgUsernameDeletionService {
	pool: PgPool,
}

impl PgUsernameDeletionService {
	pub fn new(pool: PgPool) -> Self {
		Self { pool }
	}
}

#[async_trait]
impl UsernameDeletionService for PgUsernameDeletionService {
	async fn delete_username(
		&self,
		username: &str,
		correlation_id: Uuid,
	) -> Result<(), QueueError> {
		// Start a transaction
		let mut tx = self.pool.begin().await.map_err(QueueError::DatabaseError)?;

		// TODO: Implement actual deletion logic
		// For example:
		// - Move username to deleted_usernames table
		// - Update any related records
		// - Send notifications if needed

		// Commit the transaction
		tx.commit().await.map_err(QueueError::DatabaseError)?;

		Ok(())
	}
}
