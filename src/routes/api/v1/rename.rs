use axum::Extension;
use axum_jsonschema::Json;
use http::StatusCode;
use idkit::session::VerificationLevel;
use sqlx::PgPool;

use crate::{
	blocklist::BlocklistExt,
	config::{ConfigExt, DEVICE_USERNAME_REGEX, USERNAME_REGEX},
	types::{ErrorResponse, Name, RenamePayload},
	verify,
};

#[allow(dependency_on_unit_never_type_fallback)]
pub async fn rename(
	Extension(config): ConfigExt,
	Extension(db): Extension<PgPool>,
	Extension(blocklist): BlocklistExt,
	Json(payload): Json<RenamePayload>,
) -> Result<StatusCode, ErrorResponse> {
	let Some(record) = sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE username = $1",
		&payload.old_username
	)
	.fetch_optional(&db)
	.await?
	else {
		return Err(ErrorResponse::not_found("Username not found".to_string()));
	};

	if record.nullifier_hash != payload.nullifier_hash {
		return Err(ErrorResponse::unauthorized(
			"You can't update this name".to_string(),
		));
	}

	match verify::dev_portal_verify_proof(
		payload.into_proof(),
		config.wld_app_id.to_string(),
		"username",
		(&payload.old_username, &payload.new_username),
		config.developer_portal_url.clone(),
	)
	.await
	{
		Ok(()) => {},
		Err(verify::Error::Verification(e)) => {
			tracing::error!("Rename Verification Error: {:?}", payload);
			return Err(ErrorResponse::validation_error(e.detail));
		},
		Err(_) => {
			return Err(ErrorResponse::server_error(
				"Failed to verify World ID proof".to_string(),
			))
		},
	};

	let username_regex = match payload.verification_level.0 {
		VerificationLevel::Orb => USERNAME_REGEX.clone(),
		VerificationLevel::Device => DEVICE_USERNAME_REGEX.clone(),
	};

	if !username_regex.is_match(&payload.new_username) {
		return Err(ErrorResponse::validation_error(
			"Username does not match the required pattern".to_string(),
		));
	}

	blocklist
		.ensure_valid(&payload.new_username)
		.map_err(|e| ErrorResponse::validation_error(e.to_string()))?;

	let uniqueness_check = sqlx::query!(
		"SELECT
            EXISTS(SELECT 1 FROM old_names where new_username = $1) AS has_old_username,
            EXISTS(SELECT 1 FROM names WHERE username = $2 UNION SELECT 1 FROM old_names where old_username = $2 AND new_username != $1) AS username
        ",
		&payload.old_username,
		&payload.new_username,
	)
	.fetch_one(&db)
	.await?;

	if uniqueness_check.username.unwrap_or_default() {
		return Err(ErrorResponse::validation_error(
			"Username is already taken".to_string(),
		));
	};

	let mut tx = db.begin().await?;

	if uniqueness_check.has_old_username.unwrap_or_default() {
		sqlx::query!(
			"DELETE FROM old_names WHERE new_username = $1",
			&payload.old_username
		)
		.execute(&mut *tx)
		.await?;
	}

	sqlx::query!(
		"UPDATE names SET username = $1 WHERE username = $2",
		&payload.new_username,
		&payload.old_username,
	)
	.execute(&mut *tx)
	.await?;

	sqlx::query!(
		"INSERT INTO old_names (old_username, new_username) VALUES ($1, $2)",
		&payload.old_username,
		&payload.new_username,
	)
	.execute(&mut *tx)
	.await?;

	tx.commit().await?;

	Ok(StatusCode::OK)
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Change your World App username to a new one.")
}
