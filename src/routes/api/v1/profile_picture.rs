use aide::transform::TransformOperation;
use alloy::primitives::{keccak256, PrimitiveSignature};
use aws_config::BehaviorVersion;
use aws_sdk_s3::{primitives::ByteStream, Client as S3Client};
use axum::{body::Bytes, extract::Multipart, http::StatusCode, Extension};
use base64::{engine::general_purpose::STANDARD, Engine};
use idkit::Proof;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::PublicKey as VerifyingKey;
use serde::Deserialize;
use std::collections::HashMap;
use tracing::{info, warn, Instrument};

use std::sync::Arc;

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

#[derive(Debug, Deserialize)]
struct SigningKeyResponse {
	attestation: String,
	public_key: String,
}

const MAX_SIGNING_KEYS: usize = 5;

async fn verify_key_against_db(
	db: &Db,
	recovered_verifying_key_bytes: &[u8],
) -> Result<bool, ErrorResponse> {
	let keys_str = sqlx::query_scalar!("SELECT keys FROM verifying_keys WHERE id = 1")
		.fetch_optional(&db.read_only)
		.await?;

	// If no row exists or keys are empty, return false
	let Some(keys_str) = keys_str else {
		return Ok(false);
	};

	if keys_str.is_empty() {
		return Ok(false);
	}

	// All keys are stored in uncompressed format (65 bytes), so we can do direct comparison
	let recovered_key_hex = hex::encode(recovered_verifying_key_bytes);
	let key_exists = keys_str
		.split(',')
		.any(|stored_key_hex| stored_key_hex == recovered_key_hex);

	Ok(key_exists)
}

async fn add_signing_key_to_db(db: &Db, public_key_hex: &str) -> Result<(), ErrorResponse> {
	// Fetch current keys (or None if row doesn't exist)
	let keys_str = sqlx::query_scalar!("SELECT keys FROM verifying_keys WHERE id = 1")
		.fetch_optional(&db.read_write)
		.await?;

	let mut keys: Vec<&str> = if let Some(ref keys_str) = keys_str {
		if keys_str.is_empty() {
			Vec::new()
		} else {
			keys_str.split(',').collect()
		}
	} else {
		Vec::new()
	};

	// Only add if not already present
	if !keys.contains(&public_key_hex) {
		keys.push(public_key_hex);

		// Keep only the last 5 keys (remove oldest from front)
		if keys.len() > MAX_SIGNING_KEYS {
			keys = keys[keys.len() - MAX_SIGNING_KEYS..].to_vec();
		}

		let updated_keys = keys.join(",");

		if keys_str.is_some() {
			// Update existing row
			sqlx::query!(
				"UPDATE verifying_keys SET keys = $1, updated_at = CURRENT_TIMESTAMP WHERE id = 1",
				updated_keys
			)
			.execute(&db.read_write)
			.await?;
		} else {
			// Insert new row
			sqlx::query!(
				"INSERT INTO verifying_keys (id, keys) VALUES (1, $1)
				ON CONFLICT (id) DO UPDATE SET keys = $1, updated_at = CURRENT_TIMESTAMP",
				updated_keys
			)
			.execute(&db.read_write)
			.await?;
		}
	}

	Ok(())
}

struct ProfilePictureUploadHandler {
	config: Arc<Config>,
	db: Db,
	payload: ProfilePicturePayload,
}

impl ProfilePictureUploadHandler {
	fn new(config: Arc<Config>, db: Db, payload: ProfilePicturePayload) -> Self {
		Self {
			config,
			db,
			payload,
		}
	}

	async fn verify_world_id(&self) -> Result<(), ErrorResponse> {
		let proof = self.payload.proof();
		let signal = self.payload.signature();

		if let Err(err) = verify::dev_portal_verify_proof_hex(
			proof,
			self.config.wld_app_id.to_string(),
			"username",
			signal,
			self.config.developer_portal_url.clone(),
		)
		.await
		{
			let response = match err {
				verify::Error::Verification(e) => {
					tracing::error!(
						detail = %e.detail,
						nullifier_hash = %self.payload.nullifier_hash(),
						address = %self.payload.address_checksum(),
						"Profile picture verification error",
					);
					ErrorResponse::validation_error(e.detail)
				},
				other => {
					tracing::error!(
						error = %other,
						nullifier_hash = %self.payload.nullifier_hash(),
						address = %self.payload.address_checksum(),
						"Profile picture verification request failed",
					);
					ErrorResponse::server_error("Failed to verify World ID proof".to_string())
				},
			};

			return Err(response);
		}

		Ok(())
	}

	async fn verify_username_exists(&self) -> Result<String, ErrorResponse> {
		let username_row = sqlx::query!(
			"SELECT username FROM names WHERE nullifier_hash = $1 AND address = $2",
			self.payload.nullifier_hash(),
			self.payload.address_checksum()
		)
		.fetch_optional(&self.db.read_only)
		.instrument(tracing::info_span!(
			"profile_picture_lookup",
			nullifier_hash = %self.payload.nullifier_hash(),
			address = %self.payload.address_checksum()
		))
		.await?;

		let Some(record) = username_row else {
			return Err(ErrorResponse::validation_error(
				"No record found matching provided credentials".to_string(),
			));
		};

		Ok(record.username)
	}

	fn recover_signature(&self) -> Result<Vec<u8>, ErrorResponse> {
		let address = self.payload.address();
		let profile_picture_len = self.payload.image_bytes().len();

		// Hash: wallet_address + "-" + image_bytes (matching reference implementation)
		let message_bytes = {
			let mut data = Vec::with_capacity(address.len() + 1 + profile_picture_len);
			data.extend_from_slice(address.as_bytes());
			data.push(b'-');
			data.extend_from_slice(self.payload.image_bytes());
			data
		};
		let digest = keccak256(&message_bytes);

		let signature_input = self.payload.signature();
		let signature_str = signature_input
			.strip_prefix("0x")
			.unwrap_or(signature_input);

		// Decode the hex signature (should be 65 bytes: 64-byte signature + 1-byte recovery ID)
		let signature_bytes = hex::decode(signature_str).map_err(|err| {
			warn!(error = %err, "invalid signature hex encoding");
			ErrorResponse::validation_error("Invalid signature provided".to_string())
		})?;

		if signature_bytes.len() != 65 {
			warn!(len = signature_bytes.len(), "signature must be 65 bytes");
			return Err(ErrorResponse::validation_error(
				"Invalid signature length".to_string(),
			));
		}

		let signature =
			PrimitiveSignature::try_from(signature_bytes.as_slice()).map_err(|err| {
				warn!(error = %err, "invalid signature payload");
				ErrorResponse::validation_error("Invalid signature bytes provided".to_string())
			})?;

		// recover_from_prehash expects the 32-byte hash
		let recovered_verifying_key = signature.recover_from_prehash(&digest).map_err(|err| {
			warn!(error = %err, "failed to recover verifying key from signature");
			ErrorResponse::validation_error("Unable to recover signature".to_string())
		})?;

		let recovered_verifying_key_bytes =
			recovered_verifying_key.to_encoded_point(false).to_bytes();

		Ok(recovered_verifying_key_bytes.to_vec())
	}

	async fn verify_signature(&self, recovered_key_bytes: &[u8]) -> Result<(), ErrorResponse> {
		let mut key_valid = verify_key_against_db(&self.db, recovered_key_bytes).await?;

		// If not valid, fetch the current signing key from the DF service
		if !key_valid {
			let df_url = std::env::var("DF_URL").map_err(|_| {
				warn!("DF_URL environment variable not set");
				ErrorResponse::server_error("Configuration error".to_string())
			})?;

			let client = reqwest::Client::new();
			let response = client
				.get(format!("{df_url}/v1/enclave/signing-key"))
				.send()
				.await
				.map_err(|err| {
					warn!(error = %err, "failed to fetch signing key from DF service");
					ErrorResponse::server_error("Failed to verify signature".to_string())
				})?;

			let signing_key_data: SigningKeyResponse = response.json().await.map_err(|err| {
				warn!(error = %err, "failed to parse signing key response");
				ErrorResponse::server_error("Failed to verify signature".to_string())
			})?;
			// TODO: Verify the attestation before trusting the public key

			// Decode the base64 public key from DF service
			let df_public_key_bytes =
				STANDARD
					.decode(&signing_key_data.public_key)
					.map_err(|err| {
						warn!(error = %err, "failed to decode base64 public key");
						ErrorResponse::server_error("Failed to verify signature".to_string())
					})?;

			// Parse and normalize to uncompressed format (65 bytes)
			let df_verifying_key = VerifyingKey::from_sec1_bytes(&df_public_key_bytes)
				.or_else(|_| {
					// Handle 64-byte format (add 0x04 prefix)
					if df_public_key_bytes.len() == 64 {
						let mut uncompressed = Vec::with_capacity(65);
						uncompressed.push(0x04);
						uncompressed.extend_from_slice(&df_public_key_bytes);
						VerifyingKey::from_sec1_bytes(&uncompressed)
					} else {
						Err(k256::elliptic_curve::Error)
					}
				})
				.map_err(|err| {
					warn!(error = ?err, "failed to parse DF public key");
					ErrorResponse::server_error("Failed to verify signature".to_string())
				})?;

			// Always store in uncompressed format for consistent comparison
			let df_uncompressed_bytes = df_verifying_key.to_encoded_point(false).to_bytes();
			let df_uncompressed_hex = hex::encode(&*df_uncompressed_bytes);
			add_signing_key_to_db(&self.db, &df_uncompressed_hex).await?;

			// Direct byte comparison (both are uncompressed)
			if &*df_uncompressed_bytes == recovered_key_bytes {
				key_valid = true;
				info!(
					public_key = %signing_key_data.public_key,
					"verified signature against DF signing key"
				);
			}
		}

		if !key_valid {
			return Err(ErrorResponse::validation_error(
				"Signature did not match any keys".to_string(),
			));
		}

		Ok(())
	}

	async fn upload_to_s3(&self) -> Result<(), ErrorResponse> {
		let aws_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
		let s3_client = S3Client::new(&aws_config);

		let bucket_name = "wld-usernames-profile-pictures";
		let object_key = format!("{}/profile.png", self.payload.address());

		s3_client
			.put_object()
			.bucket(bucket_name)
			.key(&object_key)
			.body(ByteStream::from(self.payload.image_bytes().to_vec()))
			.content_type("image/png")
			.send()
			.await
			.map_err(|err| {
				warn!(error = %err, address = %self.payload.address(), "failed to upload profile picture to S3");
				ErrorResponse::server_error("Failed to upload profile picture".to_string())
			})?;

		info!(
			address = %self.payload.address(),
			bucket = bucket_name,
			key = %object_key,
			"profile picture uploaded to S3"
		);

		Ok(())
	}

	async fn execute(self) -> Result<StatusCode, ErrorResponse> {
		info!(
			nullifier_hash = %self.payload.nullifier_hash(),
			address = %self.payload.address_checksum(),
			bytes = self.payload.image_bytes().len(),
			"processing profile picture upload"
		);

		self.verify_world_id().await?;
		self.verify_username_exists().await?;
		let recovered_key = self.recover_signature()?;
		self.verify_signature(&recovered_key).await?;
		self.upload_to_s3().await?;

		// TODO: Update username record with the profile picture url

		Ok(StatusCode::ACCEPTED)
	}
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
	multipart: Multipart,
) -> Result<StatusCode, ErrorResponse> {
	let payload = ProfilePicturePayload::from_multipart(multipart).await?;
	ProfilePictureUploadHandler::new(config, db, payload)
		.execute()
		.await
}

pub fn docs(op: TransformOperation) -> TransformOperation {
	op.description(
		"Upload or update a profile picture using multipart/form-data. Expects a `metadata` JSON part containing proof context and a `profile_picture` binary part with the image bytes.",
	)
}
