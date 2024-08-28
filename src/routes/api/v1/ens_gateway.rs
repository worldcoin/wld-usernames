use alloy_primitives::keccak256;
use alloy_primitives::Address;
use alloy_signer::Signer;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{eip712_domain, SolCall, SolValue};
use axum::Extension;
use axum_jsonschema::Json;
use chrono::{TimeDelta, Utc};
use num_traits::FromPrimitive;
use ruint::Uint;
use sqlx::PgPool;
use std::env;
use std::str::FromStr;

use crate::types::Method;
use crate::{
    types::{
        ENSErrorResponse, ENSQueryPayload, ENSResponse, GatewayResponse, Name, ResolveRequest,
    },
    utils::namehash,
};

pub async fn ens_gateway(
    Extension(db): Extension<PgPool>,
    Json(request_payload): Json<ENSQueryPayload>,
) -> Result<Json<ENSResponse>, ENSErrorResponse> {
    let (req_data, name, method) = decode_payload(&request_payload)
        .map_err(|_| ENSErrorResponse::new("Failed to decode payload."))?;

    let username = name
        .strip_suffix(&format!(".{}", env::var("ENS_DOMAIN").unwrap()))
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
        }
        Method::Addr(node) => {
            if node != namehash(&name) {
                return Err(ENSErrorResponse::new("Invalid node hash provided."));
            }

            (Address::parse_checksummed(record.address, None).unwrap()).abi_encode()
        }
        Method::AddrMultichain | Method::Name => {
            return Err(ENSErrorResponse::new("Not implemented."));
        }
        _ => ().abi_encode(),
    };

    sign_response(result, &req_data, request_payload.sender)
        .await
        .map(|data| Json(ENSResponse { data }))
        .map_err(|_| ENSErrorResponse::new("Failed to sign response."))
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

    let signer = PrivateKeySigner::from_str(&env::var("ENS_SIGNER_PRIVATE_KEY").unwrap()).unwrap();

    let data = GatewayResponse {
        expires_at,
        sender: sender.0,
        response_hash: keccak256(&response),
        request_hash: keccak256(request_data),
    };

    let domain = eip712_domain! {
        name: "World App Usernames",
        version: "1.0.0",
        chain_id: 1,
        verifying_contract: env::var("ENS_RESOLVER_ADDRESS").unwrap().parse().unwrap(),
        salt: keccak256(env::var("ENS_RESOLVER_SALT").unwrap()),
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
