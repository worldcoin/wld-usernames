use alloy::{
	primitives::{keccak256, Address, U64},
	signers::{local::PrivateKeySigner, Signature, Signer},
	sol_types::{SolCall, SolValue},
};
use axum::{body::Bytes, extract::Extension};
use axum_jsonschema::Json;
use chrono::{TimeDelta, Utc};
use serde_json::from_slice;
use sqlx::PgPool;
use std::{str::FromStr, sync::Arc};

use crate::{
	config::{Config, ConfigExt},
	types::{ENSErrorResponse, ENSQueryPayload, ENSResponse, Method, Name, ResolveRequest},
	utils::namehash,
};

pub async fn ens_gateway(
	Extension(config): ConfigExt,
	Extension(db): Extension<PgPool>,
	body: Bytes, // Accept the raw request body as Bytes
) -> Result<Json<ENSResponse>, ENSErrorResponse> {
	// TODO: Remove these after figuring out what ENS is failing on
	let request_payload: ENSQueryPayload = match from_slice(&body) {
		Ok(payload) => payload, // Successfully parsed
		Err(_) => {
			// Return an error response if JSON parsing fails
			return Err(ENSErrorResponse::new("Failed to parse JSON payload."));
		},
	};

	let (req_data, name, method) = decode_payload(&request_payload)
		.map_err(|_| ENSErrorResponse::new("Failed to decode payload."))?;

	let username = name
		.strip_suffix(&format!(".{}", config.ens_domain))
		.ok_or_else(|| ENSErrorResponse::new("Name not found."))?;

	let record = sqlx::query_as!(Name, "SELECT * FROM names WHERE username = $1", username)
		.fetch_one(&db)
		.await
		.map_err(|_| ENSErrorResponse::new("Name not found."))?;

	let result: Vec<u8> = match method {
		Method::Text(node, key) => {
			if node != namehash(&name) {
				return Err(ENSErrorResponse::new("Invalid node hash provided."));
			}

			match key.as_str() {
				"avatar" => {
					let Some(avatar_url) = record.profile_picture_url else {
						return Err(ENSErrorResponse::new(&format!("Record not found: {key}")));
					};

					(avatar_url).abi_encode()
				},
				// Support for other might be implemented in the future.
				_ => return Err(ENSErrorResponse::new(&format!("Record not found: {key}"))),
			}
		},
		Method::Addr(node) => {
			if node != namehash(&name) {
				return Err(ENSErrorResponse::new("Invalid node hash provided."));
			}

			(Address::parse_checksummed(record.address, None).unwrap()).abi_encode()
		},
		Method::AddrMultichain | Method::Name => {
			return Err(ENSErrorResponse::new("Not implemented."));
		},
		_ => ().abi_encode(),
	};

	sign_response(config, result, &req_data, request_payload.sender)
		.await
		.map(|data| Json(ENSResponse { data }))
		.map_err(|_| ENSErrorResponse::new("Failed to sign response."))
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("CCIP Read Gateway powering the ENS integration.")
}

fn decode_payload(payload: &ENSQueryPayload) -> Result<(Vec<u8>, String, Method), anyhow::Error> {
	let req_data = hex::decode(&payload.data[2..])?;
	let decoded_req = ResolveRequest::abi_decode(&req_data, true)?;

	Ok((
		req_data,
		decoded_req.parse_name()?,
		decoded_req.parse_method()?,
	))
}

async fn sign_response(
	config: Arc<Config>,
	response: Vec<u8>,
	request_data: &[u8],
	sender: crate::types::Address,
) -> Result<String, anyhow::Error> {
	let expires_at = Utc::now()
		.checked_add_signed(TimeDelta::hours(1))
		.unwrap()
		.timestamp();

	let signer = PrivateKeySigner::from_str(&config.private_key).unwrap();

	let data: Vec<u8> = (
		[0x19u8, 0x00u8],
		sender.0,
		U64::from(expires_at).to_be_bytes_vec(),
		keccak256(request_data).to_vec(),
		keccak256(&response).to_vec(),
	)
		.abi_encode_packed();

	let signature: Signature = signer.sign_hash(&keccak256(data)).await?;

	Ok(format!(
		"0x{}",
		hex::encode((response, expires_at, signature.as_bytes().to_vec()).abi_encode_params())
	))
}
