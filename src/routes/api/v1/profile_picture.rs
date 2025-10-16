use aws_config::BehaviorVersion;
use aws_sdk_s3::{primitives::ByteStream, Client as S3Client};
use axum::{body::Bytes, extract::Multipart, Extension};
use axum_jsonschema::Json;
use idkit::session::VerificationLevel;
use idkit::Proof;
use redis::{aio::ConnectionManager, AsyncCommands};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tracing::{info, warn};

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

const fn detect_image_type(bytes: &[u8]) -> Result<&'static str, ()> {
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
	challenge_image_hash: String, // Hash of the challenge image to verify against uploaded profile_picture
}

#[derive(Debug)]
struct ProfilePicturePayload {
	metadata: ProfilePictureMetadata,
	profile_picture_bytes: Vec<u8>,
}

struct ProfilePictureUploadHandler {
	config: Arc<Config>,
	db: Db,
	redis: ConnectionManager,
	payload: ProfilePicturePayload,
}

impl ProfilePictureUploadHandler {
	const fn new(
		config: Arc<Config>,
		db: Db,
		redis: ConnectionManager,
		payload: ProfilePicturePayload,
	) -> Self {
		Self {
			config,
			db,
			redis,
			payload,
		}
	}

	async fn verify_world_id(&self) -> Result<(), ErrorResponse> {
		let proof = self.payload.proof();

		// We only accept orb verification for profile pictures
		if proof.verification_level != VerificationLevel::Orb {
			warn!(
				?proof.verification_level,
				"Rejected profile picture upload with insufficient verification level"
			);
			return Err(ErrorResponse::bad_request(
				"insufficient_verification_level",
			));
		}

		if let Err(err) = verify::dev_portal_verify_proof(
			proof,
			self.config.wld_app_id.to_string(),
			"username",
			self.payload.address(),
			self.config.developer_portal_url.clone(),
		)
		.await
		{
			let response = match err {
				verify::Error::Verification(_) => ErrorResponse::bad_request("invalid_proof"),
				verify::Error::Reqwest(_)
				| verify::Error::Serde(_)
				| verify::Error::InvalidResponse { .. } => ErrorResponse::server_error(
					"An error occurred verifying the proof, please try again later".to_string(),
				),
			};
			return Err(response);
		}

		Ok(())
	}

	async fn verify_username_exists(&self) -> Result<String, ErrorResponse> {
		let username = sqlx::query_scalar!(
			"SELECT username FROM names WHERE LOWER(address) = LOWER($1)",
			self.payload.address()
		)
		.fetch_optional(&self.db.read_only)
		.await?;

		username.ok_or_else(|| {
			warn!(
				address = self.payload.address(),
				"Address does not have a username"
			);
			ErrorResponse::bad_request("address_without_username")
		})
	}

	fn verify_challenge_image_hash(&self) -> Result<(), ErrorResponse> {
		// Compute SHA256 hash of the uploaded profile picture
		let mut hasher = Sha256::new();
		hasher.update(self.payload.image_bytes());
		let computed_hash = hex::encode(hasher.finalize());

		// The challenge_image_hash should match exactly
		let expected_hash = self.payload.challenge_image_hash().trim_start_matches("0x");

		if computed_hash != expected_hash {
			warn!(
				computed_hash = %computed_hash,
				expected_hash = %expected_hash,
				"Challenge image hash mismatch - uploaded image does not match expected hash"
			);
			return Err(ErrorResponse::validation_error(
				"Uploaded image does not match challenge image hash".to_string(),
			));
		}

		info!(
			hash = %computed_hash,
			"Challenge image hash verified successfully"
		);

		Ok(())
	}

	async fn upload_to_s3(&self) -> Result<String, ErrorResponse> {
		let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
		let s3_client = S3Client::new(&config);

		let bucket_name = std::env::var("UPLOADS_BUCKET_NAME")
			.map_err(|_| ErrorResponse::server_error("Configuration error".to_string()))?;

		let object_key = format!("{}/profile", self.payload.address());

		s3_client
			.put_object()
			.bucket(&bucket_name)
			.key(&object_key)
			.body(ByteStream::from(self.payload.image_bytes().to_vec()))
			.content_type(
				detect_image_type(self.payload.image_bytes())
					.unwrap_or("application/octet-stream"),
			)
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
			 WHERE LOWER(address) = LOWER($2)",
			profile_picture_url,
			self.payload.address()
		)
		.execute(&self.db.read_write)
		.await?;

		Ok(profile_picture_url)
	}

	async fn invalidate_cache(&mut self, username: &str) -> Result<(), ErrorResponse> {
		use super::validate_address;

		let address_cache_key =
			format!("query_single:{}", validate_address(self.payload.address()));
		let username_cache_key = format!("query_single:{username}");

		let _: Result<(), redis::RedisError> = self.redis.del(&address_cache_key).await;
		let _: Result<(), redis::RedisError> = self.redis.del(&username_cache_key).await;

		Ok(())
	}

	async fn execute(mut self) -> Result<ProfilePictureUploadResponse, ErrorResponse> {
		info!(
			nullifier_hash = %self.payload.nullifier_hash(),
			address = %self.payload.address(),
			bytes = self.payload.image_bytes().len(),
			challenge_image_hash = %self.payload.challenge_image_hash(),
			"processing profile picture upload (v2 with attestation)"
		);

		// Verify the uploaded image matches the challenge image hash
		self.verify_challenge_image_hash()?;

		self.verify_world_id().await?;
		let username = self.verify_username_exists().await?;

		let object_key = self.upload_to_s3().await?;
		let profile_picture_url = self.update_profile_picture_url(&object_key).await?;

		// Invalidate cache for both address and username lookups
		self.invalidate_cache(&username).await?;

		info!(url = %profile_picture_url, "Profile picture uploaded and database updated successfully (v2)");

		Ok(ProfilePictureUploadResponse {
			profile_picture_url,
		})
	}
}

impl ProfilePicturePayload {
	async fn from_multipart(mut multipart: Multipart) -> Result<Self, ErrorResponse> {
		let mut fields = extract_fields_from_multipart(&mut multipart).await?;

		// Note: metadata and idempotency_key are consumed by attestation middleware for hashing
		// The request body is reconstructed by the middleware, so all fields are still available here

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

	fn address(&self) -> &str {
		&self.metadata.address
	}

	fn nullifier_hash(&self) -> &str {
		self.metadata.nullifier_hash.as_str()
	}

	fn challenge_image_hash(&self) -> &str {
		self.metadata.challenge_image_hash.as_str()
	}

	fn image_bytes(&self) -> &[u8] {
		&self.profile_picture_bytes
	}
}

/// This endpoint requires an attestation token to be provided in the request header
/// The attestation middleware handles all security verification
#[tracing::instrument(skip_all)]
pub async fn upload_profile_picture(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(redis): Extension<ConnectionManager>,
	multipart: Multipart,
) -> Result<Json<ProfilePictureUploadResponse>, ErrorResponse> {
	let payload = ProfilePicturePayload::from_multipart(multipart).await?;
	let response = ProfilePictureUploadHandler::new(config, db, redis, payload)
		.execute()
		.await?;
	Ok(Json(response))
}
