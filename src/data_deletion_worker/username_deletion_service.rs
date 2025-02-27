use alloy::primitives::Address;
use async_trait::async_trait;
use redis::{aio::ConnectionManager, AsyncCommands};
use sqlx::PgPool;
use std::str::FromStr;
use tracing::{info_span, instrument, Instrument};

use super::error::QueueError;

#[async_trait]
pub trait UsernameDeletionService: Send + Sync {
	async fn delete_username(&self, wallet_address: &str) -> Result<(), QueueError>;
}

#[allow(clippy::module_name_repetitions)]
pub struct UsernameDeletionServiceImpl {
	pool: PgPool,
	redis: ConnectionManager,
}

impl UsernameDeletionServiceImpl {
	pub const fn new(pool: PgPool, redis: ConnectionManager) -> Self {
		Self { pool, redis }
	}
}

#[async_trait]
impl UsernameDeletionService for UsernameDeletionServiceImpl {
	#[instrument(skip(self), err)]
	async fn delete_username(&self, wallet_address: &str) -> Result<(), QueueError> {
		// First, get the username(s) associated with this wallet address
		// We need this to invalidate the cache by username
		let wallet_address = Address::from_str(wallet_address).map_or_else(
			|_| wallet_address.to_string(),
			|address| address.to_checksum(None),
		);
		let usernames = sqlx::query!(
			"SELECT username FROM names WHERE address = $1",
			wallet_address
		)
		.fetch_all(&self.pool)
		.await
		.map_err(QueueError::DatabaseError)?;

		// Start a transaction to ensure atomicity
		let mut tx = self.pool.begin().await.map_err(QueueError::DatabaseError)?;

		// For each username, first delete any records in old_names that reference it
		for row in &usernames {
			let username = &row.username;

			// Delete records where this username is the new_username (referenced by foreign key)
			sqlx::query!("DELETE FROM old_names WHERE new_username = $1", username)
				.execute(&mut *tx)
				.instrument(info_span!("delete_old_names_db_query", username = username))
				.await
				.map_err(QueueError::DatabaseError)?;
		}

		// Now it's safe to delete the usernames from the names table
		sqlx::query!("DELETE FROM names WHERE address = $1", wallet_address)
			.execute(&mut *tx)
			.instrument(info_span!(
				"delete_names_db_query",
				wallet_address = wallet_address
			))
			.await
			.map_err(QueueError::DatabaseError)?;

		// Commit the transaction
		tx.commit().await.map_err(QueueError::DatabaseError)?;

		let mut redis = self.redis.clone();

		// Invalidate cache by wallet address
		let address_cache_key = format!("query_single:{wallet_address}");
		redis
			.del::<_, String>(&address_cache_key)
			.await
			.map_err(|e| QueueError::CacheInvalidationError(e.to_string()))?;

		// Invalidate cache by username for each username associated with this wallet
		for row in usernames {
			let username = row.username;

			// Invalidate query_single cache
			let username_cache_key = format!("query_single:{username}");
			redis
				.del::<_, String>(&username_cache_key)
				.await
				.map_err(|e| QueueError::CacheInvalidationError(e.to_string()))?;

			// Invalidate avatar cache
			let avatar_cache_key = format!("avatar:{username}");
			redis
				.del::<_, String>(&avatar_cache_key)
				.await
				.map_err(|e| QueueError::CacheInvalidationError(e.to_string()))?;

			// Invalidate search cache - this is less critical since it expires in 5 minutes
			// but we'll invalidate it anyway for consistency
			let search_cache_key = format!("search:{}", username.to_lowercase());
			redis
				.del::<_, String>(&search_cache_key)
				.await
				.map_err(|e| QueueError::CacheInvalidationError(e.to_string()))?;
		}

		Ok(())
	}
}
