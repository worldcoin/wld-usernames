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
				"Register Verification Error: {}, Payload: {:?}",
				e.detail,
				payload
			);
			return Err(ErrorResponse::validation_error(e.detail));
		},
		Err(e) => {
			tracing::error!(
				"Register Server Error: {}, Payload: {:?}",
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
		return Err(ErrorResponse::validation_error(
			"Username does not match the required pattern".to_string(),
		));
	}

	blocklist
		.ensure_valid(&payload.username)
		.map_err(|e| ErrorResponse::validation_error(e.to_string()))?;

	let uniqueness_span = tracing::span!(
		tracing::Level::INFO,
		"query_uniqueness_check",
		query_type = "SELECT",
		username = %payload.username
	);
	let _uniqueness_enter = uniqueness_span.enter();
	let uniqueness_check = sqlx::query!(
            "SELECT
                EXISTS(SELECT 1 FROM names WHERE nullifier_hash = $2) AS world_id,
                EXISTS(SELECT 1 FROM names WHERE username = $1 UNION SELECT 1 FROM old_names where old_username = $1) AS username",
            &payload.username,
            &payload.nullifier_hash
        )
        .fetch_one(&db.read_write)
        .await?;
	drop(_uniqueness_enter);

	if uniqueness_check.username.unwrap_or_default() {
		return Err(ErrorResponse::validation_error(
			"Username is already taken".to_string(),
		));
	};

	if uniqueness_check.world_id.unwrap_or_default() {
		return Err(ErrorResponse::validation_error(
			"This World ID has already registered a username.".to_string(),
		));
	}

	let insert_span = tracing::span!(
		parent: None,
		tracing::Level::INFO,
		"insert_new_name",
		query_type = "INSERT",
		username = %payload.username
	);
	let _insert_enter = insert_span.enter();
	Name::new(
		payload.username,
		&payload.address,
		payload.profile_picture_url,
		payload.nullifier_hash,
		&payload.verification_level,
	)
	.insert(&db.read_write, "names")
	.await?;
	drop(_insert_enter);

	Ok(StatusCode::CREATED)
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Register a World App username with World ID.")
}
