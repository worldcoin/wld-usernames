use aide::transform::TransformOperation;
use alloy::primitives::{keccak256, PrimitiveSignature};
use axum::{body::Bytes, extract::Multipart, http::StatusCode, Extension};
use idkit::Proof;
use redis::{aio::ConnectionManager, AsyncCommands};
use serde::Deserialize;
use std::{collections::HashMap, str::FromStr};
use tracing::{info, warn, Instrument};

use crate::{
	config::{ConfigExt, Db},
	types::{ErrorResponse, VerificationLevel as WrappedVerificationLevel},
	verify,
};

const FIELD_METADATA: &str = "metadata";
const FIELD_PROFILE_PICTURE: &str = "profile_picture";

async fn extract_fields_from_multipart(
	multipart: &mut Multipart,
) -> Result<HashMap<String, Bytes>, ErrorResponse> {
	let mut fields = HashMap::new();

	while let Some(field) = multipart.next_field().await.map_err(|err| {
		warn!("failed to read multipart field: {err:#}");
		ErrorResponse::bad_request("invalid_request_body")
	})? {
		let name = field.name().unwrap_or_default().to_string();
		let value = field.bytes().await.map_err(|err| {
			warn!("failed to read multipart field bytes: {err:#}");
			ErrorResponse::bad_request("invalid_request_body")
		})?;
		fields.insert(name, value);
	}

	Ok(fields)
}

#[derive(Debug, Deserialize)]
struct ProfilePictureMetadata {
	proof: String,
	merkle_root: String,
	address: String,
	nullifier_hash: String,
	verification_level: WrappedVerificationLevel,
	signature: String,
}

#[derive(Debug)]
struct ProfilePicturePayload {
	metadata: ProfilePictureMetadata,
	profile_picture_bytes: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct VerifyingKeys {
	verifying_keys: Vec<String>,
}

async fn verify_key_against_cache(
	redis: &mut ConnectionManager,
	recovered_verifying_key_bytes: &[u8],
) -> bool {
	let Ok(cached_data) = redis.get::<_, String>("verifying_keys").await else {
		return false;
	};

	let Ok(record) = serde_json::from_str::<VerifyingKeys>(&cached_data) else {
		warn!("failed to deserialize verifying key cache");
		return false;
	};

	let key_known = record.verifying_keys.iter().any(|stored_key| {
		let normalized = stored_key.trim().trim_start_matches("0x");
		match hex::decode(normalized) {
			Ok(bytes) => bytes.as_slice() == recovered_verifying_key_bytes,
			Err(err) => {
				warn!(
					error = %err,
					stored_key,
					"failed to decode stored verifying key"
				);
				false
			},
		}
	});

	if !key_known {
		warn!(
			recovered_key = %hex::encode(recovered_verifying_key_bytes),
			"recovered verifying key missing from cache"
		);
	}

	key_known
}

impl ProfilePicturePayload {
	async fn from_multipart(mut multipart: Multipart) -> Result<Self, ErrorResponse> {
		let mut fields = extract_fields_from_multipart(&mut multipart).await?;

		let metadata_bytes = fields.remove(FIELD_METADATA).ok_or_else(|| {
			ErrorResponse::validation_error(format!("Missing multipart field: {FIELD_METADATA}"))
		})?;
		let metadata: ProfilePictureMetadata = serde_json::from_slice(metadata_bytes.as_ref())
			.map_err(|err| {
				warn!(error = %err, "failed to deserialize profile picture metadata");
				ErrorResponse::validation_error("Invalid metadata payload provided".to_string())
			})?;

		let profile_picture_bytes = fields
			.remove(FIELD_PROFILE_PICTURE)
			.or_else(|| fields.remove("profile_picture_bytes"))
			.map(|bytes| bytes.to_vec())
			.ok_or_else(|| {
				ErrorResponse::validation_error(format!(
					"Missing multipart field: {FIELD_PROFILE_PICTURE}"
				))
			})?;

		Ok(Self {
			metadata,
			profile_picture_bytes,
		})
	}

	fn proof(&self) -> Proof {
		Proof {
			proof: self.metadata.proof.clone(),
			merkle_root: self.metadata.merkle_root.clone(),
			nullifier_hash: self.metadata.nullifier_hash.clone(),
			verification_level: self.metadata.verification_level.0,
		}
	}

	fn address_checksum(&self) -> &str {
		&self.metadata.address
	}

	fn address(&self) -> &str {
		&self.metadata.address
	}

	const fn signature(&self) -> &str {
		self.metadata.signature.as_str()
	}

	const fn nullifier_hash(&self) -> &str {
		self.metadata.nullifier_hash.as_str()
	}

	fn image_bytes(&self) -> &[u8] {
		&self.profile_picture_bytes
	}
}

#[tracing::instrument(skip_all)]
pub async fn upload_profile_picture(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	multipart: Multipart,
) -> Result<StatusCode, ErrorResponse> {
	let payload = ProfilePicturePayload::from_multipart(multipart).await?;
	let profile_picture_len = payload.profile_picture_bytes.len();
	let address_checksum = payload.address_checksum();
	let address = payload.address();
	let nullifier_hash = payload.nullifier_hash().to_owned();

	info!(
		nullifier_hash = %nullifier_hash,
		address = %address_checksum,
		bytes = profile_picture_len,
		"processing profile picture upload"
	);

	let proof = payload.proof();
	let signal = (nullifier_hash.as_str(), address_checksum.clone());

	if let Err(err) = verify::dev_portal_verify_proof(
		proof,
		config.wld_app_id.to_string(),
		"username",
		signal,
		config.developer_portal_url.clone(),
	)
	.await
	{
		let response = match err {
			verify::Error::Verification(e) => {
				tracing::error!(
					detail = %e.detail,
					nullifier_hash = %nullifier_hash,
					address = %address_checksum,
					"Profile picture verification error",
				);
				ErrorResponse::validation_error(e.detail)
			},
			other => {
				tracing::error!(
					error = %other,
					nullifier_hash = %nullifier_hash,
					address = %address_checksum,
					"Profile picture verification request failed",
				);
				ErrorResponse::server_error("Failed to verify World ID proof".to_string())
			},
		};

		return Err(response);
	}

	let username_row = sqlx::query!(
		"SELECT username FROM names WHERE nullifier_hash = $1 AND address = $2",
		&nullifier_hash,
		&address_checksum
	)
	.fetch_optional(&db.read_only)
	.instrument(tracing::info_span!(
		"profile_picture_lookup",
		nullifier_hash,
		address = %address_checksum
	))
	.await?;

	let Some(record) = username_row else {
		return Err(ErrorResponse::validation_error(
			"No record found matching provided credentials".to_string(),
		));
	};

	let message_bytes = {
		let mut data = Vec::with_capacity(address.len() + 1 + profile_picture_len);
		data.extend_from_slice(address.as_bytes());
		data.push(b'-');
		data.extend_from_slice(payload.image_bytes());
		data
	};
	let digest = keccak256(&message_bytes);

	let signature_input = payload.signature();
	let signature_str = signature_input
		.strip_prefix("0x")
		.unwrap_or(signature_input);
	let signature = PrimitiveSignature::from_str(signature_str).map_err(|err| {
		warn!(error = %err, "invalid signature payload");
		ErrorResponse::validation_error("Invalid signature provided".to_string())
	})?;

	let recovered_verifying_key = signature.recover_from_prehash(&digest).map_err(|err| {
		warn!(error = %err, "failed to recover verifying key from signature");
		ErrorResponse::validation_error("Invalid signature provided".to_string())
	})?;

	let recovered_verifying_key_bytes = recovered_verifying_key.to_encoded_point(false).to_bytes();

	let key_valid = verify_key_against_cache(&mut redis, &recovered_verifying_key_bytes).await;

	if !key_valid {
		return Err(ErrorResponse::validation_error(
			"Invalid signature provided".to_string(),
		));
	}

	info!(
		nullifier_hash = %nullifier_hash,
		address = %address_checksum,
		username = record.username,
		"profile picture metadata validated against stored record"
	);

	Ok(StatusCode::ACCEPTED)
}

pub fn docs(op: TransformOperation) -> TransformOperation {
	op.description(
        "Upload or update a profile picture using multipart/form-data. Expect a `metadata` JSON part containing proof context and a `profile_picture` binary part with the image bytes.",
    )
}
