use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwksKey {
	pub kid: String,
	pub kty: String,
	pub alg: String,
	#[serde(flatten)]
	pub key_data: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jwks {
	pub keys: Vec<JwksKey>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AttestationClaims {
	pub jti: String,
	pub exp: i64,
	pub iat: i64,
	#[serde(flatten)]
	pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum AttestationError {
	#[error("Missing attestation token")]
	MissingToken,

	#[error("Invalid token format: {0}")]
	InvalidToken(String),

	#[error("Missing kid in token header")]
	MissingKid,

	#[error("Failed to fetch JWKS: {0}")]
	JwksFetchError(String),

	#[error("Key not found for kid: {0}")]
	KeyNotFound(String),

	#[error("Token signature verification failed: {0}")]
	SignatureVerificationFailed(String),

	#[error("Request hash mismatch")]
	HashMismatch,

	#[error("Failed to hash request: {0}")]
	HashError(String),
}

impl IntoResponse for AttestationError {
	fn into_response(self) -> Response {
		let status = match self {
			// 401 UNAUTHORIZED - Authentication failures
			Self::MissingToken
			| Self::KeyNotFound(_)
			| Self::SignatureVerificationFailed(_)
			| Self::HashMismatch => StatusCode::UNAUTHORIZED,

			// 400 BAD_REQUEST - Client errors
			Self::InvalidToken(_) | Self::MissingKid | Self::HashError(_) => {
				StatusCode::BAD_REQUEST
			},

			// 500 INTERNAL_SERVER_ERROR - Server errors
			Self::JwksFetchError(_) => StatusCode::INTERNAL_SERVER_ERROR,
		};

		let body = serde_json::json!({
			"error": self.to_string(),
		});

		(status, axum::Json(body)).into_response()
	}
}
