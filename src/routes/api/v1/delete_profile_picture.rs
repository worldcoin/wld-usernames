use aide::transform::TransformOperation;
use axum::{extract::Query, http::StatusCode, Extension};
use redis::{aio::ConnectionManager, AsyncCommands};
use serde_json::Value;
use tracing::{info, info_span, warn, Instrument};

use crate::{
	config::{ConfigExt, Db},
	deletion,
	routes::api::v1::query_single::validate_address,
	types::{DeleteProfilePicturePayload, ErrorResponse, Name},
	verify,
};

/*
	Deprecated
	This endpoint will no longer be used as the profile picture endpoint
	We will move to a different flow.
*/

const PROOF_HEX_LEN: usize = 64 * 8;
const HASH_HEX_LEN: usize = 64;

fn is_hex_with_prefix(value: &str, expected_len: usize) -> bool {
	let Some(hex) = value.strip_prefix("0x") else {
		return false;
	};

	if hex.len() != expected_len {
		return false;
	}

	hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_uint_string(value: &str) -> bool {
	if value.is_empty() {
		return false;
	}

	if let Some(hex) = value.strip_prefix("0x") {
		return !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit());
	}

	value.chars().all(|c| c.is_ascii_digit())
}

fn is_json_uint256_array(value: &str) -> bool {
	let Ok(values) = serde_json::from_str::<Vec<Value>>(value) else {
		return false;
	};

	if values.len() != 8 {
		return false;
	}

	values.iter().all(|entry| match entry {
		Value::String(value) => is_uint_string(value),
		Value::Number(number) => number.as_u64().is_some(),
		_ => false,
	})
}

fn proof_fields_valid(proof: &str, merkle_root: &str, nullifier_hash: &str) -> bool {
	if !is_hex_with_prefix(merkle_root, HASH_HEX_LEN) {
		return false;
	}

	if !is_hex_with_prefix(nullifier_hash, HASH_HEX_LEN) {
		return false;
	}

	is_hex_with_prefix(proof, PROOF_HEX_LEN) || is_json_uint256_array(proof)
}

#[allow(dependency_on_unit_never_type_fallback)]
/// This endpoint uses a proof for authentication
/// Deletes a user-uploaded profile picture and reverts it to the default marble image.
pub async fn delete_profile_picture(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	Query(payload): Query<DeleteProfilePicturePayload>,
) -> Result<StatusCode, ErrorResponse> {
	let proof = payload.into_proof();

	if !proof_fields_valid(&proof.proof, &proof.merkle_root, &proof.nullifier_hash) {
		tracing::warn!(
			address = %payload.address,
			"Rejected delete profile picture with invalid proof format"
		);
		return Err(ErrorResponse::bad_request("invalid_proof"));
	}

	match verify::dev_portal_verify_proof(
		proof,
		config.wld_app_id.to_string(),
		"username",
		payload.address.clone(),
		config.developer_portal_url.clone(),
	)
	.await
	{
		Ok(()) => {},
		Err(verify::Error::Verification(e)) => {
			tracing::error!(
				"Delete Profile Picture Verification Error: {}, payload:{:?}",
				e.detail,
				payload
			);
			return Err(ErrorResponse::validation_error(e.detail));
		},
		Err(e) => {
			tracing::error!(
				"Delete Profile Picture Server Error: {}, payload:{:?}",
				e.to_string(),
				payload
			);
			return Err(ErrorResponse::server_error(
				"Failed to verify World ID proof".to_string(),
			));
		},
	}

	let address_checksum = validate_address(&payload.address);

	let Some(record) = sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE address = $1",
		address_checksum.clone()
	)
	.fetch_optional(&db.read_only)
	.instrument(info_span!("delete_profile_picture_fetch_record"))
	.await?
	else {
		return Err(ErrorResponse::not_found(
			"Username not found for wallet address".to_string(),
		));
	};

	if record.nullifier_hash != payload.nullifier_hash {
		return Err(ErrorResponse::unauthorized(
			"You can't update this profile picture".to_string(),
		));
	}

	let Name {
		address,
		username,
		profile_picture_url,
		minimized_profile_picture_url,
		..
	} = record;

	let cdn_base_url = std::env::var("PROFILE_PICTURE_CDN_URL").map_err(|_| {
		warn!("PROFILE_PICTURE_CDN_URL environment variable not set");
		ErrorResponse::server_error("Configuration error".to_string())
	})?;

	let marble_base_url = std::env::var("MARBLE_CDN_URL").map_err(|_| {
		warn!("MARBLE_CDN_URL environment variable not set");
		ErrorResponse::server_error("Configuration error".to_string())
	})?;

	let marble_url = format!(
		"{}/{}.png",
		marble_base_url.trim_end_matches('/'),
		address.to_lowercase()
	);
	// We use the existing schema for minimized and verified
	let minimized_marble_url = format!(
		"{}/minimized_{}.png",
		marble_base_url.trim_end_matches('/'),
		address.to_lowercase()
	);

	// If current URL is the marble we can skip the update
	if profile_picture_url.as_ref() == Some(&marble_url) {
		info!(
			address = %address,
			username = %username,
			"Profile picture already set to marble, no action taken"
		);
		return Ok(StatusCode::NO_CONTENT);
	}

	sqlx::query!(
		"UPDATE names SET profile_picture_url = $1, minimized_profile_picture_url = $2 WHERE address = $3",
		Some(marble_url.clone()),
		Some(minimized_marble_url.clone()),
		&address
	)
	.execute(&db.read_write)
	.instrument(info_span!(
		"delete_profile_picture_update_record",
		address = %address
	))
	.await?;

	if let Some(url) = profile_picture_url.as_deref() {
		deletion::mark_object_for_deletion(config.as_ref(), &cdn_base_url, url).await;
	}

	if let Some(url) = minimized_profile_picture_url.as_deref() {
		deletion::mark_object_for_deletion(config.as_ref(), &cdn_base_url, url).await;
	}

	let address_cache_key = format!("query_single:{address_checksum}");
	let username_cache_key = format!("query_single:{username}");
	let avatar_original_cache_key = format!("avatar:{username}:original");
	let avatar_minimized_cache_key = format!("avatar:{username}:minimized");

	let _: Result<(), redis::RedisError> = redis.del(address_cache_key).await;
	let _: Result<(), redis::RedisError> = redis.del(username_cache_key).await;
	let _: Result<(), redis::RedisError> = redis.del(avatar_original_cache_key).await;
	let _: Result<(), redis::RedisError> = redis.del(avatar_minimized_cache_key).await;

	info!(
		address = %address,
		username = %username,
		"Profile picture reset to marble"
	);

	Ok(StatusCode::OK)
}

pub fn docs(op: TransformOperation) -> TransformOperation {
	op.description(
		"Delete a user-uploaded profile picture and revert it to the default marble image.",
	)
}
