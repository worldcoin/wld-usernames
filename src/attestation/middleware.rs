use axum::{body::Body, extract::Request, http::HeaderMap, middleware::Next, response::Response};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use std::sync::Arc;

use super::jwks_cache::JwksCache;
use super::request_hasher::hash_request;
use super::types::{AttestationClaims, AttestationError};

const ATTESTATION_TOKEN_HEADER: &str = "attestation-gateway-token";

/// Attestation verification middleware for multipart form data requests
/// Verifies JWT signature and compares JTI with SHA256(metadata_json)
pub async fn attestation_middleware(
	jwks_cache: Arc<JwksCache>,
	headers: HeaderMap,
	request: Request,
	next: Next,
) -> Result<Response, AttestationError> {
	// Extract attestation token from header
	let token = headers
		.get(ATTESTATION_TOKEN_HEADER)
		.and_then(|v| v.to_str().ok())
		.ok_or_else(|| {
			tracing::warn!("Missing attestation token");
			AttestationError::MissingToken
		})?;

	tracing::debug!("Received attestation token");

	// Step 1: Parse token header to get kid (without verification)
	let header = decode_header(token).map_err(|e| {
		tracing::warn!("Failed to decode token header: {e}");
		AttestationError::InvalidToken(format!("Failed to decode header: {e}"))
	})?;

	let kid = header.kid.ok_or_else(|| {
		tracing::warn!("Missing kid in token header");
		AttestationError::MissingKid
	})?;

	tracing::debug!("Token kid: {kid}");

	// Step 2: Get key from cache (or fetch if not cached)
	let jwk = jwks_cache.get_key(&kid).await?;

	// Step 3: Verify the JWT signature
	let alg: Algorithm = jwk.alg.parse().map_err(|_| {
		tracing::warn!("Unsupported algorithm: {}", jwk.alg);
		AttestationError::InvalidToken(format!("Unsupported algorithm: {}", jwk.alg))
	})?;

	// Convert JWK to DecodingKey
	let decoding_key = jwk_to_decoding_key(&jwk).map_err(|e| {
		tracing::warn!("Failed to create decoding key: {e}");
		e
	})?;

	let mut validation = Validation::new(alg);
	validation.validate_exp = true;

	let token_data =
		decode::<AttestationClaims>(token, &decoding_key, &validation).map_err(|e| {
			tracing::warn!("Token verification failed: {e}");
			AttestationError::SignatureVerificationFailed(e.to_string())
		})?;

	tracing::debug!("Token signature verified");

	// Step 4: Extract request body for hashing
	let (parts, body) = request.into_parts();
	let body_bytes = axum::body::to_bytes(body, usize::MAX).await.map_err(|e| {
		tracing::warn!("Failed to read request body: {e}");
		AttestationError::HashError(format!("Failed to read body: {e}"))
	})?;

	// Step 5: Hash the request (extracts and hashes only metadata field)
	let request_hash = hash_request(&headers, body_bytes.clone())
		.await
		.map_err(|e| {
			tracing::warn!("Failed to hash request: {e}");
			e
		})?;

	tracing::debug!("Computed request hash: {request_hash}");
	tracing::debug!("Token JTI claim: {}", token_data.claims.jti);

	// Step 6: Compare JTI with request hash
	if token_data.claims.jti != request_hash {
		tracing::warn!("JTI mismatch - token JTI does not match request hash");
		return Err(AttestationError::HashMismatch);
	}

	tracing::info!("Attestation verification successful");

	// Reconstruct request with the body we consumed
	let request = Request::from_parts(parts, Body::from(body_bytes));

	Ok(next.run(request).await)
}

/// Convert JWK to DecodingKey
fn jwk_to_decoding_key(jwk: &super::jwks_cache::JwksKey) -> Result<DecodingKey, AttestationError> {
	// Serialize the JWK back to JSON for the jsonwebtoken library
	let jwk_json = serde_json::to_string(jwk)
		.map_err(|e| AttestationError::InvalidToken(format!("JWK serialization error: {e}")))?;

	DecodingKey::from_jwk(&serde_json::from_str(&jwk_json).unwrap())
		.map_err(|e| AttestationError::InvalidToken(format!("Invalid JWK: {e}")))
}
