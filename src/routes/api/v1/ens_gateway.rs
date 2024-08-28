use alloy::{
	primitives::{keccak256, Address},
	signers::{aws::AwsSigner, Signer},
	sol_types::{eip712_domain, SolCall, SolValue},
};
use axum::Extension;
use axum_jsonschema::Json;
use chrono::{TimeDelta, Utc};
use num_traits::FromPrimitive;
use ruint::Uint;
use sqlx::PgPool;
use std::sync::Arc;

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
	Extension(kms_client): Extension<aws_sdk_kms::Client>,
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
			if node != namehash(&name) {
				return Err(ENSErrorResponse::new("Invalid node hash provided."));
			}

			// Support for other will be implemented in the future.
			if key != "avatar" {
				return Err(ENSErrorResponse::new(&format!("Record not found: {key}",)));
			}

			todo!("Add avatar support");
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

	sign_response(
		kms_client,
		config,
		result,
		&req_data,
		request_payload.sender,
	)
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
	kms_client: aws_sdk_kms::Client,
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

	let signer = AwsSigner::new(kms_client, config.kms_key_id.clone(), None).await?;

	let data = GatewayResponse {
		expires_at,
		sender: sender.0,
		response_hash: keccak256(&response),
		request_hash: keccak256(request_data),
	};

	let domain = eip712_domain! {
		name: "World App Usernames",
		version: "1.0.0",
		chain_id: config.ens_chain_id,
		verifying_contract: sender.0,
		salt: config.ens_resolver_salt,
	};

	let signature = signer
		.sign_typed_data(&data, &domain)
		.await?
		.inner()
		.to_bytes()
		.to_vec();

	Ok(format!(
		"0x{}",
		hex::encode((response, expires_at, signature).abi_encode())
	))
}
