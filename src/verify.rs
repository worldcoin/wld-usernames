use backon::{ExponentialBuilder, Retryable};
use idkit::{hashing::hash_to_field, session::VerificationLevel, Proof};
use reqwest::{header, StatusCode};
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("verification failed: {0:?}")]
	Verification(ErrorResponse),
	#[error("fail to send request: {0}")]
	Reqwest(#[from] reqwest::Error),
	#[error("failed to decode response: {0}")]
	Serde(#[from] serde_json::Error),
	#[error("unexpected response: {status}, body: {body}")]
	InvalidResponse { status: StatusCode, body: String },
}

#[derive(Debug, serde::Deserialize)]
pub struct ErrorResponse {
	pub code: String,
	pub detail: String,
	pub attribute: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
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
	let packed = signal.abi_encode_packed();
	let signal_hash = if packed.is_empty() {
		None
	} else {
		Some(format!("0x{:x}", hash_to_field(&packed)))
	};

	let body = VerificationRequest {
		action: action.to_owned(),
		proof: proof.proof.clone(),
		merkle_root: proof.merkle_root.clone(),
		nullifier_hash: proof.nullifier_hash.clone(),
		verification_level: proof.verification_level,
		signal_hash,
	};

	let client = reqwest::Client::new();
	let url = format!("{developer_portal_url}/api/v2/verify/{app_id}");

	let policy = ExponentialBuilder::default()
		.with_min_delay(Duration::from_millis(100))
		.with_max_delay(Duration::from_secs(2))
		.with_jitter()
		.with_max_times(3);

	let attempt = || async {
		let resp = client
			.post(&url)
			.header(header::USER_AGENT, "idkit-rs")
			.json(&body)
			.send()
			.await
			.map_err(Error::Reqwest)?;

		match resp.status() {
			StatusCode::OK => Ok(()),
			StatusCode::BAD_REQUEST => {
				let err = resp.json::<ErrorResponse>().await.map_err(Error::Reqwest)?;
				Err(Error::Verification(err))
			},
			status => {
				let text = resp.text().await.unwrap_or_default();
				Err(Error::InvalidResponse { status, body: text })
			},
		}
	};

	attempt
		.retry(policy)
		.sleep(tokio::time::sleep)
		.when(
			|e: &Error| matches!(e, Error::InvalidResponse { status, .. } if status.is_server_error()),
		)
		.await?;

	Ok(())
}
