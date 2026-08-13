use alloy::primitives::Address;
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use sqlx::PgPool;
use std::str::FromStr;
use tracing::{info_span, instrument, Instrument};

use super::error::QueueError;
use crate::cache;

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

	/// Writes deletion tombstones (see `crate::cache`) rather than plain DELs:
	/// a DEL cannot stop a reader that loaded the row just before the delete
	/// committed from re-populating the cache afterwards, while a tombstone
	/// makes the guarded cache fills refuse the write. Keys are canonical
	/// (lowercased usernames, checksummed address), matching the read routes.
	async fn invalidate_caches(
		&self,
		wallet_address: &str,
		usernames: &[String],
	) -> Result<(), QueueError> {
		let mut redis = self.redis.clone();

		// The wallet-address key, plus per username: the query_single cache,
		// both avatar caches, and the search cache (less critical since it
		// expires in 5 minutes, but invalidated anyway for consistency).
		let mut cache_keys = vec![format!("query_single:{wallet_address}")];
		for username in usernames {
			let username = username.to_lowercase();
			cache_keys.extend([
				format!("query_single:{username}"),
				format!("avatar:{username}:original"),
				format!("avatar:{username}:minimized"),
				format!("search:{username}"),
			]);
		}

		for cache_key in &cache_keys {
			cache::write_tombstone(&mut redis, cache_key)
				.await
				.map_err(|e| QueueError::CacheInvalidationError(e.to_string()))?;
		}

		Ok(())
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
		let usernames: Vec<String> = sqlx::query!(
			"SELECT username FROM names WHERE address = $1",
			wallet_address
		)
		.fetch_all(&self.pool)
		.await
		.map_err(QueueError::DatabaseError)?
		.into_iter()
		.map(|row| row.username)
		.collect();

		// Invalidate caches BEFORE deleting the rows: if this attempt is
		// interrupted after the DB commit (crash, timeout, shutdown), the
		// retry can no longer discover the usernames from the deleted rows,
		// so their cache keys must already be gone by then. The second pass
		// after the commit clears entries repopulated by concurrent reads
		// while the rows were still live.
		self.invalidate_caches(&wallet_address, &usernames).await?;

		// Start a transaction to ensure atomicity
		let mut tx = self.pool.begin().await.map_err(QueueError::DatabaseError)?;

		// First delete any old_names records referencing this wallet's
		// usernames (foreign key on new_username). The subquery, rather than
		// the pre-captured list, covers usernames that appeared after the
		// SELECT above (e.g. a concurrent rename).
		sqlx::query!(
			"DELETE FROM old_names WHERE new_username IN (SELECT username FROM names WHERE address = $1)",
			wallet_address
		)
		.execute(&mut *tx)
		.instrument(info_span!(
			"delete_old_names_db_query",
			wallet_address = wallet_address
		))
		.await
		.map_err(QueueError::DatabaseError)?;

		// Now it's safe to delete the usernames from the names table.
		// RETURNING reveals rows inserted between the SELECT above and this
		// delete (e.g. a concurrent registration for the same wallet). Any
		// username discovered only here must be tombstoned BEFORE the commit:
		// if that fails, the transaction rolls back and the retry can
		// rediscover everything from the intact rows.
		let missed_usernames: Vec<String> = sqlx::query!(
			"DELETE FROM names WHERE address = $1 RETURNING username",
			wallet_address
		)
		.fetch_all(&mut *tx)
		.instrument(info_span!(
			"delete_names_db_query",
			wallet_address = wallet_address
		))
		.await
		.map_err(QueueError::DatabaseError)?
		.into_iter()
		.map(|row| row.username)
		.filter(|deleted| !usernames.contains(deleted))
		.collect();
		if !missed_usernames.is_empty() {
			self.invalidate_caches(&wallet_address, &missed_usernames)
				.await?;
		}

		// Commit the transaction
		tx.commit().await.map_err(QueueError::DatabaseError)?;

		self.invalidate_caches(&wallet_address, &usernames).await?;

		Ok(())
	}
}
