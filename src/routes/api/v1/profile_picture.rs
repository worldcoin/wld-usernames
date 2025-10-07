use alloy::primitives::{Keccak256, PrimitiveSignature};
use aws_config::BehaviorVersion;
use aws_sdk_s3::{primitives::ByteStream, Client as S3Client};
use axum::{body::Bytes, extract::Multipart, Extension};
use axum_jsonschema::Json;
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
	types::{
		ErrorResponse, ProfilePictureUploadResponse, VerificationLevel as WrappedVerificationLevel,
	},
	verify,
};

const FIELD_METADATA: &str = "metadata";
const FIELD_PROFILE_PICTURE: &str = "profile_picture";

fn detect_image_type(bytes: &[u8]) -> Result<&'static str, ()> {
	if bytes.len() < 12 {
		return Err(());
	}

	// Check magic bytes for web-compatible image formats only
	match bytes {
		[0xFF, 0xD8, 0xFF, ..] => Ok("image/jpeg"),
		[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, ..] => Ok("image/png"),
		[0x52, 0x49, 0x46, 0x46, _, _, _, _, 0x57, 0x45, 0x42, 0x50, ..] => Ok("image/webp"),
		_ => Err(()),
	}
}

async fn extract_fields_from_multipart(
	multipart: &mut Multipart,
) -> Result<HashMap<String, Bytes>, ErrorResponse> {
	let mut fields = HashMap::new();

	while let Some(field) = multipart.next_field().await.map_err(|err| {
		warn!("failed to read multipart field: {err:#}");
		ErrorResponse::bad_request("invalid_request_body")
	})? {
		let name = field.name().unwrap_or_default().to_string();
		let bytes = field.bytes().await.map_err(|err| {
			warn!("failed to read multipart field bytes: {err:#}");
			ErrorResponse::bad_request("invalid_request_body")
		})?;
		fields.insert(name, bytes);
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
	// TODO: Change to one per row if we decide to have multiple enclaves.
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

	// All keys are stored in compressed format (33 bytes), so we can do direct comparison
	let recovered_key_hex = hex::encode(recovered_verifying_key_bytes);
	let key_exists = keys_str
		.split(',')
		.any(|stored_key_hex| stored_key_hex == recovered_key_hex);

	Ok(key_exists)
}

async fn add_signing_key_to_db(db: &Db, public_key_hex: &str) -> Result<(), ErrorResponse> {
	// Fetch current keys (or None if row doesn't exist)
	let keys_str: Option<String> =
		sqlx::query_scalar!("SELECT keys FROM verifying_keys WHERE id = 1")
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

		// Parse wallet address to bytes (strip 0x if present, decode hex)
		let wallet_address_bytes = {
			let addr = address.trim_start_matches("0x");
			hex::decode(addr).map_err(|err| {
				warn!(error = %err, "invalid wallet address hex");
				ErrorResponse::validation_error("Invalid wallet address".to_string())
			})?
		};

		if wallet_address_bytes.len() != 20 {
			warn!(
				len = wallet_address_bytes.len(),
				"wallet address must be 20 bytes"
			);
			return Err(ErrorResponse::validation_error(
				"Invalid wallet address length".to_string(),
			));
		}

		// Hash: wallet_address_bytes + "-" + image_bytes (matching reference implementation)
		let mut hasher = Keccak256::new();
		hasher.update(&wallet_address_bytes);
		hasher.update(b"-");
		hasher.update(self.payload.image_bytes());
		let digest = hasher.finalize();

		let signature_input = self.payload.signature();
		let signature_str = signature_input
			.strip_prefix("0x")
			.unwrap_or(signature_input);

		// Decode the hex signature (should be 65 bytes: 64-byte signature + 1-byte recovery ID)
		let signature_bytes = hex::decode(signature_str).map_err(|_err| {
			ErrorResponse::validation_error("Invalid signature provided".to_string())
		})?;

		if signature_bytes.len() != 65 {
			return Err(ErrorResponse::validation_error(
				"Invalid signature length".to_string(),
			));
		}

		let signature =
			PrimitiveSignature::try_from(signature_bytes.as_slice()).map_err(|_err| {
				ErrorResponse::validation_error("Invalid signature bytes provided".to_string())
			})?;

		// recover_from_prehash expects the 32-byte hash
		let recovered_verifying_key = signature.recover_from_prehash(&digest).map_err(|err| {
			warn!(error = %err, "failed to recover verifying key from signature");
			ErrorResponse::validation_error("Unable to recover signature".to_string())
		})?;

		let recovered_verifying_key_bytes =
			recovered_verifying_key.to_encoded_point(true).to_bytes();

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
				.get(format!("{}/v1/enclave/signing-key", df_url))
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

			// Decode the base64 compressed public key (33 bytes) from DF service
			let df_public_key_bytes =
				STANDARD
					.decode(&signing_key_data.public_key)
					.map_err(|err| {
						warn!(error = %err, "failed to decode base64 public key");
						ErrorResponse::server_error("Failed to verify signature".to_string())
					})?;

			// Parse as compressed SEC1 format (33 bytes)
			let df_verifying_key =
				VerifyingKey::from_sec1_bytes(&df_public_key_bytes).map_err(|err| {
					warn!(error = ?err, "failed to parse DF public key");
					ErrorResponse::server_error("Failed to verify signature".to_string())
				})?;

			// Store in compressed format for consistent comparison
			let df_compressed_bytes = df_verifying_key.to_encoded_point(true).to_bytes();
			let df_compressed_hex = hex::encode(&*df_compressed_bytes);
			add_signing_key_to_db(&self.db, &df_compressed_hex).await?;

			// Direct byte comparison (both are compressed)
			if &*df_compressed_bytes == recovered_key_bytes {
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

	async fn upload_to_s3(&self) -> Result<String, ErrorResponse> {
		let aws_config = aws_config::load_defaults(BehaviorVersion::latest()).await;
		let s3_client = S3Client::new(&aws_config);

		let bucket_name = std::env::var("UPLOADS_BUCKET_NAME").map_err(|_| {
			warn!("UPLOADS_BUCKET_NAME environment variable not set");
			ErrorResponse::server_error("Configuration error".to_string())
		})?;
		let object_key = format!("{}/profile", self.payload.address());

		// Detect content type from magic bytes
		let content_type = detect_image_type(self.payload.image_bytes())
			.map_err(|_| ErrorResponse::server_error("Failed to detect image type".to_string()))?;

		s3_client
			.put_object()
			.bucket(bucket_name)
			.key(&object_key)
			.body(ByteStream::from(self.payload.image_bytes().to_vec()))
			.content_type(content_type)
			.send()
			.await
			.map_err(|err| {
				warn!(error = %err, address = %self.payload.address(), "failed to upload profile picture to S3");
				ErrorResponse::server_error("Failed to upload profile picture".to_string())
			})?;

		Ok(object_key)
	}

	async fn update_profile_picture_url(&self, object_key: &str) -> Result<String, ErrorResponse> {
		// Construct the CDN URL
		let cdn_base_url = std::env::var("PROFILE_PICTURE_CDN_URL").map_err(|_| {
			warn!("PROFILE_PICTURE_CDN_URL environment variable not set");
			ErrorResponse::server_error("Configuration error".to_string())
		})?;
		let profile_picture_url = format!("{}/{}", cdn_base_url.trim_end_matches('/'), object_key);

		// Update database with the profile picture URL
		sqlx::query!(
			"UPDATE names
			 SET profile_picture_url = $1, updated_at = CURRENT_TIMESTAMP
			 WHERE address = $2",
			profile_picture_url,
			self.payload.address()
		)
		.execute(&self.db.read_write)
		.await?;

		Ok(profile_picture_url)
	}

	async fn execute(self) -> Result<ProfilePictureUploadResponse, ErrorResponse> {
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

		let object_key = self.upload_to_s3().await?;
		let profile_picture_url = self.update_profile_picture_url(&object_key).await?;

		info!(url = %profile_picture_url, "Profile picture uploaded and database updated successfully");

		Ok(ProfilePictureUploadResponse {
			profile_picture_url,
		})
	}
}

impl ProfilePicturePayload {
	async fn from_multipart(mut multipart: Multipart) -> Result<Self, ErrorResponse> {
		let mut fields = extract_fields_from_multipart(&mut multipart).await?;

		let metadata_bytes = fields.remove(FIELD_METADATA).ok_or_else(|| {
			ErrorResponse::validation_error(format!("Missing multipart field: {FIELD_METADATA}"))
		})?;
		let metadata: ProfilePictureMetadata = serde_json::from_slice(metadata_bytes.as_ref())
			.map_err(|_err| {
				ErrorResponse::validation_error("Invalid metadata payload provided".to_string())
			})?;

		let profile_picture_bytes = fields
			.remove(FIELD_PROFILE_PICTURE)
			.or_else(|| fields.remove("profile_picture_bytes"))
			.ok_or_else(|| {
				ErrorResponse::validation_error(format!(
					"Missing multipart field: {FIELD_PROFILE_PICTURE}"
				))
			})?;

		// Validate the image type by checking magic bytes
		detect_image_type(&profile_picture_bytes).map_err(|_| {
			ErrorResponse::validation_error(
				"Unsupported image format. Only JPEG, PNG, and WebP are supported.".to_string(),
			)
		})?;

		Ok(Self {
			metadata,
			profile_picture_bytes: profile_picture_bytes.to_vec(),
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

	fn signature(&self) -> &str {
		self.metadata.signature.as_str()
	}

	fn nullifier_hash(&self) -> &str {
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
) -> Result<Json<ProfilePictureUploadResponse>, ErrorResponse> {
	let payload = ProfilePicturePayload::from_multipart(multipart).await?;
	let response = ProfilePictureUploadHandler::new(config, db, payload)
		.execute()
		.await?;
	Ok(Json(response))
}
