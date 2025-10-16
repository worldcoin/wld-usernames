use redis::{aio::ConnectionManager, AsyncCommands};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::types::AttestationError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwksKey {
	pub kty: String,
	pub kid: String,
	pub alg: String,
	pub n: String,
	pub e: String,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
	keys: Vec<JwksKey>,
}

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

	pub async fn get_key(&self, kid: &str) -> Result<JwksKey, AttestationError> {
		let cache_key = format!("jwks:key:{}", kid);

		// Try to get from cache
		let mut redis = self.redis.clone();
		if let Ok(cached) = redis.get::<_, String>(&cache_key).await {
			if let Ok(key) = serde_json::from_str::<JwksKey>(&cached) {
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
			.json::<JwksResponse>()
			.await
			.map_err(|e| AttestationError::JwksFetchError(e.to_string()))?;

		// Find the key
		let key = jwks
			.keys
			.into_iter()
			.find(|k| k.kid == kid)
			.ok_or_else(|| AttestationError::KeyNotFound(kid.to_string()))?;

		// Cache it
		let serialized = serde_json::to_string(&key).unwrap();
		let _: Result<(), _> = redis
			.set_ex(&cache_key, serialized, self.ttl.as_secs() as u64)
			.await;

		Ok(key)
	}
}
