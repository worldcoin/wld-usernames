use aide::transform::TransformOperation;
use aws_sdk_s3::types::{Tag, Tagging};
use axum::{http::StatusCode, Extension};
use axum_jsonschema::Json;
use redis::{aio::ConnectionManager, AsyncCommands};
use tracing::{info, info_span, warn, Instrument};

use crate::{
	config::{Config, ConfigExt, Db},
	types::{DeleteProfilePicturePayload, ErrorResponse, Name},
	verify,
};

const DELETION_TAG_KEY: &str = "pending-deletion";
const DELETION_TAG_VALUE: &str = "true";

#[tracing::instrument(skip_all)]
#[allow(dependency_on_unit_never_type_fallback)]
pub async fn delete_profile_picture(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	Json(payload): Json<DeleteProfilePicturePayload>,
) -> Result<StatusCode, ErrorResponse> {
	let address_checksum = payload.address.to_checksum(None);

	match verify::dev_portal_verify_proof(
		payload.into_proof(),
		config.wld_app_id.to_string(),
		"username",
		address_checksum.clone(),
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
	};

	let query_address = address_checksum.clone();

	let Some(record) = sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE LOWER(address) = LOWER($1)",
		query_address
	)
	.fetch_optional(&db.read_only)
	.instrument(info_span!(
		"delete_profile_picture_fetch_record",
		address = %address_checksum
	))
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

	let marble_url = format!(
		"{}/{}.png",
		cdn_base_url.trim_end_matches('/'),
		address.to_lowercase()
	);

	let minimized_marble_url = format!(
		"{}/minimized_{}.png",
		cdn_base_url.trim_end_matches('/'),
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
		mark_object_for_deletion(config.as_ref(), &cdn_base_url, url).await;
	}

	if let Some(url) = minimized_profile_picture_url.as_deref() {
		mark_object_for_deletion(config.as_ref(), &cdn_base_url, url).await;
	}

	let address_cache_key = format!("query_single:{}", address_checksum);
	let username_cache_key = format!("query_single:{}", username);
	let avatar_original_cache_key = format!("avatar:{}:original", username);
	let avatar_minimized_cache_key = format!("avatar:{}:minimized", username);

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

async fn mark_object_for_deletion(config: &Config, cdn_base_url: &str, url: &str) {
	let Some(object_key) = object_key_from_cdn_url(cdn_base_url, url) else {
		return;
	};

	let bucket = match std::env::var("UPLOADS_BUCKET_NAME") {
		Ok(bucket) => bucket,
		Err(_) => {
			warn!("UPLOADS_BUCKET_NAME environment variable not set; skipping S3 tagging");
			return;
		},
	};

	let tag = match Tag::builder()
		.key(DELETION_TAG_KEY)
		.value(DELETION_TAG_VALUE)
		.build()
	{
		Ok(tag) => tag,
		Err(err) => {
			warn!(error = %err, "Failed to construct deletion tag payload");
			return;
		},
	};

	let tagging = Tagging::builder().set_tag_set(Some(vec![tag])).build();

	if let Err(err) = config
		.s3_client()
		.put_object_tagging()
		.bucket(&bucket)
		.key(&object_key)
		.tagging(tagging)
		.send()
		.await
	{
		warn!(
			error = %err,
			bucket = %bucket,
			key = %object_key,
			"Failed to tag profile picture object for deletion"
		);
	} else {
		info!(
			bucket = %bucket,
			key = %object_key,
			"Tagged profile picture object for deferred deletion"
		);
	}
}

fn object_key_from_cdn_url(cdn_base_url: &str, full_url: &str) -> Option<String> {
	let base_url = url::Url::parse(cdn_base_url).ok()?;
	let url = url::Url::parse(full_url).ok()?;

	if base_url.scheme() != url.scheme()
		|| base_url.host_str() != url.host_str()
		|| base_url.port_or_known_default() != url.port_or_known_default()
	{
		return None;
	}

	let base_path = base_url.path().trim_end_matches('/');
	let full_path = url.path();

	let relative_path = if base_path.is_empty() || base_path == "/" {
		full_path.trim_start_matches('/')
	} else {
		full_path.strip_prefix(base_path)?.trim_start_matches('/')
	};

	if relative_path.is_empty() {
		None
	} else {
		Some(relative_path.to_string())
	}
}

pub fn docs(op: TransformOperation) -> TransformOperation {
	op.description(
		"Delete a user-uploaded profile picture and revert it to the default marble image.",
	)
}
