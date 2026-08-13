//! Username-cache helpers shared by the read routes and the invalidators
//! (data-deletion worker, rename and profile-picture routes).
//!
//! Cache keys are canonical: usernames are lowercased (every read path matches
//! usernames case-insensitively, so caller-supplied casings would otherwise
//! create cache aliases that invalidation can never enumerate) and addresses
//! use their EIP-55 checksummed form.
//!
//! Deletion writes a tombstone value instead of issuing a DEL: a plain DEL
//! cannot stop a reader that loaded the row just before the delete committed
//! from re-populating the cache afterwards. Readers treat the tombstone as a
//! miss, and cache fills go through [`set_ex_unless_tombstoned`], which
//! atomically refuses to overwrite a tombstone.

use std::{str::FromStr, sync::LazyLock};

use alloy::primitives::Address;
use redis::{aio::ConnectionManager, AsyncCommands};

use crate::utils::ONE_MINUTE_IN_SECONDS;

/// Sentinel stored under a cache key when the underlying record was deleted.
/// Not valid JSON and not a URL, so it can never be mistaken for real data.
pub const DELETION_TOMBSTONE: &str = "__deleted__";

/// Tombstones must outlive the longest cache TTL (the 24h `query_single` and
/// avatar caches), so no cache entry written before the deletion can outlast
/// its tombstone.
pub const DELETION_TOMBSTONE_TTL_SECS: u64 = ONE_MINUTE_IN_SECONDS * 60 * 24;

static GUARDED_SET_SCRIPT: LazyLock<redis::Script> = LazyLock::new(|| {
	redis::Script::new(
		r"
		if redis.call('GET', KEYS[1]) == ARGV[1] then
			return 0
		end
		redis.call('SET', KEYS[1], ARGV[2], 'EX', ARGV[3])
		return 1
		",
	)
});

static GUARDED_DEL_SCRIPT: LazyLock<redis::Script> = LazyLock::new(|| {
	redis::Script::new(
		r"
		if redis.call('GET', KEYS[1]) == ARGV[1] then
			return 0
		end
		return redis.call('DEL', KEYS[1])
		",
	)
});

/// Canonical form of a name-or-address for building cache keys: addresses get
/// their EIP-55 checksummed form, usernames are lowercased. Every writer and
/// invalidator of the `query_single:`/`avatar:` caches must agree on this.
pub fn canonical_cache_input(name_or_address: &str) -> String {
	Address::from_str(name_or_address).map_or_else(
		|_| name_or_address.to_lowercase(),
		|address| address.to_checksum(None),
	)
}

/// Result of a cache lookup that distinguishes deletion tombstones from plain
/// misses, so callers can avoid trusting lagging read replicas for records
/// that were just deleted.
pub enum Lookup {
	Hit(String),
	/// The key was tombstoned by an account deletion: the row is gone on the
	/// primary, but a read replica may still briefly return it. Callers
	/// backed by a replica must verify against the primary instead.
	Tombstoned,
	Miss,
}

/// Reads a cached value; Redis errors count as a miss.
pub async fn lookup(redis: &mut ConnectionManager, cache_key: &str) -> Lookup {
	match redis.get::<_, Option<String>>(cache_key).await {
		Ok(Some(value)) if value == DELETION_TOMBSTONE => Lookup::Tombstoned,
		Ok(Some(value)) => Lookup::Hit(value),
		Ok(None) | Err(_) => Lookup::Miss,
	}
}

/// Fills a cache entry unless the key holds a deletion tombstone. The check
/// and the write run as one atomic script, so a reader that loaded a row just
/// before its deletion committed cannot resurrect the record in the cache
/// after the deletion's invalidation pass.
pub async fn set_ex_unless_tombstoned(
	redis: &mut ConnectionManager,
	cache_key: &str,
	value: &str,
	ttl_secs: u64,
) -> Result<(), redis::RedisError> {
	let _: i32 = GUARDED_SET_SCRIPT
		.key(cache_key)
		.arg(DELETION_TOMBSTONE)
		.arg(value)
		.arg(ttl_secs)
		.invoke_async(redis)
		.await?;

	Ok(())
}

/// Invalidates a cache entry unless the key holds a deletion tombstone, as
/// one atomic script. Every non-deletion invalidator (rename, profile-picture
/// updates) must use this instead of a plain DEL: an unconditional DEL racing
/// an account deletion could erase the tombstone and reopen the window for a
/// stale reader to resurrect the deleted record.
pub async fn del_unless_tombstoned(
	redis: &mut ConnectionManager,
	cache_key: &str,
) -> Result<(), redis::RedisError> {
	let _: i32 = GUARDED_DEL_SCRIPT
		.key(cache_key)
		.arg(DELETION_TOMBSTONE)
		.invoke_async(redis)
		.await?;

	Ok(())
}

/// Marks a cache key as deleted for [`DELETION_TOMBSTONE_TTL_SECS`].
pub async fn write_tombstone(
	redis: &mut ConnectionManager,
	cache_key: &str,
) -> Result<(), redis::RedisError> {
	redis
		.set_ex::<_, _, ()>(cache_key, DELETION_TOMBSTONE, DELETION_TOMBSTONE_TTL_SECS)
		.await
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn canonical_cache_input_lowercases_usernames() {
		assert_eq!(canonical_cache_input("Alice"), "alice");
		assert_eq!(canonical_cache_input("aLiCe.1234"), "alice.1234");
	}

	#[test]
	fn canonical_cache_input_checksums_addresses() {
		// Any input casing of a valid address maps to the same checksummed key.
		let checksummed = "0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045";
		assert_eq!(
			canonical_cache_input(&checksummed.to_lowercase()),
			checksummed
		);
		assert_eq!(canonical_cache_input(checksummed), checksummed);
	}
}
