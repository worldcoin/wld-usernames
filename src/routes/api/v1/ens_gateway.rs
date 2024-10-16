use alloy::{
	primitives::{keccak256, Address},
	signers::{local::PrivateKeySigner, Signer},
	sol_types::{eip712_domain, SolCall, SolValue},
};
use axum::Extension;
use axum_jsonschema::Json;
use chrono::{TimeDelta, Utc};
use num_traits::FromPrimitive;
use ruint::Uint;
use sqlx::PgPool;
use std::{str::FromStr, sync::Arc};

use crate::{
	config::{Config, ConfigExt},
	types::{
		ENSErrorResponse, ENSQueryPayload, ENSResponse, GatewayResponse, Method, Name,
		ResolveRequest,
	},
	utils::namehash,
};

pub async fn ens_gateway(
	Extension(config): ConfigExt,
	Extension(db): Extension<PgPool>,
	Json(request_payload): Json<ENSQueryPayload>,
) -> Result<Json<ENSResponse>, ENSErrorResponse> {
	let (req_data, name, method) = decode_payload(&request_payload)
		.map_err(|_| ENSErrorResponse::new("Failed to decode payload."))?;

	let username = name
		.strip_suffix(&format!(".{}", config.ens_domain))
		.ok_or_else(|| ENSErrorResponse::new("Name not found."))?;

	let record = sqlx::query_as!(Name, "SELECT * FROM names WHERE username = $1", username)
		.fetch_one(&db)
		.await
		.map_err(|_| ENSErrorResponse::new("Name not found."))?;

	let result = match method {
		Method::Text(node, key) => {
			tracing::info!("Text: {:?}", record.address);

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
			tracing::info!("Address: {:?}", record.address);

			(Address::parse_checksummed(record.address, None).unwrap()).abi_encode()
		},
		Method::AddrMultichain | Method::Name => {
			tracing::info!("AddrMultichain: {:?}", record.address);

			return Err(ENSErrorResponse::new("Not implemented."));
		},
		_ => {
			tracing::info!("No Method: {:?}", record.address);
			().abi_encode()
		},
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
	let expires_at = Uint::from_i64(
		Utc::now()
			.checked_add_signed(TimeDelta::hours(1))
			.unwrap()
			.timestamp(),
	)
	.unwrap();

	let signer = PrivateKeySigner::from_str(&config.private_key).unwrap();

	let data = GatewayResponse {
		sender: sender.0,
		expiresAt: expires_at,
		responseHash: keccak256(&response),
		requestHash: keccak256(request_data),
	};

	let domain = eip712_domain! {
		name: "World App Usernames",
		version: "1",
		chain_id: config.ens_chain_id,
		verifying_contract: sender.0,
	};

	let signature = signer
		.sign_typed_data(&data, &domain)
		.await?
		.inner()
		.to_bytes()
		.to_vec();

	Ok(format!(
		"0x{}",
		hex::encode((response, expires_at, signature).abi_encode_params())
	))
}
