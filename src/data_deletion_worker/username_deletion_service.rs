use async_trait::async_trait;
use sqlx::PgPool;

use super::error::QueueError;

#[async_trait]
pub trait UsernameDeletionService: Send + Sync {
	async fn delete_username(&self, wallet_address: &str) -> Result<(), QueueError>;
}

#[allow(clippy::module_name_repetitions)]
pub struct UsernameDeletionServiceImpl {
	pool: PgPool,
}

impl UsernameDeletionServiceImpl {
	pub const fn new(pool: PgPool) -> Self {
		Self { pool }
	}
}

#[async_trait]
impl UsernameDeletionService for UsernameDeletionServiceImpl {
	async fn delete_username(&self, wallet_address: &str) -> Result<(), QueueError> {
		sqlx::query!("DELETE FROM names WHERE address = $1", wallet_address)
			.execute(&self.pool)
			.await
			.map_err(QueueError::DatabaseError)?;

		Ok(())
	}
}
