use jsonwebtoken::jwk::{Jwk, JwkSet};
use redis::{aio::ConnectionManager, AsyncCommands};
use std::time::Duration;

use super::types::AttestationError;

pub struct JwksCache {
	jwks_url: String,
	ttl: Duration,
	redis: ConnectionManager,
	client: reqwest::Client,
}

impl JwksCache {
	pub fn new(jwks_url: String, ttl: Duration, redis: ConnectionManager) -> Self {
		Self {
			jwks_url,
			ttl,
			redis,
			client: reqwest::Client::new(),
		}
	}

	pub async fn get_key(&self, kid: &str) -> Result<Jwk, AttestationError> {
		let cache_key = format!("jwks:key:{}", kid);

		// Try to get from cache
		let mut redis = self.redis.clone();
		if let Ok(cached) = redis.get::<_, String>(&cache_key).await {
			if let Ok(key) = serde_json::from_str::<Jwk>(&cached) {
				return Ok(key);
			}
		}

		// Fetch from URL
		let jwks = self
			.client
			.get(&self.jwks_url)
			.send()
			.await
			.map_err(|e| AttestationError::JwksFetchError(e.to_string()))?
			.json::<JwkSet>()
			.await
			.map_err(|e| AttestationError::JwksFetchError(e.to_string()))?;

		// Find the key using JwkSet's built-in find method
		let key = jwks
			.find(kid)
			.cloned()
			.ok_or_else(|| AttestationError::KeyNotFound(kid.to_string()))?;

		// Cache it
		let serialized = serde_json::to_string(&key).unwrap();
		let _: Result<(), _> = redis
			.set_ex(&cache_key, serialized, self.ttl.as_secs() as u64)
			.await;

		Ok(key)
	}
}
