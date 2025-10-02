use aide::transform::TransformOperation;
use axum::{body::Bytes, extract::Multipart, http::StatusCode, Extension};
use idkit::Proof;
use serde::Deserialize;
use std::collections::HashMap;
use tracing::warn;

use crate::{
	config::{Config, ConfigExt, Db},
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
		ErrorResponse::validation_error("invalid_request_body".to_string())
	})? {
		let name = field.name().unwrap_or_default().to_string();
		let value = field.bytes().await.map_err(|err| {
			warn!("failed to read multipart field bytes: {err:#}");
			ErrorResponse::validation_error("invalid_request_body".to_string())
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

	const fn signature(&self) -> &str {
		self.metadata.signature.as_str()
	}

	const fn nullifier_hash(&self) -> &str {
		self.metadata.nullifier_hash.as_str()
	}

	fn address_checksum(&self) -> &str {
		&self.metadata.address
	}

	fn image_bytes(&self) -> &[u8] {
		&self.profile_picture_bytes
	}
}

async fn verify_world_id(
	config: &Config,
	payload: &ProfilePicturePayload,
) -> Result<(), ErrorResponse> {
	let proof = payload.proof();
	let signal = payload.signature();

	if let Err(err) = verify::dev_portal_verify_proof_hex(
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
					nullifier_hash = %payload.nullifier_hash(),
					address = %payload.address_checksum(),
					"Profile picture verification error",
				);
				ErrorResponse::validation_error(e.detail)
			}
			other => {
				tracing::error!(
					error = %other,
					nullifier_hash = %payload.nullifier_hash(),
					address = %payload.address_checksum(),
					"Profile picture verification request failed",
				);
				ErrorResponse::server_error("Failed to verify World ID proof".to_string())
			}
		};

		return Err(response);
	}

	Ok(())
}

#[tracing::instrument(skip_all)]
pub async fn upload_profile_picture(
	Extension(config): ConfigExt,
	Extension(_db): Extension<Db>,
	multipart: Multipart,
) -> Result<StatusCode, ErrorResponse> {
	let payload = ProfilePicturePayload::from_multipart(multipart).await?;

	// Verify World ID proof with hex-encoded signature
	verify_world_id(&config, &payload).await?;

	// TODO: Add signature verification (PR 2)
	// TODO: Add S3 upload (PR 2)

	tracing::info!(
		nullifier_hash = %payload.nullifier_hash(),
		address = %payload.address_checksum(),
		bytes = payload.image_bytes().len(),
		"Profile picture upload verified (placeholder - signature verification and S3 upload coming in PR 2)"
	);

	Ok(StatusCode::ACCEPTED)
}

pub fn docs(op: TransformOperation) -> TransformOperation {
	op.description(
		"Upload or update a profile picture using multipart/form-data. Expects a `metadata` JSON part containing proof context and a `profile_picture` binary part with the image bytes. Currently validates World ID proof only.",
	)
}
