use alloy::{
	primitives::{keccak256, Address, U64},
	signers::{local::PrivateKeySigner, Signature, Signer},
	sol_types::{SolCall, SolValue},
};
use axum::{
	body::Bytes,
	extract::{Extension, Path},
};
use axum_jsonschema::Json;
use chrono::{TimeDelta, Utc};
use serde_json::from_slice;
use std::{str::FromStr, sync::Arc};
use tracing::{info_span, Instrument};

use crate::{
	config::{Config, ConfigExt, Db},
	types::{ENSErrorResponse, ENSQueryPayload, ENSResponse, Method, Name, ResolveRequest},
	utils::namehash,
};

pub async fn ens_gateway_post(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
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
	process_ens_request(config, db, request_payload).await
}

pub async fn ens_gateway_get(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Path((sender, data)): Path<(String, String)>,
) -> Result<Json<ENSResponse>, ENSErrorResponse> {
	let sender_address = crate::types::Address(
		Address::from_str(&sender).map_err(|_| ENSErrorResponse::new("Invalid sender address."))?,
	);

	let request_payload = ENSQueryPayload {
		sender: sender_address,
		data,
	};

	process_ens_request(config, db, request_payload).await
}

async fn process_ens_request(
	config: Arc<Config>,
	db: Db,
	request_payload: ENSQueryPayload,
) -> Result<Json<ENSResponse>, ENSErrorResponse> {
	let (req_data, name, method) = decode_payload(&request_payload)
		.map_err(|_| ENSErrorResponse::new("Failed to decode payload."))?;

	let username = name
		.strip_suffix(&format!(".{}", config.ens_domain))
		.ok_or_else(|| ENSErrorResponse::new("Name not found."))?;

	let record = sqlx::query_as!(Name, "SELECT * FROM names WHERE username = $1", username)
		.fetch_one(&db.read_only)
		.instrument(info_span!("ens_gateway_query_name", username = username))
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
				// hack to hide etherscan error
				"email" => "".to_string().abi_encode(),
				"url" => "".to_string().abi_encode(),

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
	let data = if payload.data.ends_with(".json") {
		&payload.data[2..payload.data.len() - 5]
	} else {
		&payload.data[2..]
	};
	let req_data = hex::decode(data)?;
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
