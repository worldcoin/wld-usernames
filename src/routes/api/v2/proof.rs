use alloy::{
	primitives::U256,
	signers::{local::PrivateKeySigner, SignerSync},
};
use idkit::hashing::hash_to_field;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::str::FromStr;

use super::Challenge;
use crate::{config::Config, types::ErrorResponse};

pub const OWNER_ACTION: &str = "world_usernames:v2:owner";

/// The signer is registered for this RP; never reuse the ENS signing key.
pub struct Settings {
	pub rp_id: String,
	pub signer: PrivateKeySigner,
}

impl std::fmt::Debug for Settings {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Settings")
			.field("rp_id", &self.rp_id)
			.finish_non_exhaustive()
	}
}

impl Settings {
	pub fn from_env() -> anyhow::Result<Option<Self>> {
		let Ok(rp_id) = std::env::var("USERNAME_V4_RP_ID") else {
			return Ok(None);
		};
		if rp_id.len() != 19
			|| !rp_id.starts_with("rp_")
			|| u64::from_str_radix(&rp_id[3..], 16).is_err()
		{
			anyhow::bail!("USERNAME_V4_RP_ID must be a registered rp_ identifier");
		}
		let signer = PrivateKeySigner::from_str(&std::env::var("USERNAME_V4_RP_SIGNING_KEY")?)?;
		Ok(Some(Self { rp_id, signer }))
	}

	pub(super) fn request(&self, challenge: &Challenge) -> Result<Value, ErrorResponse> {
		let nonce = field(&challenge.nonce)?;
		let unique = challenge.session_id.is_none();
		let action = hash(OWNER_ACTION.as_bytes());
		// Protocol RP signature v1: version || nonce || timestamps || optional action.
		let mut message = vec![1];
		message.extend(hex::decode(&nonce[2..])?);
		message.extend(challenge.created_at.to_be_bytes());
		message.extend(challenge.expires_at.to_be_bytes());
		if unique {
			message.extend(hex::decode(&action[2..])?);
		}
		let signature = self.signer.sign_message_sync(&message)?;
		let rp = u64::from_str_radix(&self.rp_id[3..], 16)?;
		Ok(json!({
			"id": challenge.id,
			"version": 1,
			"proof_type": if unique { "uniqueness" } else { "session" },
			"rp_id": self.rp_id,
			"oprf_key_id": format!("0x{rp:040x}"),
			"created_at": challenge.created_at,
			"expires_at": challenge.expires_at,
			"nonce": nonce,
			"action": if unique { Some(action) } else { None },
			"session_id": challenge.session_id.as_deref().unwrap_or("create"),
			"signature": format!("0x{}", hex::encode(signature.as_bytes())),
			"proof_requests": [{
				"identifier": "orb",
				"issuer_schema_id": 1,
				"signal": format!("0x{}", hex::encode(challenge.signal.as_bytes())),
				"genesis_issued_at_min": 0,
				"expires_at_min": challenge.created_at,
			}],
		}))
	}
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
	pub session_id: String,
	pub response: Response,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
	pub proof: [String; 5],
	pub expires_at_min: u64,
	pub nullifier: Option<String>,
	pub session_nullifier: Option<[String; 2]>,
}

pub fn hash(bytes: &[u8]) -> String {
	format!("0x{:064x}", hash_to_field(bytes))
}

pub fn field(value: &str) -> Result<String, ErrorResponse> {
	let digits = value.strip_prefix("0x").unwrap_or(value);
	if digits.is_empty() || digits.len() > 64 || !digits.bytes().all(|v| v.is_ascii_hexdigit()) {
		return Err(ErrorResponse::bad_request("Invalid field element"));
	}
	let value = U256::from_str_radix(digits, 16)?;
	let modulus = U256::from_str_radix(
		"30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000001",
		16,
	)?;
	if value >= modulus {
		return Err(ErrorResponse::bad_request("Field element out of range"));
	}
	Ok(format!("0x{value:064x}"))
}

pub fn valid_session(value: &str) -> bool {
	value.len() == 136
		&& value.starts_with("session_")
		&& value.as_bytes()[8..].iter().all(u8::is_ascii_hexdigit)
		&& &value[72..74] == "01"
		&& field(&value[8..72]).is_ok_and(|v| v != format!("0x{:064x}", 0))
		&& field(&value[72..]).is_ok()
}

/// Use only the server's RP, environment, nonce, signal, action and issuer.
/// A client's success flag, credential identifier or proof version is never authority.
pub(super) async fn verify(
	config: &Config,
	challenge: &Challenge,
	proof: &Proof,
) -> Result<Option<String>, ErrorResponse> {
	let settings = config.world_id_v4.as_ref().ok_or_else(|| {
		ErrorResponse::service_unavailable("World ID 4 usernames are not configured".into())
	})?;
	if !valid_session(&proof.session_id) {
		return Err(ErrorResponse::bad_request("Invalid session ID"));
	}
	if proof.response.expires_at_min < u64::try_from(challenge.created_at)? {
		return Err(ErrorResponse::bad_request(
			"Credential expiration constraint is stale",
		));
	}
	let unique = challenge.session_id.is_none();
	let owner = if unique {
		if proof.response.session_nullifier.is_some() {
			return Err(ErrorResponse::bad_request("Expected a uniqueness proof"));
		}
		Some(field(proof.response.nullifier.as_deref().ok_or_else(
			|| ErrorResponse::bad_request("Missing owner nullifier"),
		)?)?)
	} else {
		if challenge.session_id.as_ref() != Some(&proof.session_id)
			|| proof.response.nullifier.is_some()
		{
			return Err(ErrorResponse::unauthorized(
				"Session does not own this username".into(),
			));
		}
		if proof.response.session_nullifier.is_none() {
			return Err(ErrorResponse::bad_request("Missing session nullifier"));
		}
		None
	};
	let environment = match config.environment {
		crate::config::Environment::Production => "production",
		_ => "staging",
	};
	let mut response = json!({
		"identifier": "orb",
		"issuer_schema_id": 1,
		"signal_hash": hash(challenge.signal.as_bytes()),
		"expires_at_min": proof.response.expires_at_min,
		"credential_genesis_issued_at_min": 0,
		"proof": proof.response.proof,
	});
	if let Some(owner) = &owner {
		response["nullifier"] = json!(owner);
	} else {
		response["session_nullifier"] = json!(proof.response.session_nullifier);
	}
	let mut request = json!({
		"protocol_version": "4.0",
		"min_protocol_version": "4.0",
		"proof_type": if unique { "uniqueness" } else { "session" },
		"nonce": challenge.nonce,
		"session_id": proof.session_id,
		"environment": environment,
		"responses": [response],
	});
	if unique {
		request["action"] = json!(OWNER_ACTION);
	}
	let result = reqwest::Client::builder()
		.timeout(std::time::Duration::from_secs(15))
		.build()?
		.post(format!(
			"{}/api/v4/verify/{}",
			config.developer_portal_url.trim_end_matches('/'),
			settings.rp_id
		))
		.json(&request)
		.send()
		.await?;
	if result.status().is_server_error() {
		return Err(ErrorResponse::service_unavailable(
			"World ID verification unavailable".into(),
		));
	}
	if !result.status().is_success() {
		return Err(ErrorResponse::unauthorized("Invalid World ID proof".into()));
	}
	let result: Value = result.json().await?;
	if result["success"] != true
		|| result["protocol_version"] != "4.0"
		|| result["session_id"] != proof.session_id
		|| result["environment"] != environment
		|| (unique && result["action"] != OWNER_ACTION)
	{
		return Err(ErrorResponse::unauthorized(
			"Verifier did not establish the requested ownership".into(),
		));
	}
	if let Some(owner) = &owner {
		if result["nullifier"]
			.as_str()
			.map(field)
			.transpose()?
			.as_ref() != Some(owner)
		{
			return Err(ErrorResponse::unauthorized(
				"Verifier ownership mismatch".into(),
			));
		}
	}
	Ok(owner)
}
