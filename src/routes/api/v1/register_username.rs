use axum::Extension;
use axum_jsonschema::Json;
use http::StatusCode;
use idkit::session::VerificationLevel;

use crate::{
	blocklist::BlocklistExt,
	config::{ConfigExt, Db, DEVICE_USERNAME_REGEX, USERNAME_REGEX},
	types::{ErrorResponse, Name, RegisterUsernamePayload},
	verify,
};

#[tracing::instrument(skip_all)]
#[allow(dependency_on_unit_never_type_fallback)]
pub async fn register_username(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(blocklist): BlocklistExt,
	Json(payload): Json<RegisterUsernamePayload>,
) -> Result<StatusCode, ErrorResponse> {
	match verify::dev_portal_verify_proof(
		payload.into_proof(),
		config.wld_app_id.to_string(),
		"username",
		(&payload.username, payload.address.to_checksum(None)),
		config.developer_portal_url.clone(),
	)
	.await
	{
		Ok(()) => {},
		Err(verify::Error::Verification(e)) => {
			tracing::error!(
				"Register Verification Error: {}, payload:{:?}",
				e.detail,
				payload
			);
			return Err(ErrorResponse::validation_error(e.detail));
		},
		Err(e) => {
			tracing::error!(
				"Register Server Error: {}, payload:{:?}",
				e.to_string(),
				payload
			);
			return Err(ErrorResponse::server_error(
				"Failed to verify World ID proof".to_string(),
			));
		},
	};

	let username_regex = match payload.verification_level.0 {
		VerificationLevel::Orb => USERNAME_REGEX.clone(),
		VerificationLevel::Device => DEVICE_USERNAME_REGEX.clone(),
	};

	if !username_regex.is_match(&payload.username) {
		tracing::warn!(
			"Username does not match the required pattern, payload:{:?}",
			payload
		);
		return Err(ErrorResponse::validation_error(
			"Username does not match the required pattern".to_string(),
		));
	}

	blocklist.ensure_valid(&payload.username).map_err(|e| {
		tracing::warn!("Username is blocked, payload:{:?}", payload);
		ErrorResponse::validation_error(e.to_string())
	})?;

	let uniqueness_check = sqlx::query!(
		"SELECT
			EXISTS(SELECT 1 FROM names WHERE nullifier_hash = $2) AS world_id,
			EXISTS(SELECT 1 FROM names WHERE LOWER(username) = LOWER($1) UNION SELECT 1 FROM old_names where LOWER(old_username) = LOWER($1)) AS username",
			&payload.username,
			&payload.nullifier_hash
		)
		.fetch_one(&db.read_write)
		.await?;

	if uniqueness_check.username.unwrap_or_default() {
		tracing::warn!("Username is already taken, payload:{:?}", payload);
		return Err(ErrorResponse::validation_error(
			"Username is already taken".to_string(),
		));
	};

	if uniqueness_check.world_id.unwrap_or_default() {
		tracing::warn!(
			"This World ID has already registered a username, payload:{:?}",
			payload
		);
		return Err(ErrorResponse::validation_error(
			"This World ID has already registered a username.".to_string(),
		));
	}

	Name::new(
		payload.username,
		&payload.address,
		payload.profile_picture_url,
		payload.nullifier_hash,
		&payload.verification_level,
	)
	.insert(&db.read_write, "names")
	.await?;

	Ok(StatusCode::CREATED)
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Register a World App username with World ID.")
}
