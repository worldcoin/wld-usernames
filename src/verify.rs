use reqwest::{header, StatusCode};
use serde::Serialize;

use idkit::{hashing::hash_to_field, session::VerificationLevel, Proof};

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("verification failed: {0:?}")]
	Verification(ErrorResponse),
	#[error("fail to send request: {0}")]
	Reqwest(#[from] reqwest::Error),
	#[error("failed to decode response: {0}")]
	Serde(#[from] serde_json::Error),
	#[error("unexpected response")]
	InvalidResponse(reqwest::Response),
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)] // Fields are used on the HTTP response
pub struct ErrorResponse {
	pub code: String,
	pub detail: String,
	pub attribute: Option<String>,
}

#[derive(Debug, Serialize)]
struct VerificationRequest {
	action: String,
	proof: String,
	merkle_root: String,
	nullifier_hash: String,
	verification_level: VerificationLevel,
	#[serde(skip_serializing_if = "Option::is_none")]
	signal_hash: Option<String>,
}

/// Verify a World ID proof using the Developer Portal API.
///
/// # Errors
///
/// Errors if the proof is invalid (`Error::Verification`), or if there's an error validating the proof.
pub async fn dev_portal_verify_proof<V: alloy::sol_types::SolValue + Send>(
	proof: Proof,
	app_id: String,
	action: &str,
	signal: V,
	developer_portal_url: String,
) -> Result<(), Error> {
	let signal = signal.abi_encode_packed();

	let response = reqwest::Client::new()
		.post(format!("{developer_portal_url}/api/v2/verify/{app_id}"))
		.header(header::USER_AGENT, "idkit-rs")
		.json(&VerificationRequest {
			proof: proof.proof,
			signal_hash: if signal.is_empty() {
				None
			} else {
				Some(format!("0x{:x}", hash_to_field(&signal)))
			},
			action: action.to_string(),
			merkle_root: proof.merkle_root,
			nullifier_hash: proof.nullifier_hash,
			verification_level: proof.verification_level,
		})
		.send()
		.await?;

	match response.status() {
		StatusCode::OK => Ok(()),
		StatusCode::BAD_REQUEST => {
			Err(Error::Verification(response.json::<ErrorResponse>().await?))
		},
		_ => Err(Error::InvalidResponse(response)),
	}
}
